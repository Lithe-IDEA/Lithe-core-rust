//! Engine and session state: the session table, the per-session state behind its lock
//! and the pending request and function-call tables.

use super::*;

#[cfg(test)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub session_id: String,
    pub provider_id: String,
    /// The workspace this session serves. Replacing a workspace means stopping
    /// the session bound to the old root, so callers need to be able to tell
    /// which root a session belongs to.
    pub root_uri: String,
    pub state: LspLifecycleState,
    pub initialized: bool,
    pub open_documents: BTreeMap<String, LspClientDocument>,
    pub pending_operation_ids: Vec<String>,
    pub diagnostic_versions: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// Response handling path required by one pending JSON-RPC request.
pub(crate) enum PendingKind {
    /// Initial handshake whose result transitions the session to ready.
    Initialize,
    /// Ordinary semantic feature whose result becomes an operation event.
    Feature,
    /// Request a plugin backend sent to its own language server through
    /// `lsp.pluginServerRequest`; the answer reaches the backend verbatim.
    PluginServer,
    /// Shutdown handshake after which the engine sends `exit`.
    Shutdown,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingRequest {
    pub(crate) kind: PendingKind,
    pub(crate) operation_id: Option<String>,
    pub(crate) method: String,
    pub(crate) document_uri: Option<String>,
    /// Document version observed when a presentation request was allocated.
    pub(crate) document_version: Option<i64>,
    pub(crate) created_at: Instant,
    pub(crate) deadline: Instant,
}

/// Links one host operation to the exact backend request and function it owns.
pub(crate) struct PendingFunctionCall {
    pub(crate) function_id: String,
    /// Also serves as the opaque wire operation ID; request IDs never repeat in a session.
    pub(crate) request_id: String,
}

pub(crate) struct SessionState {
    pub(crate) lifecycle: LspLifecycleState,
    pub(crate) client: LspClientState,
    pub(crate) pending: BTreeMap<String, PendingRequest>,
    pub(crate) request_by_operation: BTreeMap<String, String>,
    /// Exact function invocation owned by each active host operation. Every
    /// terminal path removes its entry before that host operation ID can be reused.
    pub(crate) function_by_operation: BTreeMap<String, PendingFunctionCall>,
    /// Symbolic-link facts about the open documents, rebuilt lazily from
    /// `client.open_documents`.
    pub(crate) linked_document_paths: LinkedDocumentPaths,
    /// Latest workspace event per URI observed before protocol initialization.
    /// The engine owns and drains this queue exactly once after `initialized`.
    pub(crate) pending_workspace_file_changes: BTreeMap<String, WorkspaceFileChangeKind>,
    pub(crate) events: VecDeque<LspRuntimeEvent>,
    pub(crate) next_sequence: u64,
    pub(crate) initialize_deadline: Option<Instant>,
    pub(crate) shutdown_deadline: Option<Instant>,
    pub(crate) request_timeout: Duration,
    pub(crate) shutdown_timeout: Duration,
    pub(crate) terminal_event_emitted: bool,
    /// Deadline cancellations awaiting the outbound worker, never written by the monitor.
    pub(crate) deadline_cancellations: Vec<String>,
    /// Bounds an outbound maintenance write independently of caller deadlines.
    pub(crate) maintenance_write_deadline: Option<Instant>,
    /// Policy and waiting requests of a plugin-owned language server; `None`
    /// for every other session.
    pub(crate) plugin_server: Option<PluginServerState>,
    /// Bounds one write the function relay's lane makes to this session, so a
    /// backend that stops reading its stdin cannot hold the lane forever.
    pub(crate) relay_write_deadline: Option<Instant>,
    /// Backend requests for a host service awaiting `lsp.hostResponse`, by
    /// the opaque ID the host sees, with the backend's own JSON-RPC ID.
    pub(crate) host_requests: BTreeMap<String, Value>,
    pub(crate) next_host_request_id: u64,
    /// Trace is opt-in and serialized with protocol writes through outbound_order.
    pub(crate) detailed_diagnostics: bool,
    /// Desired trace setting retained until the initialize handshake permits notifications.
    pub(crate) requested_diagnostics: bool,
    /// Malformed plugin diagnostics are summarized once per process session.
    pub(crate) rejected_plugin_diagnostics: bool,
    /// Sender-side ingress budget uses the existing process clock, never a timer.
    pub(crate) plugin_diagnostics_window: Instant,
    pub(crate) plugin_diagnostics_count: u16,
}

pub(crate) struct RuntimeSession {
    /// Declarative plugin start attempt shared with `start_server`.
    pub(crate) startup_attempt: AttemptSlot,
    pub(crate) id: String,
    pub(crate) provider_id: String,
    /// Plugin that owns the process, if any; see `StartServerRequest::plugin_id`.
    pub(crate) plugin_id: Option<String>,
    pub(crate) log_identity: SessionLogIdentity,
    /// Fixed workspace host, inherited by protocol workers; never a global selection.
    pub(crate) workspace_host: Option<lithe_workspace::Host>,
    /// Plugin-declared settings answered to `workspace/configuration`.
    /// Replaced as a whole by `lsp.updateWorkspaceConfiguration`; never held
    /// across another lock.
    pub(crate) plugin_configuration: Mutex<Option<Arc<Value>>>,
    /// Workspace URI the server was initialized with; scopes indexing
    /// exclusions and other workspace-wide checks to this session's root.
    pub(crate) root_uri: String,
    /// Serializes policy eligibility with document/file-event publication and
    /// policy withdrawal. Acquire before outbound_order; Git scans stay outside
    /// the outbound and registry locks.
    pub(crate) indexing_roots: Mutex<Vec<String>>,
    pub(crate) indexing_revision: std::sync::atomic::AtomicU64,
    /// Serializes protocol-state commits with complete outbound message batches.
    /// Callers acquire this before `state` whenever an action can write to the
    /// server, preserving the same order in memory and on the wire.
    pub(crate) outbound_order: Mutex<()>,
    pub(crate) state: Mutex<SessionState>,
    pub(crate) event_signal: Condvar,
    pub(crate) process: Arc<dyn LspProcessHandle>,
    pub(crate) active: AtomicBool,
    /// The engine's cross-plugin function relay; see `engine_function_relay`.
    pub(crate) relay: Arc<RelayHub>,
}

/// The engine is a process-owning singleton in production, but it holds no
/// global state of its own beyond the session registry, so tests construct
/// private instances with a scripted launcher.
pub(crate) struct LspEngine {
    pub(crate) next_session_id: AtomicU64,
    pub(crate) next_operation_id: AtomicU64,
    /// Shared with the relay's worker, which reaches sessions by ID.
    pub(crate) sessions: Arc<Mutex<BTreeMap<String, Arc<RuntimeSession>>>>,
    /// Cross-plugin function calls; every session of this engine uses it.
    pub(crate) relay: Arc<RelayHub>,
    /// Failed wire starts have no returned identity. Retain their cleanup
    /// responsibility here and retry disposal before another process starts.
    pub(crate) failed_starts: Mutex<BTreeSet<String>>,
    pub(crate) launcher: Arc<dyn LspProcessLauncher>,
}
