//! Hooks for `lithe-core`'s end-to-end tests, which drive real Core commands
//! against an in-process host server instead of an SSH child.

use crate::sessions::{ended, lookup, nonce, registry, Connection, ConnectionTransport};
use lithe_remote::client::trust::Target;
use lithe_remote::client::{RemoteSession, TransportChain};
use std::sync::Arc;

/// Registers `session` as a connected host and returns its session id. Its
/// trust target has no hops, only the account the server reports.
pub fn register_in_process(session: RemoteSession) -> String {
    let id = nonce();
    let trust_target = Target::new(&TransportChain::local(), session.hello().host.user_id);
    registry().lock().unwrap().insert(
        id.clone(),
        Arc::new(Connection::new(
            session,
            trust_target,
            ConnectionTransport::InProcess,
        )),
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

/// Removes the session from the registry, runs the session-end hooks and
/// shuts it down, as `remote.disconnect` does.
pub fn unregister(id: &str) {
    let connection = registry().lock().unwrap().remove(id);
    if let Some(connection) = connection {
        ended(id);
        connection.close();
    }
}

/// Removes the session from the registry and runs the session-end hooks but
/// leaves its link open, as a request that already holds the session's host
/// sees when `remote.disconnect` runs at the same time. Dropping the result
/// closes the link.
pub fn forget_keeping_link(id: &str) -> KeptLink {
    let connection = registry().lock().unwrap().remove(id).unwrap();
    ended(id);
    KeptLink(connection)
}

/// An unregistered session's link, closed when dropped.
pub struct KeptLink(Arc<Connection>);

impl Drop for KeptLink {
    fn drop(&mut self) {
        self.0.close();
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

/// `remote.listContainers` with `program` in place of the engine CLI, on
/// this machine or on the host the request names.
pub fn list_containers_with(
    request: crate::containers::ListRequest,
    program: &str,
) -> Result<crate::containers::Containers, lithe_protocol::CoreError> {
    crate::containers::list_with(request, Some(program))
}

/// `remote.devcontainerUp` with `program` in place of both the Dev Container
/// CLI and the engine CLI.
pub fn devcontainer_up_with(
    request: crate::devcontainers::UpRequest,
    program: &str,
) -> Result<crate::devcontainers::Up, lithe_protocol::CoreError> {
    crate::devcontainers::up_with(request, Some(program))
}
