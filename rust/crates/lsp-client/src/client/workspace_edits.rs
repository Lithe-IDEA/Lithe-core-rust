//! Validation of versioned LSP workspace edits before native application.

use super::results::truncate_utf8_bytes;
use super::*;
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_APPLY_EDIT_LABEL_BYTES: usize = 256;

/// Validates and normalizes server-initiated `workspace/applyEdit` parameters.
///
/// Outer `Err` rejects malformed JSON-RPC parameters (`-32602`). Inner `Err`
/// reports a valid `ApplyWorkspaceEditParams` that carries unsupported resource
/// operations or non-file URIs so the client replies `{ applied: false, failureReason }`.
pub(super) fn parse_workspace_apply_edit(
    params: Option<&Value>,
    open_documents: &BTreeMap<String, LspClientDocument>,
) -> Result<Result<Value, &'static str>, &'static str> {
    let params = params
        .and_then(Value::as_object)
        .ok_or("Invalid workspace/applyEdit parameters")?;
    let edit = params
        .get("edit")
        .filter(|edit| edit.is_object())
        .ok_or("workspace/applyEdit requires an edit object")?;
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(|label| truncate_utf8_bytes(label, MAX_APPLY_EDIT_LABEL_BYTES));

    Ok(WorkspaceEdit::parse(edit, open_documents).map(|edit| {
        json!({
            "label": label,
            "changes": edit.changes,
            "documentVersions": edit.document_versions
        })
    }))
}

const MAX_WORKSPACE_EDIT_FILES: usize = 256;
const MAX_WORKSPACE_EDIT_EDITS: usize = 4096;
const MAX_WORKSPACE_EDIT_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_WORKSPACE_EDIT_URI_BYTES: usize = 4096;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkspaceEdit {
    changes: BTreeMap<String, Vec<Value>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    document_versions: BTreeMap<String, i64>,
    #[serde(skip)]
    edit_count: usize,
    #[serde(skip)]
    text_bytes: usize,
}

impl WorkspaceEdit {
    pub(super) fn parse(
        edit: &Value,
        open_documents: &BTreeMap<String, LspClientDocument>,
    ) -> Result<Self, &'static str> {
        if !edit.is_object() {
            return Err("Workspace edit must be an object.");
        }
        let mut result = Self::default();
        // The versioned form takes precedence when both representations arrive.
        // Applying both would duplicate edits and drop their ordering semantics.
        if let Some(changes) = edit.get("documentChanges") {
            let changes = changes
                .as_array()
                .ok_or("Workspace documentChanges must be an array.")?;
            for change in changes {
                if !change.is_object() {
                    return Err("Workspace documentChange entry must be an object.");
                }
                if change.get("kind").is_some() {
                    return Err(
                        "Resource create, rename, and delete operations are not supported.",
                    );
                }
                let document = change
                    .get("textDocument")
                    .and_then(Value::as_object)
                    .ok_or("Workspace documentChange requires a textDocument.")?;
                let uri = document
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or("Workspace documentChange requires a textDocument URI.")?;
                let edits = change
                    .get("edits")
                    .and_then(Value::as_array)
                    .ok_or("Workspace documentChange requires an edits array.")?;
                let version = document
                    .get("version")
                    .filter(|version| !version.is_null())
                    .map(|version| {
                        version
                            .as_i64()
                            .ok_or("Workspace edit has an invalid document version.")
                    })
                    .transpose()?;
                result.insert(uri, version, edits, open_documents)?;
            }
        } else if let Some(changes) = edit.get("changes") {
            let changes = changes
                .as_object()
                .ok_or("Workspace edit changes must be an object.")?;
            for (uri, edits) in changes {
                let edits = edits
                    .as_array()
                    .ok_or("Workspace text edits must be an array.")?;
                result.insert(uri, None, edits, open_documents)?;
            }
        }
        Ok(result)
    }

    fn insert(
        &mut self,
        uri: &str,
        version: Option<i64>,
        edits: &[Value],
        open_documents: &BTreeMap<String, LspClientDocument>,
    ) -> Result<(), &'static str> {
        if self.changes.len() >= MAX_WORKSPACE_EDIT_FILES
            || edits.len() > MAX_WORKSPACE_EDIT_EDITS - self.edit_count
        {
            return Err("Workspace edit exceeds the file or text edit limit.");
        }
        if uri.len() > MAX_WORKSPACE_EDIT_URI_BYTES {
            return Err("Workspace edit target URI is too long.");
        }
        let path = file_path_for_uri(uri)
            .filter(|path| !path.contains('\0'))
            .ok_or("Workspace edit targets a non-file or invalid file URI.")?;
        let path = if let Some(version) = version {
            let document =
                super::server_messages::open_document_for_server_uri(open_documents, uri)
                    .filter(|document| document.version > 0 && document.version == version)
                    .ok_or("Workspace edit targets an unknown or stale document version.")?;
            // Use the identity Core already returned to the host, including
            // Windows drive spelling if the language server normalized it.
            let path =
                file_path_for_uri(&document.uri).ok_or("Workspace edit targets a non-file URI.")?;
            self.document_versions.insert(path.clone(), version);
            path
        } else {
            path
        };
        if self.changes.contains_key(&path) {
            return Err("Workspace edit repeats a document target.");
        }
        let mut parsed = Vec::with_capacity(edits.len());
        for item in edits {
            parsed.push(parse_bounded_text_edit(
                item,
                &mut self.text_bytes,
                MAX_WORKSPACE_EDIT_TEXT_BYTES,
            )?);
        }
        self.edit_count += parsed.len();
        self.changes.insert(path, parsed);
        Ok(())
    }
}

pub(super) fn parse_bounded_text_edit(
    item: &Value,
    text_bytes: &mut usize,
    max_bytes: usize,
) -> Result<Value, &'static str> {
    if item.get("snippet").is_some() || item.get("annotationId").is_some() {
        return Err("Snippet and annotated text edits are not supported.");
    }
    let new_text = item
        .get("newText")
        .and_then(Value::as_str)
        .ok_or("Text edit is missing replacement text.")?;
    *text_bytes = text_bytes
        .checked_add(new_text.len())
        .filter(|bytes| *bytes <= max_bytes)
        .ok_or("Edits exceed the replacement text limit.")?;
    let range = item.get("range").ok_or("Text edit is missing a range.")?;
    let start = text_edit_position(range, "start")?;
    let end = text_edit_position(range, "end")?;
    if start > end {
        return Err("Text edit range is reversed.");
    }
    Ok(json!({
        "range": {
            "start": {"line": start.0, "utf16Column": start.1},
            "end": {"line": end.0, "utf16Column": end.1}
        },
        "newText": new_text
    }))
}

fn text_edit_position(range: &Value, end: &str) -> Result<(i32, i32), &'static str> {
    let point = range.get(end);
    let coordinate = |name| {
        point.and_then(|point| {
            let value = point.get(name)?.as_i64()?;
            i32::try_from(value).ok().filter(|value| *value >= 0)
        })
    };
    coordinate("line")
        .zip(coordinate("character"))
        .ok_or("Text edit contains an invalid position.")
}
