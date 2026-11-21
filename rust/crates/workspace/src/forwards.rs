//! `remote.startForward`, `remote.startHostForward`,
//! `remote.startReverseForward`, `remote.stopForward` and `remote.forwards`:
//! the port forwards of one open connection, as the ports page manages them.
//! `remote.parseForwardAddress` reads what the user typed into the page, a
//! port, `host:port` or a URL, without a connection.
//!
//! - A local forward listens on this machine and reaches a port as the host
//!   sees it.
//! - A host forward listens on the host and connects to another server from
//!   there; it listens on every host interface only when the request says so.
//! - A reverse forward listens on the host's loopback interface and reaches a
//!   port on this machine's loopback interface. It starts only when the
//!   remote preferences allow reverse forwards.
//!
//! Each connection owns its forwards. Closing the connection, or losing it,
//! closes every port it opened on either side (design S29), and none is
//! reopened after a reconnect. Which targets need the user's confirmation
//! (design S28), how a local port another program uses is replaced, and the
//! one limit every kind shares are the rules of
//! `lithe_remote::client::forward_set`.
//! The listening ports a forward can reach come from `ports.listening`
//! through `remote.hostRequest`.

use crate::sessions::{forget, lookup, request_error, Connection};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::forward_set::MAX_FORWARDS;
use lithe_remote::client::{
    forward_address, ForwardAddress, ForwardError, ForwardInfo, ForwardSpec, HostForwardSpec,
    ReverseForwardSpec, TargetField,
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

/// `remote.startHostForward`: a host port that connects to another server.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartHostRequest {
    session_id: String,
    /// Host port to listen on; absent or `0` lets the host choose one.
    #[serde(default)]
    listen_port: Option<u16>,
    /// Listen on every host interface instead of its loopback interface. The
    /// caller asks the user every time; Core never remembers it.
    #[serde(default)]
    all_interfaces: bool,
    /// The server to connect to, as the host resolves it.
    target_host: String,
    target_port: u16,
    /// The user confirmed a target outside the host's loopback interface.
    #[serde(default)]
    allow_non_loopback_target: bool,
}

/// `remote.startReverseForward`: a host loopback port that reaches a port on
/// this machine's loopback interface.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartReverseRequest {
    session_id: String,
    /// Host port to listen on; absent or `0` lets the host choose one.
    #[serde(default)]
    listen_port: Option<u16>,
    /// The port on this machine's loopback interface.
    local_port: u16,
    /// Current remote preferences document, `null` when the source does not
    /// exist; it decides whether reverse forwards may start.
    #[serde(default)]
    preferences: Option<String>,
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

/// Result of `remote.startForward`, `remote.startHostForward` and
/// `remote.startReverseForward`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    forward: ForwardInfo,
    /// `false` when the same rule already ran; `forward` is then that
    /// forward, unchanged.
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

/// Starts a host forward, or returns the one that already runs the same rule.
pub fn start_host(request: StartHostRequest) -> Result<Started, CoreError> {
    let connection = open(&request.session_id)?;
    let spec = HostForwardSpec {
        listen_port: request.listen_port,
        all_interfaces: request.all_interfaces,
        target_host: request.target_host,
        target_port: request.target_port,
        allow_non_loopback_target: request.allow_non_loopback_target,
    };
    let (forward, created) = connection
        .forwards
        .start_host(spec)
        .map_err(|error| forward_error(&request.session_id, error))?;
    Ok(Started { forward, created })
}

/// Starts a reverse forward when the preferences allow it, or returns the
/// one that already runs the same rule.
pub fn start_reverse(request: StartReverseRequest) -> Result<Started, CoreError> {
    if !crate::preferences::reverse_forwarding_allowed(request.preferences.as_deref())? {
        return Err(CoreError::new(
            ErrorCode::ReverseForwardDisabled,
            "Reverse port forwards are turned off in the remote workspace settings",
        ));
    }
    let connection = open(&request.session_id)?;
    let spec = ReverseForwardSpec {
        listen_port: request.listen_port,
        local_port: request.local_port,
    };
    let (forward, created) = connection
        .forwards
        .start_reverse(spec)
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

/// Stops one forward and closes the port it listens on.
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
                TargetField::LocalPort => "localPort",
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
        ForwardError::HostPortInUse => CoreError::new(
            ErrorCode::ForwardHostPortInUse,
            "Another program on the remote host already uses the port",
        ),
        ForwardError::HostPortDenied => CoreError::new(
            ErrorCode::ForwardHostPortDenied,
            "The remote host does not allow listening on the port",
        ),
        ForwardError::HostListenerUnsupported => CoreError::new(
            ErrorCode::ForwardHostUnsupported,
            "The remote server cannot listen on host ports",
        ),
        ForwardError::Remote(error) => request_error(session_id, error),
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
