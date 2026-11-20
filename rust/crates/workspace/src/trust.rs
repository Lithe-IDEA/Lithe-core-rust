//! `remote.workspaceTrust` and `remote.editWorkspaceTrust`: whether a remote
//! workspace may run project code (design S01 to S04).
//!
//! Decisions live in a machine-local document that the native client reads
//! and publishes atomically, as for local workspaces. Core resolves every key
//! over the open connection, its target and the workspace's real path on the
//! host, so a symbolic link or a spelling the host resolves elsewhere never
//! reaches a decision. The rules are in `lithe_remote::client::trust`.

use crate::sessions::{lookup, request_error};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::trust::{self, Decision, Decisions, List, Target, TrustError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// `remote.workspaceTrust`: the decisions, and the trust of one workspace
/// when `sessionId` and `workspace` are both given.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    /// Open connection the workspace was opened through.
    #[serde(default)]
    session_id: Option<String>,
    /// Absolute host path of the workspace root as opened.
    #[serde(default)]
    workspace: Option<String>,
}

/// `remote.editWorkspaceTrust`: exactly one of `decision` and `revoke`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    /// Records a decision on an open workspace.
    #[serde(default)]
    decision: Option<DecisionRequest>,
    /// Removes one entry, also for a target that is not connected.
    #[serde(default)]
    revoke: Option<RevokeRequest>,
}

/// One decision on the workspace at `workspace` over `sessionId`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecisionRequest {
    session_id: String,
    workspace: String,
    trusted: bool,
    #[serde(default)]
    location: bool,
    #[serde(default)]
    label: Option<String>,
}

/// One entry of the document to remove, as `remote.workspaceTrust` lists it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevokeRequest {
    target: Target,
    list: List,
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
    relative_path: trust::STORAGE_PATH,
    maximum_bytes: trust::MAX_BYTES,
};

/// Reads the document and, when asked, the trust of one open workspace.
pub fn state(request: StateRequest) -> Result<Value, CoreError> {
    let decisions = parse(request.document.as_deref())?;
    let mut result = json!({ "document": decisions, "storage": STORAGE });
    match (request.session_id, request.workspace) {
        (None, None) => {}
        (Some(session_id), Some(workspace)) => {
            let (target, real_path) = resolve(&session_id, &workspace)?;
            let workspace_trust = decisions
                .workspace(&target, &real_path)
                .map_err(invalid_request)?;
            result["projectCodeAllowed"] =
                json!(workspace_trust.state == trust::TrustState::Trusted);
            result["workspaceTrust"] = json!(workspace_trust);
            result["workspaceTrust"]["target"] = json!(target);
        }
        _ => return Err(invalid_request(TrustError { field: "workspace" })),
    }
    Ok(result)
}

/// Applies one decision or removal. An edit that changes nothing returns a
/// `null` document, so the native client creates no source.
pub fn edit(request: EditRequest) -> Result<Value, CoreError> {
    let before = parse(request.document.as_deref())?;
    let mut after = before.clone();
    match (request.decision, request.revoke) {
        (Some(decision), None) => {
            let (target, real_path) = resolve(&decision.session_id, &decision.workspace)?;
            after
                .decide(
                    &target,
                    &real_path,
                    Decision {
                        trusted: decision.trusted,
                        location: decision.location,
                        label: decision.label.as_deref(),
                    },
                )
                .map_err(invalid_request)?;
        }
        (None, Some(revoke)) => after
            .revoke(&revoke.target, revoke.list, &revoke.path)
            .map_err(invalid_request)?,
        _ => return Err(invalid_request(TrustError { field: "decision" })),
    }
    let changed = after != before;
    let document = if changed {
        Some(after.text().map_err(invalid_request)?)
    } else {
        None
    };
    Ok(json!({ "changed": changed, "document": document, "decisions": after }))
}

/// The connection's target and the workspace's real path on the host.
fn resolve(session_id: &str, workspace: &str) -> Result<(Target, String), CoreError> {
    if !workspace.starts_with('/') || workspace.contains('\0') {
        return Err(invalid_request(TrustError { field: "workspace" }));
    }
    let connection = lookup(session_id)?;
    let real_path = connection
        .session
        .real_path(workspace)
        .map_err(|error| request_error(session_id, error))?;
    if trust::host_path(&real_path).is_none() {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "The host returned an invalid real path",
        ));
    }
    Ok((connection.trust_target.clone(), real_path))
}

/// A stored document that cannot be decoded is a parse failure, not a bad
/// request: the caller sent what the source holds.
fn parse(document: Option<&str>) -> Result<Decisions, CoreError> {
    Decisions::parse(document).map_err(|error| {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Invalid remote workspace trust document",
        )
        .with_details(json!({ "field": error.field }).to_string())
    })
}

fn invalid_request(error: TrustError) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid remote workspace trust request",
    )
    .with_details(json!({ "field": error.field }).to_string())
}
