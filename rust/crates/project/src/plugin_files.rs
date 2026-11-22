//! Projects scoped workspace host reads into the plugin file service contract.

use crate::{
    encoding,
    files::{is_plain_text, VisibilityRules},
};
use base64::prelude::{Engine, BASE64_STANDARD};
use lithe_core_rules::file_names::FileNameKey;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_workspace::host::{
    self, ScopedDirectoryEntry, ScopedFileAccess, ScopedFileKind, ScopedFileRead, ScopedFileStat,
    ScopedSearch, MAX_SCOPED_CONFIGURATION_BYTES, MAX_SCOPED_READ_BYTES, MAX_SCOPED_SEARCH_RESULTS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_PATH_BYTES: usize = 4096;
const MAX_CURSOR_BYTES: usize = 4096;
// Leave room for the host response envelope and its cursor under 1 MiB.
const MAX_DIRECTORY_PAGE_BYTES: usize = 900 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// A read authenticated by the desktop's workspace and caller registries.
pub struct PluginFileRequest {
    /// Opaque root identity checked by the desktop and echoed in resources.
    #[serde(rename = "rootID")]
    root_id: String,
    /// Context generation used to bind an opaque pagination cursor.
    generation: u64,
    /// Distinguishes two attachments of the same session to an open workspace.
    #[serde(default)]
    attachment: u64,
    /// POSIX workspace-relative resource, or an absolute path for OpenDocument.
    path: String,
    operation: Operation,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Operation {
    Watch,
    FindFiles {
        query: String,
        file_mask: Option<String>,
        cursor: Option<String>,
        limit: Option<i64>,
    },
    FindText {
        pattern: String,
        file_mask: Option<String>,
        cursor: Option<String>,
        limit: Option<i64>,
        case_sensitive: Option<bool>,
        whole_words: Option<bool>,
        regular_expression: Option<bool>,
    },
    ReadDirectory {
        cursor: Option<String>,
        limit: Option<i64>,
    },
    Stat,
    ReadFile {
        maximum_bytes: Option<i64>,
    },
    // Internal editor navigation uses the document budget. The host service
    // selects ReadFile for every backend readFile request.
    OpenDocument {
        maximum_bytes: usize,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The last emitted entry in one directory and context generation.
struct Cursor {
    scope: String,
    directory: bool,
    name: String,
}

/// Replies to the desktop's remote workspace file request. Disk content does
/// not consult live editor buffers. There is no local or unscoped fallback.
pub fn read(request: PluginFileRequest) -> Result<Value, CoreError> {
    let path = if matches!(request.operation, Operation::OpenDocument { .. }) {
        document_path(&request.path)?
    } else {
        normalize_path(&request.path)?
    };
    let resource = |path: &str| json!({"rootID": request.root_id, "path": path});
    let search_scope = |search: &ScopedSearch| -> Result<String, CoreError> {
        if !path.is_empty() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Workspace search requires the root resource",
            ));
        }
        Ok(encoding::bytes_identity(
            &serde_json::to_vec(&(
                1,
                &request.root_id,
                request.generation,
                request.attachment,
                host::scoped_workspace_identity()?,
                search,
            ))
            .expect("Search cursor scope always encodes"),
        ))
    };
    match request.operation {
        Operation::Watch => {
            let status: ScopedFileStat = host::scoped_file(path.clone(), ScopedFileAccess::Stat)?;
            let is_directory = match status.kind {
                ScopedFileKind::Directory => true,
                ScopedFileKind::File => false,
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Workspace watches require a regular file or directory",
                    ))
                }
            };
            let visibility = VisibilityRules::new(Vec::new(), Vec::new());
            if visibility.is_hidden(&path, is_directory) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "This resource is excluded from workspace watching",
                ));
            }
            Ok(json!({"path": path, "directory": is_directory}))
        }
        Operation::FindFiles {
            query,
            file_mask,
            cursor,
            limit,
        } => {
            let search = ScopedSearch {
                query,
                file_mask: file_mask.unwrap_or_default(),
                text: false,
                case_sensitive: false,
                whole_words: false,
                regular_expression: false,
            };
            search_page(
                &request.root_id,
                search_scope(&search)?,
                search,
                cursor,
                limit,
            )
        }
        Operation::FindText {
            pattern,
            file_mask,
            cursor,
            limit,
            case_sensitive,
            whole_words,
            regular_expression,
        } => {
            let search = ScopedSearch {
                query: pattern,
                file_mask: file_mask.unwrap_or_default(),
                text: true,
                case_sensitive: case_sensitive.unwrap_or(false),
                whole_words: whole_words.unwrap_or(false),
                regular_expression: regular_expression.unwrap_or(false),
            };
            search_page(
                &request.root_id,
                search_scope(&search)?,
                search,
                cursor,
                limit,
            )
        }
        Operation::ReadDirectory { cursor, limit } => {
            let scope = encoding::bytes_identity(
                &serde_json::to_vec(&(1, &request.root_id, request.generation, &path))
                    .expect("Directory cursor scope always encodes"),
            );
            let after = cursor
                .as_deref()
                .map(|cursor| decode_cursor(cursor, &scope))
                .transpose()?;
            let entries = host::scoped_file(path.clone(), ScopedFileAccess::List)?;
            directory_page(entries, &path, scope, after, limit, resource)
        }
        Operation::Stat => {
            let status: ScopedFileStat = host::scoped_file(path.clone(), ScopedFileAccess::Stat)?;
            Ok(json!({
                "resource": resource(&path),
                "kind": plugin_kind(status.kind),
                "size": status.size,
                "modifiedMilliseconds": status.modified_milliseconds,
            }))
        }
        Operation::ReadFile { maximum_bytes } => {
            let length = maximum_bytes.map_or(MAX_SCOPED_READ_BYTES, |value| {
                usize::try_from(value).unwrap_or(0)
            });
            let data = host::scoped_file(path.clone(), ScopedFileAccess::Read { length })?;
            file_content(resource(&path), data, length)
        }
        Operation::OpenDocument { maximum_bytes } => {
            let length = maximum_bytes.min(host::MAX_TRANSFER_BYTES as usize);
            let data: ScopedFileRead =
                host::scoped_file(path.clone(), ScopedFileAccess::ReadDocument { length })?;
            Ok(json!({"path":path, "base64":data.data, "isTruncated":data.is_truncated}))
        }
    }
}

/// Resolves editor navigation paths against the bound host root before sending
/// the relative result to the server's descriptor-scoped file read.
fn document_path(path: &str) -> Result<String, CoreError> {
    if let Some(absolute) = path.strip_prefix('/') {
        let absolute = normalize_path(absolute)?;
        let (_, root) = host::scoped_workspace_identity()?;
        let root = root.trim_matches('/');
        let relative = if root.is_empty() {
            Some(absolute.as_str())
        } else {
            absolute
                .strip_prefix(root)
                .and_then(|suffix| suffix.strip_prefix('/'))
        };
        return relative.map(str::to_owned).ok_or_else(|| {
            CoreError::new(
                ErrorCode::PermissionDenied,
                "The document is outside the workspace",
            )
        });
    }
    normalize_path(path)
}

/// A project configuration read based on the source and budget Core planned.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationReadRequest {
    /// The source path relative to the workspace root.
    path: String,
    /// The maximum supported size of the original UTF-8 document.
    maximum_bytes: usize,
}

/// Reads the project layer of settings from the current workspace host.
/// Absence means inheritance. An unavailable workspace or invalid source
/// must remain an error so it never hides a failed read behind defaults.
pub fn read_configuration(request: ConfigurationReadRequest) -> Result<Value, CoreError> {
    let path = normalize_path(&request.path)?;
    let length = request.maximum_bytes;
    if path.is_empty() || !(1..=MAX_SCOPED_CONFIGURATION_BYTES).contains(&length) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid workspace configuration source or read budget",
        ));
    }
    let data: ScopedFileRead =
        match host::scoped_file(path, ScopedFileAccess::ReadConfiguration { length }) {
            Ok(data) => data,
            Err(error) if matches!(error.code, ErrorCode::WorkspaceNotFound) => {
                let _: ScopedFileStat = host::scoped_file(String::new(), ScopedFileAccess::Stat)?;
                return Ok(json!({"text": null}));
            }
            Err(error) => return Err(error),
        };
    let invalid = || {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Invalid workspace configuration text",
        )
    };
    let bytes = BASE64_STANDARD.decode(data.data).map_err(|_| invalid())?;
    if data.is_truncated || bytes.len() > length {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "The workspace configuration exceeds its read budget",
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    Ok(json!({"text": text}))
}

pub(crate) fn normalize_path(path: &str) -> Result<String, CoreError> {
    if path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Expected a workspace-relative file path",
        ));
    }
    Ok(path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/"))
}

fn plugin_kind(kind: ScopedFileKind) -> &'static str {
    match kind {
        ScopedFileKind::File => "file",
        ScopedFileKind::Directory => "directory",
        ScopedFileKind::Symlink => "symbolicLink",
        ScopedFileKind::Other => "other",
    }
}

fn decode_cursor(input: &str, scope: &str) -> Result<Cursor, CoreError> {
    let invalid = || {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid workspace directory cursor",
        )
    };
    if input.len() > MAX_CURSOR_BYTES {
        return Err(invalid());
    }
    let bytes = BASE64_STANDARD.decode(input).map_err(|_| invalid())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if cursor.scope != scope || cursor.name.is_empty() || cursor.name.contains('/') {
        return Err(invalid());
    }
    Ok(cursor)
}

fn directory_page(
    entries: Vec<ScopedDirectoryEntry>,
    directory: &str,
    scope: String,
    after: Option<Cursor>,
    limit: Option<i64>,
    resource: impl Fn(&str) -> Value,
) -> Result<Value, CoreError> {
    let hidden = VisibilityRules::new(Vec::new(), Vec::new());
    let key = |directory: bool, name: &str| (!directory, FileNameKey::new(name), name.to_owned());
    let after = after.map(|cursor| key(cursor.directory, &cursor.name));
    let mut candidates = Vec::new();
    let mut path = directory.to_owned();
    if !path.is_empty() {
        path.push('/');
    }
    let prefix = path.len();
    for entry in entries {
        let is_directory = entry.kind == ScopedFileKind::Directory;
        path.truncate(prefix);
        path.push_str(&entry.name);
        if path.len() <= MAX_PATH_BYTES && !hidden.is_hidden(&path, is_directory) {
            let key = key(is_directory, &entry.name);
            if after.as_ref().is_none_or(|after| &key > after) {
                candidates.push((key, entry));
            }
        }
    }
    candidates.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let limit = limit.unwrap_or(200).clamp(1, 1000) as usize;
    let total = candidates.len();
    let mut entries = Vec::with_capacity(total.min(limit));
    let mut bytes = 0;
    let mut last = None;
    for (_, entry) in candidates.into_iter().take(limit) {
        path.truncate(prefix);
        path.push_str(&entry.name);
        let value = json!({
            "name": entry.name,
            "resource": resource(&path),
            "kind": plugin_kind(entry.kind),
        });
        bytes += serde_json::to_vec(&value)
            .expect("Directory entries always encode")
            .len()
            + 1;
        if bytes > MAX_DIRECTORY_PAGE_BYTES {
            break;
        }
        last = Some((entry.kind == ScopedFileKind::Directory, entry.name));
        entries.push(value);
    }
    let next_cursor = last
        .filter(|_| entries.len() < total)
        .map(|(directory, name)| {
            let cursor = Cursor {
                scope,
                directory,
                name,
            };
            BASE64_STANDARD
                .encode(serde_json::to_vec(&cursor).expect("Directory cursors always encode"))
        });
    Ok(json!({"entries": entries, "nextCursor": next_cursor}))
}

fn file_content(resource: Value, data: ScopedFileRead, limit: usize) -> Result<Value, CoreError> {
    let invalid = || CoreError::new(ErrorCode::ParseFailed, "Invalid scoped file contents");
    let bytes = BASE64_STANDARD.decode(&data.data).map_err(|_| invalid())?;
    if bytes.len() > limit || (data.is_truncated && bytes.len() != limit) {
        return Err(invalid());
    }
    let identity = (!data.is_truncated).then(|| encoding::bytes_identity(&bytes));
    let mut result = json!({
        "resource": resource,
        "source": "disk",
        "byteCount": bytes.len(),
        "isTruncated": data.is_truncated,
        "identity": identity,
    });
    let max_trim = if data.is_truncated {
        3.min(bytes.len().saturating_sub(1))
    } else {
        0
    };
    // Prefer UTF-8 when the prefix ends mid-character. Other encodings use
    // the same strict detection and plain-text policy as workspace search.
    let utf8_trim =
        (0..=max_trim).find(|trim| std::str::from_utf8(&bytes[..bytes.len() - trim]).is_ok());
    let mut trims = utf8_trim.map_or(0..=max_trim, |trim| trim..=trim);
    let decoded = trims.find_map(|trim| encoding::decode(&bytes[..bytes.len() - trim], None).ok());
    match decoded {
        Some(decoded) if is_plain_text(&decoded.text) => {
            result["text"] = json!(decoded.text);
            result["encoding"] = json!(decoded.encoding.id());
        }
        _ => result["base64"] = json!(data.data),
    }
    Ok(result)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SearchCursor {
    scope: String,
    emitted: usize,
    prefix_identity: String,
}

fn search_page(
    root_id: &str,
    scope: String,
    search: ScopedSearch,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<Value, CoreError> {
    let invalid = || CoreError::new(ErrorCode::InvalidRequest, "Invalid workspace search cursor");
    let cursor = cursor
        .map(|cursor| {
            if cursor.len() > MAX_CURSOR_BYTES {
                return Err(invalid());
            }
            let decoded = BASE64_STANDARD.decode(cursor).map_err(|_| invalid())?;
            let parsed: SearchCursor = serde_json::from_slice(&decoded).map_err(|_| invalid())?;
            if parsed.scope != scope || !(1..MAX_SCOPED_SEARCH_RESULTS).contains(&parsed.emitted) {
                return Err(invalid());
            }
            Ok(parsed)
        })
        .transpose()?;
    let text = search.text;
    let found = host::scoped_search(
        search,
        lithe_core_rules::visibility::plugin_hidden_directories(),
    )?;
    let identity = |count: usize| {
        encoding::bytes_identity(
            &serde_json::to_vec(&found.matches[..count]).expect("Search matches always encode"),
        )
    };
    let offset = cursor.as_ref().map_or(0, |cursor| cursor.emitted);
    if let Some(cursor) = cursor {
        if offset > found.matches.len() || identity(offset) != cursor.prefix_identity {
            return Err(invalid());
        }
    }
    let limit = limit
        .unwrap_or(100)
        .clamp(1, MAX_SCOPED_SEARCH_RESULTS as i64) as usize;
    let mut matches = Vec::new();
    let mut bytes = 0;
    for item in found.matches.iter().skip(offset).take(limit) {
        let path = normalize_path(&item.path)?;
        if path.is_empty() || path != item.path {
            return Err(CoreError::new(
                ErrorCode::ParseFailed,
                "Invalid workspace search result",
            ));
        }
        let resource = json!({"rootID": root_id, "path": path});
        let value = if text {
            let (Some(line), Some(preview)) = (item.line.filter(|line| *line > 0), &item.preview)
            else {
                return Err(CoreError::new(
                    ErrorCode::ParseFailed,
                    "Invalid workspace text match",
                ));
            };
            json!({"resource": resource, "line": line, "preview": preview, "range": null})
        } else {
            resource
        };
        bytes += serde_json::to_vec(&value)
            .expect("Search results always encode")
            .len()
            + 1;
        if bytes > MAX_DIRECTORY_PAGE_BYTES {
            break;
        }
        matches.push(value);
    }
    let emitted = offset + matches.len();
    let next_cursor = (emitted > offset && emitted < found.matches.len()).then(|| {
        let cursor = SearchCursor {
            scope,
            emitted,
            prefix_identity: identity(emitted),
        };
        BASE64_STANDARD.encode(serde_json::to_vec(&cursor).expect("Search cursors always encode"))
    });
    Ok(json!({
        "matches": matches,
        "nextCursor": next_cursor,
        "isTruncated": found.is_truncated,
        "source": "disk",
    }))
}
