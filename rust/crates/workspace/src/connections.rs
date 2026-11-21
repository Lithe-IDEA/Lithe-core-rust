//! `remote.connections`, `remote.editConnections` and `remote.parseAddress`:
//! the SSH connections the user saved for remote development (design §3.1,
//! S08 to S10, S12, S15).
//!
//! The list lives in a machine-local document that the native client reads
//! and publishes atomically, like the trust decisions. Core validates every
//! entry, picks ids and applies one edit at a time; it never stores a secret.
//! The rules are in `lithe_remote::client::connections`.

use crate::sessions::nonce;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::connections::{
    self, Color, ConnectionError, Connections, ImportEntry, Label, LabelFill, Reason, SshConnection,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `remote.connections`: the saved connections.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    /// Words every listed entry's name, address or group contains,
    /// regardless of case. `connections` still holds every entry.
    #[serde(default)]
    query: Option<String>,
}

/// `remote.editConnections`: exactly one of `save`, `remove`,
/// `rememberFolder`, `import` and `markConnected`.
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
    #[serde(default)]
    import: Option<ImportRequest>,
    #[serde(default)]
    mark_connected: Option<IdRequest>,
}

/// `remote.parseAddress`: a quick-connect address.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddressRequest {
    text: String,
}

/// Adds a connection, or replaces the entry `id`. A missing `color` or
/// `group` saves the entry without one.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveRequest {
    #[serde(default)]
    id: Option<String>,
    name: String,
    connection: SshConnection,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    group: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdRequest {
    id: String,
}

/// Entries to add, and color and group for entries already saved.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportRequest {
    #[serde(default)]
    entries: Vec<ImportEntry>,
    #[serde(default)]
    fills: Vec<LabelFill>,
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

/// Reads the document. `sections` lists the entry ids by group, in the
/// order the window shows them.
pub fn list(request: ListRequest) -> Result<Value, CoreError> {
    let connections = parse(request.document.as_deref())?;
    let sections = match request.query.as_deref() {
        Some(query) => connections
            .sections_matching(query)
            .map_err(invalid_request)?,
        None => connections.sections(),
    };
    Ok(json!({
        "connections": connections.entries(),
        "sections": sections,
        "storage": STORAGE,
    }))
}

/// Splits `[user@]host[:port]` into connection fields, or names the field
/// to correct.
pub fn parse_address(request: AddressRequest) -> Result<Value, CoreError> {
    connections::parse_address(&request.text)
        .map(|address| json!(address))
        .map_err(invalid_request)
}

/// Applies one edit. An edit that changes nothing returns a `null`
/// document, so the native client creates no source; `id` names the entry a
/// save wrote, `ids` the entries an import added.
pub fn edit(request: EditRequest) -> Result<Value, CoreError> {
    let before = parse(request.document.as_deref())?;
    let mut after = before.clone();
    let mut id = None;
    let mut ids = None;
    let operations = [
        request.save.is_some(),
        request.remove.is_some(),
        request.remember_folder.is_some(),
        request.import.is_some(),
        request.mark_connected.is_some(),
    ];
    if operations.iter().filter(|operation| **operation).count() != 1 {
        return Err(invalid_request(ConnectionError {
            field: "operation",
            reason: Reason::Invalid,
        }));
    }
    if let Some(save) = request.save {
        let label = Label {
            color: save.color,
            group: save.group,
        };
        id = Some(
            after
                .save(
                    save.id.as_deref(),
                    &save.name,
                    save.connection,
                    label,
                    nonce,
                )
                .map_err(invalid_request)?,
        );
    } else if let Some(remove) = request.remove {
        after.remove(&remove.id);
    } else if let Some(folder) = request.remember_folder {
        after
            .remember_folder(&folder.id, &folder.path)
            .map_err(invalid_request)?;
    } else if let Some(import) = request.import {
        ids = Some(
            after
                .import(import.entries, import.fills, nonce)
                .map_err(invalid_request)?,
        );
    } else if let Some(entry) = request.mark_connected {
        after
            .mark_connected(&entry.id, now_seconds())
            .map_err(invalid_request)?;
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
        "sections": after.sections(),
    });
    if let Some(id) = id {
        result["id"] = json!(id);
    }
    if let Some(ids) = ids {
        result["ids"] = json!(ids);
    }
    Ok(result)
}

/// Seconds since the Unix epoch; 0, which the edit refuses, when the clock
/// is before it.
fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// A stored document that cannot be decoded is a parse failure, not a bad
/// request: the caller sent what the source holds.
pub(crate) fn parse(document: Option<&str>) -> Result<Connections, CoreError> {
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
