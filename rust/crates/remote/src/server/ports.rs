//! Listening ports on the remote, for automatic port forwarding.

use crate::protocol::methods::ListeningPortsResult;
use crate::protocol::RemoteError;
use crate::server::session::Session;
use crate::server::to_value;
use crate::server::{fs::io_error, platform};
use serde_json::Value;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;

pub(crate) fn pick_loopback() -> Result<Value, RemoteError> {
    // Providers bind this port themselves. The later readiness check verifies
    // ownership because the OS can give the port to another process in between.
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| io_error(error, ""))?;
    let address = listener.local_addr().map_err(|error| io_error(error, ""))?;
    to_value(address.port())
}

pub(crate) fn handle(_session: &Arc<Session>) -> Result<Value, RemoteError> {
    let mut ports =
        platform::listening_ports().map_err(|error| crate::server::fs::io_error(error, ""))?;
    // Processes that inherited one listening socket, and sockets sharing a
    // port through `SO_REUSEPORT`, list one address and port several times.
    // The entry kept is the one with a known owner and the lowest process id,
    // usually the parent that opened the socket.
    ports.sort_by(|left, right| {
        (&left.address, left.port, left.pid.is_none(), left.pid).cmp(&(
            &right.address,
            right.port,
            right.pid.is_none(),
            right.pid,
        ))
    });
    ports.dedup_by(|later, kept| later.address == kept.address && later.port == kept.port);
    to_value(ListeningPortsResult { ports })
}
