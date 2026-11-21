//! Forwards that listen on the remote host.
//!
//! - A [`HostForward`] makes the host listen on a port and connect every
//!   accepted connection to another server itself, as `ssh -L` does on a jump
//!   host. The bytes never cross the session.
//! - A [`ReverseForward`] makes the host listen on its loopback interface and
//!   hands every accepted connection to this machine, which connects it to a
//!   port on its own loopback interface, as `ssh -R 127.0.0.1:…` does.
//!
//! Both are listeners of the session: the host closes them when this client
//! disconnects, and they are not reopened after a reconnect. A reverse forward
//! only ever reaches this machine's loopback interface, so a remote user
//! cannot use it to reach the local network.
//!
//! Starting and removing a forward are recorded in the session's
//! [`ConnectionLog`] with the ports and whether each side is a loopback
//! address; hosts are not recorded.

use crate::client::connection_log::ConnectionLog;
use crate::client::forward::{forward_failed, pump, Slot, Slots, BRIDGE_LOG};
use crate::client::session::{ClientError, Event, RemoteSession};
use crate::protocol::methods::{self, ListenerAcceptedEvent, TunnelTarget};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Most connections one reverse forward bridges at once.
const MAX_CONNECTIONS: usize = 64;

/// How long a reverse connection waits for the local port to accept.
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The address a host forward listens on: loopback unless the user confirmed
/// every interface for this rule.
fn listen_host(all_interfaces: bool) -> &'static str {
    if all_interfaces {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    }
}

/// A host port that relays to another server. Dropping it closes the port;
/// connections already relayed are closed with it.
pub struct HostForward {
    session: Arc<RemoteSession>,
    listener_id: u64,
    port: u16,
    started: Instant,
    log: ConnectionLog,
}

impl HostForward {
    /// Makes the host listen on `port`, 0 for a port the host chooses, and
    /// connect every accepted connection to `target_host:target_port`. The
    /// host listens on its loopback interface unless `all_interfaces` is set.
    pub fn start(
        session: Arc<RemoteSession>,
        port: u16,
        all_interfaces: bool,
        target_host: &str,
        target_port: u16,
    ) -> Result<Self, ClientError> {
        let listener_id = session.allocate_id();
        let target = TunnelTarget::Tcp {
            host: target_host.into(),
            port: target_port,
        };
        let port =
            session.open_listener(listener_id, listen_host(all_interfaces), port, Some(target))?;
        let log = session.connection_log().clone();
        log.host_forward_started(port, all_interfaces, target_host, target_port);
        Ok(Self {
            session,
            listener_id,
            port,
            started: Instant::now(),
            log,
        })
    }

    /// The port the host listens on, with the port it chose for port 0.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The listener's id in [`RemoteSession::listeners`].
    pub fn listener_id(&self) -> u64 {
        self.listener_id
    }
}

impl Drop for HostForward {
    fn drop(&mut self) {
        let (connections, failed) = close(&self.session, self.listener_id);
        self.log
            .host_forward_removed(self.port, connections, failed, self.started.elapsed());
    }
}

/// A host loopback port whose connections reach a port on this machine's
/// loopback interface. Dropping it closes the host port; connections already
/// bridged run until either side closes.
pub struct ReverseForward {
    session: Arc<RemoteSession>,
    listener_id: u64,
    port: u16,
    stopping: Arc<AtomicBool>,
    handler: Option<JoinHandle<()>>,
    /// Connections the host handed over that could not reach the local port.
    failed: Arc<AtomicU64>,
    started: Instant,
    log: ConnectionLog,
}

impl ReverseForward {
    /// Makes the host listen on its loopback `port`, 0 for a port the host
    /// chooses, and connects every accepted connection to `local_port` on
    /// this machine's loopback interface.
    pub fn start(
        session: Arc<RemoteSession>,
        port: u16,
        local_port: u16,
    ) -> Result<Self, ClientError> {
        let listener_id = session.allocate_id();
        let events = session.subscribe("listener", listener_id);
        let port = match session.open_listener(listener_id, "127.0.0.1", port, None) {
            Ok(port) => port,
            Err(error) => {
                session.unsubscribe("listener", listener_id);
                return Err(error);
            }
        };
        let log = session.connection_log().clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicU64::new(0));
        let handler = Handler {
            session: Arc::clone(&session),
            listener_id,
            local_port,
            events,
            stopping: Arc::clone(&stopping),
            failed: Arc::clone(&failed),
            slots: Arc::new(Slots::new(MAX_CONNECTIONS)),
            log: log.clone(),
        };
        let handler = match thread::Builder::new()
            .name("remote-reverse-forward".into())
            .spawn(move || handler.run())
        {
            Ok(handler) => handler,
            Err(error) => {
                close(&session, listener_id);
                session.unsubscribe("listener", listener_id);
                return Err(ClientError::Spawn(error));
            }
        };
        log.reverse_forward_started(port, local_port);
        Ok(Self {
            session,
            listener_id,
            port,
            stopping,
            handler: Some(handler),
            failed,
            started: Instant::now(),
            log,
        })
    }

    /// The loopback port the host listens on, with the port it chose for port 0.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The listener's id in [`RemoteSession::listeners`].
    pub fn listener_id(&self) -> u64 {
        self.listener_id
    }

    /// Connections the host handed over that could not reach the local port.
    pub fn local_failures(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }
}

impl Drop for ReverseForward {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let (connections, failed) = close(&self.session, self.listener_id);
        // Ending the subscription disconnects the handler's channel, so the
        // handler returns even when no event arrives.
        self.session.unsubscribe("listener", self.listener_id);
        if let Some(handler) = self.handler.take() {
            crate::failure_log::outcome(
                "remote.client.operation_failed",
                "reverse_forward_join",
                handler.join(),
                "panic",
                "thread",
            );
        }
        self.log.host_forward_removed(
            self.port,
            connections,
            failed + self.failed.load(Ordering::Relaxed),
            self.started.elapsed(),
        );
    }
}

/// Closes a listener and returns its final connection and failure counts. A
/// session that already ended closed it on the host, and its counts are gone.
fn close(session: &RemoteSession, listener_id: u64) -> (u64, u64) {
    if session.is_closed() {
        return (0, 0);
    }
    match session.close_listener(listener_id) {
        Ok(state) => (state.connections, state.failed_connections),
        Err(error) => {
            crate::failure_log::client::<()>("listener_close", Err(error));
            (0, 0)
        }
    }
}

/// The thread that takes a reverse forward's accepted connections.
struct Handler {
    session: Arc<RemoteSession>,
    listener_id: u64,
    local_port: u16,
    events: Receiver<Event>,
    stopping: Arc<AtomicBool>,
    failed: Arc<AtomicU64>,
    slots: Arc<Slots>,
    log: ConnectionLog,
}

impl Handler {
    fn run(self) {
        // The channel disconnects when the forward is removed or the session
        // ends.
        while let Ok(event) = self.events.recv() {
            if self.stopping.load(Ordering::Acquire) {
                return;
            }
            if event.event != methods::EVENT_LISTENER_ACCEPTED {
                continue;
            }
            let accepted = match serde_json::from_value::<ListenerAcceptedEvent>(event.params) {
                Ok(accepted) => accepted,
                Err(_) => {
                    forward_failed(
                        "remote.forward.connection_failed",
                        &self.log,
                        "accepted_event",
                        "invalid",
                        "protocol",
                        &BRIDGE_LOG,
                    );
                    continue;
                }
            };
            // A connection that waits here past the host's adoption timeout
            // is closed by the host and counted there.
            let Some(slot) = self.slots.acquire(&self.stopping) else {
                return;
            };
            self.bridge(accepted.accept_id, slot);
        }
        if !self.stopping.load(Ordering::Acquire) && !self.session.is_closed() {
            // The subscription was dropped because its queue overflowed: no
            // later connection would be adopted, so the port closes now.
            close(&self.session, self.listener_id);
        }
    }

    fn bridge(&self, accept_id: u64, slot: Slot) {
        let session = Arc::clone(&self.session);
        let listener_id = self.listener_id;
        let local_port = self.local_port;
        let failed = Arc::clone(&self.failed);
        let log = self.log.clone();
        let spawned = thread::Builder::new()
            .name("remote-reverse-connection".into())
            .spawn(move || {
                let _slot = slot;
                let result = reverse_bridge(&session, listener_id, accept_id, local_port, &failed);
                if let Err(failure) = result {
                    failure.record(&log);
                }
            });
        if let Err(error) = spawned {
            // The host closes the connection when it is not adopted in time.
            forward_failed(
                "remote.forward.connection_failed",
                &self.log,
                "spawn",
                &format!("{:?}", error.kind()),
                "io",
                &BRIDGE_LOG,
            );
        }
    }
}

/// Why a reverse connection ended early.
enum ReverseFailure {
    Remote(ClientError),
    Local(io::ErrorKind),
}

impl ReverseFailure {
    fn record(&self, log: &ConnectionLog) {
        match self {
            Self::Remote(error) => {
                if let Some(category) = crate::failure_log::bridge_category(error) {
                    forward_failed(
                        "remote.forward.connection_failed",
                        log,
                        "adopt",
                        &category,
                        "remote",
                        &BRIDGE_LOG,
                    );
                }
            }
            Self::Local(kind) => forward_failed(
                "remote.forward.connection_failed",
                log,
                "local_connect",
                &format!("{kind:?}"),
                "io",
                &BRIDGE_LOG,
            ),
        }
    }
}

fn reverse_bridge(
    session: &RemoteSession,
    listener_id: u64,
    accept_id: u64,
    local_port: u16,
    failed: &AtomicU64,
) -> Result<(), ReverseFailure> {
    // The host counts connections it could not hand over.
    let tunnel = session
        .adopt_tunnel(listener_id, accept_id)
        .map_err(ReverseFailure::Remote)?;
    let local = match connect_loopback(local_port) {
        Ok(local) => local,
        Err(error) => {
            failed.fetch_add(1, Ordering::Relaxed);
            crate::failure_log::client("tunnel_close", tunnel.close());
            return Err(ReverseFailure::Local(error.kind()));
        }
    };
    crate::failure_log::teardown(
        "remote.client.operation_failed",
        "nodelay",
        local.set_nodelay(true),
    );
    pump(session, local, tunnel).map_err(ReverseFailure::Remote)
}

/// Connects to `port` on this machine's loopback interface, IPv4 first.
fn connect_loopback(port: u16) -> io::Result<TcpStream> {
    let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&v4, LOCAL_CONNECT_TIMEOUT) {
        Ok(stream) => Ok(stream),
        Err(first) => {
            let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
            // log-exempt: Same failure already recorded: the IPv4 error is returned and recorded by the caller when IPv6 fails too.
            TcpStream::connect_timeout(&v6, LOCAL_CONNECT_TIMEOUT).map_err(|_| first)
        }
    }
}
