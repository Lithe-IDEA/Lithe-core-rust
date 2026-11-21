//! Connection lifecycle facts (design S34).
//!
//! One [`ConnectionLog`] follows one connection attempt from its first step
//! until its session is released. Every fact carries the same `connection`
//! value, a random identifier made for this attempt, so all lines of one
//! connection can be read together. The application maps it to the
//! connection's display name; hosts, user names, home directories, paths,
//! prompts, answers and session tokens never enter these facts, so an exported
//! log names no host or user.
//!
//! An attempt records, in order:
//!
//! 1. `remote.connection.connect.started` when it begins;
//! 2. `remote.connection.shell_started` once authentication succeeded and the
//!    remote shell answered the probe;
//! 3. `remote.server.deploy_finished` after the installed server was reused,
//!    or uploaded and verified;
//! 4. exactly one of `remote.connection.connect.finished` (connected or
//!    cancelled) and `remote.connection.connect.failed`;
//! 5. `remote.connection.interrupted` when an established stream ends without
//!    a shutdown request;
//! 6. `remote.connection.cleaned` when the established session is released.
//!
//! Port forwards over the session add `remote.forward.started` and
//! `remote.forward.removed`; forwards that listen on the host add
//! `remote.forward.host_started` and `remote.forward.host_removed`.
//!
//! Several layers can see the same failure: the launch script, the protocol
//! handshake, and the caller that owns the SSH process. Each reports the most
//! specific stage it knows, and only the first outcome of an attempt is kept,
//! so an attempt never records two outcomes.

use crate::client::bootstrap::{ServerArtifact, ServerTarget};
use crate::client::forward_set;
use crate::client::launch::{Deployment, LaunchError};
use crate::failure_log;
use crate::protocol::methods::{encode_hex, HelloResult, TunnelTarget};
use serde_json::{json, Value};
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::BuildHasher;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub(crate) const CONNECT_STARTED: &str = "remote.connection.connect.started";
pub(crate) const CONNECT_FINISHED: &str = "remote.connection.connect.finished";
pub(crate) const CONNECT_FAILED: &str = "remote.connection.connect.failed";
pub(crate) const SHELL_STARTED: &str = "remote.connection.shell_started";
pub(crate) const DEPLOY_FINISHED: &str = "remote.server.deploy_finished";
pub(crate) const INTERRUPTED: &str = "remote.connection.interrupted";
pub(crate) const CLEANED: &str = "remote.connection.cleaned";
pub(crate) const FORWARD_STARTED: &str = "remote.forward.started";
pub(crate) const FORWARD_REMOVED: &str = "remote.forward.removed";
pub(crate) const HOST_FORWARD_STARTED: &str = "remote.forward.host_started";
pub(crate) const HOST_FORWARD_REMOVED: &str = "remote.forward.host_removed";

// Session states only move forward, in this order.

/// The session is established, or not yet.
const OPEN: u8 = 0;
/// The client asked the server to shut down; the stream ending is expected.
const SHUTTING_DOWN: u8 = 1;
/// The stream ended without a shutdown request.
const LOST: u8 = 2;
/// The session was released and `remote.connection.cleaned` is settled.
const RELEASED: u8 = 3;

/// How the client reaches the server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionKind {
    /// An SSH connection owned by the caller.
    Ssh,
    /// A WSL distribution started through `wsl.exe`, owned by the caller.
    Wsl,
    /// A container on this machine, entered through its engine's `exec`,
    /// owned by the caller.
    Container,
    /// A container on an SSH host, owned by the caller.
    SshContainer,
    /// A container inside a WSL distribution, owned by the caller.
    WslContainer,
    /// A transport chain [`crate::client::RemoteSession::connect`] spawns.
    Command,
    /// Streams the caller supplies to
    /// [`crate::client::RemoteSession::from_streams`].
    Streams,
}

impl ConnectionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Wsl => "wsl",
            Self::Container => "container",
            Self::SshContainer => "ssh-container",
            Self::WslContainer => "wsl-container",
            Self::Command => "command",
            Self::Streams => "streams",
        }
    }
}

/// Lifecycle log of one connection attempt. Clones share the attempt, so the
/// caller, the launch and the session record into the same lines.
#[derive(Clone)]
pub struct ConnectionLog(Arc<Inner>);

struct Inner {
    id: String,
    kind: ConnectionKind,
    started: Instant,
    settled: AtomicBool,
    connected: OnceLock<Instant>,
    state: AtomicU8,
}

impl fmt::Debug for ConnectionLog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ConnectionLog({})", self.0.id)
    }
}

impl ConnectionLog {
    /// Starts the log of a new attempt and records
    /// `remote.connection.connect.started`.
    pub fn begin(kind: ConnectionKind) -> Self {
        let log = Self(Arc::new(Inner {
            id: random_id(),
            kind,
            started: Instant::now(),
            settled: AtomicBool::new(false),
            connected: OnceLock::new(),
            state: AtomicU8::new(OPEN),
        }));
        log.emit_connect(CONNECT_STARTED, vec![]);
        log
    }

    /// The `connection` value of every fact of this attempt: 16 random hex
    /// digits that only correlate log lines. It is not a session id or a
    /// capability.
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// Records that the attempt failed at `stage` with a stable category,
    /// unless an outcome is already recorded. `stage` is one of `probe`,
    /// `artifact`, `deploy`, `handshake`, `authentication`, `network` and
    /// `connect`; `code` and `domain` are controlled vocabulary, never error
    /// text or peer output.
    pub fn connect_failed(&self, stage: &'static str, code: &str, domain: &'static str) {
        if self.settle() {
            self.emit_connect(
                CONNECT_FAILED,
                vec![
                    ("stage", json!(stage)),
                    ("error", json!("Remote connection failed")),
                    ("error_code", json!(code)),
                    ("error_domain", json!(domain)),
                    ("duration_ms", millis(self.0.started.elapsed())),
                ],
            );
        }
    }

    /// Records that the user cancelled the attempt before the session opened,
    /// unless an outcome is already recorded. Cancellation is not a failure.
    pub fn connect_cancelled(&self) {
        if self.settle() {
            self.emit_connect(
                CONNECT_FINISHED,
                vec![
                    ("outcome", json!("cancelled")),
                    ("duration_ms", millis(self.0.started.elapsed())),
                ],
            );
        }
    }

    /// Records the completed handshake. The session's lifetime starts here.
    pub(crate) fn connected(&self, hello: &HelloResult) {
        if !self.settle() {
            return;
        }
        let now = *self.0.connected.get_or_init(Instant::now);
        let mut fields = vec![
            ("outcome", json!("connected")),
            ("persistent", json!(hello.session.persistent)),
            ("resumed", json!(hello.session.resumed)),
            ("duration_ms", millis(now - self.0.started)),
        ];
        // The version is peer output; one the catalog would refuse is left out
        // instead of losing the whole fact.
        if is_version(&hello.server_version) {
            fields.push(("server_version", json!(hello.server_version)));
        }
        self.emit_connect(CONNECT_FINISHED, fields);
    }

    /// Records a launch failure at `stage`. A shell that never answered the
    /// probe is left to the caller, which knows from the SSH process whether
    /// authentication, the host key or the network failed.
    pub(crate) fn launch_failed(&self, stage: &'static str, error: &LaunchError) {
        if stage == "probe" && matches!(error, LaunchError::NoShell) {
            return;
        }
        let domain = match error {
            LaunchError::Io(_) => "io",
            _ => "remote",
        };
        self.connect_failed(stage, &error.failure_category(), domain);
    }

    /// Records that the remote shell answered the probe, so authentication
    /// succeeded. `target` is `None` when no server build fits the host.
    pub(crate) fn shell_started(&self, target: Option<ServerTarget>) {
        let target = target.map_or("unsupported", ServerTarget::rust_target);
        self.emit(SHELL_STARTED, vec![("target", json!(target))]);
    }

    /// Records how the server was put in place on the host.
    pub(crate) fn deployed(
        &self,
        artifact: &ServerArtifact,
        deployment: &Deployment,
        bytes: usize,
        duration: Duration,
    ) {
        let action = if deployment.uploaded {
            "uploaded"
        } else {
            "reused"
        };
        let mut fields = vec![
            ("target", json!(artifact.target().rust_target())),
            ("server_version", json!(artifact.version())),
            ("action", json!(action)),
            ("verification", json!(deployment.verification.as_str())),
            ("duration_ms", millis(duration)),
        ];
        if deployment.uploaded {
            fields.push(("bytes", json!(bytes)));
        }
        self.emit(DEPLOY_FINISHED, fields);
    }

    /// Marks that the client asked the server to shut down, so the stream
    /// ending afterwards is not an interruption. A session already lost or
    /// released keeps that state.
    pub(crate) fn shutdown_requested(&self) {
        self.0.state.fetch_max(SHUTTING_DOWN, Ordering::AcqRel);
    }

    /// Records that the server stream ended with `cause`, a controlled token.
    /// Nothing is recorded after a shutdown request or release, or before the
    /// handshake completed, whose failure the handshake records itself.
    pub(crate) fn stream_ended(&self, cause: &str) {
        if self
            .0
            .state
            .compare_exchange(OPEN, LOST, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if let Some(connected) = self.0.connected.get() {
            self.emit(
                INTERRUPTED,
                vec![
                    ("cause", json!(cause)),
                    ("duration_ms", millis(connected.elapsed())),
                ],
            );
        }
    }

    /// Records the release of an established session once. `persistent`
    /// sessions keep running on the host after this client leaves.
    pub(crate) fn released(&self, persistent: bool) {
        let previous = self.0.state.swap(RELEASED, Ordering::AcqRel);
        let Some(connected) = self.0.connected.get() else {
            return;
        };
        let ended = match previous {
            RELEASED => return,
            SHUTTING_DOWN => "shutdown",
            LOST => "lost",
            _ if persistent => "detached",
            _ => "closed",
        };
        self.emit(
            CLEANED,
            vec![
                ("ended", json!(ended)),
                ("duration_ms", millis(connected.elapsed())),
            ],
        );
    }

    /// Records a forward listening on `local` for `target`. The remote host
    /// and socket path stay out: only the port and whether the host is a
    /// loopback name or address are recorded.
    pub(crate) fn forward_started(&self, local: SocketAddr, target: &TunnelTarget) {
        let mut fields = vec![
            ("local_port", json!(local.port())),
            ("bind_scope", json!(scope(local.ip().is_loopback()))),
        ];
        match target {
            TunnelTarget::Tcp { host, port } => {
                fields.push(("target_kind", json!("tcp")));
                fields.push(("remote_port", json!(port)));
                fields.push((
                    "target_scope",
                    json!(scope(forward_set::is_loopback_host(host))),
                ));
            }
            TunnelTarget::Unix { .. } => fields.push(("target_kind", json!("unix"))),
        }
        self.emit(FORWARD_STARTED, fields);
    }

    /// Records a forward that stopped after accepting `connections`.
    pub(crate) fn forward_removed(&self, local_port: u16, connections: u64, duration: Duration) {
        self.emit(
            FORWARD_REMOVED,
            vec![
                ("local_port", json!(local_port)),
                ("connections", json!(connections)),
                ("duration_ms", millis(duration)),
            ],
        );
    }

    /// Records a host port that relays to `target_host:target_port`. Only the
    /// ports and whether each side is a loopback address are recorded.
    pub(crate) fn host_forward_started(
        &self,
        listen_port: u16,
        all_interfaces: bool,
        target_host: &str,
        target_port: u16,
    ) {
        self.emit(
            HOST_FORWARD_STARTED,
            vec![
                ("kind", json!("host")),
                ("listen_port", json!(listen_port)),
                ("bind_scope", json!(scope(!all_interfaces))),
                ("target_port", json!(target_port)),
                (
                    "target_scope",
                    json!(scope(forward_set::is_loopback_host(target_host))),
                ),
            ],
        );
    }

    /// Records a host loopback port whose connections reach `local_port` on
    /// this machine's loopback.
    pub(crate) fn reverse_forward_started(&self, listen_port: u16, local_port: u16) {
        self.emit(
            HOST_FORWARD_STARTED,
            vec![
                ("kind", json!("reverse")),
                ("listen_port", json!(listen_port)),
                ("bind_scope", json!(scope(true))),
                ("local_port", json!(local_port)),
            ],
        );
    }

    /// Records a host port that stopped after accepting `connections`, of
    /// which `failed_connections` never reached their target.
    pub(crate) fn host_forward_removed(
        &self,
        listen_port: u16,
        connections: u64,
        failed_connections: u64,
        duration: Duration,
    ) {
        self.emit(
            HOST_FORWARD_REMOVED,
            vec![
                ("listen_port", json!(listen_port)),
                ("connections", json!(connections)),
                ("failed_connections", json!(failed_connections)),
                ("duration_ms", millis(duration)),
            ],
        );
    }

    /// True for the first outcome of the attempt.
    fn settle(&self) -> bool {
        !self.0.settled.swap(true, Ordering::AcqRel)
    }

    fn emit(&self, event: &str, mut fields: Vec<(&'static str, Value)>) {
        fields.push(("connection", json!(self.0.id)));
        failure_log::record(event, &fields);
    }

    /// The connect span also names the transport; the other facts identify
    /// the attempt by `connection` alone.
    fn emit_connect(&self, event: &str, mut fields: Vec<(&'static str, Value)>) {
        fields.push(("transport", json!(self.0.kind.as_str())));
        self.emit(event, fields);
    }
}

fn millis(duration: Duration) -> Value {
    json!(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

fn scope(loopback: bool) -> &'static str {
    if loopback {
        "loopback"
    } else {
        "network"
    }
}

/// Mirrors the catalog's `version` type: 1 to 64 ASCII letters, digits and `._+-`.
fn is_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

/// 64 bits from the standard library's randomly keyed hasher. The counter
/// keeps two attempts in one process apart even if the keys repeated.
fn random_id() -> String {
    static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
    let value =
        RandomState::new().hash_one((ATTEMPTS.fetch_add(1, Ordering::Relaxed), std::process::id()));
    encode_hex(&value.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_distinct_lowercase_hex() {
        let first = random_id();
        let second = random_id();
        assert_eq!(first.len(), 16);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert_ne!(first, second);
    }

    #[test]
    fn server_versions_outside_the_catalog_vocabulary_are_left_out() {
        assert!(is_version("0.8.11+build.3"));
        assert!(!is_version(""));
        assert!(!is_version("1.0 beta"));
        assert!(!is_version(&"1".repeat(65)));
    }
}
