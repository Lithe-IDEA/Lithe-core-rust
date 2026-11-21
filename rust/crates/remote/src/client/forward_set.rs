//! The port forwards of one session, as the ports page manages them.
//!
//! A [`ForwardSet`] starts, lists and stops the forwards of one
//! [`RemoteSession`] and owns them until they stop. A forward is one of
//! three [`ForwardKind`]s:
//!
//! - **Local** ([`ForwardSpec`]): a port on this machine's loopback interface
//!   reaches a target as the remote host sees it, through tunnels.
//! - **Host** ([`HostForwardSpec`]): a port on the remote host reaches another
//!   server, and the host connects to it itself.
//! - **Reverse** ([`ReverseForwardSpec`]): a port on the remote host's
//!   loopback interface reaches a port on this machine's loopback interface.
//!
//! The rules:
//!
//! - **Targets** are named as the remote host sees them. A host listening on
//!   every interface (`0.0.0.0`, `::`) is reached through the host's own
//!   loopback address. A target that is not the host's loopback interface
//!   reaches another machine through the host, for example a database on
//!   the host's private network, so starting it requires
//!   `allow_non_loopback_target`, which the application sets only after the
//!   user confirmed that target (design S28).
//! - **Local ports.** A local forward listens on `127.0.0.1`. When the
//!   requested port is taken locally, it listens on a free port instead and
//!   reports [`ForwardInfo::local_port_changed`], so the user sees which
//!   address to open instead of a failure.
//! - **Host ports.** A host forward listens on the host's loopback interface
//!   unless [`HostForwardSpec::all_interfaces`] is set, which the
//!   application sets only after the user confirmed it for this rule. A
//!   reverse forward always listens on the host's loopback interface. A
//!   requested host port that is taken fails with
//!   [`ForwardError::HostPortInUse`] instead of moving: other machines rely
//!   on that port.
//! - **Session end** releases every forward and closes their ports (design
//!   S29). The host closes its listeners when this client disconnects, and
//!   nothing reopens them after a reconnect.
//!
//! Starting a forward identical to a running one returns the running one, so
//! a repeated click does not open a second port. All kinds count against
//! [`MAX_FORWARDS`] together.

use crate::client::forward::PortForward;
use crate::client::host_forward::{HostForward, ReverseForward};
use crate::client::session::{ClientError, RemoteSession};
use crate::protocol::message::ErrorCode;
use crate::protocol::methods::{Feature, TunnelTarget};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::Duration;

/// Most forwards one session keeps at once, of all kinds together. Each one
/// holds a port and a thread; a page of forwards is far below this.
pub const MAX_FORWARDS: usize = 64;

/// Longest host name a target may have, as in DNS.
const MAX_HOST_BYTES: usize = 253;

/// Bound for checking whether a local program already uses a port. Loopback
/// connections are refused or accepted at once; this only covers a stalled
/// local stack.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// The loopback address local and reverse forwards use on this machine, and
/// host listeners use on the host unless a rule binds every interface.
const LOOPBACK: &str = "127.0.0.1";

/// The address a host forward listens on when it binds every interface.
const ALL_INTERFACES: &str = "0.0.0.0";

/// What a local forward forwards.
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

/// A port on the remote host that reaches another server.
#[derive(Clone, Debug)]
pub struct HostForwardSpec {
    /// Host port to listen on; `None` or `0` lets the host choose one.
    pub listen_port: Option<u16>,
    /// Listen on every interface of the host instead of its loopback
    /// interface. The user confirms this each time a rule starts.
    pub all_interfaces: bool,
    /// Host as the remote host resolves it, as in [`ForwardSpec`].
    pub target_host: String,
    /// TCP port on the target, 1 to 65535.
    pub target_port: u16,
    /// The user confirmed a target outside the remote host's loopback
    /// interface (design S28).
    pub allow_non_loopback_target: bool,
}

/// A port on the remote host's loopback interface that reaches a port on
/// this machine's loopback interface.
#[derive(Clone, Debug)]
pub struct ReverseForwardSpec {
    /// Host port to listen on; `None` or `0` lets the host choose one.
    pub listen_port: Option<u16>,
    /// Port on this machine's loopback interface, 1 to 65535.
    pub local_port: u16,
}

/// Where a forward listens and where its connections go.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    /// A local port reaches a target through the host.
    Local,
    /// A host port reaches another server; the host connects to it.
    Host,
    /// A host loopback port reaches this machine's loopback interface.
    Reverse,
}

/// A running forward, as listed on the ports page.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardInfo {
    /// Identifier within the session, never reused while the session lasts.
    pub id: u64,
    /// Which kind of forward this is.
    pub kind: ForwardKind,
    /// This machine's side: the loopback address a local forward listens on,
    /// or that a reverse forward connects to. Absent for host forwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_host: Option<String>,
    /// The local port a local forward listens on, or that a reverse forward
    /// connects to. Absent for host forwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    /// Local port a local forward's request asked for, when it named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_local_port: Option<u16>,
    /// The requested local port was taken, so the local forward listens on
    /// `local_port` instead.
    pub local_port_changed: bool,
    /// The address a host or reverse forward listens on, on the host:
    /// `127.0.0.1`, or `0.0.0.0` for every interface. Absent for local
    /// forwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_listen_host: Option<String>,
    /// The port a host or reverse forward listens on, on the host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_listen_port: Option<u16>,
    /// Host port the request asked for, when it named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_host_port: Option<u16>,
    /// A host forward listens on every interface of the host.
    pub all_interfaces: bool,
    /// Target host of a local or host forward as sent to the remote host,
    /// after mapping a wildcard address to the host's loopback address.
    /// Absent for reverse forwards, whose target is `local_port`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_host: Option<String>,
    /// Target TCP port of a local or host forward.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
    /// The target is a loopback interface: the host's for local and host
    /// forwards, this machine's for reverse forwards.
    pub loopback_target: bool,
    /// Connections the forward has accepted so far.
    pub connections: u64,
    /// Accepted connections of a host or reverse forward that never reached
    /// their target. Local forwards report 0.
    pub failed_connections: u64,
}

/// The malformed part of a forward's request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetField {
    /// The target host.
    Host,
    /// The target port.
    Port,
    /// [`ReverseForwardSpec::local_port`].
    LocalPort,
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
    /// The remote server predates host listeners, so only local forwards
    /// work on this connection.
    HostListenerUnsupported,
    /// Another program on the host uses the requested host port.
    HostPortInUse,
    /// The host does not let the server listen on the requested port, such
    /// as a port below 1024 for a user without that privilege.
    HostPortDenied,
    /// The host refused to listen for another reason.
    Remote(ClientError),
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
            Self::InvalidTarget(TargetField::LocalPort) => {
                formatter.write_str("invalid reverse forward local port")
            }
            Self::TargetNotConfirmed => formatter
                .write_str("the forward target is outside the remote host and was not confirmed"),
            Self::LimitReached => {
                write!(formatter, "a session keeps at most {MAX_FORWARDS} forwards")
            }
            Self::SessionClosed => formatter.write_str("the remote connection closed"),
            Self::Listen(error) => write!(formatter, "cannot listen on a local port: {error}"),
            Self::HostListenerUnsupported => {
                formatter.write_str("the remote server cannot listen on host ports")
            }
            Self::HostPortInUse => formatter.write_str("the host port is in use"),
            Self::HostPortDenied => {
                formatter.write_str("the host does not allow listening on that port")
            }
            Self::Remote(error) => write!(formatter, "the host cannot listen: {error}"),
        }
    }
}

impl std::error::Error for ForwardError {}

impl From<ClientError> for ForwardError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Remote(remote) if remote.code == ErrorCode::AddressInUse => {
                Self::HostPortInUse
            }
            ClientError::Remote(remote) if remote.code == ErrorCode::PermissionDenied => {
                Self::HostPortDenied
            }
            ClientError::Disconnected { .. } => Self::SessionClosed,
            error => Self::Remote(error),
        }
    }
}

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
    /// Host-side forwards being started without the lock, counted against
    /// [`MAX_FORWARDS`].
    starting: usize,
    forwards: BTreeMap<u64, Entry>,
}

enum Entry {
    Local {
        forward: PortForward,
        requested_local_port: Option<u16>,
        local_port_changed: bool,
        target: TargetHost,
        target_port: u16,
    },
    Host {
        forward: HostForward,
        requested_port: Option<u16>,
        all_interfaces: bool,
        target: TargetHost,
        target_port: u16,
    },
    Reverse {
        forward: ReverseForward,
        requested_port: Option<u16>,
        local_port: u16,
    },
}

impl Entry {
    fn info(&self, id: u64) -> ForwardInfo {
        let empty = ForwardInfo {
            id,
            kind: ForwardKind::Local,
            local_host: None,
            local_port: None,
            requested_local_port: None,
            local_port_changed: false,
            host_listen_host: None,
            host_listen_port: None,
            requested_host_port: None,
            all_interfaces: false,
            target_host: None,
            target_port: None,
            loopback_target: false,
            connections: 0,
            failed_connections: 0,
        };
        match self {
            Self::Local {
                forward,
                requested_local_port,
                local_port_changed,
                target,
                target_port,
            } => {
                let local = forward.local_address();
                ForwardInfo {
                    local_host: Some(local.ip().to_string()),
                    local_port: Some(local.port()),
                    requested_local_port: *requested_local_port,
                    local_port_changed: *local_port_changed,
                    target_host: Some(target.host.clone()),
                    target_port: Some(*target_port),
                    loopback_target: target.loopback,
                    connections: forward.connections(),
                    ..empty
                }
            }
            Self::Host {
                forward,
                requested_port,
                all_interfaces,
                target,
                target_port,
            } => ForwardInfo {
                kind: ForwardKind::Host,
                host_listen_host: Some(listen_host(*all_interfaces).into()),
                host_listen_port: Some(forward.port()),
                requested_host_port: *requested_port,
                all_interfaces: *all_interfaces,
                target_host: Some(target.host.clone()),
                target_port: Some(*target_port),
                loopback_target: target.loopback,
                ..empty
            },
            Self::Reverse {
                forward,
                requested_port,
                local_port,
            } => ForwardInfo {
                kind: ForwardKind::Reverse,
                local_host: Some(LOOPBACK.into()),
                local_port: Some(*local_port),
                host_listen_host: Some(LOOPBACK.into()),
                host_listen_port: Some(forward.port()),
                requested_host_port: *requested_port,
                loopback_target: true,
                failed_connections: forward.local_failures(),
                ..empty
            },
        }
    }

    /// The host listener behind a host or reverse forward.
    fn listener_id(&self) -> Option<u64> {
        match self {
            Self::Local { .. } => None,
            Self::Host { forward, .. } => Some(forward.listener_id()),
            Self::Reverse { forward, .. } => Some(forward.listener_id()),
        }
    }

    /// Whether this forward already serves a request for the same rule. A
    /// request that lets the system choose a port matches any port.
    fn serves(&self, other: &Entry) -> bool {
        let same_port = |requested: &Option<u16>, port: u16| requested.is_none_or(|r| r == port);
        match (self, other) {
            (
                Self::Host {
                    forward,
                    all_interfaces,
                    target,
                    target_port,
                    ..
                },
                Self::Host {
                    requested_port,
                    all_interfaces: other_all,
                    target: other_target,
                    target_port: other_port,
                    ..
                },
            ) => {
                all_interfaces == other_all
                    && target.host == other_target.host
                    && target_port == other_port
                    && same_port(requested_port, forward.port())
            }
            (
                Self::Reverse {
                    forward,
                    local_port,
                    ..
                },
                Self::Reverse {
                    requested_port,
                    local_port: other_local,
                    ..
                },
            ) => local_port == other_local && same_port(requested_port, forward.port()),
            _ => false,
        }
    }
}

fn listen_host(all_interfaces: bool) -> &'static str {
    if all_interfaces {
        ALL_INTERFACES
    } else {
        LOOPBACK
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

    /// Starts a local forward, or returns the forward that already reaches
    /// the same target. The flag is `true` for a new forward.
    pub fn start(&self, spec: ForwardSpec) -> Result<(ForwardInfo, bool), ForwardError> {
        let target = confirmed_target(
            &spec.target_host,
            spec.target_port,
            spec.allow_non_loopback_target,
        )?;
        let requested = spec.local_port.filter(|port| *port != 0);
        // Held while the forward starts, so two requests for one target
        // cannot both open a local port. Opening a local port never waits
        // for the session.
        let mut state = lock(&self.state);
        if state.closed || self.session.is_closed() {
            return Err(ForwardError::SessionClosed);
        }
        if let Some((id, entry)) = state.forwards.iter().find(|(_, entry)| {
            matches!(entry, Entry::Local { target: running, target_port, .. }
                if running.host == target.host && *target_port == spec.target_port)
        }) {
            return Ok((entry.info(*id), false));
        }
        if state.forwards.len() + state.starting >= MAX_FORWARDS {
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
        let entry = Entry::Local {
            forward,
            requested_local_port: requested,
            local_port_changed,
            target,
            target_port: spec.target_port,
        };
        let info = entry.info(id);
        state.forwards.insert(id, entry);
        Ok((info, true))
    }

    /// Starts a host forward, or returns the one that already serves the
    /// same rule. The flag is `true` for a new forward.
    pub fn start_host(&self, spec: HostForwardSpec) -> Result<(ForwardInfo, bool), ForwardError> {
        let target = confirmed_target(
            &spec.target_host,
            spec.target_port,
            spec.allow_non_loopback_target,
        )?;
        let requested = spec.listen_port.filter(|port| *port != 0);
        let reservation = self.reserve()?;
        let forward = HostForward::start(
            Arc::clone(&self.session),
            requested.unwrap_or(0),
            spec.all_interfaces,
            &target.host,
            spec.target_port,
        )?;
        reservation.commit(Entry::Host {
            forward,
            requested_port: requested,
            all_interfaces: spec.all_interfaces,
            target,
            target_port: spec.target_port,
        })
    }

    /// Starts a reverse forward, or returns the one that already serves the
    /// same rule. The flag is `true` for a new forward. The application
    /// offers reverse forwards only when the user allowed them.
    pub fn start_reverse(
        &self,
        spec: ReverseForwardSpec,
    ) -> Result<(ForwardInfo, bool), ForwardError> {
        if spec.local_port == 0 {
            return Err(ForwardError::InvalidTarget(TargetField::LocalPort));
        }
        let requested = spec.listen_port.filter(|port| *port != 0);
        let reservation = self.reserve()?;
        let forward = ReverseForward::start(
            Arc::clone(&self.session),
            requested.unwrap_or(0),
            spec.local_port,
        )?;
        reservation.commit(Entry::Reverse {
            forward,
            requested_port: requested,
            local_port: spec.local_port,
        })
    }

    /// Takes a place for a host-side forward. The lock is not held while
    /// the host answers: the session's close hook takes it from the thread
    /// that delivers the answer.
    fn reserve(&self) -> Result<Reservation<'_>, ForwardError> {
        if !self
            .session
            .hello()
            .host
            .features
            .contains(&Feature::Listener)
        {
            return Err(ForwardError::HostListenerUnsupported);
        }
        let mut state = lock(&self.state);
        if state.closed || self.session.is_closed() {
            return Err(ForwardError::SessionClosed);
        }
        if state.forwards.len() + state.starting >= MAX_FORWARDS {
            return Err(ForwardError::LimitReached);
        }
        state.starting += 1;
        Ok(Reservation { set: self })
    }

    /// Stops one forward and closes its port. Connections a local or reverse
    /// forward already accepted run until either side closes them; a host
    /// forward closes its relayed connections. Returns whether the forward
    /// existed.
    pub fn stop(&self, id: u64) -> bool {
        let removed = lock(&self.state).forwards.remove(&id);
        // Dropped outside the lock: stopping waits for the acceptor thread
        // or the host.
        removed.is_some()
    }

    /// The running forwards, by identifier, with the host's current counts
    /// for host and reverse forwards.
    pub fn list(&self) -> Result<Vec<ForwardInfo>, ForwardError> {
        let (mut forwards, listeners) = {
            let state = lock(&self.state);
            if state.closed || self.session.is_closed() {
                return Err(ForwardError::SessionClosed);
            }
            let forwards: Vec<ForwardInfo> = state
                .forwards
                .iter()
                .map(|(id, entry)| entry.info(*id))
                .collect();
            let listeners: Vec<(usize, u64)> = state
                .forwards
                .values()
                .enumerate()
                .filter_map(|(index, entry)| entry.listener_id().map(|id| (index, id)))
                .collect();
            (forwards, listeners)
        };
        if listeners.is_empty() {
            return Ok(forwards);
        }
        // Asked outside the lock, for the same reason as in `reserve`.
        let counts: HashMap<u64, (u64, u64)> = self
            .session
            .listeners()?
            .into_iter()
            .map(|state| {
                (
                    state.listener_id,
                    (state.connections, state.failed_connections),
                )
            })
            .collect();
        for (index, listener_id) in listeners {
            if let Some((connections, failed)) = counts.get(&listener_id) {
                let info = &mut forwards[index];
                info.connections = *connections;
                info.failed_connections += failed;
            }
        }
        Ok(forwards)
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

/// A place taken for a host-side forward that is starting.
struct Reservation<'a> {
    set: &'a ForwardSet,
}

impl Reservation<'_> {
    /// Adds the started forward, or returns the forward that served the same
    /// rule meanwhile; the new one then closes again. A set that closed
    /// meanwhile closes it too.
    fn commit(self, entry: Entry) -> Result<(ForwardInfo, bool), ForwardError> {
        let mut state = lock(&self.set.state);
        if state.closed {
            drop(state);
            drop(entry);
            return Err(ForwardError::SessionClosed);
        }
        if let Some((id, running)) = state
            .forwards
            .iter()
            .find(|(_, running)| running.serves(&entry))
        {
            let info = running.info(*id);
            drop(state);
            drop(entry);
            return Ok((info, false));
        }
        state.next_id += 1;
        let id = state.next_id;
        let info = entry.info(id);
        state.forwards.insert(id, entry);
        Ok((info, true))
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        lock(&self.set.state).starting -= 1;
    }
}

/// Validates a target host and port, and requires the confirmation for a
/// target outside the host's loopback interface.
fn confirmed_target(
    host: &str,
    port: u16,
    allow_non_loopback_target: bool,
) -> Result<TargetHost, ForwardError> {
    let target = TargetHost::parse(host)?;
    if port == 0 {
        return Err(ForwardError::InvalidTarget(TargetField::Port));
    }
    if !target.loopback && !allow_non_loopback_target {
        return Err(ForwardError::TargetNotConfirmed);
    }
    Ok(target)
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

/// True for a host a forward can target: an IP address, IPv6 in brackets or
/// not, or a host name.
pub(crate) fn is_target_host(host: &str) -> bool {
    TargetHost::parse(host).is_ok()
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
