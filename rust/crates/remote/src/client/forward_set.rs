//! The port forwards of one session, as the ports page manages them.
//!
//! A [`ForwardSet`] starts, lists and stops the [`PortForward`]s of one
//! [`RemoteSession`] and owns them until they stop:
//!
//! - **Targets** are named as the remote host sees them. A host listening on
//!   every interface (`0.0.0.0`, `::`) is reached through the host's own
//!   loopback address. A target that is not the host's loopback interface
//!   reaches another machine through the host, for example a database on
//!   the host's private network, so starting it requires
//!   [`ForwardSpec::allow_non_loopback_target`], which the application sets
//!   only after the user confirmed that target (design S28).
//! - **Local ports.** A forward listens on `127.0.0.1`. When the requested
//!   port is taken locally, it listens on a free port instead and reports
//!   [`ForwardInfo::local_port_changed`], so the user sees which address to
//!   open instead of a failure.
//! - **Session end** releases every forward and closes their local ports
//!   (design S29). A stopped connection never leaves a local port that
//!   accepts connections and then drops them.
//!
//! Starting the same target twice returns the forward that already exists,
//! so a repeated click does not open a second local port.

use crate::client::forward::PortForward;
use crate::client::session::RemoteSession;
use crate::protocol::methods::TunnelTarget;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::Duration;

/// Most forwards one session keeps at once. Each one holds a local port and
/// an acceptor thread; a page of forwards is far below this.
pub const MAX_FORWARDS: usize = 64;

/// Longest host name a target may have, as in DNS.
const MAX_HOST_BYTES: usize = 253;

/// Bound for checking whether a local program already uses a port. Loopback
/// connections are refused or accepted at once; this only covers a stalled
/// local stack.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// What to forward.
#[derive(Clone, Debug)]
pub struct ForwardSpec {
    /// Host as the remote host resolves it: an IP address, with or without
    /// brackets for IPv6, or a name such as `localhost`.
    pub target_host: String,
    /// TCP port on the target, 1 to 65535.
    pub target_port: u16,
    /// Local port to listen on; `None` or `0` lets the system choose one.
    pub local_port: Option<u16>,
    /// The user confirmed a target outside the remote host's loopback
    /// interface (design S28).
    pub allow_non_loopback_target: bool,
}

/// A running forward, as listed on the ports page.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardInfo {
    /// Identifier within the session, never reused while the session lasts.
    pub id: u64,
    /// Local address the forward listens on, `127.0.0.1`.
    pub local_host: String,
    /// Local port the forward listens on.
    pub local_port: u16,
    /// Local port the request asked for, when it named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_local_port: Option<u16>,
    /// The requested local port was taken, so the forward listens on
    /// `local_port` instead.
    pub local_port_changed: bool,
    /// Target host as sent to the remote host, after mapping a wildcard
    /// address to the host's loopback address.
    pub target_host: String,
    /// Target TCP port.
    pub target_port: u16,
    /// The target is the remote host's loopback interface.
    pub loopback_target: bool,
    /// Local connections the forward has accepted so far.
    pub connections: u64,
}

/// The malformed part of a forward's target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetField {
    /// [`ForwardSpec::target_host`].
    Host,
    /// [`ForwardSpec::target_port`].
    Port,
}

/// Why a forward did not start or the set cannot answer.
#[derive(Debug)]
pub enum ForwardError {
    /// The target host or port is malformed.
    InvalidTarget(TargetField),
    /// The target is outside the remote host's loopback interface and the
    /// request did not confirm it (design S28).
    TargetNotConfirmed,
    /// The session already has [`MAX_FORWARDS`] forwards.
    LimitReached,
    /// The session ended; its forwards were released.
    SessionClosed,
    /// No local port could be opened, not even one the system chose.
    Listen(io::Error),
}

impl fmt::Display for ForwardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(TargetField::Host) => {
                formatter.write_str("invalid forward target host")
            }
            Self::InvalidTarget(TargetField::Port) => {
                formatter.write_str("invalid forward target port")
            }
            Self::TargetNotConfirmed => formatter
                .write_str("the forward target is outside the remote host and was not confirmed"),
            Self::LimitReached => {
                write!(formatter, "a session keeps at most {MAX_FORWARDS} forwards")
            }
            Self::SessionClosed => formatter.write_str("the remote connection closed"),
            Self::Listen(error) => write!(formatter, "cannot listen on a local port: {error}"),
        }
    }
}

impl std::error::Error for ForwardError {}

/// The forwards of one session.
pub struct ForwardSet {
    session: Arc<RemoteSession>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// Set when the session ended or the set closed; nothing starts after it.
    closed: bool,
    next_id: u64,
    forwards: BTreeMap<u64, Entry>,
}

struct Entry {
    forward: PortForward,
    requested_local_port: Option<u16>,
    local_port_changed: bool,
    target_host: String,
    target_port: u16,
    loopback_target: bool,
}

impl Entry {
    fn info(&self, id: u64) -> ForwardInfo {
        let local = self.forward.local_address();
        ForwardInfo {
            id,
            local_host: local.ip().to_string(),
            local_port: local.port(),
            requested_local_port: self.requested_local_port,
            local_port_changed: self.local_port_changed,
            target_host: self.target_host.clone(),
            target_port: self.target_port,
            loopback_target: self.loopback_target,
            connections: self.forward.connections(),
        }
    }
}

impl ForwardSet {
    /// An empty set whose forwards end together with `session`.
    pub fn new(session: Arc<RemoteSession>) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        // Weak, because the forwards own the session: a strong reference in
        // the session's own hook would keep both alive forever.
        let released = Arc::downgrade(&state);
        session.on_closed(move || release_in_background(&released));
        Self { session, state }
    }

    /// Starts forwarding, or returns the forward that already reaches the
    /// same target. The flag is `true` for a new forward.
    pub fn start(&self, spec: ForwardSpec) -> Result<(ForwardInfo, bool), ForwardError> {
        let target = TargetHost::parse(&spec.target_host)?;
        if spec.target_port == 0 {
            return Err(ForwardError::InvalidTarget(TargetField::Port));
        }
        if !target.loopback && !spec.allow_non_loopback_target {
            return Err(ForwardError::TargetNotConfirmed);
        }
        let requested = spec.local_port.filter(|port| *port != 0);
        // Held while the forward starts, so two requests for one target
        // cannot both open a local port.
        let mut state = lock(&self.state);
        if state.closed || self.session.is_closed() {
            return Err(ForwardError::SessionClosed);
        }
        if let Some((id, entry)) = state.forwards.iter().find(|(_, entry)| {
            entry.target_host == target.host && entry.target_port == spec.target_port
        }) {
            return Ok((entry.info(*id), false));
        }
        if state.forwards.len() >= MAX_FORWARDS {
            return Err(ForwardError::LimitReached);
        }
        let tunnel = TunnelTarget::Tcp {
            host: target.host.clone(),
            port: spec.target_port,
        };
        let forward = listen(&self.session, requested, tunnel)?;
        let local_port_changed =
            requested.is_some_and(|port| port != forward.local_address().port());
        state.next_id += 1;
        let id = state.next_id;
        let entry = Entry {
            forward,
            requested_local_port: requested,
            local_port_changed,
            target_host: target.host,
            target_port: spec.target_port,
            loopback_target: target.loopback,
        };
        let info = entry.info(id);
        state.forwards.insert(id, entry);
        Ok((info, true))
    }

    /// Stops one forward and closes its local port. Connections it already
    /// accepted run until either side closes them. Returns whether the
    /// forward existed.
    pub fn stop(&self, id: u64) -> bool {
        let removed = lock(&self.state).forwards.remove(&id);
        // Dropped outside the lock: stopping waits for the acceptor thread.
        removed.is_some()
    }

    /// The running forwards, by identifier.
    pub fn list(&self) -> Result<Vec<ForwardInfo>, ForwardError> {
        let state = lock(&self.state);
        if state.closed || self.session.is_closed() {
            return Err(ForwardError::SessionClosed);
        }
        Ok(state
            .forwards
            .iter()
            .map(|(id, entry)| entry.info(*id))
            .collect())
    }

    /// Stops every forward and refuses new ones, before the session closes.
    pub fn close(&self) {
        let forwards = take_all(&self.state);
        drop(forwards);
    }
}

impl Drop for ForwardSet {
    fn drop(&mut self) {
        self.close();
    }
}

/// Marks the set closed and takes its forwards, which the caller drops
/// outside the lock.
fn take_all(state: &Mutex<State>) -> BTreeMap<u64, Entry> {
    let mut state = lock(state);
    state.closed = true;
    std::mem::take(&mut state.forwards)
}

/// Releases the forwards of an ended session. Called by the session's reader
/// or writer thread, which must not wait for acceptor threads, so the
/// forwards stop on a thread of their own.
fn release_in_background(state: &Weak<Mutex<State>>) {
    let Some(state) = state.upgrade() else {
        return;
    };
    let forwards = take_all(&state);
    if forwards.is_empty() {
        return;
    }
    let spawned = thread::Builder::new()
        .name("remote-forward-release".into())
        .spawn(move || drop(forwards));
    // Without a thread, the closure that never ran is dropped with the
    // error, which stops the forwards here; each stop is bounded.
    crate::failure_log::io("remote.client.operation_failed", "forward_release", spawned);
}

/// Opens the forward on `requested`, or on a free port when another local
/// program uses that port.
fn listen(
    session: &Arc<RemoteSession>,
    requested: Option<u16>,
    target: TunnelTarget,
) -> Result<PortForward, ForwardError> {
    let any_port = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let Some(port) = requested else {
        return PortForward::start(Arc::clone(session), any_port, target)
            .map_err(ForwardError::Listen);
    };
    if !in_use_locally(port) {
        let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        // A port bound without listening, or one below 1024 that needs
        // privileges, is just as taken. The caller learns about the free
        // port from `local_port_changed` instead of an error.
        if let Ok(forward) = PortForward::start(Arc::clone(session), bind, target.clone()) {
            return Ok(forward);
        }
    }
    PortForward::start(Arc::clone(session), any_port, target).map_err(ForwardError::Listen)
}

/// Whether a local program accepts connections on `port` of either loopback
/// address.
///
/// Binding alone does not show this. On macOS and Windows a socket bound to
/// `127.0.0.1` opens next to a program that listens on every interface and
/// then receives that program's loopback connections, so the forward would
/// silently take over the local program's `localhost` address. A program
/// listening on `::1` only would instead keep the `localhost` connections of
/// browsers that try IPv6 first, and the forward would seem not to work.
fn in_use_locally(port: u16) -> bool {
    [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ]
    .into_iter()
    .any(|address| {
        TcpStream::connect_timeout(&SocketAddr::new(address, port), PROBE_TIMEOUT).is_ok()
    })
}

/// A validated target host and whether it is the remote host's loopback
/// interface.
#[derive(Debug, Eq, PartialEq)]
struct TargetHost {
    host: String,
    loopback: bool,
}

impl TargetHost {
    /// Accepts IP addresses, IPv6 in brackets or not, and DNS names. A
    /// wildcard address becomes the loopback address of its family, and an
    /// IPv4-mapped IPv6 address its IPv4 form. Names are not resolved:
    /// only `localhost` counts as loopback, as the remote host resolves
    /// other names, possibly elsewhere.
    fn parse(host: &str) -> Result<Self, ForwardError> {
        let address = match host
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            // Brackets only ever enclose an IPv6 address.
            Some(inner) => Some(
                inner
                    .parse::<Ipv6Addr>()
                    .map(IpAddr::V6)
                    .map_err(|_| ForwardError::InvalidTarget(TargetField::Host))?,
            ),
            None => host.parse::<IpAddr>().ok(),
        };
        if let Some(address) = address {
            let address = match address.to_canonical() {
                IpAddr::V4(address) if address.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(address) if address.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
                address => address,
            };
            return Ok(Self {
                host: address.to_string(),
                loopback: address.is_loopback(),
            });
        }
        if !is_host_name(host) {
            return Err(ForwardError::InvalidTarget(TargetField::Host));
        }
        let host = host.to_ascii_lowercase();
        Ok(Self {
            loopback: host == "localhost",
            host,
        })
    }
}

/// True for a host name the remote resolver can look up: 1 to 253 bytes of
/// letters, digits, `-`, `_` and `.`, not starting with `-` or `.`, so it can
/// never read as an option or an address with a port.
fn is_host_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && !host.starts_with(['-', '.'])
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

/// True for the host's loopback interface: `localhost` or a loopback address,
/// IPv6 in brackets or not and IPv4-mapped included. Names are not resolved.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    TargetHost::parse(host).is_ok_and(|target| target.loopback && !is_wildcard(host))
}

/// True for an address that listens on every interface, such as `0.0.0.0`.
fn is_wildcard(host: &str) -> bool {
    host.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host)
        .parse::<IpAddr>()
        .is_ok_and(|address| address.to_canonical().is_unspecified())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(host: &str) -> TargetHost {
        TargetHost::parse(host).unwrap()
    }

    #[test]
    fn wildcard_and_mapped_targets_become_the_hosts_loopback_address() {
        for (host, expected) in [
            ("0.0.0.0", "127.0.0.1"),
            ("::", "::1"),
            ("[::]", "::1"),
            ("::ffff:127.0.0.1", "127.0.0.1"),
            ("::ffff:0.0.0.0", "127.0.0.1"),
            ("[::1]", "::1"),
            ("127.0.0.2", "127.0.0.2"),
            ("LocalHost", "localhost"),
        ] {
            assert_eq!(
                target(host),
                TargetHost {
                    host: expected.into(),
                    loopback: true,
                },
                "{host}"
            );
        }
    }

    #[test]
    fn other_machines_are_targets_outside_the_loopback_interface() {
        for (host, expected) in [
            ("10.0.0.5", "10.0.0.5"),
            ("::ffff:10.0.0.5", "10.0.0.5"),
            ("[fe80::1]", "fe80::1"),
            ("db.internal", "db.internal"),
            ("localhost.example.com", "localhost.example.com"),
            ("my_service", "my_service"),
        ] {
            assert_eq!(
                target(host),
                TargetHost {
                    host: expected.into(),
                    loopback: false,
                },
                "{host}"
            );
        }
    }

    #[test]
    fn malformed_target_hosts_are_rejected_before_anything_is_sent() {
        let long = "a".repeat(MAX_HOST_BYTES + 1);
        for host in [
            "",
            "-oProxyCommand",
            ".hidden",
            "host name",
            "host:80",
            "[::1",
            "[db.internal]",
            "[127.0.0.1]",
            "[]",
            "host\n",
            long.as_str(),
        ] {
            assert!(
                matches!(
                    TargetHost::parse(host),
                    Err(ForwardError::InvalidTarget(TargetField::Host))
                ),
                "{host:?}"
            );
        }
    }

    #[test]
    fn the_log_counts_only_the_loopback_interface_and_never_a_wildcard_as_loopback() {
        for host in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "127.8.9.1",
            "::1",
            "[::1]",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "0.0.0.0",
            "[::]",
            "10.0.0.1",
            "example.com",
            "localhost.example.com",
            "[::2]",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn a_port_only_a_local_program_listens_on_counts_as_in_use() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        assert!(in_use_locally(listener.local_addr().unwrap().port()));
    }
}
