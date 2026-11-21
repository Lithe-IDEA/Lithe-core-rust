//! `remote.startForward`, `remote.stopForward` and `remote.forwards`: the
//! port forwards of one open connection, as the ports page manages them.
//! `remote.parseForwardAddress` reads what the user typed into the page, a
//! port, `host:port` or a URL, without a connection.
//!
//! Each connection owns its forwards. Closing the connection, or losing it,
//! closes every local port it opened (design S29). Which targets need the
//! user's confirmation (design S28), and how a local port another program
//! uses is replaced, are the rules of `lithe_remote::client::forward_set`.
//! The listening ports a forward can reach come from `ports.listening`
//! through `remote.hostRequest`.

use crate::sessions::{forget, lookup, Connection};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::forward_set::MAX_FORWARDS;
use lithe_remote::client::{
    forward_address, ForwardAddress, ForwardError, ForwardInfo, ForwardSpec, TargetField,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// `remote.startForward`: one forward to a port as the host sees it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    session_id: String,
    /// An IP address, with or without brackets for IPv6, or a host name the
    /// remote host resolves, such as `localhost`.
    target_host: String,
    target_port: u16,
    /// Local port to listen on; absent or `0` lets the system choose one.
    #[serde(default)]
    local_port: Option<u16>,
    /// The user confirmed a target outside the host's loopback interface.
    #[serde(default)]
    allow_non_loopback_target: bool,
}

/// `remote.parseForwardAddress`: an address typed into the ports page.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddressRequest {
    text: String,
}

/// `remote.stopForward`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopRequest {
    session_id: String,
    forward_id: u64,
}

/// `remote.forwards`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListRequest {
    session_id: String,
}

/// `remote.startForward` result.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    forward: ForwardInfo,
    /// `false` when a forward to the same target already ran; `forward` is
    /// then that forward, unchanged.
    created: bool,
}

/// `remote.stopForward` result.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    /// `false` when no forward of the connection had that id.
    stopped: bool,
}

/// `remote.forwards` result.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forwards {
    /// Running forwards, by id.
    forwards: Vec<ForwardInfo>,
}

/// Starts a forward, or returns the one that already reaches the target.
pub fn start(request: StartRequest) -> Result<Started, CoreError> {
    let connection = open(&request.session_id)?;
    let spec = ForwardSpec {
        target_host: request.target_host,
        target_port: request.target_port,
        local_port: request.local_port,
        allow_non_loopback_target: request.allow_non_loopback_target,
    };
    let (forward, created) = connection
        .forwards
        .start(spec)
        .map_err(|error| forward_error(&request.session_id, error))?;
    Ok(Started { forward, created })
}

/// Reads an address typed into the ports page into the target of a forward.
/// Nothing is resolved or contacted.
pub fn parse_address(request: AddressRequest) -> Result<ForwardAddress, CoreError> {
    forward_address::parse(&request.text).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid port forward address")
            .with_details(json!({ "field": "text", "reason": error.reason() }).to_string())
    })
}

/// Stops one forward and closes its local port.
pub fn stop(request: StopRequest) -> Result<Stopped, CoreError> {
    let connection = open(&request.session_id)?;
    Ok(Stopped {
        stopped: connection.forwards.stop(request.forward_id),
    })
}

/// Lists the running forwards of one connection.
pub fn list(request: ListRequest) -> Result<Forwards, CoreError> {
    let connection = open(&request.session_id)?;
    let forwards = connection
        .forwards
        .list()
        .map_err(|error| forward_error(&request.session_id, error))?;
    Ok(Forwards { forwards })
}

/// The registered connection, while its session still runs. A session that
/// ended already released its forwards, so its connection is forgotten.
fn open(session_id: &str) -> Result<Arc<Connection>, CoreError> {
    let connection = lookup(session_id)?;
    if connection.session.is_closed() {
        return Err(forward_error(session_id, ForwardError::SessionClosed));
    }
    Ok(connection)
}

/// Gives each forward failure its own category.
fn forward_error(session_id: &str, error: ForwardError) -> CoreError {
    match error {
        ForwardError::InvalidTarget(field) => {
            let field = match field {
                TargetField::Host => "targetHost",
                TargetField::Port => "targetPort",
            };
            CoreError::new(ErrorCode::InvalidRequest, "Invalid port forward target")
                .with_details(json!({ "field": field }).to_string())
        }
        ForwardError::TargetNotConfirmed => CoreError::new(
            ErrorCode::ForwardTargetNotConfirmed,
            "The forward reaches a machine other than the remote host and needs confirmation",
        ),
        ForwardError::LimitReached => CoreError::new(
            ErrorCode::ForwardLimitReached,
            "The remote connection has the most port forwards it keeps",
        )
        .with_details(json!({ "maximum": MAX_FORWARDS }).to_string()),
        ForwardError::SessionClosed => {
            forget(session_id);
            CoreError::new(ErrorCode::ConnectionClosed, "The remote connection closed")
        }
        // The system could not open even a port of its own choice, so no
        // stable category applies; the error kind says why.
        ForwardError::Listen(error) => {
            CoreError::new(ErrorCode::Unknown, "No local port could be opened")
                .with_details(json!({ "io": format!("{:?}", error.kind()) }).to_string())
        }
    }
}
