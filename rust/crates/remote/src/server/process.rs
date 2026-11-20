//! Processes spawned on behalf of the client.
//!
//! Each process leads its own session and process group. Output is streamed
//! as events in bounded chunks; the exit event is sent after the process's
//! own output has drained, so clients can treat it as the end of the stream.
//!
//! The exit event reports the leader. Other members of its session, such as
//! build daemons started in the background, may outlive it; the server keeps
//! owning the session until it is empty, so `process.kill` and the final
//! cleanup still reach them.

use crate::protocol::methods::{
    self, decode_bytes, encode_bytes, EnvironmentSource, KillParams, OutputStream,
    ProcessExitEvent, ProcessOutputEvent, ProcessParams, ProcessState, SpawnParams, SpawnResult,
    WriteStdinParams,
};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::connection::Context;
use crate::server::fs::{host_path, io_error};
use crate::server::platform::{self, ChildKind};
use crate::server::resources::Registry;
use crate::server::session::Session;
use crate::server::threads::Parked;
use crate::server::{parse, to_value};
use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const OUTPUT_CHUNK_BYTES: usize = 32 * 1024;

/// Pending standard-input writes per process. A full queue answers `BUSY`
/// instead of blocking the dispatcher on a child that stopped reading.
pub(crate) const INPUT_QUEUE_CAPACITY: usize = 64;

/// How long output readers may keep a pipe open after the process exited.
/// Grandchildren that inherited the pipe can hold it open indefinitely; their
/// later output is dropped so the exit event remains the last event.
pub(crate) const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);

/// Interval for checking whether signalled sessions are empty during
/// cleanup. Members that are not the server's children report no exit to
/// it, so the wait polls them within the caller's deadline.
pub(crate) const SESSION_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The leader of a session the server started, kept after the leader exits
/// for as long as the session has members.
///
/// POSIX does not reuse a process id while a process group or session with
/// that id exists, so signalling a kept leader id cannot reach an unrelated
/// process.
pub(crate) struct Leader {
    pub(crate) pid: u32,
    /// Set when the leader exited and its exit event was sent.
    pub(crate) exited: bool,
}

impl Leader {
    pub(crate) fn new(pid: u32) -> Self {
        Self { pid, exited: false }
    }
}

/// Forgets entries whose leader exited and whose session is empty. One call
/// checks all exited leaders together, scanning the process table at most
/// once.
pub(crate) fn prune<T>(registry: &Registry<T>, leader: impl Fn(&T) -> &Leader) {
    let exited: Vec<u32> = registry
        .lock()
        .values()
        .map(&leader)
        .filter(|leader| leader.exited)
        .map(|leader| leader.pid)
        .collect();
    if exited.is_empty() {
        return;
    }
    let live = platform::live_sessions(&exited);
    registry.retain(|entry| {
        let leader = leader(entry);
        !leader.exited || live.contains(&leader.pid)
    });
}

/// Whether any entry is kept only for its session's remaining members.
pub(crate) fn has_exited_leaders<T>(
    registry: &Registry<T>,
    leader: impl Fn(&T) -> &Leader,
) -> bool {
    registry.lock().values().any(|entry| leader(entry).exited)
}

pub(crate) struct ProcessTable {
    registry: Registry<RunningProcess>,
}

struct RunningProcess {
    leader: Leader,
    argv: Vec<String>,
    /// `None` when standard input was not piped or has been closed.
    stdin: Option<SyncSender<Vec<u8>>>,
}

impl ProcessTable {
    pub(crate) fn new() -> Self {
        Self {
            registry: Registry::new("process"),
        }
    }

    pub(crate) fn handle(
        &self,
        context: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value, RemoteError> {
        match method {
            methods::PROCESS_SPAWN => to_value(self.spawn(&context.session, parse(params)?)?),
            methods::PROCESS_WRITE_STDIN => self.write_stdin(parse(params)?).map(|()| Value::Null),
            methods::PROCESS_CLOSE_STDIN => self.close_stdin(parse(params)?).map(|()| Value::Null),
            methods::PROCESS_KILL => self.kill(parse(params)?).map(|()| Value::Null),
            _ => Err(RemoteError::new(
                ErrorCode::UnknownMethod,
                format!("unknown method {method}"),
            )),
        }
    }

    pub(crate) fn states(&self) -> Vec<ProcessState> {
        self.registry
            .lock()
            .iter()
            .map(|(process_id, process)| ProcessState {
                process_id: *process_id,
                pid: process.leader.pid,
                argv: process.argv.clone(),
                leader_exited: process.leader.exited,
            })
            .collect()
    }

    fn spawn(
        &self,
        session: &Arc<Session>,
        params: SpawnParams,
    ) -> Result<SpawnResult, RemoteError> {
        session.ids.observe(params.process_id);
        prune(&self.registry, |process| &process.leader);
        self.registry.ensure_free(params.process_id)?;
        let Some((program, arguments)) = params.argv.split_first() else {
            return Err(RemoteError::new(ErrorCode::InvalidRequest, "argv is empty"));
        };
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(if params.pipe_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if params.environment == EnvironmentSource::Login {
            let login = session.environment.resolve(session, false);
            command.env_clear().envs(login.environment);
        }
        command.envs(&params.env);
        if let Some(cwd) = &params.cwd {
            command.current_dir(host_path(cwd)?);
        }
        platform::prepare_child(&mut command, ChildKind::Plain);
        // Every thread the child needs starts before the child does.
        let stdin_writer = if params.pipe_stdin {
            Some(InputWriter::<ChildStdin>::start(drop)?)
        } else {
            None
        };
        let monitor = Self::monitor(session, params.process_id)?;
        let program_name = program.clone();
        let mut child = session
            .spawner
            .run(move || command.spawn())
            .map_err(|error| spawn_error(&program_name, error))?;

        let pid = child.id();
        let stdin = stdin_writer
            .zip(child.stdin.take())
            .map(|(writer, pipe)| writer.attach(pipe));
        let entry = RunningProcess {
            leader: Leader::new(pid),
            argv: params.argv.clone(),
            stdin,
        };
        if let Err(error) = self.registry.insert(params.process_id, entry) {
            // Another spawn with the same id won the race; this child was
            // never visible to the client, so it must not keep running.
            let _ = platform::signal_process_group(pid, libc::SIGKILL);
            let _ = child.wait();
            return Err(error);
        }
        let pipes: [Option<Box<dyn Read + Send>>; 2] = [
            child.stdout.take().map(|pipe| Box::new(pipe) as _),
            child.stderr.take().map(|pipe| Box::new(pipe) as _),
        ];
        monitor.follow(pipes, child);
        Ok(SpawnResult { pid })
    }

    /// Readers for standard output and standard error, in that order, and
    /// the waiter that reports the exit.
    fn monitor(
        session: &Arc<Session>,
        process_id: u64,
    ) -> Result<Monitor<Box<dyn Read + Send>>, RemoteError> {
        let forwards = [OutputStream::Stdout, OutputStream::Stderr].map(|stream| {
            let session = Arc::clone(session);
            let forward: Forward = Box::new(move |chunk| {
                session.link.emit(
                    methods::EVENT_PROCESS_OUTPUT,
                    ProcessOutputEvent {
                        process_id,
                        stream,
                        data: encode_bytes(chunk),
                    },
                );
            });
            forward
        });
        let session = Arc::clone(session);
        Monitor::start(forwards, move |status| {
            let registry = &session.processes.registry;
            if let Some(process) = registry.lock().get_mut(&process_id) {
                process.leader.exited = true;
                process.stdin = None;
            }
            prune(registry, |process| &process.leader);
            let (exit_code, signal) = exit_parts(status);
            session.link.emit(
                methods::EVENT_PROCESS_EXIT,
                ProcessExitEvent {
                    process_id,
                    exit_code,
                    signal,
                },
            );
        })
    }

    fn write_stdin(&self, params: WriteStdinParams) -> Result<(), RemoteError> {
        let data = decode_bytes(&params.data)
            .map_err(|error| RemoteError::new(ErrorCode::InvalidRequest, error.to_string()))?;
        let processes = self.registry.lock();
        let process = processes
            .get(&params.process_id)
            .filter(|process| !process.leader.exited)
            .ok_or_else(|| self.registry.unknown(params.process_id))?;
        let Some(stdin) = &process.stdin else {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "standard input is not open",
            ));
        };
        queue_input(stdin, data)
    }

    fn close_stdin(&self, params: ProcessParams) -> Result<(), RemoteError> {
        let mut processes = self.registry.lock();
        let process = processes
            .get_mut(&params.process_id)
            .filter(|process| !process.leader.exited)
            .ok_or_else(|| self.registry.unknown(params.process_id))?;
        process.stdin = None;
        Ok(())
    }

    /// Signals the whole session, also after the leader exited while other
    /// members still run.
    fn kill(&self, params: KillParams) -> Result<(), RemoteError> {
        // A session that emptied since the last check must not be signalled:
        // its id may already belong to an unrelated process.
        prune(&self.registry, |process| &process.leader);
        let pid = self
            .registry
            .lock()
            .get(&params.process_id)
            .map(|process| process.leader.pid)
            .ok_or_else(|| self.registry.unknown(params.process_id))?;
        let signal = if params.force {
            libc::SIGKILL
        } else {
            libc::SIGTERM
        };
        match platform::signal_session(pid, signal) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                prune(&self.registry, |process| &process.leader);
                Err(self.registry.unknown(params.process_id))
            }
            Err(error) => Err(io_error(error, "")),
        }
    }

    /// Forgets sessions that emptied after their leader exited.
    pub(crate) fn reap(&self) {
        if has_exited_leaders(&self.registry, |process| &process.leader) {
            prune(&self.registry, |process| &process.leader);
        }
    }

    /// Whether no started process, nor any member of its session, may still
    /// be running. A session is forgotten only after [`Self::reap`] found it
    /// empty, so background children of an exited leader count as running.
    pub(crate) fn is_empty(&self) -> bool {
        self.registry.lock().is_empty()
    }

    /// Ends every owned session: `SIGTERM`, a bounded grace period, then
    /// `SIGKILL` for sessions that still have members.
    pub(crate) fn terminate_all(&self, grace: Duration) {
        terminate_groups(
            &self.registry,
            grace,
            |process| &process.leader,
            libc::SIGTERM,
        );
    }
}

/// Signals every session in `registry` with `first`, then `SIGKILL`, waiting
/// up to `grace` after each signal until the sessions are empty. A session
/// counts as gone only when no member is left, not when its leader exited.
pub(crate) fn terminate_groups<T>(
    registry: &Registry<T>,
    grace: Duration,
    leader: impl Fn(&T) -> &Leader + Copy,
    first: libc::c_int,
) {
    for signal in [first, libc::SIGKILL] {
        prune(registry, leader);
        let leaders: Vec<u32> = registry
            .lock()
            .values()
            .map(|entry| leader(entry).pid)
            .collect();
        if leaders.is_empty() {
            return;
        }
        for pid in leaders {
            let _ = platform::signal_session(pid, signal);
        }
        let deadline = Instant::now() + grace;
        loop {
            prune(registry, leader);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if registry.lock().is_empty() {
                return;
            }
            if remaining.is_zero() {
                break;
            }
            // Leader exits wake the wait; other members are only observed by
            // polling.
            registry.wait_for_change(remaining.min(SESSION_POLL_INTERVAL));
        }
    }
    prune(registry, leader);
    let survivors = registry.lock().len();
    if survivors > 0 {
        eprintln!("lithe-remote-server: {survivors} process groups did not exit after SIGKILL");
    }
}

pub(crate) fn spawn_error(program: &str, error: io::Error) -> RemoteError {
    RemoteError::new(
        ErrorCode::SpawnFailed,
        format!("cannot start {program}: {error}"),
    )
    .with_details(json!({ "program": program, "osError": error.raw_os_error() }))
}

pub(crate) fn exit_parts(status: io::Result<ExitStatus>) -> (Option<i32>, Option<i32>) {
    match status {
        Ok(status) => (status.code(), status.signal()),
        Err(error) => {
            eprintln!("lithe-remote-server: waiting for a child failed: {error}");
            (None, None)
        }
    }
}

/// The queue and thread that write a child's or stream's input, started
/// before the child or stream exists (see [`crate::server::threads`]).
pub(crate) struct InputWriter<W> {
    queue: SyncSender<Vec<u8>>,
    thread: Parked<W>,
}

impl<W: Write + Send + 'static> InputWriter<W> {
    /// Starts the writer thread. Once the queue's sender is dropped (close or
    /// exit), the queued chunks are still written and `finish` receives the
    /// pipe; after a failed write the pipe is dropped without `finish`.
    /// Dropping a pipe delivers end-of-file to its reader.
    pub(crate) fn start(finish: impl FnOnce(W) + Send + 'static) -> Result<Self, RemoteError> {
        let (queue, chunks): (_, Receiver<Vec<u8>>) = mpsc::sync_channel(INPUT_QUEUE_CAPACITY);
        let thread = Parked::start("remote-input", move |mut pipe: W| {
            while let Ok(chunk) = chunks.recv() {
                if pipe.write_all(&chunk).is_err() {
                    return;
                }
            }
            finish(pipe);
        })?;
        Ok(Self { queue, thread })
    }

    /// Hands `pipe` to the writer and returns the queue that feeds it.
    pub(crate) fn attach(self, pipe: W) -> SyncSender<Vec<u8>> {
        self.thread.run(pipe);
        self.queue
    }
}

pub(crate) fn queue_input(queue: &SyncSender<Vec<u8>>, data: Vec<u8>) -> Result<(), RemoteError> {
    match queue.try_send(data) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(RemoteError::new(
            ErrorCode::Busy,
            "the process is not reading its input",
        )),
        Err(TrySendError::Disconnected(_)) => Err(RemoteError::new(
            ErrorCode::InvalidRequest,
            "input is closed",
        )),
    }
}

/// A chunk forwarder of one output pipe.
pub(crate) type Forward = Box<dyn Fn(&[u8]) + Send>;

/// The output readers and the exit waiter of one child, started before the
/// child (see [`crate::server::threads`]).
///
/// Readers forward chunks through an [`OutputGate`]. The waiter reaps the
/// child, lets the readers drain, and then runs the exit action, so the exit
/// event follows the child's own output.
pub(crate) struct Monitor<R> {
    readers: Vec<Parked<R>>,
    waiter: Parked<Child>,
}

impl<R: Read + Send + 'static> Monitor<R> {
    /// Starts one reader per entry of `forwards`, and the waiter that runs
    /// `exit` with the child's status.
    pub(crate) fn start(
        forwards: impl IntoIterator<Item = Forward>,
        exit: impl FnOnce(io::Result<ExitStatus>) + Send + 'static,
    ) -> Result<Self, RemoteError> {
        let mut gate = OutputGate::new();
        let readers = forwards
            .into_iter()
            .map(|forward| gate.reader(forward))
            .collect::<Result<Vec<_>, _>>()?;
        let waiter = Parked::start("remote-wait", move |mut child: Child| {
            let status = child.wait();
            gate.finish(|| exit(status));
        })?;
        Ok(Self { readers, waiter })
    }

    /// Follows `child`. `pipes` pairs with the forwards given to
    /// [`Monitor::start`], in order; a missing pipe leaves its reader unused.
    pub(crate) fn follow(self, pipes: impl IntoIterator<Item = Option<R>>, child: Child) {
        for (reader, pipe) in self.readers.into_iter().zip(pipes) {
            if let Some(pipe) = pipe {
                reader.run(pipe);
            }
        }
        self.waiter.run(child);
    }
}

/// Orders a child's output before its exit event.
///
/// Readers forward chunks while holding the gate; `finish` waits briefly for
/// the readers to reach end-of-file, then closes the gate and runs the exit
/// action under it. Output read after that is dropped, because it can only
/// come from descendants that inherited the pipe. A reader that never
/// receives its pipe only makes `finish` wait out the grace period.
struct OutputGate {
    closed: Arc<Mutex<bool>>,
    drained: (mpsc::Sender<()>, Receiver<()>),
    readers: usize,
}

impl OutputGate {
    fn new() -> Self {
        Self {
            closed: Arc::new(Mutex::new(false)),
            drained: mpsc::channel(),
            readers: 0,
        }
    }

    fn reader<R: Read + Send + 'static>(
        &mut self,
        forward: Forward,
    ) -> Result<Parked<R>, RemoteError> {
        let closed = Arc::clone(&self.closed);
        let drained = self.drained.0.clone();
        let reader = Parked::start("remote-output", move |mut pipe: R| {
            let mut buffer = vec![0_u8; OUTPUT_CHUNK_BYTES];
            loop {
                let read = match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    // A terminal reports EIO once every slave descriptor is
                    // closed, which is its end-of-file.
                    Err(_) => break,
                };
                let closed = crate::server::lock(&closed);
                if !*closed {
                    forward(&buffer[..read]);
                }
            }
            let _ = drained.send(());
        })?;
        self.readers += 1;
        Ok(reader)
    }

    fn finish(self, exit: impl FnOnce()) {
        let deadline = Instant::now() + OUTPUT_DRAIN_GRACE;
        for _ in 0..self.readers {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if self.drained.1.recv_timeout(remaining).is_err() {
                break;
            }
        }
        let mut closed = crate::server::lock(&self.closed);
        *closed = true;
        exit();
    }
}
