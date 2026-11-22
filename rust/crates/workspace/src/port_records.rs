//! `remote.portRecords`, `remote.editPortRecords` and
//! `remote.restoreForwards`: what the ports page remembers of a remote
//! workspace after it closes or the connection drops (design §8, S27, S28).
//!
//! The records live in a machine-local document that the native client reads
//! and publishes atomically, next to the remote preferences. Core keys them
//! like workspace trust, by the open connection's target and the workspace's
//! real path on the host, so another host, account, container or folder never
//! sees them. After a reconnect, `remote.restoreForwards` starts again the
//! local forwards the user left running, each with the confirmation it was
//! given; forwarding rules on the host never start without the user. The
//! rules are in `lithe_remote::client::port_records`.

use crate::forwards::{forward_error, open};
use crate::sessions::workspace_key;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::port_records::{
    self, PortRecord, PortRecordError, PortRecords, Rule, WorkspaceRecords,
};
use lithe_remote::client::{ForwardError, ForwardSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `remote.portRecords` and `remote.restoreForwards`: one open workspace.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    /// Open connection the workspace was opened through.
    session_id: String,
    /// Absolute host path of the workspace root as opened.
    workspace: String,
}

/// `remote.editPortRecords`: exactly one of `port`, `removePort`, `rule` and
/// `removeRule`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    /// Open connection the workspace was opened through.
    session_id: String,
    /// Absolute host path of the workspace root as opened.
    workspace: String,
    /// Replaces what one row remembers; a row with nothing left is removed.
    #[serde(default)]
    port: Option<PortRecord>,
    /// Forgets one row.
    #[serde(default)]
    remove_port: Option<RowRequest>,
    /// Records a rule, or renames the same rule.
    #[serde(default)]
    rule: Option<Rule>,
    /// Forgets one rule, whatever its label.
    #[serde(default)]
    remove_rule: Option<Rule>,
}

/// One row: the port on the address it is listed under.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RowRequest {
    #[serde(default)]
    host: Option<String>,
    port: u16,
}

/// Where the native client keeps the document.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Storage {
    relative_path: &'static str,
    maximum_bytes: usize,
}

const STORAGE: Storage = Storage {
    relative_path: port_records::STORAGE_PATH,
    maximum_bytes: port_records::MAX_BYTES,
};

/// The change one edit makes.
enum Change {
    Port(PortRecord),
    RemovePort(RowRequest),
    Rule(Rule),
    RemoveRule(Rule),
}

/// Reads what the ports page remembers of one open workspace.
pub fn read(request: WorkspaceRequest) -> Result<Value, CoreError> {
    let records = parse(request.document.as_deref())?;
    let (target, real_path) = workspace_key(&request.session_id, &request.workspace)?;
    let workspace = records
        .records(&target, &real_path)
        .map_err(invalid_request)?;
    Ok(json!({ "records": workspace, "workspace": real_path, "storage": STORAGE }))
}

/// Applies one change. An edit that changes nothing returns a `null`
/// document, so the native client creates no source.
pub fn edit(request: EditRequest) -> Result<Value, CoreError> {
    let before = parse(request.document.as_deref())?;
    let change = match (
        request.port,
        request.remove_port,
        request.rule,
        request.remove_rule,
    ) {
        (Some(record), None, None, None) => Change::Port(record),
        (None, Some(row), None, None) => Change::RemovePort(row),
        (None, None, Some(rule), None) => Change::Rule(rule),
        (None, None, None, Some(rule)) => Change::RemoveRule(rule),
        _ => return Err(invalid_request(PortRecordError { field: "change" })),
    };
    let (target, real_path) = workspace_key(&request.session_id, &request.workspace)?;
    let mut after = before.clone();
    match change {
        Change::Port(record) => after.set_port(&target, &real_path, record),
        Change::RemovePort(row) => {
            after.remove_port(&target, &real_path, row.host.as_deref(), row.port)
        }
        Change::Rule(rule) => after.set_rule(&target, &real_path, rule),
        Change::RemoveRule(rule) => after.remove_rule(&target, &real_path, rule),
    }
    .map_err(invalid_request)?;
    let changed = after != before;
    let document = if changed {
        Some(after.text().map_err(invalid_request)?)
    } else {
        None
    };
    let records = after
        .records(&target, &real_path)
        .map_err(invalid_request)?;
    Ok(json!({ "changed": changed, "document": document, "records": records }))
}

/// One forward `remote.restoreForwards` started, or why it did not start.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Restored {
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    port: u16,
    #[serde(flatten)]
    outcome: Outcome,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Outcome {
    Started {
        forward: lithe_remote::client::ForwardInfo,
        created: bool,
    },
    Failed {
        error: CoreError,
    },
}

/// Starts again the local forwards the user left running in the workspace,
/// in row order. Each starts like `remote.startForward`, with the local port
/// the row records and the confirmation it was given; one that fails is
/// reported and the others still start. Rules are not started.
pub fn restore(request: WorkspaceRequest) -> Result<Value, CoreError> {
    let records = parse(request.document.as_deref())?;
    let (target, real_path) = workspace_key(&request.session_id, &request.workspace)?;
    let workspace: WorkspaceRecords = records
        .records(&target, &real_path)
        .map_err(invalid_request)?;
    let connection = open(&request.session_id)?;
    let mut forwards = Vec::new();
    for (record, target_host) in workspace.restorable() {
        let spec = ForwardSpec {
            target_host: target_host.to_owned(),
            target_port: record.port,
            local_port: Some(record.local_port.unwrap_or(record.port)),
            allow_non_loopback_target: record.confirmed_target,
        };
        let outcome = match connection.forwards.start(spec) {
            Ok((forward, created)) => Outcome::Started { forward, created },
            // Without a session no other forward can start either.
            Err(ForwardError::SessionClosed) => {
                return Err(forward_error(
                    &request.session_id,
                    ForwardError::SessionClosed,
                ))
            }
            Err(error) => Outcome::Failed {
                error: forward_error(&request.session_id, error),
            },
        };
        forwards.push(Restored {
            host: record.host.clone(),
            port: record.port,
            outcome,
        });
    }
    Ok(json!({ "forwards": forwards }))
}

/// A stored document that cannot be decoded is a parse failure, not a bad
/// request: the caller sent what the source holds.
fn parse(document: Option<&str>) -> Result<PortRecords, CoreError> {
    PortRecords::parse(document).map_err(|error| {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Invalid remote port records document",
        )
        .with_details(json!({ "field": error.field }).to_string())
    })
}

fn invalid_request(error: PortRecordError) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid remote port records request",
    )
    .with_details(json!({ "field": error.field }).to_string())
}
