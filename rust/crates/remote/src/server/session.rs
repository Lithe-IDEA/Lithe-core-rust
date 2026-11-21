//! Session state that outlives individual connections.

use crate::protocol::methods::{encode_hex, ServerState, SessionToken};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::environment::EnvironmentCache;
use crate::server::link::Link;
use crate::server::listener::ListenerTable;
use crate::server::process::ProcessTable;
use crate::server::resources::{IdWatermark, InFlight, PendingStarts};
use crate::server::spawner::Spawner;
use crate::server::terminal::TerminalTable;
use crate::server::tunnel::TunnelTable;
use crate::server::walk::WalkTable;
use crate::server::watch::WatchTable;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default time a persistent session without a client and without running
/// programs survives.
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Upper bound clients may request, so an idle daemon that nobody comes back
/// to does not linger on a shared host for days. Running processes and
/// terminals keep a session regardless; see [`Session::keeps_programs`].
const MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// Random bytes in a persistent session's token: 128 bits cannot be guessed
/// by a local program in any useful time.
const SESSION_TOKEN_BYTES: usize = 16;

/// A fresh token from the kernel's random source.
fn generate_token() -> std::io::Result<SessionToken> {
    let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(SessionToken::new(encode_hex(&bytes)))
}

/// Compares secrets in time that depends only on their lengths, so response
/// timing does not reveal how much of a guess was right.
fn same_secret(presented: &[u8], expected: &[u8]) -> bool {
    presented.len() == expected.len()
        && presented
            .iter()
            .zip(expected)
            .fold(0, |difference, (left, right)| difference | (left ^ right))
            == 0
}

/// Time process groups get to exit after `SIGTERM` before `SIGKILL`.
pub(crate) const TERMINATION_GRACE: Duration = Duration::from_secs(2);

/// Time requests still running when the session ends get to finish. A write
/// that is cut off leaves its temporary sibling next to the user's file; one
/// request moves at most 8 MiB, so a healthy disk finishes well within this.
const WORKER_DRAIN_GRACE: Duration = Duration::from_secs(5);

/// How often sessions kept for their remaining members are checked. A
/// process id can only be reused after its session emptied, so forgetting an
/// emptied session promptly keeps a later signal from reaching an unrelated
/// process that received the same id.
const REAP_INTERVAL: Duration = Duration::from_millis(500);

/// Prunes emptied sessions until the session is dropped.
fn start_reaper(session: std::sync::Weak<Session>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("remote-reaper".into())
        .spawn(move || loop {
            std::thread::sleep(REAP_INTERVAL);
            let Some(session) = session.upgrade() else {
                return;
            };
            session.processes.reap();
            session.terminals.reap();
            session.uploads.expire();
        })
        .map(|_| ())
}

pub(crate) struct Session {
    pub(crate) persistent: bool,
    pub(crate) link: Link,
    pub(crate) spawner: Spawner,
    pub(crate) ids: IdWatermark,
    pub(crate) environment: EnvironmentCache,
    pub(crate) processes: ProcessTable,
    pub(crate) terminals: TerminalTable,
    pub(crate) watches: WatchTable,
    pub(crate) tunnels: TunnelTable,
    pub(crate) listeners: ListenerTable,
    pub(crate) walks: WalkTable,
    pub(crate) uploads: crate::server::upload::UploadTable,
    pub(crate) run_directories: crate::server::run_directory::RunDirectories,
    /// Requests running on worker threads, drained before the session ends.
    pub(crate) workers: InFlight,
    pub(crate) starts: Arc<PendingStarts>,
    /// Late publications must be reclaimed after shutdown starts, including
    /// persistent sessions whose disconnected resources normally survive.
    ending: AtomicBool,
    /// Whether any client completed a handshake before, which makes the
    /// next one a resumption.
    had_client: AtomicBool,
    /// Secret later clients of a persistent session must present; `None`
    /// for a `--stdio` session, which has exactly one connection.
    token: Option<SessionToken>,
    next_connection: AtomicU64,
    idle_timeout: Mutex<Duration>,
    /// Runs once when a client requests `server.shutdown`, before the
    /// response. The daemon uses it to stop being reachable at once.
    shutdown_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Session {
    pub(crate) fn new(persistent: bool) -> std::io::Result<Arc<Self>> {
        let session = Arc::new(Self {
            persistent,
            link: Link::new(),
            spawner: Spawner::new()?,
            ids: IdWatermark::default(),
            environment: EnvironmentCache::default(),
            processes: ProcessTable::new(),
            terminals: TerminalTable::new(),
            watches: WatchTable::new(),
            tunnels: TunnelTable::new(),
            listeners: ListenerTable::new(),
            walks: WalkTable::new(),
            uploads: crate::server::upload::UploadTable::default(),
            run_directories: crate::server::run_directory::RunDirectories::new(
                crate::server::platform::current_account().home_directory,
            ),
            workers: InFlight::default(),
            starts: Arc::default(),
            ending: AtomicBool::new(false),
            had_client: AtomicBool::new(false),
            token: if persistent {
                Some(generate_token()?)
            } else {
                None
            },
            next_connection: AtomicU64::new(0),
            idle_timeout: Mutex::new(DEFAULT_IDLE_TIMEOUT),
            shutdown_hook: Mutex::new(None),
        });
        start_reaper(Arc::downgrade(&session))?;
        Ok(session)
    }

    pub(crate) fn set_shutdown_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *crate::server::lock(&self.shutdown_hook) = Some(Box::new(hook));
    }

    /// Called for `server.shutdown` before it is answered.
    pub(crate) fn begin_shutdown(&self) {
        self.ending.store(true, Ordering::Release);
        if let Some(hook) = crate::server::lock(&self.shutdown_hook).take() {
            hook();
        }
    }

    pub(crate) fn is_ending(&self) -> bool {
        self.ending.load(Ordering::Acquire)
    }

    pub(crate) fn next_connection_id(&self) -> u64 {
        self.next_connection.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Admits a connection's `server.hello` and returns whether a client was
    /// attached before. The first client of a persistent session is admitted
    /// whatever it presents, since a fresh session cannot know any token;
    /// every later one must present this session's token.
    pub(crate) fn admit(&self, presented: Option<&SessionToken>) -> Result<bool, RemoteError> {
        let Some(token) = &self.token else {
            return Ok(self.had_client.swap(true, Ordering::AcqRel));
        };
        if self
            .had_client
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(false);
        }
        match presented {
            Some(presented)
                if same_secret(presented.as_str().as_bytes(), token.as_str().as_bytes()) =>
            {
                Ok(true)
            }
            _ => Err(RemoteError::new(
                ErrorCode::InvalidSessionToken,
                "the session already has a client; reattaching needs the session token it issued",
            )),
        }
    }

    /// Token a client presents to reattach, returned by every handshake.
    pub(crate) fn token(&self) -> Option<&SessionToken> {
        self.token.as_ref()
    }

    /// Whether processes or terminals still run. A persistent session keeps
    /// them however long it has no client: ending it would kill a build or a
    /// shell the user expects to find again. Only `server.shutdown`, or the
    /// user stopping them, ends them.
    pub(crate) fn keeps_programs(&self) -> bool {
        !self.processes.is_empty() || !self.terminals.is_empty()
    }

    pub(crate) fn set_idle_timeout(&self, seconds: u64) {
        *crate::server::lock(&self.idle_timeout) =
            Duration::from_secs(seconds).min(MAX_IDLE_TIMEOUT);
    }

    pub(crate) fn idle_timeout(&self) -> Duration {
        *crate::server::lock(&self.idle_timeout)
    }

    /// Running times are measured to one moment, so entries of one reply
    /// compare correctly.
    pub(crate) fn state(&self) -> ServerState {
        let now = Instant::now();
        ServerState {
            processes: self.processes.states(now),
            terminals: self.terminals.states(now),
            watches: self.watches.states(),
        }
    }

    /// Resources bound to a client connection. Tunnels end because their
    /// local sockets are gone; listeners close because a host port stays
    /// open only while the client that holds the rule is connected; walks end
    /// because their results would only fill the replay buffer.
    pub(crate) fn release_connection_resources(&self, connection: u64) {
        self.uploads.release(connection);
        self.listeners.close_owned_by(connection);
        self.tunnels.close_owned_by(connection);
        self.walks.cancel_owned_by(connection);
    }

    /// Ends everything the session started.
    pub(crate) fn terminate_all(&self) {
        self.ending.store(true, Ordering::Release);
        self.listeners.close_all();
        self.tunnels.close_all();
        self.walks.cancel_all();
        self.watches.stop_all();
        self.terminals.terminate_all(TERMINATION_GRACE);
        self.processes.terminate_all(TERMINATION_GRACE);
        // Running writes finish, and clean up their temporary files on
        // failure, before the process exits and stops their threads.
        let unfinished = self.workers.drain(WORKER_DRAIN_GRACE);
        if unfinished > 0 {
            eprintln!("lithe-remote-server: {unfinished} requests did not finish");
        }
        self.uploads.clear();
        // Processes must be stopped before their read-only runtime inputs go.
        self.run_directories.clear();
    }
}
