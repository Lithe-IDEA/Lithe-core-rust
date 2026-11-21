//! Listening ports on the remote, for automatic port forwarding.

use crate::protocol::methods::ListeningPortsResult;
use crate::protocol::RemoteError;
use crate::server::platform;
use crate::server::session::Session;
use crate::server::to_value;
use serde_json::Value;
use std::sync::Arc;

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
