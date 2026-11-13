//! Platform-neutral conversion of absolute filesystem paths to LSP file URIs.

use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

/// Largest UTF-8 spelling of a Windows extended-length path (32,767 UTF-16
/// units); shared by native path and server document identity validation.
pub const MAX_PATH_BYTES: usize = 4 * 32_767;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    path: String,
    platform: DocumentPathPlatform,
}

/// Path syntax of a native absolute path; `auto` recognizes drive and UNC paths.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentPathPlatform {
    /// POSIX syntax: the path must start with `/`.
    Posix,
    /// Windows syntax: drive letters, UNC shares and extended-length prefixes.
    Windows,
    /// Windows when the path names a drive or a UNC share, POSIX otherwise.
    Auto,
}

/// `lsp.documentUri`: the canonical file URI of `path` under `platform`.
pub fn document_uri_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|_| invalid())?;
    Ok(json!({"uri": document_uri(&request.path, request.platform)?}))
}

impl DocumentPathPlatform {
    /// Canonical file URI of an absolute native path; see `lsp.documentUri`.
    pub fn document_uri(self, path: &str) -> Result<String, CoreError> {
        document_uri(path, self)
    }

    /// True when `path` uses Windows syntax: drive or UNC, or `windows` explicitly.
    pub fn is_windows_path(self, path: &str) -> bool {
        match self {
            DocumentPathPlatform::Windows => true,
            DocumentPathPlatform::Posix => false,
            DocumentPathPlatform::Auto => {
                path.starts_with("\\\\")
                    || path.starts_with("//")
                    || path.as_bytes().get(1) == Some(&b':')
            }
        }
    }
}

fn document_uri(path: &str, platform: DocumentPathPlatform) -> Result<String, CoreError> {
    if path.len() > MAX_PATH_BYTES || path.contains('\0') {
        return Err(invalid());
    }
    let platform = match platform {
        DocumentPathPlatform::Auto
            if path.starts_with("\\\\")
                || path.starts_with("//")
                || (path.as_bytes().get(1) == Some(&b':')) =>
        {
            DocumentPathPlatform::Windows
        }
        DocumentPathPlatform::Auto => DocumentPathPlatform::Posix,
        platform => platform,
    };
    let path = match platform {
        DocumentPathPlatform::Posix => {
            if !path.starts_with('/') {
                return Err(invalid());
            }
            path.to_owned()
        }
        DocumentPathPlatform::Windows => {
            let path = path.replace('\\', "/");
            if let Some(rest) = path.strip_prefix("//?/UNC/") {
                format!("//{rest}")
            } else if let Some(rest) = path.strip_prefix("//?/") {
                rest.to_owned()
            } else {
                path
            }
        }
        DocumentPathPlatform::Auto => unreachable!("auto platform is resolved above"),
    };
    let mut url = Url::parse("file:///").expect("constant file URI must parse");
    let rest = match platform {
        DocumentPathPlatform::Posix => &path[1..],
        DocumentPathPlatform::Windows if path.starts_with("//") => {
            let (host, rest) = path[2..].split_once('/').ok_or_else(invalid)?;
            if host.is_empty() || rest.is_empty() {
                return Err(invalid());
            }
            url.set_host(Some(host)).map_err(|_| invalid())?;
            rest
        }
        DocumentPathPlatform::Windows => {
            let bytes = path.as_bytes();
            if bytes.len() < 3
                || !bytes[0].is_ascii_alphabetic()
                || bytes[1] != b':'
                || bytes[2] != b'/'
            {
                return Err(invalid());
            }
            path.as_str()
        }
        DocumentPathPlatform::Auto => unreachable!("auto platform is resolved above"),
    };
    // url::PathSegmentsMut ignores `..` rather than removing the preceding
    // component. Resolve dot segments before inserting and keep drive/share
    // roots intact. Empty trailing components retain a directory separator.
    let root_components = usize::from(matches!(platform, DocumentPathPlatform::Windows));
    let mut normalized = Vec::new();
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if normalized.len() > root_components {
                    normalized.pop();
                }
            }
            segment => normalized.push(segment),
        }
    }
    let trailing_slash = rest.ends_with('/') || rest.ends_with("/.") || rest.ends_with("/..");
    {
        let mut segments = url.path_segments_mut().map_err(|_| invalid())?;
        segments.clear();
        for segment in normalized {
            segments.push(segment);
        }
        if trailing_slash || rest.is_empty() {
            segments.push("");
        }
    }
    Ok(url.into())
}

/// Accept a native path alongside existing string URIs without another IPC
/// round trip. Only URI fields of named LSP operations are interpreted; opaque
/// server payloads and virtual URIs remain untouched.
pub fn resolve_request_paths(command: &str, payload: &mut Value) -> Result<(), CoreError> {
    match command {
        "lsp.startServer" => resolve_field(payload, "rootUri"),
        "lsp.request" => {
            resolve_field(payload, "uri")?;
            if let Some(diagnostics) = payload.get_mut("diagnostics").and_then(Value::as_array_mut)
            {
                for diagnostic in diagnostics {
                    if let Some(related) = diagnostic
                        .get_mut("relatedInformation")
                        .and_then(Value::as_array_mut)
                    {
                        for item in related {
                            if let Some(location) = item.get_mut("location") {
                                resolve_field(location, "uri")?;
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        "lsp.syncDocument" | "lsp.saveDocument" | "lsp.closeDocument" => {
            resolve_field(payload, "uri")
        }
        "lsp.workspaceFilesChanged" => {
            if let Some(changes) = payload.get_mut("changes").and_then(Value::as_array_mut) {
                for change in changes {
                    resolve_field(change, "uri")?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Resolve a named URI field at a command-specific boundary.
fn resolve_field(parent: &mut Value, key: &str) -> Result<(), CoreError> {
    let Some(value) = parent.get_mut(key) else {
        return Ok(());
    };
    if value.is_object() {
        let request: Request = serde_json::from_value(value.take()).map_err(|_| invalid())?;
        *value = Value::String(document_uri(&request.path, request.platform)?);
    }
    Ok(())
}

fn invalid() -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Invalid absolute document path")
}
