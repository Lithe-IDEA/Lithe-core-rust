//! Typed operations on a [`RemoteSession`].
//!
//! Each long-lived resource gets an id from [`RemoteSession::allocate_id`]
//! and a subscription *before* it is created, so no early event is lost.
//! Dropping a handle only ends the subscription; the remote resource keeps
//! running until it is stopped explicitly, which is what lets terminals and
//! builds survive a reconnect.

use crate::client::session::{ClientError, Event, RemoteSession};
use crate::protocol::methods::{
    self, decode_bytes, encode_bytes, CopyParams, CopyResult, CreateDirParams,
    EnvironmentResolveParams, EnvironmentResolveResult, EnvironmentSource, FileMetadata,
    FileVersion, KillParams, ListeningPort, ListeningPortsResult, OutputStream, PathParams,
    ProcessExitEvent, ProcessOutputEvent, ProcessParams, ReadDirResult, ReadFileParams,
    ReadFileResult, RealPathResult, RemoveParams, RenameParams, ServerState, SetPermissionsParams,
    SpawnParams, SpawnResult, TerminalExitEvent, TerminalOpenParams, TerminalOpenResult,
    TerminalOutputEvent, TerminalParams, TerminalResizeParams, TerminalWriteParams,
    TunnelClosedEvent, TunnelDataEvent, TunnelOpenParams, TunnelParams, TunnelTarget,
    TunnelWriteParams, VolumeInfo, WalkDoneEvent, WalkEntriesEvent, WalkEntry, WalkParams,
    WalkStartParams, WatchChangedEvent, WatchErrorEvent, WatchParams, WatchStartParams,
    WatchStartResult, WriteFileParams, MAX_TRANSFER_BYTES,
};
use crate::protocol::{ErrorCode, RemoteError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// Deadline for ordinary requests.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Resolution may start the login shell twice with a 10-second bound each.
const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound for retrying input the remote side is not reading yet (`BUSY`).
const INPUT_RETRY_DEADLINE: Duration = Duration::from_secs(30);
const INPUT_RETRY_START: Duration = Duration::from_millis(1);
const INPUT_RETRY_MAX: Duration = Duration::from_millis(50);

/// Options of [`RemoteSession::write_file`].
#[derive(Clone, Debug, Default)]
pub struct WriteOptions {
    /// Version last read; the write fails with `CONFLICT` if the file changed.
    pub expected: Option<FileVersion>,
    /// Replace an existing file without an `expected` check.
    pub overwrite: bool,
    /// Create missing parent directories first.
    pub create_parents: bool,
}

/// What to run in [`RemoteSession::spawn`].
#[derive(Clone, Debug, Default)]
pub struct SpawnSpec {
    /// Program and arguments; a program without `/` is resolved through `PATH`.
    pub argv: Vec<String>,
    /// Absolute host working directory; the server's own when absent.
    pub cwd: Option<String>,
    /// Base environment the `env` variables are applied to.
    pub environment: EnvironmentSource,
    /// Variables added to, or replacing, the base environment.
    pub env: BTreeMap<String, String>,
    /// Keep standard input open for [`ProcessHandle::write_stdin`].
    pub pipe_stdin: bool,
}

/// What to open in [`RemoteSession::open_terminal`].
#[derive(Clone, Debug)]
pub struct TerminalSpec {
    /// `None` starts the user's login shell.
    pub argv: Option<Vec<String>>,
    /// Absolute host working directory; when absent, the user's home, or `/`
    /// without one.
    pub cwd: Option<String>,
    /// Variables added to the server environment.
    pub env: BTreeMap<String, String>,
    /// Initial width in character cells.
    pub columns: u16,
    /// Initial height in character cells.
    pub rows: u16,
}

/// A finished process's collected output.
#[derive(Debug, Default)]
pub struct ProcessOutput {
    /// Everything the process wrote to standard output.
    pub stdout: Vec<u8>,
    /// Everything the process wrote to standard error.
    pub stderr: Vec<u8>,
    /// Exit report; [`ProcessHandle::wait`] only returns once it arrived.
    pub exit: Option<ProcessExitEvent>,
}

/// An event of one spawned process.
#[derive(Debug)]
pub enum ProcessEvent {
    /// Decoded bytes from one output stream.
    Output(OutputStream, Vec<u8>),
    /// The process exited; no output follows.
    Exit(ProcessExitEvent),
}

/// An event of one terminal.
#[derive(Debug)]
pub enum TerminalEvent {
    /// Decoded bytes the terminal's program wrote.
    Output(Vec<u8>),
    /// The terminal's program exited; no output follows.
    Exit(TerminalExitEvent),
}

/// An event of one watch.
#[derive(Debug)]
pub enum WatchEvent {
    /// Coalesced changes below the watched root.
    Changed(WatchChangedEvent),
    /// The watch degraded or ended.
    Error(WatchErrorEvent),
}

/// An event of one tunnel.
#[derive(Debug)]
pub enum TunnelEvent {
    /// Decoded bytes the target sent.
    Data(Vec<u8>),
    /// The tunnel ended; no data follows.
    Closed(TunnelClosedEvent),
}

/// Every entry of a finished walk.
#[derive(Debug)]
pub struct WalkOutcome {
    /// Entries in walk order.
    pub entries: Vec<WalkEntry>,
    /// The walk's final report.
    pub done: WalkDoneEvent,
}

impl RemoteSession {
    fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, ClientError> {
        self.request(method, params, REQUEST_TIMEOUT)
    }

    fn call_unit<P: Serialize>(&self, method: &str, params: &P) -> Result<(), ClientError> {
        self.request::<P, Value>(method, params, REQUEST_TIMEOUT)
            .map(|_| ())
    }

    /// Sends input, retrying with backoff while the remote side reports
    /// `BUSY` because the receiving program is not reading yet.
    fn call_input<P: Serialize>(&self, method: &str, params: &P) -> Result<(), ClientError> {
        let deadline = Instant::now() + INPUT_RETRY_DEADLINE;
        let mut delay = INPUT_RETRY_START;
        loop {
            match self.call_unit(method, params) {
                Err(ClientError::Remote(error))
                    if error.code == ErrorCode::Busy && Instant::now() < deadline =>
                {
                    thread::sleep(delay);
                    delay = (delay * 2).min(INPUT_RETRY_MAX);
                }
                result => return result,
            }
        }
    }

    /// Sends `server.ping` and returns the round-trip time.
    pub fn ping(&self) -> Result<Duration, ClientError> {
        let started = Instant::now();
        self.call_unit(methods::SERVER_PING, &json!({}))?;
        Ok(started.elapsed())
    }

    /// Lists the processes, terminals and watches the session still runs.
    pub fn state(&self) -> Result<ServerState, ClientError> {
        self.call(methods::SERVER_STATE, &json!({}))
    }

    /// Metadata of `path` itself, without following a final link.
    pub fn stat(&self, path: &str) -> Result<FileMetadata, ClientError> {
        self.call(methods::FS_STAT, &PathParams { path: path.into() })
    }

    /// Lists one directory, sorted by name byte order.
    pub fn read_dir(&self, path: &str) -> Result<ReadDirResult, ClientError> {
        self.call(methods::FS_READ_DIR, &PathParams { path: path.into() })
    }

    /// Reads a whole file in chunks. Fails with `CONFLICT` when the file
    /// changes while it is being read, rather than returning a mixture.
    pub fn read_file(&self, path: &str) -> Result<(Vec<u8>, FileMetadata), ClientError> {
        let mut content = Vec::new();
        let mut first: Option<FileMetadata> = None;
        loop {
            let chunk: ReadFileResult = self.call(
                methods::FS_READ_FILE,
                &ReadFileParams {
                    path: path.into(),
                    offset: content.len() as u64,
                    length: MAX_TRANSFER_BYTES,
                },
            )?;
            if let Some(first) = &first {
                if first.size != chunk.metadata.size
                    || first.modified_ms != chunk.metadata.modified_ms
                {
                    return Err(ClientError::Remote(RemoteError::new(
                        ErrorCode::Conflict,
                        "the file changed while it was read",
                    )));
                }
            }
            content.extend(decode(&chunk.data)?);
            let eof = chunk.eof;
            first.get_or_insert(chunk.metadata);
            if eof {
                return Ok((content, first.expect("set above")));
            }
        }
    }

    /// Replaces the whole file atomically and returns its metadata after the write.
    pub fn write_file(
        &self,
        path: &str,
        content: &[u8],
        options: WriteOptions,
    ) -> Result<FileMetadata, ClientError> {
        self.call(
            methods::FS_WRITE_FILE,
            &WriteFileParams {
                path: path.into(),
                data: encode_bytes(content),
                expected: options.expected,
                overwrite: options.overwrite,
                create_parents: options.create_parents,
            },
        )
    }

    /// Creates a directory; with `recursive`, also its missing parents.
    pub fn create_dir(&self, path: &str, recursive: bool) -> Result<(), ClientError> {
        self.call_unit(
            methods::FS_CREATE_DIR,
            &CreateDirParams {
                path: path.into(),
                recursive,
                mode: None,
            },
        )
    }

    /// Removes a path; a non-empty directory needs `recursive`.
    pub fn remove(&self, path: &str, recursive: bool) -> Result<(), ClientError> {
        self.call_unit(
            methods::FS_REMOVE,
            &RemoveParams {
                path: path.into(),
                recursive,
            },
        )
    }

    /// Renames or moves `from` to `to`.
    pub fn rename(&self, from: &str, to: &str, overwrite: bool) -> Result<(), ClientError> {
        self.call_unit(
            methods::FS_RENAME,
            &RenameParams {
                from: from.into(),
                to: to.into(),
                overwrite,
            },
        )
    }

    /// Copies a file or, with `recursive`, a tree. Special files inside a
    /// tree are skipped and counted in the result.
    pub fn copy(
        &self,
        from: &str,
        to: &str,
        recursive: bool,
        overwrite: bool,
    ) -> Result<CopyResult, ClientError> {
        self.call(
            methods::FS_COPY,
            &CopyParams {
                from: from.into(),
                to: to.into(),
                recursive,
                overwrite,
            },
        )
    }

    /// Resolves every link of `path`.
    pub fn real_path(&self, path: &str) -> Result<String, ClientError> {
        self.call::<_, RealPathResult>(methods::FS_REAL_PATH, &PathParams { path: path.into() })
            .map(|result| result.path)
    }

    /// Sets the permission bits of `path`.
    pub fn set_permissions(&self, path: &str, mode: u32) -> Result<(), ClientError> {
        self.call_unit(
            methods::FS_SET_PERMISSIONS,
            &SetPermissionsParams {
                path: path.into(),
                mode,
            },
        )
    }

    /// Reports whether the volume holding `path` is case-sensitive.
    pub fn volume_info(&self, path: &str) -> Result<VolumeInfo, ClientError> {
        self.call(methods::FS_VOLUME_INFO, &PathParams { path: path.into() })
    }

    /// Walks `root` and collects every entry until the walk finishes or
    /// `timeout` elapses, in which case the walk is cancelled.
    pub fn walk(
        &self,
        root: &str,
        exclude_names: &[&str],
        max_entries: Option<u64>,
        timeout: Duration,
    ) -> Result<WalkOutcome, ClientError> {
        let walk_id = self.allocate_id();
        let events = self.subscribe("walk", walk_id);
        let result = (|| {
            self.call_unit(
                methods::WALK_START,
                &WalkStartParams {
                    walk_id,
                    root: root.into(),
                    exclude_names: exclude_names.iter().map(|name| name.to_string()).collect(),
                    max_entries,
                },
            )?;
            let deadline = Instant::now() + timeout;
            let mut entries = Vec::new();
            loop {
                let event = match receive(&events, deadline) {
                    Ok(event) => event,
                    Err(error) => {
                        crate::failure_log::client(
                            "walk_cancel",
                            self.call_unit(methods::WALK_CANCEL, &WalkParams { walk_id }),
                        );
                        return Err(error);
                    }
                };
                match event.event.as_str() {
                    methods::EVENT_WALK_ENTRIES => {
                        entries.extend(event.decode::<WalkEntriesEvent>()?.entries)
                    }
                    methods::EVENT_WALK_DONE => {
                        return Ok(WalkOutcome {
                            entries,
                            done: event.decode()?,
                        })
                    }
                    _ => {}
                }
            }
        })();
        self.unsubscribe("walk", walk_id);
        result
    }

    /// Spawns a process, subscribing to its events before it starts.
    pub fn spawn(&self, spec: SpawnSpec) -> Result<ProcessHandle<'_>, ClientError> {
        let process_id = self.allocate_id();
        let events = self.subscribe("process", process_id);
        let result: Result<SpawnResult, _> = self.call(
            methods::PROCESS_SPAWN,
            &SpawnParams {
                process_id,
                argv: spec.argv,
                cwd: spec.cwd,
                environment: spec.environment,
                env: spec.env,
                pipe_stdin: spec.pipe_stdin,
            },
        );
        match result {
            Ok(spawned) => Ok(ProcessHandle {
                session: self,
                id: process_id,
                pid: spawned.pid,
                events,
            }),
            Err(error) => {
                self.unsubscribe("process", process_id);
                Err(error)
            }
        }
    }

    /// Opens a terminal, subscribing to its events before it starts.
    pub fn open_terminal(&self, spec: TerminalSpec) -> Result<TerminalHandle<'_>, ClientError> {
        let terminal_id = self.allocate_id();
        let events = self.subscribe("terminal", terminal_id);
        let result: Result<TerminalOpenResult, _> = self.call(
            methods::TERMINAL_OPEN,
            &TerminalOpenParams {
                terminal_id,
                argv: spec.argv,
                cwd: spec.cwd,
                env: spec.env,
                columns: spec.columns,
                rows: spec.rows,
            },
        );
        match result {
            Ok(opened) => Ok(TerminalHandle {
                session: self,
                id: terminal_id,
                pid: opened.pid,
                events,
            }),
            Err(error) => {
                self.unsubscribe("terminal", terminal_id);
                Err(error)
            }
        }
    }

    /// Attaches to a terminal that already runs in a persistent session,
    /// for example after reconnecting.
    pub fn attach_terminal(&self, terminal_id: u64, pid: u32) -> TerminalHandle<'_> {
        TerminalHandle {
            session: self,
            id: terminal_id,
            pid,
            events: self.subscribe("terminal", terminal_id),
        }
    }

    /// Watches `root` recursively, subscribing to its events before the watch
    /// starts.
    pub fn watch(
        &self,
        root: &str,
        exclude_names: &[&str],
    ) -> Result<WatchHandle<'_>, ClientError> {
        let watch_id = self.allocate_id();
        let events = self.subscribe("watch", watch_id);
        let result: Result<WatchStartResult, _> = self.call(
            methods::WATCH_START,
            &WatchStartParams {
                watch_id,
                root: root.into(),
                exclude_names: exclude_names.iter().map(|name| name.to_string()).collect(),
            },
        );
        match result {
            Ok(started) => Ok(WatchHandle {
                session: self,
                id: watch_id,
                watched_directories: started.watched_directories,
                events,
            }),
            Err(error) => {
                self.unsubscribe("watch", watch_id);
                Err(error)
            }
        }
    }

    /// Returns the user's login environment; `refresh` re-runs the login shell
    /// instead of using the session's cached result.
    pub fn resolve_environment(
        &self,
        refresh: bool,
    ) -> Result<EnvironmentResolveResult, ClientError> {
        self.request(
            methods::ENVIRONMENT_RESOLVE,
            &EnvironmentResolveParams { refresh },
            ENVIRONMENT_TIMEOUT,
        )
    }

    /// Lists the TCP ports listening on the host.
    pub fn listening_ports(&self) -> Result<Vec<ListeningPort>, ClientError> {
        self.call::<_, ListeningPortsResult>(methods::PORTS_LISTENING, &json!({}))
            .map(|result| result.ports)
    }

    /// Connects a tunnel to `target`, subscribing to its events before it opens.
    pub fn open_tunnel(&self, target: TunnelTarget) -> Result<TunnelHandle<'_>, ClientError> {
        let tunnel_id = self.allocate_id();
        let events = self.subscribe("tunnel", tunnel_id);
        match self.call_unit(
            methods::TUNNEL_OPEN,
            &TunnelOpenParams { tunnel_id, target },
        ) {
            Ok(()) => Ok(TunnelHandle {
                session: self,
                id: tunnel_id,
                events,
            }),
            Err(error) => {
                self.unsubscribe("tunnel", tunnel_id);
                Err(error)
            }
        }
    }
}

/// A spawned process. Dropping it only ends the subscription; the process
/// keeps running until it exits or is killed.
pub struct ProcessHandle<'a> {
    session: &'a RemoteSession,
    /// Client-chosen process id.
    pub id: u64,
    /// Operating-system pid, for diagnostics only.
    pub pid: u32,
    events: Receiver<Event>,
}

impl ProcessHandle<'_> {
    /// Writes to standard input, retrying while the process is not reading.
    pub fn write_stdin(&self, data: &[u8]) -> Result<(), ClientError> {
        self.session.call_input(
            methods::PROCESS_WRITE_STDIN,
            &json!({ "processId": self.id, "data": encode_bytes(data) }),
        )
    }

    /// Closes standard input, so the process reads end-of-file.
    pub fn close_stdin(&self) -> Result<(), ClientError> {
        self.session.call_unit(
            methods::PROCESS_CLOSE_STDIN,
            &ProcessParams {
                process_id: self.id,
            },
        )
    }

    /// Sends `SIGTERM`, or `SIGKILL` with `force`, to every process of the
    /// process's session.
    pub fn kill(&self, force: bool) -> Result<(), ClientError> {
        self.session.call_unit(
            methods::PROCESS_KILL,
            &KillParams {
                process_id: self.id,
                force,
            },
        )
    }

    /// Next event, or `None` once the subscription ended.
    pub fn next_event(&self, timeout: Duration) -> Result<Option<ProcessEvent>, ClientError> {
        let Some(event) = receive_optional(&self.events, timeout)? else {
            return Ok(None);
        };
        Ok(Some(match event.event.as_str() {
            methods::EVENT_PROCESS_EXIT => ProcessEvent::Exit(event.decode()?),
            _ => {
                let output: ProcessOutputEvent = event.decode()?;
                ProcessEvent::Output(output.stream, decode(&output.data)?)
            }
        }))
    }

    /// Collects output until the process exits or `timeout` elapses.
    pub fn wait(&self, timeout: Duration) -> Result<ProcessOutput, ClientError> {
        let deadline = Instant::now() + timeout;
        let mut output = ProcessOutput::default();
        loop {
            match self.next_event(deadline.saturating_duration_since(Instant::now()))? {
                Some(ProcessEvent::Output(OutputStream::Stdout, bytes)) => {
                    output.stdout.extend(bytes)
                }
                Some(ProcessEvent::Output(OutputStream::Stderr, bytes)) => {
                    output.stderr.extend(bytes)
                }
                Some(ProcessEvent::Exit(exit)) => {
                    output.exit = Some(exit);
                    return Ok(output);
                }
                None => return Err(self.session_closed()),
            }
        }
    }

    fn session_closed(&self) -> ClientError {
        ClientError::Disconnected {
            diagnostics: String::new(),
        }
    }
}

impl Drop for ProcessHandle<'_> {
    fn drop(&mut self) {
        self.session.unsubscribe("process", self.id);
    }
}

/// An open terminal. Dropping it only ends the subscription; the terminal
/// keeps running until it exits or is closed.
pub struct TerminalHandle<'a> {
    session: &'a RemoteSession,
    /// Client-chosen terminal id.
    pub id: u64,
    /// Operating-system pid of the terminal's program, for diagnostics only.
    pub pid: u32,
    events: Receiver<Event>,
}

impl TerminalHandle<'_> {
    /// Writes input, retrying while the terminal is not read.
    pub fn write(&self, data: &[u8]) -> Result<(), ClientError> {
        self.session.call_input(
            methods::TERMINAL_WRITE,
            &TerminalWriteParams {
                terminal_id: self.id,
                data: encode_bytes(data),
            },
        )
    }

    /// Changes the terminal's size in character cells.
    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), ClientError> {
        self.session.call_unit(
            methods::TERMINAL_RESIZE,
            &TerminalResizeParams {
                terminal_id: self.id,
                columns,
                rows,
            },
        )
    }

    /// Hangs up the terminal; its exit event follows.
    pub fn close(&self) -> Result<(), ClientError> {
        self.session.call_unit(
            methods::TERMINAL_CLOSE,
            &TerminalParams {
                terminal_id: self.id,
            },
        )
    }

    /// Next event, or `None` once the subscription ended.
    pub fn next_event(&self, timeout: Duration) -> Result<Option<TerminalEvent>, ClientError> {
        let Some(event) = receive_optional(&self.events, timeout)? else {
            return Ok(None);
        };
        Ok(Some(match event.event.as_str() {
            methods::EVENT_TERMINAL_EXIT => TerminalEvent::Exit(event.decode()?),
            _ => TerminalEvent::Output(decode(&event.decode::<TerminalOutputEvent>()?.data)?),
        }))
    }
}

impl Drop for TerminalHandle<'_> {
    fn drop(&mut self) {
        self.session.unsubscribe("terminal", self.id);
    }
}

/// An active watch. Dropping it only ends the subscription; the watch keeps
/// running until it is stopped.
pub struct WatchHandle<'a> {
    session: &'a RemoteSession,
    /// Client-chosen watch id.
    pub id: u64,
    /// Directories registered by a per-directory backend.
    pub watched_directories: Option<u64>,
    events: Receiver<Event>,
}

impl WatchHandle<'_> {
    /// Next event, or `None` once the subscription ended.
    pub fn next_event(&self, timeout: Duration) -> Result<Option<WatchEvent>, ClientError> {
        let Some(event) = receive_optional(&self.events, timeout)? else {
            return Ok(None);
        };
        Ok(Some(match event.event.as_str() {
            methods::EVENT_WATCH_ERROR => WatchEvent::Error(event.decode()?),
            _ => WatchEvent::Changed(event.decode()?),
        }))
    }

    /// Stops the watch.
    pub fn stop(&self) -> Result<(), ClientError> {
        self.session
            .call_unit(methods::WATCH_STOP, &WatchParams { watch_id: self.id })
    }
}

impl Drop for WatchHandle<'_> {
    fn drop(&mut self) {
        self.session.unsubscribe("watch", self.id);
    }
}

/// An open tunnel. Dropping it only ends the subscription; the tunnel stays
/// open until it is closed or the connection ends.
pub struct TunnelHandle<'a> {
    session: &'a RemoteSession,
    /// Client-chosen tunnel id.
    pub id: u64,
    events: Receiver<Event>,
}

impl<'a> TunnelHandle<'a> {
    /// Sends bytes, retrying while the target is not reading.
    pub fn write(&self, data: &[u8]) -> Result<(), ClientError> {
        self.writer().write(data)
    }

    /// Ends the outgoing direction; see [`TunnelWriter::shutdown_write`].
    pub fn shutdown_write(&self) -> Result<(), ClientError> {
        self.writer().shutdown_write()
    }

    /// Aborts both directions; see [`TunnelWriter::close`].
    pub fn close(&self) -> Result<(), ClientError> {
        self.writer().close()
    }

    /// Write-only half that can move to another thread, while this handle
    /// keeps receiving.
    pub fn writer(&self) -> TunnelWriter<'a> {
        TunnelWriter {
            session: self.session,
            id: self.id,
        }
    }

    /// Next event, or `None` once the subscription ended.
    pub fn next_event(&self, timeout: Duration) -> Result<Option<TunnelEvent>, ClientError> {
        let Some(event) = receive_optional(&self.events, timeout)? else {
            return Ok(None);
        };
        Ok(Some(match event.event.as_str() {
            methods::EVENT_TUNNEL_CLOSED => {
                TunnelEvent::Closed(event.decode::<TunnelClosedEvent>()?)
            }
            _ => TunnelEvent::Data(decode(&event.decode::<TunnelDataEvent>()?.data)?),
        }))
    }
}

/// The write side of a tunnel, see [`TunnelHandle::writer`].
#[derive(Clone, Copy)]
pub struct TunnelWriter<'a> {
    session: &'a RemoteSession,
    id: u64,
}

impl TunnelWriter<'_> {
    /// Sends bytes, retrying while the target is not reading.
    pub fn write(&self, data: &[u8]) -> Result<(), ClientError> {
        self.session.call_input(
            methods::TUNNEL_WRITE,
            &TunnelWriteParams {
                tunnel_id: self.id,
                data: encode_bytes(data),
            },
        )
    }

    /// Ends the outgoing direction after the queued data, like shutting down
    /// a socket's write side; the remote reply keeps arriving.
    pub fn shutdown_write(&self) -> Result<(), ClientError> {
        self.session.call_unit(
            methods::TUNNEL_SHUTDOWN_WRITE,
            &TunnelParams { tunnel_id: self.id },
        )
    }

    /// Aborts both directions; queued data is discarded.
    pub fn close(&self) -> Result<(), ClientError> {
        self.session
            .call_unit(methods::TUNNEL_CLOSE, &TunnelParams { tunnel_id: self.id })
    }
}

impl Drop for TunnelHandle<'_> {
    fn drop(&mut self) {
        self.session.unsubscribe("tunnel", self.id);
    }
}

fn decode(data: &str) -> Result<Vec<u8>, ClientError> {
    decode_bytes(data).map_err(|error| ClientError::Protocol(error.to_string()))
}

fn receive(events: &Receiver<Event>, deadline: Instant) -> Result<Event, ClientError> {
    match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(event) => Ok(event),
        Err(RecvTimeoutError::Timeout) => Err(ClientError::Timeout),
        Err(RecvTimeoutError::Disconnected) => Err(ClientError::Disconnected {
            diagnostics: String::new(),
        }),
    }
}

/// Like [`receive`], but a closed subscription is `None` instead of an error.
fn receive_optional(
    events: &Receiver<Event>,
    timeout: Duration,
) -> Result<Option<Event>, ClientError> {
    match events.recv_timeout(timeout) {
        Ok(event) => Ok(Some(event)),
        Err(RecvTimeoutError::Timeout) => Err(ClientError::Timeout),
        Err(RecvTimeoutError::Disconnected) => Ok(None),
    }
}
