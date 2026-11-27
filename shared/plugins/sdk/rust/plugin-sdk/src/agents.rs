//! Native agent sessions and the `agents` host service (plan API-45).
//!
//! Core starts an agent's CLI and relays it through the plugin backend: the
//! CLI's standard output arrives as `lithe/host/agents/didOutput`, the user's
//! prompts, cancellations and permission decisions as `didPrompt`,
//! `didCancel` and `didDecidePermission`, and the backend answers with
//! `lithe/host/agents/write` (the CLI's standard input) and
//! `lithe/host/agents/report` (session events). A translation is only right
//! when it sees the CLI's lines in order, so the SDK gives every session one
//! worker thread that runs its handler's methods one at a time, in the order
//! Core sent the notifications.

use crate::backend::{failure_kind, Shared};
use crate::error::FunctionError;
use crate::host::Host;
use crate::panic_boundary::catch as catch_unwind;
use crate::protocol::methods;
use crate::SdkError;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

/// What `lithe/host/agents/didStart` says about a session Core started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSessionStart {
    /// Core's ID of the session, such as `agent-1`.
    pub session: String,
    /// The `agents[].id` from the manifest.
    pub agent_id: String,
    /// The working directory the CLI runs in.
    pub cwd: String,
    /// The CLI's own session ID to resume, when the user resumes one.
    pub resume: Option<String>,
}

/// The plugin's translation of one native session. Every method runs on
/// the session's own thread, one at a time and in the order Core sent the
/// notifications, so a method may block on host requests.
///
/// A failing [`prompt`](Self::prompt) ends its turn: the SDK reports
/// `turnEnded` with reason `failed`. Other failures are only logged.
pub trait AgentSession: Send {
    /// Lines the CLI wrote to standard output, without line breaks.
    fn output(&mut self, lines: Vec<String>) -> Result<(), FunctionError>;
    /// The user's prompt for `turn`; write what the CLI expects.
    fn prompt(&mut self, turn: u64, text: String) -> Result<(), FunctionError>;
    /// The user cancelled `turn`.
    fn cancel(&mut self, _turn: u64) -> Result<(), FunctionError> {
        Ok(())
    }
    /// The user answered permission request `permission` with `option`.
    fn decide_permission(
        &mut self,
        turn: u64,
        permission: String,
        option: String,
    ) -> Result<(), FunctionError>;
    /// The CLI ended; the session gets no further calls.
    fn exited(&mut self, _exit_code: Option<i64>) {}
}

/// The host methods of one session: [`write`](Self::write) to the CLI's
/// standard input and [`report`](Self::report) session events.
#[derive(Clone)]
pub struct AgentSessionHandle {
    id: String,
    host: Host,
}

impl AgentSessionHandle {
    /// Core's ID of the session.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Host services outside the session, such as other plugins' functions.
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// `lithe/host/agents/write`: 1-256 lines for the CLI's standard input,
    /// without line breaks; Core adds the newline.
    pub fn write<S: AsRef<str>>(&self, lines: &[S]) -> Result<(), SdkError> {
        self.host.agents().write(&self.id, lines)
    }

    /// `lithe/host/agents/report`: 1-256 session events, applied together.
    pub fn report(&self, events: &[Value]) -> Result<(), SdkError> {
        self.host.agents().report(&self.id, events)
    }
}

/// The host's `agents` service. Core answers it; each method needs its own
/// grant: `write` and `report` `agents.native`, `usage` `agents.usage.read`.
pub struct Agents {
    pub(crate) host: Host,
}

impl Agents {
    /// Writes `lines` to the standard input of `session`'s CLI.
    pub fn write<S: AsRef<str>>(&self, session: &str, lines: &[S]) -> Result<(), SdkError> {
        let lines: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();
        self.host
            .request(
                methods::AGENTS_WRITE,
                json!({ "session": session, "lines": lines }),
            )
            .map(drop)
    }

    /// Reports `events` of `session`; one refused event refuses them all.
    pub fn report(&self, session: &str, events: &[Value]) -> Result<(), SdkError> {
        self.host
            .request(
                methods::AGENTS_REPORT,
                json!({ "session": session, "events": events }),
            )
            .map(drop)
    }

    /// The usage totals `{ sinceMs, untilMs, summaries, rateLimits }` for
    /// `{ groupBy, sinceMs?, untilMs?, utcOffsetSeconds? }`.
    pub fn usage(&self, params: Value) -> Result<Value, SdkError> {
        match self.host.request(methods::AGENTS_USAGE, params)? {
            result @ Value::Object(_) => Ok(result),
            _ => Err(SdkError::new(
                "sdk.host.invalidResult",
                "lithe/host/agents/usage answered with something other than an object.",
            )),
        }
    }
}

pub(crate) type AgentFactory = Arc<
    dyn Fn(AgentSessionStart, AgentSessionHandle) -> Result<Box<dyn AgentSession>, FunctionError>
        + Send
        + Sync,
>;

/// One notification for a session's worker.
enum Notice {
    Output(Vec<String>),
    Prompt(u64, String),
    Cancel(u64),
    Decide(u64, String, String),
    Exit(Option<i64>),
}

/// The workers of the live sessions, by session ID. When the backend stops
/// serving, the queues are dropped and each worker ends after its current
/// call.
#[derive(Default)]
pub(crate) struct AgentWorkers {
    sessions: Mutex<HashMap<String, mpsc::Sender<Notice>>>,
}

pub(crate) fn is_agent_notification(method: &str) -> bool {
    [
        methods::AGENTS_DID_START,
        methods::AGENTS_DID_OUTPUT,
        methods::AGENTS_DID_PROMPT,
        methods::AGENTS_DID_CANCEL,
        methods::AGENTS_DID_DECIDE_PERMISSION,
        methods::AGENTS_DID_EXIT,
    ]
    .contains(&method)
}

impl AgentWorkers {
    /// Routes one `lithe/host/agents/did*` notification. Core checked its
    /// shape; a notification of a session the SDK did not start is dropped.
    pub(crate) fn route(
        &self,
        shared: &Arc<Shared>,
        factory: Option<&AgentFactory>,
        method: &str,
        params: &Value,
    ) {
        let session = params["session"].as_str().unwrap_or_default().to_string();
        if method == methods::AGENTS_DID_START {
            let Some(factory) = factory else {
                return shared.failed("notification", "agent_unhandled");
            };
            let start = AgentSessionStart {
                session: session.clone(),
                agent_id: text(&params["agentID"]),
                cwd: text(&params["cwd"]),
                resume: params["resume"].as_str().map(str::to_string),
            };
            let (sender, receiver) = mpsc::channel();
            let spawned = {
                let shared = Arc::clone(shared);
                let factory = Arc::clone(factory);
                thread::Builder::new()
                    .name(format!("lithe-agent-{session}"))
                    .spawn(move || run(&shared, &factory, start, &receiver))
            };
            match spawned {
                Ok(_) => {
                    lock(&self.sessions).insert(session, sender);
                }
                Err(_) => shared.failed("notification", "worker_start_failed"),
            }
            return;
        }
        let notice = match method {
            methods::AGENTS_DID_OUTPUT => Notice::Output(
                params["lines"]
                    .as_array()
                    .map(|lines| lines.iter().map(text).collect())
                    .unwrap_or_default(),
            ),
            methods::AGENTS_DID_PROMPT => {
                Notice::Prompt(turn(&params["turn"]), text(&params["text"]))
            }
            methods::AGENTS_DID_CANCEL => Notice::Cancel(turn(&params["turn"])),
            methods::AGENTS_DID_DECIDE_PERMISSION => Notice::Decide(
                turn(&params["turn"]),
                text(&params["permission"]),
                text(&params["option"]),
            ),
            _ => Notice::Exit(params["exitCode"].as_i64()),
        };
        let exit = matches!(notice, Notice::Exit(_));
        let mut sessions = lock(&self.sessions);
        let sender = if exit {
            sessions.remove(&session)
        } else {
            sessions.get(&session).cloned()
        };
        drop(sessions);
        match sender {
            Some(sender) => {
                // A worker that already ended has nothing left to run.
                let _ = sender.send(notice);
            }
            None if factory.is_some() => shared.failed("notification", "agent_session_unknown"),
            None => {}
        }
    }
}

/// Runs one session: starts its handler, then its notifications in order.
fn run(
    shared: &Arc<Shared>,
    factory: &AgentFactory,
    start: AgentSessionStart,
    receiver: &mpsc::Receiver<Notice>,
) {
    let handle = AgentSessionHandle {
        id: start.session.clone(),
        host: Host::new(Arc::clone(shared), Vec::new()),
    };
    let mut handler = match catch_unwind(AssertUnwindSafe(|| factory(start, handle.clone()))) {
        Ok(Ok(handler)) => Some(handler),
        Ok(Err(error)) => {
            shared.failed("notification", failure_kind(&error));
            None
        }
        Err(_) => {
            shared.failed("notification", "panic");
            None
        }
    };
    while let Ok(notice) = receiver.recv() {
        let prompt = match &notice {
            Notice::Prompt(turn, _) => Some(*turn),
            _ => None,
        };
        let exit = matches!(notice, Notice::Exit(_));
        let outcome = match handler.as_mut() {
            Some(handler) => catch_unwind(AssertUnwindSafe(|| deliver(handler.as_mut(), notice))),
            None => Ok(Err(FunctionError::Cancelled)),
        };
        let failed = match outcome {
            Ok(Ok(())) => false,
            Ok(Err(FunctionError::Cancelled)) => true,
            Ok(Err(error)) => {
                shared.failed("notification", failure_kind(&error));
                true
            }
            Err(_) => {
                shared.failed("notification", "panic");
                true
            }
        };
        if let (true, Some(turn)) = (failed, prompt) {
            // No handler runs the turn, so it ends here. The handler may
            // have ended it already; Core then refuses the second end.
            let _ =
                handle.report(&[json!({ "kind": "turnEnded", "turn": turn, "reason": "failed" })]);
        }
        if exit {
            break;
        }
    }
}

fn deliver(handler: &mut dyn AgentSession, notice: Notice) -> Result<(), FunctionError> {
    match notice {
        Notice::Output(lines) => handler.output(lines),
        Notice::Prompt(turn, text) => handler.prompt(turn, text),
        Notice::Cancel(turn) => handler.cancel(turn),
        Notice::Decide(turn, permission, option) => {
            handler.decide_permission(turn, permission, option)
        }
        Notice::Exit(code) => {
            handler.exited(code);
            Ok(())
        }
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn turn(value: &Value) -> u64 {
    value.as_u64().unwrap_or_default()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
