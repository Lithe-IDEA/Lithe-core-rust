//! `remote.connections` and `remote.editConnections`: the SSH connections the
//! user saved for remote development (design §3.1, S08 to S10, S12, S15).
//!
//! The list lives in a machine-local document that the native client reads
//! and publishes atomically, like the trust decisions. Core validates every
//! entry, picks ids and applies one edit at a time; it never stores a secret.
//! The rules are in `lithe_remote::client::connections`.

use crate::sessions::nonce;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::connections::{
    self, ConnectionError, Connections, Reason, SshConnection,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `remote.connections`: the saved connections.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
}

/// `remote.editConnections`: exactly one of `save`, `remove` and
/// `rememberFolder`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    #[serde(default)]
    save: Option<SaveRequest>,
    #[serde(default)]
    remove: Option<IdRequest>,
    #[serde(default)]
    remember_folder: Option<FolderRequest>,
}

/// Adds a connection, or replaces the entry `id`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveRequest {
    #[serde(default)]
    id: Option<String>,
    name: String,
    connection: SshConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdRequest {
    id: String,
}

/// A host folder opened over the entry `id`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FolderRequest {
    id: String,
    path: String,
}

/// Where the native client keeps the document.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Storage {
    relative_path: &'static str,
    maximum_bytes: usize,
}

const STORAGE: Storage = Storage {
    relative_path: connections::STORAGE_PATH,
    maximum_bytes: connections::MAX_BYTES,
};

/// Reads the document.
pub fn list(request: ListRequest) -> Result<Value, CoreError> {
    let connections = parse(request.document.as_deref())?;
    Ok(json!({ "connections": connections.entries(), "storage": STORAGE }))
}

/// Applies one edit. An edit that changes nothing returns a `null`
/// document, so the native client creates no source; `id` names the entry a
/// save wrote.
pub fn edit(request: EditRequest) -> Result<Value, CoreError> {
    let before = parse(request.document.as_deref())?;
    let mut after = before.clone();
    let mut id = None;
    match (request.save, request.remove, request.remember_folder) {
        (Some(save), None, None) => {
            id = Some(
                after
                    .save(save.id.as_deref(), &save.name, save.connection, nonce)
                    .map_err(invalid_request)?,
            );
        }
        (None, Some(remove), None) => after.remove(&remove.id),
        (None, None, Some(folder)) => after
            .remember_folder(&folder.id, &folder.path)
            .map_err(invalid_request)?,
        _ => {
            return Err(invalid_request(ConnectionError {
                field: "operation",
                reason: Reason::Invalid,
            }))
        }
    }
    let changed = after != before;
    let document = if changed {
        Some(after.text().map_err(invalid_request)?)
    } else {
        None
    };
    let mut result = json!({
        "changed": changed,
        "document": document,
        "connections": after.entries(),
    });
    if let Some(id) = id {
        result["id"] = json!(id);
    }
    Ok(result)
}

/// A stored document that cannot be decoded is a parse failure, not a bad
/// request: the caller sent what the source holds.
fn parse(document: Option<&str>) -> Result<Connections, CoreError> {
    Connections::parse(document).map_err(|error| {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Invalid remote connections document",
        )
        .with_details(json!(error).to_string())
    })
}

/// A refused connection or edit, with the field and the reason as details.
pub(crate) fn invalid_request(error: ConnectionError) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote connection")
        .with_details(json!(error).to_string())
}

#[cfg(test)]
mod tests;
