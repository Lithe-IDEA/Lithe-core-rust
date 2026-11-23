//! Bounded disk reads for the plugin workspace file surface (plan API-06).
//!
//! Paths and limits are shared policy; only the no-follow handle operations
//! belong to lithe-platform. Explicit reads ignore visibility filters, and no
//! command consults an editor buffer. Directory cursors retain no server state.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use lithe_platform::scoped_read::{Directory, EntryMetadata, Kind};
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_FILE_BYTES: usize = 128 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const PAGE_BYTES: usize = 900 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResourceRequest {
    root: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectoryRequest {
    root: String,
    path: String,
    cursor: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    hidden_directory_names: Vec<String>,
    #[serde(default)]
    hidden_file_patterns: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileRequest {
    root: String,
    path: String,
    maximum_bytes: Option<usize>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    scope: String,
    after: String,
}

/// Builds the shared invalid-request refusal for workspace disk methods.
pub(crate) fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

/// Decodes strict command payloads without admitting unknown options.
pub(crate) fn decode<T: for<'de> Deserialize<'de>>(payload: Value) -> Result<T, CoreError> {
    serde_json::from_value(payload)
        .map_err(|error| invalid("Invalid workspace read request").with_details(error.to_string()))
}

/// Maps native I/O categories to stable Core workspace failures.
pub(crate) fn failure(error: io::Error) -> CoreError {
    let code = match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => ErrorCode::EntryNotFound,
        io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        io::ErrorKind::InvalidInput => ErrorCode::InvalidRequest,
        io::ErrorKind::TimedOut => ErrorCode::TimedOut,
        io::ErrorKind::Interrupted => ErrorCode::Cancelled,
        _ => ErrorCode::Unknown,
    };
    CoreError::new(code, "Could not read the workspace entry").with_details(error.to_string())
}

/// Checks cooperative cancellation and the bounded disk-operation deadline.
pub(crate) fn check(deadline: Instant) -> Result<(), CoreError> {
    lithe_protocol::cancellation::check()?;
    if Instant::now() >= deadline {
        return Err(CoreError::new(
            ErrorCode::TimedOut,
            "Workspace read deadline expired",
        ));
    }
    Ok(())
}

/// Requires one portable, normalized resource path under the authorized root.
pub(crate) fn validate_path(path: &str) -> Result<(), CoreError> {
    if path.len() > MAX_PATH_BYTES
        || path.contains('\\')
        || (!path.is_empty()
            && lithe_platform::paths::invalid_entry_path(path, lithe_platform::Platform::HOST))
    {
        return Err(invalid(
            "Path must use '/' and name an entry below its root",
        ));
    }
    Ok(())
}

/// Opens an absolute authorized root that anchors no-follow descendant reads.
pub(crate) fn root(value: &str) -> Result<Directory, CoreError> {
    if !Path::new(value).is_absolute() {
        return Err(invalid("Workspace root must be absolute"));
    }
    Directory::open_root(Path::new(value)).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CoreError::new(ErrorCode::WorkspaceNotFound, "Workspace does not exist")
        } else {
            failure(error)
        }
    })
}

/// Walks validated directory components through held no-follow handles.
pub(crate) fn directory(mut root: Directory, path: &str) -> Result<Directory, CoreError> {
    for component in path.split('/').filter(|part| !part.is_empty()) {
        lithe_protocol::cancellation::check()?;
        root = root.directory(OsStr::new(component)).map_err(failure)?;
    }
    Ok(root)
}

/// Opens the parent of a validated resource without following plugin links.
pub(crate) fn parent(root: Directory, path: &str) -> Result<(Directory, &str), CoreError> {
    let (parents, name) = path.rsplit_once('/').unwrap_or(("", path));
    Ok((directory(root, parents)?, name))
}

fn kind(value: Kind) -> &'static str {
    match value {
        Kind::File => "file",
        Kind::Directory => "directory",
        Kind::Symlink => "symlink",
        Kind::Other => "other",
    }
}

/// Reads a live directory page, ordered by UTF-8 name bytes, omitting links and
/// hidden entries. A cursor binds the root, directory and effective filters.
pub fn read_directory_command(payload: Value) -> Result<Value, CoreError> {
    let request: DirectoryRequest = decode(payload)?;
    validate_path(&request.path)?;
    let limit = request.limit.unwrap_or(200);
    if !(1..=1000).contains(&limit) {
        return Err(invalid("Directory limit must be 1-1000"));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    check(deadline)?;
    let root_handle = root(&request.root)?;
    let root_path = Path::new(&request.root).canonicalize().map_err(failure)?;
    let rules = crate::files::VisibilityRules::new(
        request.hidden_directory_names,
        request.hidden_file_patterns,
    );
    let scope = crate::encoding::bytes_identity(
        &serde_json::to_vec(&json!([
            root_path,
            request.path,
            rules.hidden_directories,
            rules.hidden_file_patterns
        ]))
        .expect("cursor scope encodes"),
    );
    let after = match request.cursor {
        None => String::new(),
        Some(cursor) => {
            if cursor.len() > 8192 {
                return Err(invalid("Invalid directory cursor"));
            }
            let cursor: Cursor = URL_SAFE_NO_PAD
                .decode(cursor)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .ok_or_else(|| invalid("Invalid directory cursor"))?;
            if cursor.scope != scope
                || cursor.after.is_empty()
                || cursor.after.contains('/')
                || cursor.after.contains('\\')
            {
                return Err(invalid("Directory cursor does not belong to this request"));
            }
            cursor.after
        }
    };
    let directory = directory(root_handle, &request.path)?;
    let mut candidates = BTreeMap::new();
    directory
        .visit(|name, info| {
            check(deadline).map_err(|error| {
                io::Error::new(
                    match error.code {
                        ErrorCode::Cancelled => io::ErrorKind::Interrupted,
                        _ => io::ErrorKind::TimedOut,
                    },
                    error.message,
                )
            })?;
            let Some(name) = name.to_str() else {
                return Ok(());
            };
            let path = if request.path.is_empty() {
                name.to_string()
            } else {
                format!("{}/{name}", request.path)
            };
            if name <= after.as_str()
                || info.kind == Kind::Symlink
                || validate_path(&path).is_err()
                || rules.is_hidden(&path, info.kind == Kind::Directory)
            {
                return Ok(());
            }
            candidates.insert(
                name.to_string(),
                json!({"name":name,"path":path,"kind":kind(info.kind)}),
            );
            if candidates.len() > limit + 1 {
                candidates.pop_last();
            }
            Ok(())
        })
        .map_err(failure)?;
    let mut entries = Vec::with_capacity(limit);
    let mut bytes = 0;
    let mut last = None;
    let mut has_more = false;
    for (name, entry) in candidates {
        // Reserve each resource's root ID and JSON envelope added by the host.
        let size = serde_json::to_vec(&entry).expect("entry encodes").len() + 256;
        if entries.len() == limit || bytes + size > PAGE_BYTES {
            has_more = true;
            break;
        }
        bytes += size;
        last = Some(name);
        entries.push(entry);
    }
    let cursor = if has_more {
        Some(
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&Cursor {
                    scope,
                    after: last.expect("one legal entry fits in the page budget"),
                })
                .expect("cursor encodes"),
            ),
        )
    } else {
        None
    };
    check(deadline)?;
    Ok(json!({"entries":entries,"nextCursor":cursor}))
}

/// Stats the leaf itself, allowing a link only as the final component.
pub fn stat_command(payload: Value) -> Result<Value, CoreError> {
    let request: ResourceRequest = decode(payload)?;
    validate_path(&request.path)?;
    lithe_protocol::cancellation::check()?;
    let root = root(&request.root)?;
    let info: EntryMetadata = if request.path.is_empty() {
        root.metadata().map_err(failure)?
    } else {
        let (parent, name) = parent(root, &request.path)?;
        parent.child_metadata(OsStr::new(name)).map_err(failure)?
    };
    Ok(
        json!({"path":request.path,"kind":kind(info.kind),"size":info.size,"modifiedMilliseconds":info.modified_milliseconds}),
    )
}

/// Returns at most 128 KiB of disk bytes. Strict UTF-8 without binary controls
/// is text; every other prefix is base64, including an incomplete UTF-8 tail.
pub fn read_file_command(payload: Value) -> Result<Value, CoreError> {
    let request: FileRequest = decode(payload)?;
    validate_path(&request.path)?;
    if request.path.is_empty() {
        return Err(invalid("A file path cannot name the root"));
    }
    let maximum = request.maximum_bytes.unwrap_or(MAX_FILE_BYTES);
    if !(1..=MAX_FILE_BYTES).contains(&maximum) {
        return Err(invalid("maximumBytes must be 1-131072"));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    check(deadline)?;
    let (parent, name) = parent(root(&request.root)?, &request.path)?;
    let mut file = parent
        .file(OsStr::new(name))
        .map_err(failure)?
        .take((maximum + 1) as u64);
    let mut bytes = Vec::with_capacity(maximum + 1);
    let mut buffer = [0u8; 8192];
    loop {
        check(deadline)?;
        let count = file.read(&mut buffer).map_err(failure)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let truncated = bytes.len() > maximum;
    bytes.truncate(maximum);
    let mut result = json!({"path":request.path,"source":"disk","byteCount":bytes.len(),"isTruncated":truncated,
        "identity":{"sha256":crate::encoding::bytes_identity(&bytes),"byteCount":bytes.len(),"isTruncated":truncated}});
    match std::str::from_utf8(&bytes).ok().filter(|text| {
        !text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\r' | '\n'))
    }) {
        Some(text) => {
            result["text"] = json!(text);
            result["encoding"] = json!("utf-8");
        }
        None => {
            result["base64"] = json!(base64::engine::general_purpose::STANDARD.encode(&bytes));
        }
    }
    Ok(result)
}
