//! Local port forwarding through tunnels.
//!
//! A [`PortForward`] listens on a local address. Every accepted connection
//! opens its own tunnel to the remote target and copies bytes both ways, so
//! `http://127.0.0.1:<local port>` in a browser reaches a server that only
//! listens on the remote machine's loopback interface.
//!
//! Forwards listen on loopback addresses only. A forward on every interface
//! would hand the remote service, often a development server without any
//! authentication, to everyone on the local network, so
//! [`PortForward::start`] refuses other addresses instead of leaving the
//! choice to each caller.
//!
//! One forward bridges at most 64 connections at once. Further connections
//! are accepted only when one of them ends: until then they wait in the
//! system's listen queue, as with a busy server, instead of costing threads
//! here and a tunnel on the remote, which limits tunnels per session.
//!
//! Starting and removing a forward are recorded in the session's
//! [`ConnectionLog`] with the local port and the number of accepted
//! connections; the remote host and socket path are not recorded.

use crate::client::api::TunnelEvent;
use crate::client::connection_log::ConnectionLog;
use crate::client::session::{ClientError, RemoteSession};
use crate::protocol::methods::TunnelTarget;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const READ_CHUNK_BYTES: usize = 32 * 1024;

/// Most connections one forward bridges at once.
const MAX_CONNECTIONS: usize = 64;

/// Pause after a failed `accept`.
const ACCEPT_FAILURE_BACKOFF: Duration = Duration::from_millis(200);

/// How often an acceptor waiting for a free connection slot checks whether
/// its forward is being removed.
const STOP_CHECK: Duration = Duration::from_millis(200);

/// Bound for the connection that wakes the acceptor when the forward is
/// removed, so that removing a forward never hangs on its own listener.
const WAKEUP_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a tunnel waits for remote data before checking whether the
/// local side has gone away. Data ends the wait immediately.
const IDLE_CHECK: Duration = Duration::from_secs(1);

/// A running local port forward. Dropping it stops accepting connections;
/// connections already accepted run until either side closes.
pub struct PortForward {
    local_address: SocketAddr,
    stopping: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
    /// Local connections handed to a bridge so far.
    accepted: Arc<AtomicU64>,
    started: Instant,
    log: ConnectionLog,
}

impl PortForward {
    /// Starts forwarding `bind` to `target`. `bind` must be a loopback
    /// address such as `127.0.0.1` or `::1`; any other address fails with
    /// [`io::ErrorKind::InvalidInput`] before a socket is opened. Use port 0
    /// to let the system choose a free local port, then read
    /// [`PortForward::local_address`].
    pub fn start(
        session: Arc<RemoteSession>,
        bind: SocketAddr,
        target: TunnelTarget,
    ) -> io::Result<Self> {
        if !bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "port forwards listen on loopback addresses only",
            ));
        }
        let listener = TcpListener::bind(bind)?;
        let local_address = listener.local_addr()?;
        let stopping = Arc::new(AtomicBool::new(false));
        let accepted = Arc::new(AtomicU64::new(0));
        let log = session.connection_log().clone();
        let started = Instant::now();
        let acceptor = Acceptor {
            listener,
            session,
            target: target.clone(),
            stopping: Arc::clone(&stopping),
            accepted: Arc::clone(&accepted),
            log: log.clone(),
            slots: Arc::new(Slots::new(MAX_CONNECTIONS)),
        };
        let acceptor = thread::Builder::new()
            .name("remote-forward".into())
            .spawn(move || acceptor.run())?;
        log.forward_started(local_address, &target);
        Ok(Self {
            local_address,
            stopping,
            acceptor: Some(acceptor),
            accepted,
            started,
            log,
        })
    }

    /// Address the forward listens on, with the port the system chose for port 0.
    pub fn local_address(&self) -> SocketAddr {
        self.local_address
    }

    /// Connects to the listener so that an acceptor waiting in `accept`
    /// observes `stopping`, and returns whether joining the acceptor is
    /// bounded. An acceptor waiting for a free slot needs no wakeup.
    fn wake_acceptor(&self) -> bool {
        match TcpStream::connect_timeout(&self.local_address, WAKEUP_TIMEOUT) {
            Ok(_) => true,
            // log-exempt: Expected absence: ConnectionRefused means the listener closed because its acceptor exited after the caller checked it.
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => true,
            Err(error) => {
                crate::failure_log::io::<()>(
                    "remote.client.operation_failed",
                    "acceptor_wakeup",
                    Err(error),
                );
                false
            }
        }
    }
}

impl Drop for PortForward {
    /// Stops accepting new connections. Connections already bridged continue
    /// until either side closes them.
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(acceptor) = self.acceptor.take() {
            // Without a wakeup, for example when no descriptor is left for
            // it, the acceptor may wait in `accept` until some connection
            // arrives. It is then left to end on that connection, which
            // keeps the listener open until it does, rather than blocking
            // the caller.
            if acceptor.is_finished() || self.wake_acceptor() {
                crate::failure_log::outcome(
                    "remote.client.operation_failed",
                    "forward_acceptor_join",
                    acceptor.join(),
                    "panic",
                    "thread",
                );
            }
        }
        self.log.forward_removed(
            self.local_address.port(),
            self.accepted.load(Ordering::Relaxed),
            self.started.elapsed(),
        );
    }
}

/// The thread that accepts local connections and starts their bridges.
struct Acceptor {
    listener: TcpListener,
    session: Arc<RemoteSession>,
    target: TunnelTarget,
    stopping: Arc<AtomicBool>,
    accepted: Arc<AtomicU64>,
    log: ConnectionLog,
    slots: Arc<Slots>,
}

impl Acceptor {
    fn run(self) {
        loop {
            // A connection is accepted only once it can be bridged.
            let Some(slot) = self.slots.acquire(&self.stopping) else {
                return;
            };
            let connection = self.listener.accept();
            if self.stopping.load(Ordering::Acquire) {
                return;
            }
            match connection {
                Ok((connection, _)) => self.bridge(connection, slot),
                Err(error) => {
                    forward_failed(
                        "remote.forward.accept_failed",
                        &self.log,
                        "accept",
                        &format!("{:?}", error.kind()),
                        "io",
                        &ACCEPT_LOG,
                    );
                    // Failures such as EMFILE repeat immediately; pausing
                    // keeps the acceptor from spinning.
                    thread::sleep(ACCEPT_FAILURE_BACKOFF);
                }
            }
        }
    }

    fn bridge(&self, connection: TcpStream, slot: Slot) {
        let session = Arc::clone(&self.session);
        let target = self.target.clone();
        let log = self.log.clone();
        let spawned = thread::Builder::new()
            .name("remote-forward-connection".into())
            .spawn(move || {
                let _slot = slot;
                if let Err(error) = bridge(&session, connection, target) {
                    if let Some(category) = crate::failure_log::bridge_category(&error) {
                        forward_failed(
                            "remote.forward.connection_failed",
                            &log,
                            "bridge",
                            &category,
                            "remote",
                            &BRIDGE_LOG,
                        );
                    }
                }
            });
        match spawned {
            Ok(_) => {
                self.accepted.fetch_add(1, Ordering::Relaxed);
            }
            // The closure that never ran is dropped with the error, which
            // closes the local connection and frees its slot.
            Err(error) => forward_failed(
                "remote.forward.connection_failed",
                &self.log,
                "spawn",
                &format!("{:?}", error.kind()),
                "io",
                &BRIDGE_LOG,
            ),
        }
    }
}

/// The connections one forward is bridging, at most `limit` at once.
struct Slots {
    limit: usize,
    active: Mutex<usize>,
    freed: Condvar,
}

impl Slots {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            active: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Waits for a free slot, or returns `None` once the forward stops.
    fn acquire(self: &Arc<Self>, stopping: &AtomicBool) -> Option<Slot> {
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        while *active >= self.limit {
            if stopping.load(Ordering::Acquire) {
                return None;
            }
            active = self
                .freed
                .wait_timeout(active, STOP_CHECK)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *active += 1;
        Some(Slot(Arc::clone(self)))
    }
}

/// One bridged connection's slot, freed when its bridge ends, including when
/// the bridge thread never starts or panics.
struct Slot(Arc<Slots>);

impl Drop for Slot {
    fn drop(&mut self) {
        *self.0.active.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.freed.notify_one();
    }
}

fn bridge(
    session: &RemoteSession,
    local: TcpStream,
    target: TunnelTarget,
) -> Result<(), ClientError> {
    crate::failure_log::teardown(
        "remote.client.operation_failed",
        "nodelay",
        local.set_nodelay(true),
    );
    let tunnel = match session.open_tunnel(target) {
        Ok(tunnel) => tunnel,
        Err(error) => {
            // log-exempt: Same failure already recorded: the tunnel setup error propagates to the bridge boundary and is recorded there, so closing the local connection afterwards is not recorded again.
            let _ = local.shutdown(Shutdown::Both);
            return Err(error);
        }
    };
    let mut to_remote = local.try_clone().map_err(ClientError::Spawn)?;
    let mut to_local = local;
    let local_done = Arc::new(AtomicBool::new(false));

    thread::scope(|scope| {
        let local_done_writer = Arc::clone(&local_done);
        let writer = tunnel.writer();
        let upload = thread::Builder::new()
            .name("remote-forward-upload".into())
            .spawn_scoped(scope, move || {
                let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
                let local_ended = loop {
                    match to_remote.read(&mut buffer) {
                        Ok(0) => break true,
                        Ok(read) => {
                            if writer.write(&buffer[..read]).is_err() {
                                break false;
                            }
                        }
                        Err(_) => break false,
                    }
                };
                local_done_writer.store(true, Ordering::Release);
                if local_ended {
                    // The local client finished sending, as HTTP/1.0 clients
                    // and `nc -N` do, and now waits for the reply: pass the
                    // half-close on after the queued data, and keep receiving.
                    crate::failure_log::client("tunnel_half_close", writer.shutdown_write());
                } else {
                    crate::failure_log::client("tunnel_close", writer.close());
                }
            });
        if let Err(error) = upload {
            crate::failure_log::client("tunnel_close", tunnel.close());
            // log-exempt: Same failure already recorded: the spawn error propagates to the bridge boundary and is recorded there, so closing the local connection afterwards is not recorded again.
            let _ = to_local.shutdown(Shutdown::Both);
            return Err(ClientError::Spawn(error));
        }
        loop {
            match tunnel.next_event(IDLE_CHECK) {
                Ok(Some(TunnelEvent::Data(bytes))) => {
                    if to_local.write_all(&bytes).is_err() {
                        crate::failure_log::client("tunnel_close", tunnel.close());
                        break;
                    }
                }
                Ok(Some(TunnelEvent::Closed(_))) | Ok(None) => break,
                Err(ClientError::Timeout) => {
                    if local_done.load(Ordering::Acquire) && session.is_closed() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        crate::failure_log::teardown(
            "remote.client.operation_failed",
            "socket_shutdown",
            to_local.shutdown(Shutdown::Both),
        );
        Ok(())
    })
}

// Each forward failure family emits at most once per five seconds process-wide,
// matching the catalog's rate=12 even across concurrent forwarded connections.
const FAILURE_LOG_INTERVAL: Duration = Duration::from_secs(5);
static ACCEPT_LOG: Mutex<Option<Instant>> = Mutex::new(None);
static BRIDGE_LOG: Mutex<Option<Instant>> = Mutex::new(None);

fn permit_failure(last: &mut Option<Instant>, now: Instant) -> bool {
    if last.is_some_and(|previous| now.saturating_duration_since(previous) < FAILURE_LOG_INTERVAL) {
        return false;
    }
    *last = Some(now);
    true
}

fn forward_failed(
    event: &str,
    log: &ConnectionLog,
    stage: &'static str,
    code: &str,
    domain: &'static str,
    gate: &Mutex<Option<Instant>>,
) {
    let permitted = permit_failure(
        &mut gate.lock().unwrap_or_else(|error| error.into_inner()),
        Instant::now(),
    );
    if permitted {
        crate::failure_log::failed_for(event, log.id(), stage, code, domain);
    }
}

#[cfg(test)]
mod failure_tests {
    #[test]
    fn forwarding_failure_rate_is_bounded_without_real_time_waits() {
        let now = std::time::Instant::now();
        let mut last = None;
        assert!(super::permit_failure(&mut last, now));
        assert!(!super::permit_failure(&mut last, now));
        assert!(!super::permit_failure(
            &mut last,
            now + std::time::Duration::from_secs(4)
        ));
        assert!(super::permit_failure(
            &mut last,
            now + super::FAILURE_LOG_INTERVAL
        ));
    }
}

#[cfg(test)]
mod slot_tests {
    use super::Slots;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn a_full_forward_admits_a_connection_once_another_ends() {
        let slots = Arc::new(Slots::new(2));
        let running = AtomicBool::new(false);
        let first = slots.acquire(&running).unwrap();
        let _second = slots.acquire(&running).unwrap();
        // A stopping forward gives up instead of waiting for a free slot.
        assert!(slots.acquire(&AtomicBool::new(true)).is_none());
        drop(first);
        assert!(slots.acquire(&running).is_some());
    }
}
