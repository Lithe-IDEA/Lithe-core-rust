//! Long-lived remote connections.
//!
//! `remote.connect` starts one client process, OpenSSH for a host,
//! `wsl.exe` for a WSL distribution on this machine or the container
//! engine's CLI for a container on this machine, that probes the host,
//! installs the server when needed and starts it (see
//! `lithe_remote::client::launch`), so the user authenticates once. The
//! connection then stays open in this registry until `remote.disconnect`;
//! requests such as `remote.readDir` use it by id. The client runs in its own
//! process tree, which closing a connection ends, together with the
//! connection's port forwards.
//!
//! The server runs a persistent session under a key (`--connect`), so the
//! processes and terminals it runs survive the connection. Closing a
//! connection ends them unless the client chose to keep them
//! (`remote.disconnect` with `programs: keep`); a later `remote.connect` that
//! presents the key and the session's token attaches to them again. A
//! session nobody kept, such as one whose client crashed, ends with its
//! programs once the server's idle timeout passes without a client.

use crate::probe::{
    classify, classify_container, classify_wsl, container_status, diagnostics, relay_challenge,
    ssh_hop, ProbeStatus, ProbedHost,
};
use crate::programs::RunningProgram;
use lithe_git_host::ProcessTree;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::connections::{ConnectionError, Reason, SshConnection};
use lithe_remote::client::launch::{
    bound_launch_command, launch_bound, LaunchError, Launched, Stage,
};
use lithe_remote::client::session::{ClientError, ConnectOptions, ResumeMode};
use lithe_remote::client::ssh_identity;
use lithe_remote::client::trust::Target;
use lithe_remote::client::{
    wsl, ArtifactError, ConnectionKind, ConnectionLog, ContainerConnection, ForwardSet, Hop,
    RemoteSession, ServerArtifact, ServerTarget, TransportChain, WslConnection,
};
use lithe_remote::protocol::message::ErrorCode as RemoteErrorCode;
use lithe_remote::protocol::methods::{self, SessionToken};
use lithe_request_context::events as execution_events;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Upper bound for connecting, including the time the user needs to answer
/// prompts and the upload of the server.
const CONNECT_DEADLINE: Duration = Duration::from_secs(5 * 60);

/// Interval for relaying prompts and checking cancellation while the remote
/// shell runs; it adds no noticeable latency to a network connection.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Time the protocol handshake may take once the server started.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline for one request over an open connection.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Time the client gets to exit and print its reason after the stream ended.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Bytes of the client's standard error kept while a connection is open.
const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// A server build shipped with the host.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ServerInput {
    /// Rust target triple, for example `x86_64-unknown-linux-musl`.
    target: String,
    /// Absolute local path of the binary.
    path: String,
    /// Hex SHA-256 the binary must have, taken from the manifest built and
    /// signed with the application rather than from the file at `path`.
    sha256: String,
}

/// `remote.connect`: the host, the server builds Core may install, and where
/// to install them. At most one of `connection` and `wsl` names the host;
/// `container` then names a running container on that host, or, on its own,
/// one on this machine.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectRequest {
    /// A host reached through OpenSSH.
    #[serde(default)]
    connection: Option<SshConnection>,
    /// A WSL distribution on this machine, reached through `wsl.exe`.
    #[serde(default)]
    wsl: Option<WslConnection>,
    /// A running container, reached through its engine's `exec`, on the
    /// host the other fields name or on this machine.
    #[serde(default)]
    container: Option<ContainerConnection>,
    /// Remote preferences document, whose SSH program starts OpenSSH;
    /// `null` or absent is every value automatic. Unused without
    /// `connection`.
    #[serde(default)]
    preferences: Option<String>,
    servers: Vec<ServerInput>,
    server_version: String,
    /// Install root on the remote; `None` is `$HOME/.lithe-server`. The
    /// server keeps its persistent sessions' state below it too.
    #[serde(default)]
    install_root: Option<String>,
    /// The persistent session an earlier connection to this host kept, to
    /// attach to again; `None` starts a new session.
    #[serde(default)]
    resume: Option<ResumeInput>,
}

/// A persistent session `remote.connect` returned earlier.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResumeInput {
    /// [`OpenedSession::session_key`].
    key: String,
    /// [`OpenedSession::session_token`]; a secret.
    token: SessionToken,
    /// Opaque identity returned with this token. Legacy records are never
    /// sent automatically, even when their saved connection id matches.
    #[serde(default)]
    binding: Option<String>,
}

/// Longest session key, which the server limits too.
const MAX_SESSION_KEY_CHARS: usize = 64;

/// Longest session token accepted from a client; the server issues 32 hex
/// digits.
const MAX_SESSION_TOKEN_CHARS: usize = 128;

impl ResumeInput {
    /// Refuses a key the server would refuse, so it never reaches a remote
    /// command line, and a token that is not one the server issues.
    fn check(&self) -> Result<(), CoreError> {
        let key_valid = !self.key.is_empty()
            && self.key.chars().count() <= MAX_SESSION_KEY_CHARS
            && self
                .key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        let token = self.token.as_str();
        let token_valid = !token.is_empty()
            && token.len() <= MAX_SESSION_TOKEN_CHARS
            && token.chars().all(|c| c.is_ascii_hexdigit());
        if key_valid && token_valid && self.binding.as_deref().is_none_or(methods::is_sha256_hex) {
            return Ok(());
        }
        Err(crate::connections::invalid_request(ConnectionError {
            field: "resume",
            reason: Reason::Invalid,
        }))
    }
}

/// The session `remote.connect` opened.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedSession {
    id: String,
    /// Whether this connection uploaded the server.
    uploaded: bool,
    host_name: String,
    user_name: Option<String>,
    home_directory: Option<String>,
    /// Whether the host can start Windows programs through WSL
    /// interoperability; `null` outside WSL. Shown to the user only: a WSL
    /// host gets no more trust than any other (S31).
    windows_interop: Option<bool>,
    /// Names the persistent session on the host, for a later `resume`.
    session_key: String,
    /// Secret a later `resume` presents; the client stores it like a
    /// password and never logs it.
    session_token: Option<SessionToken>,
    /// Reusable verified identity; absent when only this connection is known.
    session_binding: Option<String>,
    /// The supplied resume belonged to this verified target and was tried.
    /// A mismatched or legacy record remains in the native secret store.
    resume_consumed: bool,
    /// Whether this connection attached to the session `resume` named.
    /// `false` after a `resume` means that session had ended, or belonged
    /// to another client, and a new one started.
    resumed: bool,
    /// Processes and terminals the resumed session still runs, which an
    /// earlier connection kept; empty for a new session.
    kept_programs: Vec<RunningProgram>,
}

/// `remote.connect` result. Connection failures are results, not errors.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    /// `connected` only when `session` is set.
    status: ProbeStatus,
    session: Option<OpenedSession>,
    /// Platform reported by the remote shell, when it ran.
    host: Option<ProbedHost>,
    diagnostics: String,
}

/// One open host connection in the registry.
pub struct Connection {
    /// Framed workspace session to the host server, shared with the port
    /// forwards that tunnel through it.
    pub session: Arc<RemoteSession>,
    /// Git executable path and version found on the host, once resolved.
    pub git_tool: Mutex<Option<(String, String)>>,
    /// Where the connection ends, the key of its workspaces' trust together
    /// with their real paths. Fixed when the connection opens.
    pub(crate) trust_target: Target,
    /// Port forwards of the ports page; they end with the session.
    pub(crate) forwards: ForwardSet,
    pub(crate) transport: ConnectionTransport,
    /// What the connection has of the programs earlier connections started.
    pub(crate) history: History,
}

/// What a connection has of the processes earlier connections to the same
/// persistent session started, which `remote.attachProcess` reattaches to.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct History {
    /// Whether the connection resumed a session an earlier one kept.
    pub resumed: bool,
    /// Whether it received every event of those processes after the last one
    /// earlier connections showed. `false` after the application restarted,
    /// the host dropped events past its retention limit, or an earlier
    /// connection discarded events nobody attached to.
    pub complete: bool,
}

/// Owns the native transport independently of the framed workspace session.
pub(crate) enum ConnectionTransport {
    /// The client process, OpenSSH or `wsl.exe`, and its process tree.
    Process {
        child: Mutex<Child>,
        tree: ProcessTree,
    },
    #[cfg(feature = "test-support")]
    InProcess,
}

impl Connection {
    /// A registered connection over `session`, with no forwards yet.
    pub(crate) fn new(
        session: RemoteSession,
        trust_target: Target,
        transport: ConnectionTransport,
        history: History,
    ) -> Self {
        let session = Arc::new(session);
        Self {
            forwards: ForwardSet::new(Arc::clone(&session)),
            session,
            git_tool: Mutex::new(None),
            trust_target,
            transport,
            history,
        }
    }

    /// Ends the session together with every program it runs.
    pub(crate) fn close(&self) {
        // Local ports close first, so nothing new is accepted into a tunnel
        // the shutdown below is about to end.
        self.forwards.close();
        self.request_quietly(methods::SERVER_SHUTDOWN, "shutdown");
        self.end_transport();
    }

    /// Leaves the session's programs running on the host for a later
    /// connection: `server.keep`, then the connection ends without
    /// `server.shutdown`. Returns whether the host keeps them. A session
    /// that refused, or whose connection was already lost, ends them once
    /// its idle timeout passes.
    pub(crate) fn detach(&self) -> bool {
        self.forwards.close();
        let kept = self.request_quietly(methods::SERVER_KEEP, "keep");
        self.end_transport();
        kept
    }

    /// Sends `method` with the exit grace as its deadline, recording a
    /// failure other than a peer that already closed at WARN.
    fn request_quietly(&self, method: &str, stage: &str) -> bool {
        match self
            .session
            .request_value(method, serde_json::Value::Null, EXIT_GRACE)
        {
            Ok(_) => true,
            Err(error) => {
                // A peer that already closed needs no request; other failures use WARN.
                if !matches!(error, ClientError::Disconnected { .. }) {
                    lithe_diagnostics::core_events::recovery_failed(
                        "remote.session.cleanup_failed",
                        stage,
                        &error.failure_category(),
                        "remote",
                    );
                }
                false
            }
        }
    }

    /// Ends the client process and its tree. A persistent session on the
    /// host outlives it.
    fn end_transport(&self) {
        match &self.transport {
            ConnectionTransport::Process { child, tree } => {
                let mut child = child
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Err(error) = tree.kill() {
                    record_cleanup_io("tree-kill", &error);
                }
                if let Err(error) = child.kill() {
                    // Killing an already exited child is an expected missing process.
                    if !matches!(child.try_wait(), Ok(Some(_))) {
                        record_cleanup_io("kill", &error);
                    }
                }
                if let Err(error) = child.wait() {
                    record_cleanup_io("wait", &error);
                }
                // A Unix process group counts an unreaped root as alive, so the
                // tree is awaited only after the wait above; what remains are
                // descendants that may outlive the session.
                if let Err(error) = tree.terminate(EXIT_GRACE) {
                    record_cleanup_io("tree-terminate", &error);
                }
            }
            #[cfg(feature = "test-support")]
            ConnectionTransport::InProcess => {}
        }
    }
}

/// Records one failed client process cleanup step. The caller keeps its own
/// result; WARN reports a process that may outlive the session.
fn record_cleanup_io(stage: &str, error: &std::io::Error) {
    lithe_diagnostics::core_events::recovery_failed(
        "remote.session.cleanup_failed",
        stage,
        &format!("{:?}", error.kind()),
        "io",
    );
}

/// Kills the client's process tree and reaps its root, recording each failed step.
fn reap(tree: &ProcessTree, child: &mut Child) {
    if let Err(error) = tree.kill() {
        record_cleanup_io("tree-kill", &error);
    }
    if let Err(error) = child.wait() {
        record_cleanup_io("wait", &error);
    }
}

/// Set once the application is quitting; connections still being opened stop.
static CLOSING: AtomicBool = AtomicBool::new(false);

/// Number of `connect` calls that have not returned yet.
static CONNECTING: AtomicUsize = AtomicUsize::new(0);

/// Counts a connection attempt so application exit can wait for its OpenSSH
/// tree to be killed.
struct ConnectingGuard;

impl ConnectingGuard {
    fn enter() -> Self {
        CONNECTING.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ConnectingGuard {
    fn drop(&mut self) {
        CONNECTING.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn registry() -> &'static Mutex<HashMap<String, Arc<Connection>>> {
    static CONNECTIONS: OnceLock<Mutex<HashMap<String, Arc<Connection>>>> = OnceLock::new();
    CONNECTIONS.get_or_init(Mutex::default)
}

pub(crate) fn lookup(id: &str) -> Result<Arc<Connection>, CoreError> {
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(id)
        .cloned()
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::ConnectionClosed,
                "The remote connection is closed",
            )
        })
}

/// Releases process-wide state another crate keys by a session id, such as
/// the search indexes `lithe-project` builds over a remote workspace.
pub type SessionEndHook = fn(&str);

fn session_end_hooks() -> &'static Mutex<Vec<SessionEndHook>> {
    static HOOKS: OnceLock<Mutex<Vec<SessionEndHook>>> = OnceLock::new();
    HOOKS.get_or_init(Mutex::default)
}

/// Runs `hook` with the id of every session that leaves the registry from now
/// on: `remote.disconnect`, a broken link that forgets the session, and
/// application exit. Session ids are never reused, so state keyed by an ended
/// id can never be reached again. Registering a hook twice runs it once.
///
/// A caller that may insert state for a session after the session ended must
/// insert first and then check [`is_registered`]: either the check sees the
/// session gone, or the hook runs after the insert and removes it.
pub fn on_session_end(hook: SessionEndHook) {
    let mut hooks = session_end_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !hooks
        .iter()
        .any(|existing| std::ptr::fn_addr_eq(*existing, hook))
    {
        hooks.push(hook);
    }
}

/// Whether `id` names a session that is still registered.
pub fn is_registered(id: &str) -> bool {
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(id)
}

/// Runs the session-end hooks for `id`, which has just left the registry.
pub(crate) fn ended(id: &str) {
    let hooks = session_end_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for hook in hooks {
        hook(id);
    }
}

/// The key of a workspace's machine-local records, its trust and its port
/// records: the connection's target and the workspace's real path as the host
/// resolves it, so a symbolic link or another spelling of the path never
/// reaches a record of a different folder. `workspace` is the absolute host
/// path the workspace was opened at.
pub(crate) fn workspace_key(
    session_id: &str,
    workspace: &str,
) -> Result<(Target, String), CoreError> {
    if !workspace.starts_with('/') || workspace.contains('\0') {
        return Err(
            CoreError::new(ErrorCode::InvalidRequest, "Invalid remote workspace path")
                .with_details(serde_json::json!({ "field": "workspace" }).to_string()),
        );
    }
    let connection = lookup(session_id)?;
    let real_path = connection
        .session
        .real_path(workspace)
        .map_err(|error| request_error(session_id, error))?;
    if lithe_remote::client::trust::host_path(&real_path).is_none() {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "The host returned an invalid real path",
        ));
    }
    Ok((connection.trust_target.clone(), real_path))
}

/// Removes a connection whose session ended and closes it in the background,
/// so the host reconnects instead of retrying a dead session.
pub(crate) fn forget(id: &str) {
    let connection = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(id);
    if let Some(connection) = connection {
        ended(id);
        std::thread::spawn(move || connection.close());
    }
}

/// Opens a connection and registers it. A `resume` whose token the host
/// refuses starts a new session instead, so a stale or foreign token never
/// locks the user out of the host.
pub fn connect(request: ConnectRequest) -> Result<ConnectResult, CoreError> {
    connect_with(request, None)
}

/// The local program that reaches the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Client {
    /// OpenSSH, which asks its questions through AskPass.
    Ssh,
    /// `wsl.exe`, which starts a local distribution and asks nothing.
    Wsl,
    /// The CLI of a container engine on this machine, which asks nothing.
    Container,
}

impl Client {
    /// The client `request` names. A request that names neither SSH, WSL
    /// nor a container, or both SSH and WSL, is refused by [`first_hop`]; it
    /// counts as SSH until then.
    fn of(request: &ConnectRequest) -> Self {
        match (&request.connection, &request.wsl, &request.container) {
            (None, Some(_), _) => Self::Wsl,
            (None, None, Some(_)) => Self::Container,
            _ => Self::Ssh,
        }
    }

    /// How the log names the attempt: the client, and whether the chain
    /// then enters a container.
    fn log_kind(self, container: bool) -> ConnectionKind {
        match (self, container) {
            (Self::Ssh, false) => ConnectionKind::Ssh,
            (Self::Ssh, true) => ConnectionKind::SshContainer,
            (Self::Wsl, false) => ConnectionKind::Wsl,
            (Self::Wsl, true) => ConnectionKind::WslContainer,
            (Self::Container, _) => ConnectionKind::Container,
        }
    }

    fn invalid(self) -> &'static str {
        match self {
            Self::Ssh => "Invalid SSH connection",
            Self::Wsl => "Invalid WSL connection",
            Self::Container => "Invalid container connection",
        }
    }

    fn start_failed(self) -> &'static str {
        match self {
            Self::Ssh => "Could not start the SSH client",
            Self::Wsl => "Could not start wsl.exe",
            Self::Container => "Could not start the container engine",
        }
    }

    fn supervise_failed(self) -> &'static str {
        match self {
            Self::Ssh => "Could not supervise the SSH client",
            Self::Wsl => "Could not supervise wsl.exe",
            Self::Container => "Could not supervise the container engine",
        }
    }

    fn timed_out(self) -> &'static str {
        match self {
            Self::Ssh => "The SSH connection did not finish in time",
            Self::Wsl => "The WSL distribution did not finish starting in time",
            Self::Container => "The container did not finish starting the server in time",
        }
    }

    /// The client's standard error as text; `wsl.exe` may print UTF-16.
    fn text(self, stderr: &[u8]) -> String {
        match self {
            Self::Ssh | Self::Container => String::from_utf8_lossy(stderr).into_owned(),
            Self::Wsl => wsl::output_text(stderr),
        }
    }
}

/// The hops of the chain: [`first_hop`], then the container when the
/// request names one on the host `connection` or `wsl` reaches.
fn chain_hops(request: &ConnectRequest, program: Option<&str>) -> Result<Vec<Hop>, CoreError> {
    let mut hops = vec![first_hop(request, program)?];
    if let (false, Some(container)) = (
        matches!(hops[0], Hop::Container(_)),
        request.container.as_ref(),
    ) {
        let hop = container
            .hop()
            .map_err(crate::connections::invalid_request)?;
        hops.push(Hop::Container(hop));
    }
    Ok(hops)
}

/// The hop that starts the chain, with `program` in place of the system
/// client when set; OpenSSH is otherwise the preferences' SSH program, and
/// a container engine its CLI on `PATH`. At most one of `connection` and
/// `wsl` may be set; without either, `container` must be.
fn first_hop(request: &ConnectRequest, program: Option<&str>) -> Result<Hop, CoreError> {
    let refused =
        |field, reason| crate::connections::invalid_request(ConnectionError { field, reason });
    match (&request.connection, &request.wsl) {
        (Some(connection), None) => {
            let mut hop = ssh_hop(connection)?;
            hop.program = crate::preferences::ssh_program(program, request.preferences.as_deref())?;
            Ok(Hop::Ssh(hop))
        }
        (None, Some(distribution)) => {
            let mut hop = distribution
                .hop()
                .map_err(crate::connections::invalid_request)?;
            if let Some(program) = program {
                hop.program = program.into();
            }
            Ok(Hop::Wsl(hop))
        }
        (None, None) => {
            let Some(container) = &request.container else {
                return Err(refused("connection", Reason::Required));
            };
            let mut hop = container
                .hop()
                .map_err(crate::connections::invalid_request)?;
            if let Some(program) = program {
                hop.program = program.into();
            }
            Ok(Hop::Container(hop))
        }
        (Some(_), Some(_)) => Err(refused("wsl", Reason::Invalid)),
    }
}

/// `program` replaces the system client (`ssh` on `PATH`, `wsl.exe`, or the
/// container engine's CLI for a container on this machine); tests use a
/// stand-in.
///
/// The attempt is recorded in a [`ConnectionLog`]. The launch and the
/// handshake record the failures they see themselves; this records what only
/// the client process shows (authentication, host key, network),
/// cancellation and local errors, as stable categories without diagnostics
/// or host names.
pub(crate) fn connect_with(
    request: ConnectRequest,
    program: Option<&str>,
) -> Result<ConnectResult, CoreError> {
    let client = Client::of(&request);
    let log = ConnectionLog::begin(client.log_kind(request.container.is_some()));
    let mut refused = false;
    let mut result = connect_logged(
        &request,
        request.resume.as_ref(),
        client,
        program,
        &log,
        &mut refused,
    );
    if refused {
        // The session under that key belongs to another client, or replaced
        // the one the token was issued for.
        result = connect_logged(&request, None, client, program, &log, &mut refused);
        if let Ok(ConnectResult {
            session: Some(session),
            ..
        }) = &mut result
        {
            session.resume_consumed = true;
        }
    }
    match &result {
        Ok(result) => {
            if let Some((stage, code)) = failure_of(result.status) {
                log.connect_failed(stage, code, "remote");
            }
        }
        Err(error) if matches!(error.code, ErrorCode::Cancelled) => log.connect_cancelled(),
        Err(error) => {
            let code = serde_json::to_value(&error.code).unwrap_or_default();
            log.connect_failed("connect", code.as_str().unwrap_or("unknown"), "core");
        }
    }
    result
}

/// Log stage and category of an attempt that ended with `status`, or `None`
/// when it connected. Each status has its own category.
pub(crate) fn failure_of(status: ProbeStatus) -> Option<(&'static str, &'static str)> {
    match status {
        ProbeStatus::Connected => None,
        ProbeStatus::UnsupportedPlatform => Some(("probe", "unsupported-platform")),
        ProbeStatus::AuthenticationFailed => Some(("authentication", "authentication-failed")),
        ProbeStatus::HostKeyRejected => Some(("authentication", "host-key-rejected")),
        ProbeStatus::Unreachable => Some(("network", "unreachable")),
        ProbeStatus::Failed => Some(("connect", "failed")),
    }
}

/// One connection attempt. Sets `refused` when the host refused the token
/// of `resume`.
fn connect_logged(
    request: &ConnectRequest,
    resume: Option<&ResumeInput>,
    client: Client,
    program: Option<&str>,
    log: &ConnectionLog,
    refused: &mut bool,
) -> Result<ConnectResult, CoreError> {
    if let Some(resume) = resume {
        resume.check()?;
    }
    // Only the build for the host's platform is read and checked, once the
    // probe reports it; here the targets and hashes are just validated.
    let servers = request
        .servers
        .iter()
        .map(|server| {
            let target = server_target(&server.target).ok_or_else(|| {
                CoreError::new(ErrorCode::InvalidRequest, "Unknown remote server target")
                    .with_details(server.target.clone())
            })?;
            if !methods::is_sha256_hex(&server.sha256) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "A remote server SHA-256 must be 64 hexadecimal digits",
                )
                .with_details(server.target.clone()));
            }
            Ok((target, server.path.clone(), server.sha256.clone()))
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    let server_version = request.server_version.clone();
    let rejected = Arc::new(Mutex::new(None::<ArtifactError>));
    let rejected_in_launch = Arc::clone(&rejected);
    let _connecting = ConnectingGuard::enter();

    let chain = TransportChain::new(chain_hops(request, program)?);
    let evidence = if client == Client::Ssh {
        Some(crate::ssh_evidence::Evidence::new().map_err(|error| {
            CoreError::new(
                ErrorCode::ProcessStartFailed,
                "Could not prepare SSH identity evidence",
            )
            .with_details(error.to_string())
        })?)
    } else {
        None
    };
    let evidence = Arc::new(Mutex::new(evidence));
    let mut argv = chain
        .command(&bound_launch_command(request.install_root.as_deref()))
        .map_err(|error| {
            CoreError::new(ErrorCode::InvalidRequest, client.invalid())
                .with_details(error.to_string())
        })?;
    if let Some(evidence) = evidence.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        argv.splice(1..1, evidence.arguments());
    }
    let (program, arguments) = argv.split_first().expect("a hop always produces a command");
    let mut process = Command::new(program);
    process
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Before the AskPass variables, which must reach ssh.
    chain.restrict_environment(&mut process);
    // OpenSSH asks for passwords, passphrases and host keys through AskPass;
    // `wsl.exe` starts a local distribution as a local user and asks nothing,
    // and so does a local container engine.
    let mut authentication = match client {
        Client::Ssh => {
            let authentication =
                lithe_git_host::authentication::Session::new().map_err(|error| {
                    CoreError::new(
                        ErrorCode::ProcessStartFailed,
                        "Could not prepare SSH authentication",
                    )
                    .with_details(error.to_string())
                })?;
            authentication.configure(&mut process).map_err(|error| {
                CoreError::new(
                    ErrorCode::ProcessStartFailed,
                    "Could not configure SSH authentication",
                )
                .with_details(error.to_string())
            })?;
            Some(authentication)
        }
        Client::Wsl | Client::Container => None,
    };
    ProcessTree::prepare(&mut process);
    let mut child = process.spawn().map_err(|error| {
        CoreError::new(ErrorCode::RuntimeMissing, client.start_failed())
            .with_details(error.to_string())
    })?;
    let tree = ProcessTree::adopt(&mut child).map_err(|error| {
        CoreError::new(ErrorCode::ProcessStartFailed, client.supervise_failed())
            .with_details(error.to_string())
    })?;
    let ssh_pid = child.id();
    let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_reader = Mutex::new(
        child
            .stderr
            .take()
            .map(|stderr| keep_tail(stderr, Arc::clone(&stderr_tail))),
    );
    let mut input = child.stdin.take().expect("stdin is piped");
    let mut output = child.stdout.take().expect("stdout is piped");

    // The remote shell is driven on a helper thread with blocking reads;
    // this thread keeps relaying prompts and observing cancellation. Killing
    // the tree ends the helper's reads.
    let (finished, launched) = mpsc::channel();
    // Events can only be sent from the request's thread, so the helper hands
    // its stages over.
    let (stage_sender, stages) = mpsc::channel();
    let launch_log = log.clone();
    let resume_for_launch = resume.cloned();
    let chain_target = Target::new(&chain, None);
    let evidence_for_launch = Arc::clone(&evidence);
    std::thread::spawn(move || {
        let artifact_for = |report: &lithe_remote::client::bootstrap::ProbeReport| {
            let target = report.target()?;
            let (_, path, sha256) = servers.iter().find(|(known, _, _)| *known == target)?;
            match ServerArtifact::read_verified(
                target,
                Path::new(path),
                server_version.clone(),
                sha256,
            ) {
                Ok(artifact) => Some(artifact),
                Err(error) => {
                    *rejected_in_launch
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
                    None
                }
            }
        };
        let mut selected = None;
        let result = launch_bound(
            &mut output,
            &mut input,
            artifact_for,
            &launch_log,
            |stage| {
                // log-exempt: User cancellation: abandon drops the stage receiver before killing the SSH tree.
                let _ = stage_sender.send(stage);
            },
            |server_identity| {
                let actual = evidence_for_launch
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_mut()
                    .and_then(|evidence| evidence.identity(ssh_pid));
                let binding = if client == Client::Ssh && actual.is_none() {
                    None
                } else {
                    Some(ssh_identity::digest(&(
                        &chain_target,
                        actual,
                        server_identity,
                    )))
                };
                let consumed = resume_for_launch
                    .as_ref()
                    .is_some_and(|resume| resume_matches(resume, binding.as_deref()));
                let key = if consumed {
                    resume_for_launch.as_ref().unwrap().key.clone()
                } else {
                    nonce()
                };
                selected = Some((key.clone(), binding, consumed));
                key
            },
        );
        // log-exempt: User cancellation: abandon drops the launch receiver before killing the SSH tree.
        let _ = finished.send((result, output, input, selected));
    });
    let deadline = Instant::now() + CONNECT_DEADLINE;
    let abandon = |error: CoreError, child: &mut Child| {
        reap(&tree, child);
        Err(error)
    };
    let (result, output, input, selected) = loop {
        if let Err(error) = lithe_protocol::cancellation::check() {
            return abandon(error, &mut child);
        }
        if CLOSING.load(Ordering::SeqCst) {
            return abandon(
                CoreError::new(ErrorCode::Cancelled, "The application is closing"),
                &mut child,
            );
        }
        if Instant::now() >= deadline {
            return abandon(
                CoreError::new(ErrorCode::TimedOut, client.timed_out()),
                &mut child,
            );
        }
        relay_stages(&stages);
        if let Some(authentication) = authentication.as_mut() {
            match authentication.poll() {
                Ok(challenges) => challenges.into_iter().for_each(relay_challenge),
                Err(error) => {
                    return abandon(
                        CoreError::new(
                            ErrorCode::ProcessFailed,
                            "SSH authentication transport failed",
                        )
                        .with_details(error.to_string()),
                        &mut child,
                    )
                }
            }
        }
        match launched.recv_timeout(POLL_INTERVAL) {
            Ok(value) => break value,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return abandon(
                    CoreError::new(
                        ErrorCode::ProcessFailed,
                        "The remote shell stopped unexpectedly",
                    ),
                    &mut child,
                )
            }
        }
    };
    // The helper sent its last stages before the result.
    relay_stages(&stages);

    // Only called once the OpenSSH tree has ended, so joining the reader
    // cannot block; it guarantees the last bytes OpenSSH printed (the reason
    // it gave up) are in the buffer before the failure is classified.
    let tail = |tail: &Mutex<VecDeque<u8>>| {
        let reader = stderr_reader
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(reader) = reader {
            if reader.join().is_err() {
                lithe_diagnostics::core_events::recovery_failed(
                    "remote.session.diagnostics_failed",
                    "reader",
                    "reader_panicked",
                    "thread",
                );
            }
        }
        let mut bytes: Vec<u8> = tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .copied()
            .collect();
        if let Some(evidence) = evidence.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            bytes.extend(evidence.diagnostics());
        }
        bytes
    };
    let failed =
        |status: ProbeStatus, host: Option<ProbedHost>, diagnostics: String| ConnectResult {
            status,
            session: None,
            host,
            diagnostics,
        };
    let launched = match result {
        Ok(launched) => launched,
        Err(LaunchError::NoShell { output }) => {
            // The client prints why it gave up only as it exits.
            let status = wait_for_exit(&mut child, EXIT_GRACE);
            reap(&tree, &mut child);
            let stderr = tail(&stderr_tail);
            let mut probe = match (client, request.container.as_ref()) {
                (Client::Container, Some(container)) => {
                    classify_container(status, &stderr, container.engine)
                }
                (Client::Wsl, _) => classify_wsl(status, &output, &stderr),
                _ => classify(status, &[], &stderr),
            };
            // The engine on the host printed why it gave up, after the SSH
            // or WSL client had connected.
            if let (Client::Ssh | Client::Wsl, Some(_)) = (client, &request.container) {
                if let Some(status) = container_status(&client.text(&stderr)) {
                    probe.status = status;
                }
            }
            return Ok(failed(probe.status, None, probe.diagnostics));
        }
        Err(LaunchError::NoArtifact(report)) => {
            let report = *report;
            reap(&tree, &mut child);
            let rejected = rejected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(error) = rejected {
                return Err(CoreError::new(
                    ErrorCode::RemoteServerInvalid,
                    "The remote server build for this host could not be read or is not the expected build",
                )
                .with_details(error.to_string()));
            }
            let host = ProbedHost {
                server_target: report.target().map(ServerTarget::rust_target),
                system: report.system,
                machine: report.machine,
                home_directory: report.home,
                details: report.details,
            };
            return Ok(failed(
                ProbeStatus::UnsupportedPlatform,
                Some(host),
                String::new(),
            ));
        }
        Err(error) => {
            reap(&tree, &mut child);
            let stderr = diagnostics(&client.text(&tail(&stderr_tail)));
            return Ok(failed(
                ProbeStatus::Failed,
                None,
                join_diagnostics(&error.to_string(), &stderr),
            ));
        }
    };
    let Launched { report, uploaded } = launched;
    let (session_key, session_binding, resume_consumed) =
        selected.expect("bound launch selects a key before returning");
    let host = ProbedHost {
        server_target: report.target().map(ServerTarget::rust_target),
        system: report.system,
        machine: report.machine,
        home_directory: report.home,
        details: report.details,
    };
    let options = ConnectOptions {
        handshake_timeout: HANDSHAKE_TIMEOUT,
        session_token: resume
            .filter(|_| resume_consumed)
            .map(|resume| resume.token.clone()),
        // Events start only after process events are parked below, so the
        // ones a resumed session replays wait for `remote.attachProcess`.
        resume: ResumeMode::Deferred,
        log: Some(log.clone()),
        ..ConnectOptions::default()
    };
    let (session, events) = match RemoteSession::from_streams(output, input, &options) {
        Ok(value) => value,
        Err(error) => {
            *refused = resume_consumed
                && matches!(
                    &error,
                    ClientError::Remote(remote)
                        if remote.code == RemoteErrorCode::InvalidSessionToken
                );
            // Grace expiry is advisory: the following checked kill/wait owns the final cleanup result.
            wait_for_exit(&mut child, EXIT_GRACE);
            reap(&tree, &mut child);
            let stderr = diagnostics(&client.text(&tail(&stderr_tail)));
            return Ok(failed(
                ProbeStatus::Failed,
                Some(host),
                join_diagnostics(&error.to_string(), &stderr),
            ));
        }
    };
    let capabilities = session.hello().host.clone();
    let info = session.hello().session.clone();
    // Events of resources an earlier connection started wait for
    // `remote.attachProcess`; everything else nobody subscribed to is
    // drained, so the session's reader never blocks on a full channel. The
    // drain runs before `server.resume` because the replay follows at once.
    session.park("process");
    std::thread::spawn(move || for _ in events {});
    let earlier = info
        .resumed
        .then(|| crate::resumption::continuation(&session_key))
        .flatten();
    let history_complete = match session.resume(earlier.map(|earlier| earlier.after_seq)) {
        Ok(resumed) => {
            !info.resumed || (earlier.is_some_and(|earlier| earlier.complete) && !resumed.truncated)
        }
        Err(error) => {
            drop(session);
            // Grace expiry is advisory: the following checked kill/wait owns the final cleanup result.
            wait_for_exit(&mut child, EXIT_GRACE);
            reap(&tree, &mut child);
            let stderr = diagnostics(&client.text(&tail(&stderr_tail)));
            return Ok(failed(
                ProbeStatus::Failed,
                Some(host),
                join_diagnostics(&error.to_string(), &stderr),
            ));
        }
    };
    crate::resumption::remember(&session_key, session.event_position(), history_complete);
    let kept_programs = if info.resumed {
        crate::programs::running(&session).unwrap_or_else(|error| {
            // The connection works without the list; the client can ask again.
            lithe_diagnostics::core_events::recovery_failed(
                "remote.session.state_failed",
                "kept_programs",
                &error.failure_category(),
                "remote",
            );
            Vec::new()
        })
    } else {
        Vec::new()
    };
    let id = nonce();
    let mut trust_target = Target::new(&chain, capabilities.user_id);
    trust_target.identity = Some(
        session_binding
            .clone()
            .unwrap_or_else(|| ssh_identity::digest(&nonce())),
    );
    if session_binding.is_none() {
        lithe_diagnostics::core_events::recovery_failed(
            "remote.session.identity_rejected",
            "identity",
            "unverified_route",
            "ssh",
        );
    } else if resume.is_some() && !resume_consumed {
        lithe_diagnostics::core_events::recovery_failed(
            "remote.session.identity_rejected",
            "resume",
            "binding_mismatch",
            "core",
        );
    }
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            id.clone(),
            Arc::new(Connection::new(
                session,
                trust_target,
                ConnectionTransport::Process {
                    child: Mutex::new(child),
                    tree,
                },
                History {
                    resumed: info.resumed,
                    complete: history_complete,
                },
            )),
        );
    Ok(ConnectResult {
        status: ProbeStatus::Connected,
        session: Some(OpenedSession {
            id,
            uploaded,
            host_name: capabilities.host_name,
            user_name: capabilities.user_name,
            home_directory: capabilities.home_directory,
            windows_interop: capabilities.windows_interop,
            session_key,
            session_token: info.session_token,
            session_binding,
            resume_consumed,
            resumed: info.resumed,
            kept_programs,
        }),
        host: Some(host),
        diagnostics: String::new(),
    })
}

/// A saved connection id is only a lookup hint, never authority to send a
/// token. Both sides must name the same verified identity before writing.
fn resume_matches(resume: &ResumeInput, binding: Option<&str>) -> bool {
    binding.is_some() && resume.binding.as_deref() == binding
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn legacy_changed_and_unverified_targets_never_receive_tokens() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/remote/session-binding-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let resume = ResumeInput {
                key: fixture["session"]["sessionKey"].as_str().unwrap().into(),
                token: SessionToken::new(fixture["session"]["sessionToken"].as_str().unwrap()),
                binding: case["binding"].as_str().map(str::to_owned),
            };
            resume.check().unwrap();
            assert_eq!(
                resume_matches(&resume, case["actual"].as_str()),
                case["consumed"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
        }
    }
}

/// `remote.readDir`: one directory on a connected host.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadDirRequest {
    session_id: String,
    /// Absolute path on the remote host.
    path: String,
}

/// Lists a remote directory; the result is the protocol's `fs.readDir`.
pub fn read_dir(request: ReadDirRequest) -> Result<serde_json::Value, CoreError> {
    let connection = lookup(&request.session_id)?;
    connection
        .session
        .request_value(
            methods::FS_READ_DIR,
            serde_json::json!({ "path": request.path }),
            REQUEST_TIMEOUT,
        )
        .map_err(|error| request_error(&request.session_id, error))
}

/// Slowest link a save is expected to survive, in bytes per second; it
/// extends the request deadline for large files.
const WRITE_BYTES_PER_SECOND: u64 = 64 * 1024;

/// Largest file opened for editing; bigger files stay on the host.
const MAX_EDIT_BYTES: u64 = 16 * 1024 * 1024;

/// `remote.readFile`: one file on a connected host.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadFileRequest {
    session_id: String,
    path: String,
}

/// Reads a whole file for the editor: `{ data (base64), size, modifiedMs }`.
/// The version identifies the read for a later conflict-checked write.
pub fn read_file(request: ReadFileRequest) -> Result<serde_json::Value, CoreError> {
    let connection = lookup(&request.session_id)?;
    // The read reports metadata from its opened file. A separate stat would
    // race that open and wait outside the read's cancellation and deadline.
    let mut stopped = None;
    let read = connection.session.read_file_limited(
        &request.path,
        MAX_EDIT_BYTES as usize,
        Duration::from_secs(120),
        || {
            stopped = lithe_protocol::cancellation::check().err();
            stopped.is_some()
        },
    );
    if let Some(error) = stopped {
        return Err(error);
    }
    let (content, metadata) = read.map_err(|error| {
        if matches!(&error, ClientError::Remote(remote) if remote.code == RemoteErrorCode::TooLarge)
        {
            CoreError::new(ErrorCode::NotSupported, "The file is too large to open")
        } else {
            request_error(&request.session_id, error)
        }
    })?;
    Ok(serde_json::json!({
        "data": methods::encode_bytes(&content),
        "size": metadata.size,
        "modifiedMs": metadata.modified_ms,
        "revision": metadata.revision,
    }))
}

/// `remote.writeFile`: new content for one file on a connected host.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteFileRequest {
    session_id: String,
    path: String,
    /// Base64 content that replaces the whole file.
    data: String,
    /// Version last read. When present, a changed file is not replaced and
    /// the result is `conflict`; when absent, the file must not exist yet.
    #[serde(default)]
    expected: Option<methods::FileVersion>,
    /// Editor workspace boundary; absent for explicit general host writes.
    #[serde(default)]
    root: Option<String>,
}

/// Saves a file atomically on the host: `{ status: "saved", size,
/// modifiedMs }`, or `{ status: "conflict" }` when it changed since it was
/// read or, for a new file, when one appeared.
pub fn write_file(request: WriteFileRequest) -> Result<serde_json::Value, CoreError> {
    let connection = lookup(&request.session_id)?;
    if request.root.is_some()
        && !connection
            .session
            .hello()
            .host
            .features
            .contains(&methods::Feature::GuardedWrites)
    {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "The host server does not support scoped saves",
        ));
    }
    let content = methods::decode_bytes(&request.data).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "File content is not valid base64",
        )
        .with_details(error.to_string())
    })?;
    let options = lithe_remote::client::api::WriteOptions {
        expected: request.expected,
        overwrite: false,
        create_parents: false,
        root: request.root,
    };
    // The whole file travels in one request, so the deadline grows with it.
    let timeout =
        REQUEST_TIMEOUT + Duration::from_secs(content.len() as u64 / WRITE_BYTES_PER_SECOND);
    let params = methods::WriteFileParams {
        path: request.path.clone(),
        data: methods::encode_bytes(&content),
        expected: options.expected,
        overwrite: options.overwrite,
        create_parents: options.create_parents,
        root: options.root,
    };
    let written = connection
        .session
        .request_value(
            methods::FS_WRITE_FILE,
            serde_json::to_value(&params).expect("write parameters serialize"),
            timeout,
        )
        .and_then(|value| {
            serde_json::from_value::<methods::FileMetadata>(value)
                .map_err(|error| ClientError::Protocol(error.to_string()))
        });
    match written {
        Ok(metadata) => Ok(serde_json::json!({
            "status": "saved",
            "size": metadata.size,
            "modifiedMs": metadata.modified_ms,
            "revision": metadata.revision,
        })),
        Err(ClientError::Remote(error))
            if matches!(
                error.code,
                lithe_remote::protocol::ErrorCode::Conflict
                    | lithe_remote::protocol::ErrorCode::AlreadyExists
            ) =>
        {
            Ok(serde_json::json!({ "status": "conflict" }))
        }
        Err(error) => Err(request_error(&request.session_id, error)),
    }
}

/// `remote.disconnect`: the session to close and what happens to the
/// programs it runs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisconnectRequest {
    session_id: String,
    #[serde(default)]
    programs: ProgramsOnClose,
}

/// What closing a connection does to the session's programs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ProgramsOnClose {
    /// They end with the session.
    #[default]
    End,
    /// They keep running for a later `remote.connect` with `resume`.
    Keep,
}

/// `remote.disconnect` result.
#[derive(Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectResult {
    /// Whether an open connection was closed.
    pub closed: bool,
    /// Whether the host keeps the programs running; `false` when they end,
    /// including when keeping them was asked but the host could not.
    pub kept: bool,
}

/// Closes a connection; closing an unknown or closed one succeeds.
pub fn disconnect(request: DisconnectRequest) -> DisconnectResult {
    let connection = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&request.session_id);
    let Some(connection) = connection else {
        return DisconnectResult::default();
    };
    ended(&request.session_id);
    let kept = match request.programs {
        ProgramsOnClose::End => {
            connection.close();
            false
        }
        ProgramsOnClose::Keep => connection.detach(),
    };
    DisconnectResult { closed: true, kept }
}

/// Closes every open connection, for application exit. Connections close in
/// parallel so a dead host costs one grace period, not one per connection.
pub fn close_all() -> usize {
    CLOSING.store(true, Ordering::SeqCst);
    let count = close_registered();
    // Attempts still opening notice the flag within one poll interval and
    // kill their OpenSSH tree; give them a moment to do so.
    let deadline = Instant::now() + EXIT_GRACE;
    while CONNECTING.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
    }
    count
}

/// Closes the registered connections in parallel.
pub(crate) fn close_registered() -> usize {
    close_registry(registry())
}

/// Drains the owned registry, allowing isolated lifecycle tests without closing
/// unrelated sessions concurrently exercising the application registry.
pub(crate) fn close_registry(connections: &Mutex<HashMap<String, Arc<Connection>>>) -> usize {
    let connections: Vec<(String, Arc<Connection>)> = connections
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .collect();
    let count = connections.len();
    let connections: Vec<Arc<Connection>> = connections
        .into_iter()
        .map(|(id, connection)| {
            ended(&id);
            connection
        })
        .collect();
    let closers: Vec<_> = connections
        .into_iter()
        .map(|connection| std::thread::spawn(move || connection.close()))
        .collect();
    for closer in closers {
        if closer.join().is_err() {
            // Parallel close owns recovery; WARN reports a distinct worker panic.
            lithe_diagnostics::core_events::recovery_failed(
                "remote.session.cleanup_failed",
                "closer",
                "closer_panicked",
                "thread",
            );
        }
    }
    count
}

/// Translates host transport failures without losing their stable remote category.
pub fn request_error(session_id: &str, error: ClientError) -> CoreError {
    match error {
        ClientError::Remote(error) => {
            let code = match error.code {
                lithe_remote::protocol::ErrorCode::NotFound => ErrorCode::WorkspaceNotFound,
                lithe_remote::protocol::ErrorCode::PermissionDenied => ErrorCode::PermissionDenied,
                _ => ErrorCode::ProcessFailed,
            };
            CoreError::new(code, error.message)
                .with_details(serde_json::json!({"remoteCode": error.code}).to_string())
        }
        ClientError::Timeout => CoreError::new(
            ErrorCode::TimedOut,
            "The remote host did not answer in time",
        ),
        ClientError::Cancelled => {
            CoreError::new(ErrorCode::Cancelled, "The remote operation was cancelled")
        }
        ClientError::Disconnected { diagnostics } => {
            // A broken connection cannot recover.
            forget(session_id);
            CoreError::new(ErrorCode::ConnectionClosed, "The remote connection closed")
                .with_details(diagnostics)
        }
        other => CoreError::new(ErrorCode::ProcessFailed, "The remote request failed")
            .with_details(other.to_string()),
    }
}

fn server_target(name: &str) -> Option<ServerTarget> {
    [
        ServerTarget::LinuxX86_64,
        ServerTarget::LinuxAarch64,
        ServerTarget::MacosX86_64,
        ServerTarget::MacosAarch64,
    ]
    .into_iter()
    .find(|target| target.rust_target() == name)
}

fn wait_for_exit(child: &mut Child, grace: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            _ => return None,
        }
    }
}

/// Relays the stages the launch reached as `remoteStage` events. Upload
/// progress that piled up since the last poll is sent once, with the latest
/// count.
fn relay_stages(stages: &mpsc::Receiver<Stage>) {
    let mut upload = None;
    for stage in stages.try_iter() {
        if matches!(stage, Stage::Uploading { .. }) {
            upload = Some(stage);
            continue;
        }
        if let Some(upload) = upload.take() {
            execution_events::emit(stage_event(upload));
        }
        execution_events::emit(stage_event(stage));
    }
    if let Some(upload) = upload {
        execution_events::emit(stage_event(upload));
    }
}

/// The `remoteStage` event of `stage`.
pub(crate) fn stage_event(stage: Stage) -> serde_json::Value {
    match stage {
        Stage::Authenticated => {
            serde_json::json!({ "type": "remoteStage", "stage": "authenticated" })
        }
        Stage::PlatformChecked => {
            serde_json::json!({ "type": "remoteStage", "stage": "platformChecked" })
        }
        Stage::Uploading { sent, total } => serde_json::json!({
            "type": "remoteStage",
            "stage": "uploading",
            "sentBytes": sent,
            "totalBytes": total,
        }),
        Stage::Installed { uploaded } => serde_json::json!({
            "type": "remoteStage",
            "stage": "installed",
            "uploaded": uploaded,
        }),
    }
}

fn join_diagnostics(summary: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}\n{stderr}")
    }
}

fn keep_tail(
    mut stderr: impl Read + Send + 'static,
    tail: Arc<Mutex<VecDeque<u8>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => return,
                Err(error) => {
                    // Once per reader completion; WARN explains truncated launch evidence.
                    lithe_diagnostics::core_events::recovery_failed(
                        "remote.session.diagnostics_failed",
                        "stderr",
                        &format!("{:?}", error.kind()),
                        "io",
                    );
                    return;
                }
                Ok(read) => {
                    let mut tail = tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    tail.extend(&buffer[..read]);
                    let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                    tail.drain(..excess);
                }
            }
        }
    })
}

/// Unguessable connection id, so one host request cannot address a
/// connection it did not open by counting.
pub(crate) fn nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
