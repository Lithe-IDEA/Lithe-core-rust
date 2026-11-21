//! Host ports that accept connections for forwarding rules the user added.
//!
//! A host rule (`relay`) connects every accepted connection to another
//! endpoint as the host sees it, such as a database on a second server, and
//! copies the bytes on the host; nothing crosses the protocol stream. A
//! reverse rule hands every accepted connection to the client as a
//! `listener.accepted` event, and the client adopts it as a tunnel that it
//! connects to a port on the user's machine. A listener belongs to the client
//! connection that opened it and closes with it; clients do not reopen one on
//! their own, so a host port is open only while the user's rule is.

use crate::protocol::methods::{
    self, ListenerAcceptedEvent, ListenerAdoptParams, ListenerOpenParams, ListenerOpened,
    ListenerParams, ListenerState, ListenersResult, TunnelTarget,
};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::connection::Context;
use crate::server::resources::Registry;
use crate::server::session::Session;
use crate::server::threads::unavailable;
use crate::server::tunnel::{self, Stream};
use crate::server::{lock, parse};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Listeners one session keeps open: one per rule. Clients cap their rules
/// at the same number.
const MAX_LISTENERS: usize = 64;
/// Relayed connections of one listener at once. Further connections wait in
/// the kernel's backlog until one ends.
const MAX_RELAYS_PER_LISTENER: usize = 64;
/// Relayed connections of all of a session's listeners at once. Each holds
/// two sockets and two threads; connections over the limit are closed and
/// counted as failed.
const MAX_RELAYS: usize = 256;
/// Accepted connections of one reverse listener waiting for the client.
/// Further connections wait in the kernel's backlog.
const MAX_PENDING: usize = 16;
/// How long an accepted connection waits for `listener.adopt`. A client that
/// missed the event or cannot serve it must not hold the slot.
const ADOPT_TIMEOUT: Duration = Duration::from_secs(10);
/// Pause after a failed `accept`, such as when the process is out of file
/// descriptors, so the loop does not spin while the condition lasts.
const ACCEPT_RETRY: Duration = Duration::from_millis(200);
/// How long `listener.close` waits for the port to be released, so that a
/// rule edited to the same port can bind it again at once.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

pub(crate) struct ListenerTable {
    registry: Registry<OpenListener>,
    /// Relayed connections of all listeners, against [`MAX_RELAYS`].
    relays: Arc<AtomicUsize>,
}

struct OpenListener {
    connection: u64,
    port: u16,
    shared: Arc<Shared>,
    /// Disconnects when the accept loop has ended and released the port.
    ended: Receiver<()>,
}

impl OpenListener {
    fn state(&self, listener_id: u64) -> ListenerState {
        ListenerState {
            listener_id,
            port: self.port,
            connections: self.shared.connections.load(Ordering::Relaxed),
            failed_connections: self.shared.failed.load(Ordering::Relaxed),
        }
    }
}

/// What the accept loop, relays and requests share about one listener.
struct Shared {
    state: Mutex<State>,
    /// Write side of the accept loop's wake-up socket. A byte means the
    /// state changed; it is non-blocking, and a full buffer already holds a
    /// wake-up.
    waker: UnixStream,
    connections: AtomicU64,
    failed: AtomicU64,
}

#[derive(Default)]
struct State {
    stopping: bool,
    next_accept: u64,
    /// The sockets of every relayed connection, shut down on close.
    relays: BTreeMap<u64, Vec<Stream>>,
    /// Accepted connections waiting for `listener.adopt`, with the moment
    /// they are given up.
    pending: BTreeMap<u64, (TcpStream, Instant)>,
}

impl Shared {
    fn wake(&self) {
        let _ = (&self.waker).write(&[1]);
    }

    /// Ends the accept loop and every connection the listener still holds.
    fn stop(&self) {
        let mut state = lock(&self.state);
        state.stopping = true;
        for streams in std::mem::take(&mut state.relays).into_values() {
            for stream in streams {
                stream.shutdown(Shutdown::Both);
            }
        }
        state.pending.clear();
        drop(state);
        self.wake();
    }
}

impl ListenerTable {
    pub(crate) fn new() -> Self {
        Self {
            registry: Registry::bounded("listener", MAX_LISTENERS),
            relays: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn handle(
        &self,
        context: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value, RemoteError> {
        match method {
            methods::LISTENER_OPEN => {
                let port = self.open(context, parse(params)?)?;
                crate::server::to_value(ListenerOpened { port })
            }
            methods::LISTENER_CLOSE => {
                let params: ListenerParams = parse(params)?;
                let listener = self
                    .registry
                    .remove(params.listener_id)
                    .ok_or_else(|| self.registry.unknown(params.listener_id))?;
                listener.shared.stop();
                // The loop ends right after the wake-up; the wait only
                // bounds a host that does not schedule it.
                if listener.ended.recv_timeout(CLOSE_WAIT) == Err(RecvTimeoutError::Timeout) {
                    eprintln!("lithe-remote-server: a listener did not release its port in time");
                }
                crate::server::to_value(listener.state(params.listener_id))
            }
            methods::LISTENER_ADOPT => {
                self.adopt(context, parse(params)?)?;
                Ok(Value::Null)
            }
            methods::LISTENER_LIST => crate::server::to_value(ListenersResult {
                listeners: self
                    .registry
                    .lock()
                    .iter()
                    .map(|(listener_id, listener)| listener.state(*listener_id))
                    .collect(),
            }),
            _ => Err(RemoteError::new(
                ErrorCode::UnknownMethod,
                format!("unknown method {method}"),
            )),
        }
    }

    /// Closes the listeners of a client connection that ended. Their accept
    /// loops end on their own; nothing waits for them.
    pub(crate) fn close_owned_by(&self, connection: u64) {
        self.registry.retain(|listener| {
            let keep = listener.connection != connection;
            if !keep {
                listener.shared.stop();
            }
            keep
        });
    }

    pub(crate) fn close_all(&self) {
        self.registry.retain(|listener| {
            listener.shared.stop();
            false
        });
    }

    fn open(&self, context: &Context, params: ListenerOpenParams) -> Result<u16, RemoteError> {
        let address = bind_address(&params)?;
        let session = &context.session;
        session.ids.observe(params.listener_id);
        self.registry.ensure_free(params.listener_id)?;
        let listener = TcpListener::bind(address).map_err(bind_error)?;
        let port = listener
            .local_addr()
            .map_err(|error| crate::server::fs::io_error(error, ""))?
            .port();
        let io_failure = |error: io::Error| crate::server::fs::io_error(error, "");
        // Accepting must not block: the loop only accepts after `poll`
        // reported a connection, which may be gone by then.
        listener.set_nonblocking(true).map_err(io_failure)?;
        let (waker, wakeups) = UnixStream::pair().map_err(io_failure)?;
        waker.set_nonblocking(true).map_err(io_failure)?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            waker,
            connections: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        });
        let (finished, ended) = mpsc::channel::<()>();
        let accept = Accept {
            session: Arc::clone(session),
            listener_id: params.listener_id,
            listener,
            wakeups,
            shared: Arc::clone(&shared),
            relay: params.relay,
            relays: Arc::clone(&self.relays),
        };
        // The id is registered before the loop runs, so the first
        // `listener.accepted` event never names an unknown listener.
        self.registry.insert(
            params.listener_id,
            OpenListener {
                connection: context.connection,
                port,
                shared: Arc::clone(&shared),
                ended,
            },
        )?;
        let spawned = thread::Builder::new()
            .name("remote-listener".into())
            .spawn(move || {
                accept.run();
                drop(finished);
            });
        if let Err(error) = spawned {
            self.registry.remove(params.listener_id);
            return Err(unavailable(error));
        }
        Ok(port)
    }

    fn adopt(&self, context: &Context, params: ListenerAdoptParams) -> Result<(), RemoteError> {
        let shared = {
            let listeners = self.registry.lock();
            let listener = listeners
                .get(&params.listener_id)
                .ok_or_else(|| self.registry.unknown(params.listener_id))?;
            Arc::clone(&listener.shared)
        };
        let waiting = lock(&shared.state).pending.remove(&params.accept_id);
        let Some((stream, _)) = waiting else {
            return Err(RemoteError::new(
                ErrorCode::UnknownResource,
                format!(
                    "connection {} of listener {} is not waiting",
                    params.accept_id, params.listener_id
                ),
            ));
        };
        shared.wake();
        let adopted = context
            .session
            .tunnels
            .adopt(context, params.tunnel_id, Stream::Tcp(stream));
        if adopted.is_err() {
            shared.failed.fetch_add(1, Ordering::Relaxed);
        }
        adopted
    }
}

/// The address `listener.open` binds. Host rules may listen on every
/// interface because the user confirmed that; reverse rules reach the user's
/// machine and therefore listen on loopback only.
fn bind_address(params: &ListenerOpenParams) -> Result<SocketAddr, RemoteError> {
    let host: IpAddr = params.host.parse().map_err(|_| {
        RemoteError::new(
            ErrorCode::InvalidRequest,
            "a listener host must be an IP address",
        )
    })?;
    let allowed = host.is_loopback() || (params.relay.is_some() && host.is_unspecified());
    if !allowed {
        return Err(RemoteError::new(
            ErrorCode::InvalidRequest,
            "a listener binds a loopback address, or every interface for a host rule",
        ));
    }
    Ok(SocketAddr::new(host, params.port))
}

fn bind_error(error: io::Error) -> RemoteError {
    let code = match error.kind() {
        io::ErrorKind::AddrInUse => ErrorCode::AddressInUse,
        io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        _ => ErrorCode::Io,
    };
    RemoteError::new(code, format!("cannot listen: {error}"))
        .with_details(json!({ "osError": error.raw_os_error() }))
}

/// The accept loop of one listener and what it needs.
struct Accept {
    session: Arc<Session>,
    listener_id: u64,
    listener: TcpListener,
    /// Read side of [`Shared::waker`].
    wakeups: UnixStream,
    shared: Arc<Shared>,
    relay: Option<TunnelTarget>,
    relays: Arc<AtomicUsize>,
}

impl Accept {
    fn run(mut self) {
        loop {
            let (room, deadline) = {
                let mut state = lock(&self.shared.state);
                if state.stopping {
                    return;
                }
                let now = Instant::now();
                // Dropping a connection the client never adopted closes it,
                // and it never reached its target.
                let waiting = state.pending.len();
                state.pending.retain(|_, (_, deadline)| *deadline > now);
                let expired = (waiting - state.pending.len()) as u64;
                self.shared.failed.fetch_add(expired, Ordering::Relaxed);
                let room = if self.relay.is_some() {
                    state.relays.len() < MAX_RELAYS_PER_LISTENER
                } else {
                    state.pending.len() < MAX_PENDING
                };
                let deadline = state
                    .pending
                    .values()
                    .map(|(_, deadline)| deadline.saturating_duration_since(now))
                    .min();
                (room, deadline)
            };
            let (woken, connecting) = match self.wait(room, deadline) {
                Ok(ready) => ready,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            if woken {
                let mut drained = [0_u8; 64];
                if matches!(self.wakeups.read(&mut drained), Ok(0) | Err(_)) {
                    return;
                }
            }
            if connecting {
                self.accept();
            }
        }
    }

    /// Waits until the state changes, a connection arrives while there is
    /// room for it, or the first waiting connection is given up. Returns
    /// whether the wake-up socket and the listener are readable.
    fn wait(&self, room: bool, deadline: Option<Duration>) -> io::Result<(bool, bool)> {
        let mut descriptors = [
            libc::pollfd {
                fd: self.wakeups.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let count = if room { 2 } else { 1 };
        let timeout = deadline.map_or(-1, |deadline| {
            // Rounded up, so the loop does not wake just before the deadline.
            deadline
                .as_millis()
                .saturating_add(1)
                .min(libc::c_int::MAX as u128) as libc::c_int
        });
        // SAFETY: the array is live and `count` does not exceed its length.
        let ready = unsafe { libc::poll(descriptors.as_mut_ptr(), count, timeout) };
        if ready == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok((
            descriptors[0].revents != 0,
            room && descriptors[1].revents != 0,
        ))
    }

    fn accept(&self) {
        let stream = match self.listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return,
            Err(_) => {
                thread::sleep(ACCEPT_RETRY);
                return;
            }
        };
        // BSD hosts hand out accepted sockets with the listener's
        // non-blocking flag; the copies below block.
        if stream.set_nonblocking(false).is_err() {
            return;
        }
        let _ = stream.set_nodelay(true);
        self.shared.connections.fetch_add(1, Ordering::Relaxed);
        match &self.relay {
            Some(target) => self.relay(stream, target.clone()),
            None => self.hand_over(stream),
        }
    }

    fn hand_over(&self, stream: TcpStream) {
        let accept_id = {
            let mut state = lock(&self.shared.state);
            if state.stopping {
                return;
            }
            let accept_id = state.next_accept;
            state.next_accept += 1;
            state
                .pending
                .insert(accept_id, (stream, Instant::now() + ADOPT_TIMEOUT));
            accept_id
        };
        self.session.link.emit(
            methods::EVENT_LISTENER_ACCEPTED,
            ListenerAcceptedEvent {
                listener_id: self.listener_id,
                accept_id,
            },
        );
    }

    fn relay(&self, stream: TcpStream, target: TunnelTarget) {
        let admitted = self
            .relays
            .try_update(Ordering::AcqRel, Ordering::Acquire, |relays| {
                (relays < MAX_RELAYS).then_some(relays + 1)
            })
            .is_ok();
        if !admitted {
            self.shared.failed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let relay = Relay {
            shared: Arc::clone(&self.shared),
            relays: Arc::clone(&self.relays),
            id: 0,
        };
        let Some(relay) = relay.register(&stream) else {
            return;
        };
        let spawned = thread::Builder::new().name("remote-relay".into()).spawn({
            let relay = relay.clone();
            move || relay.run(stream, &target)
        });
        if spawned.is_err() {
            relay.fail();
        }
    }
}

/// One relayed connection. Ending it releases its slot and wakes the accept
/// loop, whatever the reason.
#[derive(Clone)]
struct Relay {
    shared: Arc<Shared>,
    relays: Arc<AtomicUsize>,
    id: u64,
}

impl Relay {
    /// Records the accepted socket so that close can shut it down. Returns
    /// `None`, with the slot released, when the listener is stopping.
    fn register(mut self, stream: &TcpStream) -> Option<Self> {
        let clone = stream.try_clone().ok();
        let shared = Arc::clone(&self.shared);
        let mut state = lock(&shared.state);
        let (Some(clone), false) = (clone, state.stopping) else {
            drop(state);
            self.release();
            return None;
        };
        self.id = state.next_accept;
        state.next_accept += 1;
        state.relays.insert(self.id, vec![Stream::Tcp(clone)]);
        Some(self)
    }

    fn run(self, accepted: TcpStream, target: &TunnelTarget) {
        let Ok(upstream) = tunnel::connect(target) else {
            self.fail();
            return;
        };
        let streams = (|| {
            Some((
                accepted.try_clone().ok()?,
                upstream.try_clone().ok()?,
                upstream.try_clone().ok()?,
            ))
        })();
        let Some((answer, request, tracked)) = streams else {
            self.fail();
            return;
        };
        {
            let mut state = lock(&self.shared.state);
            let Some(tracked_streams) = state.relays.get_mut(&self.id) else {
                // Closed while connecting.
                drop(state);
                upstream.shutdown(Shutdown::Both);
                self.end();
                return;
            };
            tracked_streams.push(tracked);
        }
        let Ok(reply) = thread::Builder::new()
            .name("remote-relay".into())
            .spawn(move || pipe(upstream, Stream::Tcp(answer)))
        else {
            request.shutdown(Shutdown::Both);
            self.fail();
            return;
        };
        pipe(Stream::Tcp(accepted), request);
        let _ = reply.join();
        self.end();
    }

    fn fail(&self) {
        self.shared.failed.fetch_add(1, Ordering::Relaxed);
        self.end();
    }

    fn end(&self) {
        let streams = lock(&self.shared.state).relays.remove(&self.id);
        for stream in streams.into_iter().flatten() {
            stream.shutdown(Shutdown::Both);
        }
        self.release();
    }

    fn release(&self) {
        self.relays.fetch_sub(1, Ordering::AcqRel);
        self.shared.wake();
    }
}

/// Copies one direction until end-of-file, then half-closes the other side
/// so its peer can still answer. A failure ends both directions.
fn pipe(mut from: Stream, mut to: Stream) {
    match io::copy(&mut from, &mut to) {
        Ok(_) => to.shutdown(Shutdown::Write),
        Err(_) => {
            from.shutdown(Shutdown::Both);
            to.shutdown(Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(host: &str, relay: bool) -> ListenerOpenParams {
        ListenerOpenParams {
            listener_id: 1,
            host: host.into(),
            port: 0,
            relay: relay.then(|| TunnelTarget::Tcp {
                host: "127.0.0.1".into(),
                port: 1,
            }),
        }
    }

    #[test]
    fn reverse_listeners_bind_loopback_and_host_rules_may_bind_every_interface() {
        assert!(bind_address(&params("127.0.0.1", false)).is_ok());
        assert!(bind_address(&params("::1", false)).is_ok());
        assert!(bind_address(&params("0.0.0.0", true)).is_ok());
        assert!(bind_address(&params("::", true)).is_ok());
        for (host, relay) in [
            ("0.0.0.0", false),
            ("::", false),
            ("192.168.1.2", true),
            ("localhost", true),
        ] {
            let error = bind_address(&params(host, relay)).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidRequest, "relay={relay}");
        }
    }

    #[test]
    fn a_taken_port_reports_address_in_use() {
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let error = TcpListener::bind(("127.0.0.1", port))
            .map_err(bind_error)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::AddressInUse);
    }
}
