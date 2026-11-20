//! Hooks for `lithe-core`'s end-to-end tests, which drive real Core commands
//! against an in-process host server instead of an SSH child.

use crate::sessions::{lookup, nonce, registry, Connection, ConnectionTransport};
use lithe_remote::client::trust::Target;
use lithe_remote::client::{RemoteSession, TransportChain};
use std::sync::{Arc, Mutex};

/// Registers `session` as a connected host and returns its session id. Its
/// trust target has no hops, only the account the server reports.
pub fn register_in_process(session: RemoteSession) -> String {
    let id = nonce();
    let trust_target = Target::new(&TransportChain::local(), session.hello().host.user_id);
    registry().lock().unwrap().insert(
        id.clone(),
        Arc::new(Connection {
            session,
            git_tool: Mutex::new(None),
            trust_target,
            transport: ConnectionTransport::InProcess,
        }),
    );
    id
}

/// The registered connection for `id`.
pub fn connection(id: &str) -> Arc<Connection> {
    lookup(id).unwrap()
}

/// Shuts the session down but keeps it registered, as a dropped link does.
pub fn drop_link(id: &str) {
    lookup(id).unwrap().close();
}

/// Removes the session from the registry and shuts it down.
pub fn unregister(id: &str) {
    if let Some(connection) = registry().lock().unwrap().remove(id) {
        connection.close();
    }
}

/// A fresh unguessable id, the same shape as session ids.
pub fn unique_id() -> String {
    nonce()
}

/// Host stat requests issued on the current thread so far.
pub fn stat_requests() -> usize {
    crate::fs::stat_requests_for_test()
}
