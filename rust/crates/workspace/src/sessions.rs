//! Long-lived remote connections.
//!
//! `remote.connect` starts one client process, OpenSSH for a host or
//! `wsl.exe` for a WSL distribution on this machine, that probes the host,
//! installs the server when needed and starts it (see
//! `lithe_remote::client::launch`), so the user authenticates once. The
//! connection then stays open in this registry until `remote.disconnect`;
//! requests such as `remote.readDir` use it by id. The client runs in its own
//! process tree, which closing a connection ends, together with the
//! connection's port forwards.

use crate::probe::{
    classify, classify_wsl, diagnostics, relay_challenge, ssh_hop, ProbeStatus, ProbedHost,
};
use lithe_git_host::ProcessTree;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::connections::{ConnectionError, Reason, SshConnection};
use lithe_remote::client::launch::{launch, launch_command, LaunchError, Launched, Stage};
use lithe_remote::client::session::{ClientError, ConnectOptions};
use lithe_remote::client::trust::Target;
use lithe_remote::client::{
    wsl, ArtifactError, ConnectionKind, ConnectionLog, ForwardSet, Hop, RemoteSession,
    ServerArtifact, ServerTarget, TransportChain, WslConnection,
};
use lithe_remote::protocol::methods;
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
/// to install them. Exactly one of `connection` and `wsl` names the host.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectRequest {
    /// A host reached through OpenSSH.
    #[serde(default)]
    connection: Option<SshConnection>,
    /// A WSL distribution on this machine, reached through `wsl.exe`.
    #[serde(default)]
    wsl: Option<WslConnection>,
    servers: Vec<ServerInput>,
    server_version: String,
    /// Install root on the remote; `None` is `$HOME/.lithe-server`.
    #[serde(default)]
    install_root: Option<String>,
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
    ) -> Self {
        let session = Arc::new(session);
        Self {
            forwards: ForwardSet::new(Arc::clone(&session)),
            session,
            git_tool: Mutex::new(None),
            trust_target,
            transport,
        }
    }

    pub(crate) fn close(&self) {
        // Local ports close first, so nothing new is accepted into a tunnel
        // the shutdown below is about to end.
        self.forwards.close();
        if let Err(error) = self.session.request_value(
            methods::SERVER_SHUTDOWN,
            serde_json::Value::Null,
            EXIT_GRACE,
        ) {
            // A peer that already closed needs no shutdown; other failures use WARN.
            if !matches!(error, ClientError::Disconnected { .. }) {
                lithe_diagnostics::core_events::recovery_failed(
                    "remote.session.cleanup_failed",
                    "shutdown",
                    &error.failure_category(),
                    "remote",
                );
            }
        }
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

/// Removes a connection whose session ended and closes it in the background,
/// so the host reconnects instead of retrying a dead session.
pub(crate) fn forget(id: &str) {
    if let Some(connection) = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(id)
    {
        std::thread::spawn(move || connection.close());
    }
}

/// Opens a connection and registers it.
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
}

impl Client {
    /// The client `request` names. A request that names both or neither is
    /// refused by [`first_hop`]; it counts as SSH until then.
    fn of(request: &ConnectRequest) -> Self {
        if request.wsl.is_some() && request.connection.is_none() {
            Self::Wsl
        } else {
            Self::Ssh
        }
    }

    fn log_kind(self) -> ConnectionKind {
        match self {
            Self::Ssh => ConnectionKind::Ssh,
            Self::Wsl => ConnectionKind::Wsl,
        }
    }

    fn invalid(self) -> &'static str {
        match self {
            Self::Ssh => "Invalid SSH connection",
            Self::Wsl => "Invalid WSL connection",
        }
    }

    fn start_failed(self) -> &'static str {
        match self {
            Self::Ssh => "Could not start the SSH client",
            Self::Wsl => "Could not start wsl.exe",
        }
    }

    fn supervise_failed(self) -> &'static str {
        match self {
            Self::Ssh => "Could not supervise the SSH client",
            Self::Wsl => "Could not supervise wsl.exe",
        }
    }

    fn timed_out(self) -> &'static str {
        match self {
            Self::Ssh => "The SSH connection did not finish in time",
            Self::Wsl => "The WSL distribution did not finish starting in time",
        }
    }

    /// The client's standard error as text; `wsl.exe` may print UTF-16.
    fn text(self, stderr: &[u8]) -> String {
        match self {
            Self::Ssh => String::from_utf8_lossy(stderr).into_owned(),
            Self::Wsl => wsl::output_text(stderr),
        }
    }
}

/// The hop that starts the chain, with `program` in place of the system
/// client when set. Exactly one of `connection` and `wsl` must be set.
fn first_hop(request: &ConnectRequest, program: Option<&str>) -> Result<Hop, CoreError> {
    let refused =
        |field, reason| crate::connections::invalid_request(ConnectionError { field, reason });
    match (&request.connection, &request.wsl) {
        (Some(connection), None) => {
            let mut hop = ssh_hop(connection)?;
            if let Some(program) = program {
                hop.program = program.into();
            }
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
        (None, None) => Err(refused("connection", Reason::Required)),
        (Some(_), Some(_)) => Err(refused("wsl", Reason::Invalid)),
    }
}

/// `program` replaces the system client (`ssh` on `PATH`, or `wsl.exe`);
/// tests use a stand-in.
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
    let log = ConnectionLog::begin(client.log_kind());
    let result = connect_logged(request, client, program, &log);
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

fn connect_logged(
    request: ConnectRequest,
    client: Client,
    program: Option<&str>,
    log: &ConnectionLog,
) -> Result<ConnectResult, CoreError> {
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

    let chain = TransportChain::new(vec![first_hop(&request, program)?]);
    let argv = chain
        .command(&launch_command(
            request.install_root.as_deref(),
            &["--stdio"],
        ))
        .map_err(|error| {
            CoreError::new(ErrorCode::InvalidRequest, client.invalid())
                .with_details(error.to_string())
        })?;
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
    // `wsl.exe` starts a local distribution as a local user and asks nothing.
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
        Client::Wsl => None,
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
        let result = launch(
            &mut output,
            &mut input,
            artifact_for,
            &launch_log,
            |stage| {
                // log-exempt: User cancellation: abandon drops the stage receiver before killing the SSH tree.
                let _ = stage_sender.send(stage);
            },
        );
        // log-exempt: User cancellation: abandon drops the launch receiver before killing the SSH tree.
        let _ = finished.send((result, output, input));
    });
    let deadline = Instant::now() + CONNECT_DEADLINE;
    let abandon = |error: CoreError, child: &mut Child| {
        reap(&tree, child);
        Err(error)
    };
    let (result, output, input) = loop {
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
        let bytes: Vec<u8> = tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .copied()
            .collect();
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
        Err(LaunchError::NoShell) => {
            // The client prints why it gave up only as it exits.
            let status = wait_for_exit(&mut child, EXIT_GRACE);
            reap(&tree, &mut child);
            let stderr = tail(&stderr_tail);
            let probe = match client {
                Client::Ssh => classify(status, &[], &stderr),
                Client::Wsl => classify_wsl(status, &stderr),
            };
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
    let host = ProbedHost {
        server_target: report.target().map(ServerTarget::rust_target),
        system: report.system,
        machine: report.machine,
        home_directory: report.home,
        details: report.details,
    };
    let options = ConnectOptions {
        handshake_timeout: HANDSHAKE_TIMEOUT,
        log: Some(log.clone()),
        ..ConnectOptions::default()
    };
    let (session, events) = match RemoteSession::from_streams(output, input, &options) {
        Ok(value) => value,
        Err(error) => {
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
    // Process events are not used yet; draining keeps the session's reader
    // from blocking on a full channel.
    std::thread::spawn(move || for _ in events {});
    let capabilities = session.hello().host.clone();
    let id = nonce();
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            id.clone(),
            Arc::new(Connection::new(
                session,
                Target::new(&chain, capabilities.user_id),
                ConnectionTransport::Process {
                    child: Mutex::new(child),
                    tree,
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
        }),
        host: Some(host),
        diagnostics: String::new(),
    })
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
    let metadata = connection
        .session
        .stat(&request.path)
        .map_err(|error| request_error(&request.session_id, error))?;
    if metadata.size > MAX_EDIT_BYTES {
        return Err(
            CoreError::new(ErrorCode::NotSupported, "The file is too large to open")
                .with_details(format!("{} bytes", metadata.size)),
        );
    }
    let (content, metadata) = connection
        .session
        .read_file(&request.path)
        .map_err(|error| request_error(&request.session_id, error))?;
    // The file may have grown since `stat`.
    if content.len() as u64 > MAX_EDIT_BYTES {
        return Err(
            CoreError::new(ErrorCode::NotSupported, "The file is too large to open")
                .with_details(format!("{} bytes", content.len())),
        );
    }
    Ok(serde_json::json!({
        "data": methods::encode_bytes(&content),
        "size": metadata.size,
        "modifiedMs": metadata.modified_ms,
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
}

/// Saves a file atomically on the host: `{ status: "saved", size,
/// modifiedMs }`, or `{ status: "conflict" }` when it changed since it was
/// read or, for a new file, when one appeared.
pub fn write_file(request: WriteFileRequest) -> Result<serde_json::Value, CoreError> {
    let connection = lookup(&request.session_id)?;
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

/// `remote.disconnect`: the session to close.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisconnectRequest {
    session_id: String,
}

/// Closes a connection; closing an unknown or closed one succeeds.
pub fn disconnect(request: DisconnectRequest) -> bool {
    let connection = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&request.session_id);
    let Some(connection) = connection else {
        return false;
    };
    connection.close();
    true
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
    let connections: Vec<Arc<Connection>> = connections
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .map(|(_, connection)| connection)
        .collect();
    let count = connections.len();
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
