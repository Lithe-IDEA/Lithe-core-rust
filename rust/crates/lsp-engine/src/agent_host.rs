//! Engine side of agent sessions (plan API-45).
//!
//! Core's agent runtime owns the agent CLI processes; the plugin backend only
//! translates between the CLI's own protocol and Lithe's session events. Three
//! paths connect the two, and each stays inside Core:
//!
//! - A backend's `lithe/host/agents/*` requests reach the installed
//!   [`AgentHost`], which checks that the session is the backend's own and
//!   answers. The host application never sees them.
//! - The runtime sends the backend the agent-session notifications with
//!   [`notify_agent_backend`]. The host may not send them.
//! - The runtime calls the backend's launch and session-file functions with
//!   [`call_agent_function`], a blocking call whose answer never becomes a host
//!   event.
//!
//! The runtime's threads call into the engine with no lock of their own held,
//! and the engine calls the [`AgentHost`] while it holds the backend's session
//! lock, so an `AgentHost` must never call back into the engine from
//! [`AgentHost::answer`] or [`AgentHost::backend_ended`].

use super::{
    engine, json_rpc_error_with_data, json_rpc_notification, json_rpc_result, LspLifecycleState,
    LspRuntimeError, PluginFunctionRequest, RuntimeSession,
};
use lithe_plugin_rpc::{codes, host_rpc_code, FunctionReference, AGENTS_WRITE, INVOKE_METHOD};
use lithe_protocol::{CoreError, ErrorCode};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

/// Operation IDs of the calls the agent runtime makes. The host may not use
/// them, so a host event never answers one of these calls.
pub const AGENT_OPERATION_PREFIX: &str = "agent-call:";

/// Time a call may still take after its own deadline before the caller stops
/// waiting: the session monitor ends an overdue call itself, so this only
/// guards against an answer that is lost.
const CALL_GRACE: Duration = Duration::from_secs(5);

/// The backend that sent a `lithe/host/agents/*` request.
#[derive(Debug, Clone, Copy)]
pub struct AgentBackend<'a> {
    /// The engine session of the backend process.
    pub session_id: &'a str,
    /// The plugin that owns the backend.
    pub plugin_id: &'a str,
}

/// Why the agent runtime refused a backend request. `code` is a stable
/// `plugin.host.*` or `plugin.agents.*` code, sent as `error.data.code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHostFailure {
    pub code: String,
    pub message: String,
}

impl AgentHostFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// The agent runtime as the engine reaches it.
pub trait AgentHost: Send + Sync {
    /// Answers one `lithe/host/agents/*` request of a backend. Called with the
    /// backend's session lock held: it must not block on the backend or call
    /// into the engine.
    fn answer(
        &self,
        backend: AgentBackend<'_>,
        method: &str,
        params: &Value,
    ) -> Result<Value, AgentHostFailure>;

    /// The backend session is stopping or has stopped; the agent sessions it
    /// translated end. Called with the session lock held, like `answer`.
    fn backend_ended(&self, session_id: &str);
}

static AGENT_HOST: RwLock<Option<Arc<dyn AgentHost>>> = RwLock::new(None);

/// Installs the agent runtime. A later call replaces it.
pub fn install_agent_host(host: Arc<dyn AgentHost>) {
    *AGENT_HOST.write().unwrap_or_else(PoisonError::into_inner) = Some(host);
}

fn agent_host() -> Option<Arc<dyn AgentHost>> {
    AGENT_HOST
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Answers a backend's `lithe/host/agents/*` request. Returns the message to
/// write to that backend.
pub(crate) fn answer_request(
    session: &RuntimeSession,
    method: &str,
    request: &Value,
) -> Result<Vec<String>, CoreError> {
    let rpc_id = request.get("id").cloned().unwrap_or(Value::Null);
    let params = request.get("params").unwrap_or(&Value::Null);
    let outcome = match (agent_host(), session.plugin_id.as_deref()) {
        (Some(host), Some(plugin_id)) => host.answer(
            AgentBackend {
                session_id: &session.id,
                plugin_id,
            },
            method,
            params,
        ),
        _ => Err(AgentHostFailure::new(
            codes::HOST_UNAVAILABLE,
            "This Lithe build does not run agent sessions.",
        )),
    };
    let message = match outcome {
        Ok(result) => json_rpc_result(&rpc_id, result)?,
        Err(failure) => json_rpc_error_with_data(
            &rpc_id,
            host_rpc_code(&failure.code),
            &lithe_plugin_rpc::bounded_host_message(&failure.message),
            json!({ "code": failure.code }),
        )?,
    };
    Ok(vec![message])
}

/// Tells the agent runtime that a backend is ending.
pub(crate) fn observe_lifecycle(session: &RuntimeSession, lifecycle: LspLifecycleState) {
    if !matches!(
        lifecycle,
        LspLifecycleState::Stopping | LspLifecycleState::Stopped | LspLifecycleState::Failed
    ) || session.plugin_id.is_none()
    {
        return;
    }
    if let Some(host) = agent_host() {
        host.backend_ended(&session.id);
    }
}

/// The ready backend session of `plugin_id` that serves `function_id`, if one
/// runs. A session of the same workspace root is preferred when `root_uri`
/// names one.
pub fn agent_backend_session(
    plugin_id: &str,
    function_id: &str,
    root_uri: Option<&str>,
) -> Option<String> {
    let candidates: Vec<Arc<RuntimeSession>> = {
        let sessions = engine()
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        sessions
            .values()
            .filter(|session| {
                session.function_relay_enabled && session.plugin_id.as_deref() == Some(plugin_id)
            })
            .cloned()
            .collect()
    };
    let serving: Vec<_> = candidates
        .into_iter()
        .filter(|session| session.serves_function(function_id))
        .collect();
    serving
        .iter()
        .find(|session| Some(session.root_uri.as_str()) == root_uri)
        .or_else(|| serving.first())
        .map(|session| session.id.clone())
}

/// Sends one agent-session notification to a backend that holds the agents
/// grant. The write is bounded like a relayed call's: a backend that stops
/// reading its input fails and its process is killed.
pub fn notify_agent_backend(
    session_id: &str,
    method: &str,
    params: Value,
) -> Result<(), CoreError> {
    if !lithe_plugin_rpc::is_agent_notification(method) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "This is not an agent-session notification.",
        ));
    }
    let session = engine().session(session_id)?;
    session.bounded_relay_write(|| session.agent_notify(method, params))
}

impl RuntimeSession {
    fn agent_notify(&self, method: &str, params: Value) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let state = self.lock_state()?;
            if self.plugin_id.is_none()
                || state.lifecycle != LspLifecycleState::Ready
                || !state.client.initialized
                || state.client.plugin_function_version.is_none()
            {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The agent plugin's backend is not ready.",
                ));
            }
            if !state.client.host_services.contains(AGENTS_WRITE) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The plugin backend was not granted agent sessions.",
                ));
            }
            json_rpc_notification(method, params)?
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "agentNotify")
    }
}

/// How a call the agent runtime made ended without a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCallFailure {
    /// A `plugin.function.*` code, or the engine's code when the call could
    /// not be sent.
    pub code: String,
    pub message: String,
    /// The plugin's own error code, when the function returned one.
    pub plugin_error_code: Option<String>,
}

type Waiters = Mutex<BTreeMap<String, mpsc::Sender<Result<Value, AgentCallFailure>>>>;

static WAITERS: Waiters = Mutex::new(BTreeMap::new());
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

/// Calls one function of a backend and waits for its answer. `timeout` is the
/// call's deadline; the session ends an overdue call with
/// `plugin.function.timeout`.
pub fn call_agent_function(
    session_id: &str,
    plugin_id: &str,
    function_id: &str,
    params: Value,
    context: Option<Value>,
    timeout: Duration,
) -> Result<Value, AgentCallFailure> {
    let operation_id = format!(
        "{AGENT_OPERATION_PREFIX}{}",
        NEXT_CALL.fetch_add(1, Ordering::Relaxed)
    );
    let (sender, receiver) = mpsc::channel();
    lock_waiters().insert(operation_id.clone(), sender);
    let request = PluginFunctionRequest {
        session_id: session_id.to_string(),
        operation_id: Some(operation_id.clone()),
        function: FunctionReference {
            plugin_id: plugin_id.to_string(),
            function_id: function_id.to_string(),
        },
        params,
        timeout_milliseconds: Some(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
        context,
    };
    let sent = engine()
        .session(session_id)
        .and_then(|session| session.function_request(request, operation_id.clone(), None));
    if let Err(error) = sent {
        lock_waiters().remove(&operation_id);
        return Err(AgentCallFailure {
            code: error
                .details
                .clone()
                .unwrap_or_else(|| codes::UNAVAILABLE.to_string()),
            message: error.message,
            plugin_error_code: None,
        });
    }
    match receiver.recv_timeout(timeout + CALL_GRACE) {
        Ok(outcome) => outcome,
        Err(_) => {
            lock_waiters().remove(&operation_id);
            if let Ok(session) = engine().session(session_id) {
                let _ = session.cancel_operation(&operation_id);
            }
            Err(AgentCallFailure {
                code: "requestTimeout".to_string(),
                message: "The agent plugin did not answer in time.".to_string(),
                plugin_error_code: None,
            })
        }
    }
}

/// Whether the host may not use this operation ID.
pub(crate) fn is_agent_operation_id(operation_id: &str) -> bool {
    operation_id.starts_with(AGENT_OPERATION_PREFIX)
}

/// Hands the end of an agent call to its caller. Returns `false` when the
/// operation is not one; the event then goes on as usual.
pub(crate) fn finish_call(
    operation_id: &str,
    method: &str,
    result: Option<Value>,
    error: Option<LspRuntimeError>,
) -> bool {
    if method != INVOKE_METHOD || !is_agent_operation_id(operation_id) {
        return false;
    }
    let outcome = match error {
        Some(error) => Err(AgentCallFailure {
            code: error.code,
            message: error.message,
            plugin_error_code: error.plugin_error_code,
        }),
        None => Ok(result
            .and_then(|wrapped| wrapped.get("value").cloned())
            .unwrap_or(Value::Null)),
    };
    if let Some(waiter) = lock_waiters().remove(operation_id) {
        let _ = waiter.send(outcome);
    }
    true
}

fn lock_waiters(
) -> std::sync::MutexGuard<'static, BTreeMap<String, mpsc::Sender<Result<Value, AgentCallFailure>>>>
{
    WAITERS.lock().unwrap_or_else(PoisonError::into_inner)
}
