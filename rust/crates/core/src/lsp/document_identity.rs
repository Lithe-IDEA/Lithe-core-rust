//! Lexical workspace identities for diagnostics; the host owns native file I/O.

use crate::lsp::{
    workspace_edit_plan::native_path, workspace_scope::WorkspaceScope, DocumentPathPlatform,
    MAX_PATH_BYTES,
};
use crate::protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    workspace_root: String,
    /// File URI, absolute native path, or workspace-relative document path.
    input: String,
    platform: DocumentPathPlatform,
}

/// Resolves one server-supplied identity to a native path and workspace-relative
/// display path. Non-file URIs, ambiguous drive-relative paths and outside targets
/// return a null document. The result grants no permission to follow symlinks.
pub(crate) fn resolve_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid document identity request",
        )
        .with_details(error.to_string())
    })?;
    let platform = if request.platform.is_windows_path(&request.workspace_root) {
        DocumentPathPlatform::Windows
    } else {
        DocumentPathPlatform::Posix
    };
    let scope = WorkspaceScope::new(&request.workspace_root, platform)?;
    let windows = matches!(platform, DocumentPathPlatform::Windows);
    let missing = || json!({"document": null});
    if request.input.is_empty()
        || request.input.len() > MAX_PATH_BYTES
        || request.input.contains('\0')
    {
        return Ok(missing());
    }
    // URL parsing handles localhost, drive authorities and escaped components;
    // reuse workspace-edit path decoding instead of maintaining another decoder.
    let normalized;
    let input = if request
        .input
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        let Ok(url) = Url::parse(&request.input) else {
            return Ok(missing());
        };
        if (!windows && url.host_str().is_some())
            || !url.username().is_empty()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Ok(missing());
        }
        normalized = url.to_string();
        normalized.as_str()
    } else {
        &request.input
    };
    let Some(mut path) = native_path(input) else {
        return Ok(missing());
    };
    if windows {
        path = path.replace('\\', "/");
        let bytes = path.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'/';
        if !drive && !path.starts_with("//") {
            if path.starts_with('/') || path.contains(':') {
                return Ok(missing());
            }
            path = format!(
                "{}/{}",
                request.workspace_root.trim_end_matches(['/', '\\']),
                path
            );
        }
    } else if !path.starts_with('/') {
        path = format!("{}/{}", request.workspace_root.trim_end_matches('/'), path);
    }
    let Ok(uri) = platform.document_uri(&path) else {
        return Ok(missing());
    };
    let Some(relative) = scope.relative_path(&path)? else {
        return Ok(missing());
    };
    let Some(path) = native_path(&uri) else {
        return Ok(missing());
    };
    let path = if windows {
        path.replace('/', "\\")
    } else {
        path
    };
    Ok(json!({"document": {"path": path, "relativePath": relative}}))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    #[test]
    fn matches_shared_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/lsp/document-identity-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let response: Value = serde_json::from_str(&crate::execute_json(
                &json!({"command":"lsp.documentIdentity", "payload":case["request"]}).to_string(),
            ))
            .unwrap();
            if let Some(code) = case.get("error") {
                assert_eq!(&response["error"]["code"], code, "{case}");
            } else {
                assert_eq!(response["data"], case["expected"], "{case}: {response}");
            }
        }
    }
}
