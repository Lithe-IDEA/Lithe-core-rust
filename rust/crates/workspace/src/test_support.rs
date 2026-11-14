//! Hooks for `lithe-core`'s end-to-end tests, which drive real Core commands
//! against an in-process host server instead of an SSH child.

use crate::sessions::{lookup, nonce, registry, Connection, ConnectionTransport};
use lithe_remote::client::RemoteSession;
use std::sync::{Arc, Mutex};

/// Registers `session` as a connected host and returns its session id.
pub fn register_in_process(session: RemoteSession) -> String {
    let id = nonce();
    registry().lock().unwrap().insert(
        id.clone(),
        Arc::new(Connection {
            session,
            git_tool: Mutex::new(None),
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
