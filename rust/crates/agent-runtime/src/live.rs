//! Live agent sessions: the agent CLI processes Core runs for native
//! sessions, and the requests their plugin backends make about them.
//!
//! Each session has three threads. The output thread reads the CLI's stdout
//! line by line and hands the lines to the backend in `didOutput` batches; the
//! error thread keeps the last bytes of stderr for the user to see when the
//! CLI fails; the input thread writes what the backend sends with
//! `lithe/host/agents/write`, so a CLI that stops reading its input never
//! blocks the backend or the engine. When stdout ends, the output thread waits
//! for the process, ends its process tree and tells the backend and the host.
//!
//! Lithe never logs what flows through a session: logs carry the opaque
//! session ID, the plugin and agent IDs and stable codes only.

use crate::events::{EventQueue, ReportRefusal, SessionEvent, TurnState, MAX_EVENTS_PER_REPORT};
use crate::{log, log_fields, Runtime};
use lithe_lsp_engine::agent_host::{AgentBackend, AgentHostFailure};
use lithe_lsp_transport::process::{LspProcessHandle, LspProcessSpec};
use lithe_plugin_rpc::{
    codes, AGENTS_DID_CANCEL, AGENTS_DID_DECIDE_PERMISSION, AGENTS_DID_EXIT, AGENTS_DID_OUTPUT,
    AGENTS_DID_PROMPT, AGENTS_DID_START,
};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_request_context::Source;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Most sessions of one agent that run at once.
pub(crate) const MAX_SESSIONS_PER_AGENT: usize = 4;

/// Most lines one `lithe/host/agents/write` carries.
const MAX_LINES_PER_WRITE: usize = 256;

/// Most bytes one `lithe/host/agents/write` carries.
const MAX_WRITE_BYTES: usize = 16 * 1024 * 1024;

/// Writes the input thread holds before `lithe/host/agents/write` answers
/// that the CLI is not keeping up.
const WRITE_QUEUE: usize = 64;

/// Longest stdout line handed to the backend. A longer line is dropped and
/// the host is told.
const MAX_OUTPUT_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Most lines of one `didOutput`.
const MAX_OUTPUT_BATCH_LINES: usize = 64;

/// Most bytes of one `didOutput`; a single longer line still goes alone.
const MAX_OUTPUT_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// Bytes of stderr kept for the user.
const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// How long a closed session has to exit by itself before Core ends it.
const CLOSE_GRACE: Duration = Duration::from_secs(3);

/// How long Core waits for the exit status once stdout has ended.
const EXIT_WAIT: Duration = Duration::from_secs(3);

/// How long Core waits for the process tree to be gone.
const DISPOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest prompt, in bytes.
const MAX_PROMPT_BYTES: usize = 1024 * 1024;

/// Most environment variables the host adds for a CLI.
const MAX_ENVIRONMENT_VARIABLES: usize = 256;

/// Longest environment value the host adds for a CLI.
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 32 * 1024;

/// Why a session ended. The first reason recorded wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    /// The CLI exited by itself.
    Exited,
    /// The user closed the session.
    Closed,
    /// The plugin backend stopped.
    BackendEnded,
    /// Core could not reach the backend or the CLI.
    Failed,
}

impl EndReason {
    fn token(self) -> &'static str {
        match self {
            EndReason::Exited => "exited",
            EndReason::Closed => "closed",
            EndReason::BackendEnded => "backendEnded",
            EndReason::Failed => "failed",
        }
    }
}

/// One running agent CLI.
pub(crate) struct LiveSession {
    pub id: String,
    pub plugin_id: String,
    pub agent_id: String,
    /// The engine session of the backend that translates this CLI.
    pub backend: String,
    started: Instant,
    handle: Arc<dyn LspProcessHandle>,
    input: SyncSender<Vec<String>>,
    pub turns: Mutex<TurnState>,
    end: Mutex<Option<EndReason>>,
    stderr_tail: Mutex<VecDeque<u8>>,
}

impl LiveSession {
    fn mark_end(&self, reason: EndReason) {
        let mut end = self.end.lock().unwrap_or_else(PoisonError::into_inner);
        if end.is_none() {
            *end = Some(reason);
        }
    }

    fn end_reason(&self) -> EndReason {
        self.end
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(EndReason::Exited)
    }

    fn owned_by(&self, backend: &AgentBackend<'_>) -> bool {
        self.plugin_id == backend.plugin_id && self.backend == backend.session_id
    }

    fn lock_turns(&self) -> std::sync::MutexGuard<'_, TurnState> {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stderr_tail(&self) -> String {
        let tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let bytes: Vec<u8> = tail.iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Every running session and the events the host reads.
#[derive(Default)]
pub(crate) struct Live {
    sessions: Mutex<BTreeMap<String, Arc<LiveSession>>>,
    next_id: AtomicU64,
    pub queue: EventQueue,
}

impl Live {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<LiveSession>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, session: &str) -> Option<Arc<LiveSession>> {
        self.lock().get(session).cloned()
    }

    fn session(&self, session: &str) -> Result<Arc<LiveSession>, CoreError> {
        self.get(session).ok_or_else(|| {
            CoreError::new(
                ErrorCode::EntryNotFound,
                "The agent session is not running.",
            )
            .with_details("agents.unknownSession")
        })
    }

    fn running(&self, plugin_id: &str, agent_id: &str) -> usize {
        self.lock()
            .values()
            .filter(|session| session.plugin_id == plugin_id && session.agent_id == agent_id)
            .count()
    }

    /// The running sessions, for a host that opens its agent view again.
    pub fn list(&self) -> Value {
        let sessions: Vec<Value> = self
            .lock()
            .values()
            .map(|session| {
                let turns = session.lock_turns();
                let mut entry = json!({
                    "session": session.id,
                    "pluginID": session.plugin_id,
                    "agentID": session.agent_id,
                    "pendingPermissions": turns.pending.keys().collect::<Vec<_>>(),
                });
                if let Some(turn) = turns.active_turn {
                    entry["activeTurn"] = json!(turn);
                }
                if let Some(id) = &turns.agent_session_id {
                    entry["agentSessionID"] = json!(id);
                }
                entry
            })
            .collect();
        json!({ "sessions": sessions })
    }

    /// Ends the sessions of the agents `ending` selects, for example when the
    /// user withdraws `agents.run` or removes the plugin.
    pub fn end_where(&self, ending: impl Fn(&str, &str) -> bool) {
        let ending: Vec<Arc<LiveSession>> = self
            .lock()
            .values()
            .filter(|session| ending(&session.plugin_id, &session.agent_id))
            .cloned()
            .collect();
        for session in ending {
            session.mark_end(EndReason::Closed);
            session.handle.terminate();
        }
    }

    /// The backend ended: its CLIs end too. Runs with the backend's session
    /// lock held, so it only signals; the output threads clean up.
    pub fn backend_ended(&self, backend: &str) {
        let ending: Vec<Arc<LiveSession>> = self
            .lock()
            .values()
            .filter(|session| session.backend == backend)
            .cloned()
            .collect();
        for session in ending {
            session.mark_end(EndReason::BackendEnded);
            session.handle.terminate();
        }
    }

    /// Answers `lithe/host/agents/write`.
    pub fn write(
        &self,
        backend: AgentBackend<'_>,
        params: &Value,
    ) -> Result<Value, AgentHostFailure> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WriteParams {
            session: String,
            lines: Vec<String>,
        }
        let params: WriteParams = serde_json::from_value(params.clone()).map_err(|error| {
            AgentHostFailure::new(
                codes::HOST_INVALID_PARAMS,
                format!("Invalid write: {error}"),
            )
        })?;
        let session = self.owned(&backend, &params.session)?;
        if params.lines.is_empty()
            || params.lines.len() > MAX_LINES_PER_WRITE
            || params.lines.iter().map(String::len).sum::<usize>() > MAX_WRITE_BYTES
            || params.lines.iter().any(|line| line.contains(['\n', '\r']))
        {
            return Err(AgentHostFailure::new(
                codes::HOST_INVALID_PARAMS,
                "A write carries 1 to 256 lines of at most 16 MiB in total, without line breaks.",
            ));
        }
        match session.input.try_send(params.lines) {
            Ok(()) => Ok(json!({})),
            Err(TrySendError::Full(_)) => Err(AgentHostFailure::new(
                codes::HOST_LIMIT_EXCEEDED,
                "The agent CLI is not reading its input; try again later.",
            )),
            Err(TrySendError::Disconnected(_)) => Err(AgentHostFailure::new(
                codes::HOST_NOT_FOUND,
                "The agent session has ended.",
            )),
        }
    }

    /// Answers `lithe/host/agents/report`: checks the events against the
    /// session's turns and queues them for the host.
    pub fn report(
        &self,
        runtime: &Arc<Runtime>,
        backend: AgentBackend<'_>,
        params: &Value,
    ) -> Result<Value, AgentHostFailure> {
        let refuse = |code: &'static str, message: String| {
            log(
                "agent.report.rejected",
                [("plugin", json!(backend.plugin_id)), ("code", json!(code))],
            );
            AgentHostFailure::new(code, message)
        };
        let session_id = params
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let session = self.owned(&backend, session_id)?;
        let Some(raw) = params.get("events").and_then(Value::as_array) else {
            return Err(refuse(
                codes::HOST_INVALID_PARAMS,
                "A report carries an events array.".to_string(),
            ));
        };
        if params.as_object().is_some_and(|object| object.len() != 2)
            || raw.is_empty()
            || raw.len() > MAX_EVENTS_PER_REPORT
        {
            return Err(refuse(
                codes::HOST_INVALID_PARAMS,
                "A report carries session and 1 to 256 events.".to_string(),
            ));
        }
        let mut events = Vec::with_capacity(raw.len());
        for (index, event) in raw.iter().enumerate() {
            let event: SessionEvent = serde_json::from_value(event.clone()).map_err(|error| {
                refuse(
                    codes::HOST_INVALID_PARAMS,
                    format!("Event {index} of the report is malformed: {error}"),
                )
            })?;
            events.push(event);
        }
        session
            .lock_turns()
            .apply(&events)
            .map_err(|ReportRefusal { code, message }| refuse(code, message))?;
        for event in &events {
            if let SessionEvent::RateLimits(limits) = event {
                runtime.records.note_rate_limits(
                    &crate::catalog::record_key(&session.plugin_id, &session.agent_id),
                    limits.clone(),
                    crate::now_ms(),
                );
            }
            let value = serde_json::to_value(event).unwrap_or(Value::Null);
            self.queue.push(&session.id, value);
        }
        Ok(json!({}))
    }

    fn owned(
        &self,
        backend: &AgentBackend<'_>,
        session: &str,
    ) -> Result<Arc<LiveSession>, AgentHostFailure> {
        self.get(session)
            .filter(|live| live.owned_by(backend))
            .ok_or_else(|| {
                AgentHostFailure::new(
                    codes::HOST_NOT_FOUND,
                    "The backend runs no agent session with this ID.",
                )
            })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartRequest {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    #[serde(rename = "agentID")]
    agent_id: String,
    /// The directory the agent works in.
    cwd: String,
    /// The agent's tool as the host resolved it from the plugin's
    /// `requiresTools`.
    tool_path: String,
    /// The workspace root the session belongs to, so the backend of that
    /// workspace translates it.
    #[serde(rename = "rootURI", default)]
    root_uri: Option<String>,
    /// The agent CLI's own ID of a session to resume.
    #[serde(default)]
    resume: Option<String>,
    /// Whether the user trusts the workspace.
    #[serde(default)]
    workspace_trusted: bool,
    /// Variables the host adds to the CLI's environment, for example the
    /// login shell's `PATH`.
    #[serde(default)]
    environment: BTreeMap<String, String>,
}

fn refusal(code: ErrorCode, details: &'static str, message: &str) -> CoreError {
    CoreError::new(code, message).with_details(details)
}

fn require_user(action: &str) -> Result<(), CoreError> {
    if lithe_request_context::trace::current().source == Source::User {
        Ok(())
    } else {
        Err(refusal(
            ErrorCode::PermissionDenied,
            "agents.userActionRequired",
            &format!("Only the user can {action} an agent session."),
        ))
    }
}

/// `agents.start`: starts a native session of an agent.
pub(crate) fn start(runtime: &Arc<Runtime>, payload: Value) -> Result<Value, CoreError> {
    let request: StartRequest = crate::decode(payload)?;
    let plugin_id = request.plugin_id.clone();
    let agent_id = request.agent_id.clone();
    let outcome = start_checked(runtime, request);
    if let Err(error) = &outcome {
        log(
            "agent.session.start_failed",
            [
                ("plugin", json!(plugin_id)),
                ("agent", json!(agent_id)),
                (
                    "reason",
                    json!(error.details.as_deref().unwrap_or("agents.startFailed")),
                ),
            ],
        );
    }
    outcome
}

fn start_checked(runtime: &Arc<Runtime>, request: StartRequest) -> Result<Value, CoreError> {
    require_user("start")?;
    if lithe_workspace::is_remote() {
        return Err(refusal(
            ErrorCode::NotSupported,
            "agents.remoteWorkspace",
            "Agent sessions run only in local workspaces.",
        ));
    }
    if !request.workspace_trusted {
        return Err(refusal(
            ErrorCode::PermissionDenied,
            "agents.untrustedWorkspace",
            "Agent sessions run only in trusted workspaces.",
        ));
    }
    let Some(agent) = runtime.catalog.find(&request.plugin_id, &request.agent_id) else {
        return Err(refusal(
            ErrorCode::EntryNotFound,
            "agents.unknownAgent",
            "No installed plugin declares this agent.",
        ));
    };
    if !agent.may_run {
        return Err(refusal(
            ErrorCode::PermissionDenied,
            "agents.runNotGranted",
            "The user has not allowed this plugin to run its agents.",
        ));
    }
    let Some(native) = agent.agent.native.clone() else {
        return Err(refusal(
            ErrorCode::NotSupported,
            "agents.noNativeMode",
            "The agent has no native session mode.",
        ));
    };
    if runtime.live.running(&request.plugin_id, &request.agent_id) >= MAX_SESSIONS_PER_AGENT {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.tooManySessions",
            "This agent already runs as many sessions as Lithe allows.",
        ));
    }
    let cwd = PathBuf::from(&request.cwd);
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.invalidDirectory",
            "The agent's working directory is not an existing absolute directory.",
        ));
    }
    let tool = PathBuf::from(&request.tool_path);
    if !tool.is_absolute() || !tool.is_file() {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.toolNotFound",
            "The agent's tool is not an existing absolute file.",
        ));
    }
    check_environment(&request.environment)?;
    if request
        .resume
        .as_deref()
        .is_some_and(|resume| resume.is_empty() || resume.len() > 1024)
    {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.invalidResume",
            "The session to resume has an empty or overlong ID.",
        ));
    }

    let link = &runtime.link;
    let Some(backend) = link.session_for(
        &request.plugin_id,
        &native.function,
        request.root_uri.as_deref(),
    ) else {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.backendUnavailable",
            "The agent plugin's backend is not running.",
        ));
    };
    let answer = link
        .call(
            &backend,
            &request.plugin_id,
            &native.function,
            lithe_plugin_agents::launch_params(
                &request.agent_id,
                &request.cwd,
                request.resume.as_deref(),
            ),
            Duration::from_millis(native.timeout_milliseconds()),
        )
        .map_err(|failure| {
            CoreError::new(
                ErrorCode::ProcessFailed,
                "The agent plugin could not prepare the session.",
            )
            .with_details(launch_failure_code(&failure.code))
        })?;
    let launch =
        lithe_plugin_agents::decode_native_launch(&agent.agent, answer).map_err(|rejection| {
            CoreError::new(ErrorCode::InvalidRequest, rejection.message)
                .with_details(rejection.code)
        })?;

    let streams = link
        .launcher()
        .launch(LspProcessSpec {
            executable: tool,
            arguments: launch.arguments,
            working_directory: cwd,
            environment: request.environment,
        })
        .map_err(|_| {
            refusal(
                ErrorCode::ProcessStartFailed,
                "agents.processStartFailed",
                "Could not start the agent CLI.",
            )
        })?;

    let id = format!(
        "agent-{}",
        runtime.live.next_id.fetch_add(1, Ordering::Relaxed) + 1
    );
    let (input, input_queue) = mpsc::sync_channel(WRITE_QUEUE);
    let session = Arc::new(LiveSession {
        id: id.clone(),
        plugin_id: request.plugin_id.clone(),
        agent_id: request.agent_id.clone(),
        backend: backend.clone(),
        started: Instant::now(),
        handle: streams.handle.clone(),
        input,
        turns: Mutex::new(TurnState::default()),
        end: Mutex::new(None),
        stderr_tail: Mutex::new(VecDeque::new()),
    });
    runtime.live.lock().insert(id.clone(), session.clone());

    let mut started = lithe_plugin_agents::launch_params(
        &request.agent_id,
        &request.cwd,
        request.resume.as_deref(),
    );
    started["session"] = json!(id);
    if link.notify(&backend, AGENTS_DID_START, started).is_err() {
        runtime.live.lock().remove(&id);
        streams.handle.terminate();
        let _ = streams.handle.dispose(DISPOSE_TIMEOUT);
        return Err(refusal(
            ErrorCode::ProcessFailed,
            "agents.backendUnavailable",
            "The agent plugin's backend stopped while the session started.",
        ));
    }

    let mut event = json!({
        "kind": "started",
        "pluginID": request.plugin_id,
        "agentID": request.agent_id,
        "mode": "native",
    });
    if let Some(resume) = &request.resume {
        event["resume"] = json!(resume);
    }
    runtime.live.queue.push(&id, event);
    log(
        "agent.session.started",
        [
            ("plugin", json!(session.plugin_id)),
            ("agent", json!(session.agent_id)),
            ("session", json!(id)),
            ("mode", json!("native")),
        ],
    );

    // A stream thread that cannot start ends the session as failed: without
    // its input the CLI never gets the backend's writes, and without a stderr
    // reader the CLI can block on a full pipe. The session already started,
    // so the host still gets its `exited` event.
    let handle = streams.handle.clone();
    let input_started = spawn("agent-input", move || write_input(handle, input_queue));
    let errors_session = session.clone();
    let errors = streams.errors;
    let errors_started = spawn("agent-errors", move || read_errors(&errors_session, errors));
    if !(input_started && errors_started) {
        session.mark_end(EndReason::Failed);
        session.handle.terminate();
    }
    let output_runtime = runtime.clone();
    let output_session = session.clone();
    let output = streams.output;
    let output_started = spawn("agent-output", move || {
        read_output(&output_runtime, &output_session, output);
        finish(&output_runtime, &output_session);
    });
    if !output_started {
        // Only the output thread ends a session, so end this one here.
        session.mark_end(EndReason::Failed);
        session.handle.terminate();
        finish(runtime, &session);
    }
    Ok(json!({ "session": id, "mode": "native" }))
}

fn launch_failure_code(code: &str) -> &'static str {
    if code == "requestTimeout" || code.ends_with(".timeout") {
        "agents.launchTimeout"
    } else {
        "agents.launchFailed"
    }
}

fn check_environment(environment: &BTreeMap<String, String>) -> Result<(), CoreError> {
    let valid = environment.len() <= MAX_ENVIRONMENT_VARIABLES
        && environment.iter().all(|(name, value)| {
            !name.is_empty()
                && name.len() <= 256
                && !name.contains(['=', '\0'])
                && value.len() <= MAX_ENVIRONMENT_VALUE_BYTES
                && !value.contains('\0')
        });
    if valid {
        Ok(())
    } else {
        Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.invalidEnvironment",
            "The environment for the agent CLI is invalid.",
        ))
    }
}

#[cfg(test)]
thread_local! {
    /// Thread names [`spawn`] refuses on this thread, standing in for a
    /// thread the system cannot start.
    pub(crate) static REFUSED_THREADS: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Starts a named thread; false when the system could not start it.
fn spawn(name: &'static str, work: impl FnOnce() + Send + 'static) -> bool {
    #[cfg(test)]
    if REFUSED_THREADS.with(|refused| refused.borrow().contains(&name)) {
        return false;
    }
    thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
        .is_ok()
}

fn write_input(handle: Arc<dyn LspProcessHandle>, queue: Receiver<Vec<String>>) {
    for lines in queue {
        for line in lines {
            let mut frame = line.into_bytes();
            frame.push(b'\n');
            if handle.write_input(&frame).is_err() {
                return;
            }
        }
    }
}

fn read_errors(session: &LiveSession, mut errors: Box<dyn Read + Send>) {
    let mut buffer = [0u8; 4096];
    loop {
        match errors.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                let mut tail = session
                    .stderr_tail
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                tail.extend(&buffer[..count]);
                let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                tail.drain(..excess);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

enum Line {
    Text(String),
    TooLong,
    End,
}

/// Reads one line of at most `limit` bytes, without its line break. A longer
/// line is consumed and reported as [`Line::TooLong`].
fn read_line(reader: &mut impl BufRead, limit: usize) -> io::Result<Line> {
    let mut bytes = Vec::new();
    let mut too_long = false;
    let mut read_any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            if !read_any {
                return Ok(Line::End);
            }
            break;
        }
        read_any = true;
        let (used, found) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        let content = &available[..if found { used - 1 } else { used }];
        if !too_long {
            if bytes.len() + content.len() > limit {
                too_long = true;
                bytes = Vec::new();
            } else {
                bytes.extend_from_slice(content);
            }
        }
        reader.consume(used);
        if found {
            break;
        }
    }
    if too_long {
        return Ok(Line::TooLong);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Line::Text(String::from_utf8(bytes).unwrap_or_else(
        |error| String::from_utf8_lossy(error.as_bytes()).into_owned(),
    )))
}

fn read_output(runtime: &Arc<Runtime>, session: &Arc<LiveSession>, output: Box<dyn Read + Send>) {
    let mut reader = BufReader::with_capacity(64 * 1024, output);
    let mut batch: Vec<String> = Vec::new();
    let mut batch_bytes = 0usize;
    loop {
        let line = read_line(&mut reader, MAX_OUTPUT_LINE_BYTES);
        let ended = match line {
            Ok(Line::Text(line)) => {
                batch_bytes += line.len();
                batch.push(line);
                false
            }
            Ok(Line::TooLong) => {
                runtime
                    .live
                    .queue
                    .push(&session.id, json!({ "kind": "outputLineDropped" }));
                false
            }
            Ok(Line::End) | Err(_) => true,
        };
        let flush = ended
            || batch.len() >= MAX_OUTPUT_BATCH_LINES
            || batch_bytes >= MAX_OUTPUT_BATCH_BYTES
            || reader.buffer().is_empty();
        if flush && !batch.is_empty() {
            let lines = std::mem::take(&mut batch);
            batch_bytes = 0;
            let params = json!({ "session": session.id, "lines": lines });
            if runtime
                .link
                .notify(&session.backend, AGENTS_DID_OUTPUT, params)
                .is_err()
            {
                // The backend can no longer translate the CLI: end it rather
                // than let it act without anyone seeing what it does.
                session.mark_end(EndReason::Failed);
                session.handle.terminate();
                return;
            }
        }
        if ended {
            return;
        }
    }
}

fn finish(runtime: &Arc<Runtime>, session: &Arc<LiveSession>) {
    let handle = &session.handle;
    let deadline = Instant::now() + EXIT_WAIT;
    let mut status = handle.exit_status();
    while status.is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
        status = handle.exit_status();
    }
    if status.is_none() {
        handle.terminate();
    }
    let disposed = handle.dispose(DISPOSE_TIMEOUT).is_ok();
    let exit_code = status.or_else(|| handle.exit_status()).flatten();
    session.mark_end(EndReason::Exited);
    let reason = if disposed {
        session.end_reason()
    } else {
        EndReason::Failed
    };

    runtime.live.lock().remove(&session.id);
    if reason != EndReason::BackendEnded {
        let mut params = json!({ "session": session.id });
        if let Some(code) = exit_code {
            params["exitCode"] = json!(code);
        }
        let _ = runtime
            .link
            .notify(&session.backend, AGENTS_DID_EXIT, params);
    }
    let mut event = json!({ "kind": "exited", "reason": reason.token() });
    if let Some(code) = exit_code {
        event["exitCode"] = json!(code);
    }
    let tail = session.stderr_tail();
    if !tail.trim().is_empty() {
        event["stderrTail"] = json!(tail);
    }
    runtime.live.queue.push(&session.id, event);

    let mut fields = vec![
        ("session", json!(session.id)),
        ("reason", json!(reason.token())),
        (
            "duration",
            json!(u64::try_from(session.started.elapsed().as_millis()).unwrap_or(u64::MAX)),
        ),
    ];
    if let Some(code) = exit_code {
        fields.push(("exit_code", json!(code)));
    }
    log_fields("agent.session.finished", fields.into_iter());
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRequest {
    session: String,
}

/// `agents.prompt`: sends the user's prompt as the next turn.
pub(crate) fn prompt(runtime: &Arc<Runtime>, payload: Value) -> Result<Value, CoreError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PromptRequest {
        session: String,
        text: String,
    }
    let request: PromptRequest = crate::decode(payload)?;
    require_user("prompt")?;
    if request.text.trim().is_empty() || request.text.len() > MAX_PROMPT_BYTES {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.invalidPrompt",
            "A prompt has text of at most 1 MiB.",
        ));
    }
    let session = runtime.live.session(&request.session)?;
    let Some(turn) = session.lock_turns().start_turn() else {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.turnInProgress",
            "The agent is still working on the previous prompt.",
        ));
    };
    runtime
        .live
        .queue
        .push(&session.id, json!({ "kind": "turnStarted", "turn": turn }));
    let params = json!({ "session": session.id, "turn": turn, "text": request.text });
    if runtime
        .link
        .notify(&session.backend, AGENTS_DID_PROMPT, params)
        .is_err()
    {
        let mut turns = session.lock_turns();
        if turns.active_turn == Some(turn) {
            turns.active_turn = None;
        }
        drop(turns);
        runtime.live.queue.push(
            &session.id,
            json!({ "kind": "turnEnded", "turn": turn, "reason": "failed" }),
        );
        return Err(refusal(
            ErrorCode::ProcessFailed,
            "agents.backendUnavailable",
            "The agent plugin's backend did not take the prompt.",
        ));
    }
    Ok(json!({ "turn": turn }))
}

/// `agents.cancel`: asks the agent to stop the turn in progress.
pub(crate) fn cancel(runtime: &Arc<Runtime>, payload: Value) -> Result<Value, CoreError> {
    let request: SessionRequest = crate::decode(payload)?;
    let session = runtime.live.session(&request.session)?;
    let Some(turn) = session.lock_turns().active_turn else {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "agents.noTurnInProgress",
            "The agent is not working on a prompt.",
        ));
    };
    runtime
        .link
        .notify(
            &session.backend,
            AGENTS_DID_CANCEL,
            json!({ "session": session.id, "turn": turn }),
        )
        .map_err(|_| {
            refusal(
                ErrorCode::ProcessFailed,
                "agents.backendUnavailable",
                "The agent plugin's backend did not take the cancellation.",
            )
        })?;
    Ok(json!({ "turn": turn }))
}

/// `agents.respondPermission`: the user's answer to a permission request.
pub(crate) fn respond_permission(
    runtime: &Arc<Runtime>,
    payload: Value,
) -> Result<Value, CoreError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PermissionAnswer {
        session: String,
        permission: String,
        option: String,
    }
    let request: PermissionAnswer = crate::decode(payload)?;
    require_user("answer a permission request of")?;
    let session = runtime.live.session(&request.session)?;
    let turn = session
        .lock_turns()
        .decide(&request.permission, &request.option)
        .map_err(|code| {
            refusal(
                ErrorCode::InvalidRequest,
                code,
                "The agent is not waiting for this answer.",
            )
        })?;
    runtime.live.queue.push(
        &session.id,
        json!({
            "kind": "permissionDecided",
            "turn": turn,
            "permission": request.permission,
            "option": request.option,
        }),
    );
    runtime
        .link
        .notify(
            &session.backend,
            AGENTS_DID_DECIDE_PERMISSION,
            json!({
                "session": session.id,
                "turn": turn,
                "permission": request.permission,
                "option": request.option,
            }),
        )
        .map_err(|_| {
            refusal(
                ErrorCode::ProcessFailed,
                "agents.backendUnavailable",
                "The agent plugin's backend did not take the answer.",
            )
        })?;
    Ok(json!({ "turn": turn }))
}

/// `agents.close`: closes the CLI's input, then ends it if it does not exit.
pub(crate) fn close(runtime: &Arc<Runtime>, payload: Value) -> Result<Value, CoreError> {
    let request: SessionRequest = crate::decode(payload)?;
    let session = runtime.live.session(&request.session)?;
    session.mark_end(EndReason::Closed);
    session.handle.close_input();
    let handle = session.handle.clone();
    let waiting = spawn("agent-close", move || {
        let deadline = Instant::now() + CLOSE_GRACE;
        while handle.exit_status().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        if handle.exit_status().is_none() {
            handle.terminate();
        }
    });
    if !waiting {
        // Nothing can wait out the grace period, so end the CLI now.
        session.handle.terminate();
    }
    Ok(json!({}))
}

/// `agents.waitEvents`: the next session events for the host.
pub(crate) fn wait_events(runtime: &Arc<Runtime>, payload: Value) -> Result<Value, CoreError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct WaitRequest {
        #[serde(default)]
        after: u64,
        #[serde(default)]
        timeout_milliseconds: u64,
        #[serde(default)]
        limit: Option<usize>,
    }
    let request: WaitRequest = crate::decode(payload)?;
    Ok(runtime.live.queue.wait(
        request.after,
        Duration::from_millis(request.timeout_milliseconds),
        request.limit.unwrap_or(256),
    ))
}
