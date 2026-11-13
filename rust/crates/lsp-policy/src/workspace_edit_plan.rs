//! Target validation and commit order for language-server workspace edits
//! (`lsp.planWorkspaceEdit`).
//!
//! A rename or code action may touch many files. Core decides, before anything
//! is read or written, whether every target is a workspace file that may be
//! edited, which targets are open editor documents and which are closed files,
//! and the order in which the host commits them. The host owns reading,
//! encoding-preserving writes, editor buffers, stale-input checks and the
//! rollback the plan prescribes.

use crate::workspace_scope::{comparable, WorkspaceScope};
use lithe_document_uri::{native_path, DocumentPathPlatform};
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Map, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenDocument {
    path: String,
    #[serde(default)]
    is_read_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    platform: DocumentPathPlatform,
    workspace_root: String,
    /// Text edits per target, keyed by a file URI or an absolute native path,
    /// as `lsp.request` projects `WorkspaceEdit.changes`/`documentChanges`.
    changes: Map<String, Value>,
    /// Open, non-virtual editor documents of this workspace.
    #[serde(default)]
    open_documents: Vec<OpenDocument>,
}

struct Target {
    /// Comparable URI identity used to merge keys naming one file.
    identity: String,
    path: String,
    relative_path: String,
    open_document: bool,
    edits: Vec<Value>,
}

/// Plans one workspace edit.
///
/// Returns `{ rejection, targets }`. On success `rejection` is null and
/// `targets` lists every target with edits in commit order: first the closed
/// files (`effect: "writeFile"`), then the open documents
/// (`effect: "updateDocument"`), each group ordered by `relativePath`. The host
/// computes every new text before committing, writes the files in order, and
/// when a write fails restores the files already written in reverse order and
/// updates no document. Open targets report the host's own document `path`.
/// Keys that name the same file are merged in key order. A batch with any
/// target outside the workspace (including non-file URIs) or any read-only
/// open document returns `rejection: { code: "outsideWorkspace" |
/// "readOnlyDocument", path }` and no targets. Unparseable native paths return
/// `invalid_request`.
pub fn plan_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid workspace edit plan request",
        )
        .with_details(error.to_string())
    })?;
    let scope = WorkspaceScope::new(&request.workspace_root, request.platform)?;
    let reject = |code: &str, path: &str| {
        Ok(json!({"rejection": {"code": code, "path": path}, "targets": []}))
    };
    let mut targets: Vec<Target> = Vec::new();
    // serde_json maps iterate in key order, which keeps rejection and merge
    // order deterministic for the same edit.
    for (key, edits) in &request.changes {
        let Some(path) = native_path(key) else {
            return reject("outsideWorkspace", key);
        };
        let Some(relative_path) = scope.relative_path(&path)? else {
            return reject("outsideWorkspace", &path);
        };
        let identity = comparable(&path, request.platform)?;
        let mut open = None;
        for document in &request.open_documents {
            if comparable(&document.path, request.platform)? == identity {
                open = Some(document);
                break;
            }
        }
        if open.is_some_and(|document| document.is_read_only) {
            return reject("readOnlyDocument", &path);
        }
        let edits = edits.as_array().cloned().unwrap_or_default();
        if let Some(existing) = targets
            .iter_mut()
            .find(|target| target.identity == identity)
        {
            existing.edits.extend(edits);
            continue;
        }
        targets.push(Target {
            identity,
            path: open.map_or(path, |document| document.path.clone()),
            relative_path,
            open_document: open.is_some(),
            edits,
        });
    }
    targets.retain(|target| !target.edits.is_empty());
    targets.sort_by(|left, right| {
        (left.open_document, &left.relative_path).cmp(&(right.open_document, &right.relative_path))
    });
    Ok(json!({
        "rejection": null,
        "targets": targets.into_iter().map(|target| json!({
            "path": target.path,
            "relativePath": target.relative_path,
            "effect": if target.open_document { "updateDocument" } else { "writeFile" },
            "edits": target.edits,
        })).collect::<Vec<_>>(),
    }))
}
