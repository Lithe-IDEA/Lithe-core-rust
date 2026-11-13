//! Stateful language-server sessions coordinating client state and child processes.

use lithe_lsp_client::{
    client_apply_server_message_for_plugin, client_initialize_for_plugin, json_rpc_error_with_code,
    json_rpc_error_with_data, json_rpc_notification, json_rpc_result,
};
mod logging;
mod scan_settings;

use lithe_diagnostics::plugin_operations::{
    self as plugin_operations, Attempt, AttemptHandoff, AttemptSlot, Stage as AttemptStage,
    LANGUAGE_SERVER_START,
};
use lithe_lsp_client::{
    client_change_document, client_close_document, client_feature_request, client_open_document,
    client_shutdown, ClientChangeDocumentRequest, ClientCloseDocumentRequest, ClientFeatureRequest,
    ClientInitializeRequest, ClientOpenDocumentRequest, ClientShutdownRequest, LspClientDiagnostic,
    LspClientDocument, LspClientState, LspDocumentContentChange, LspPosition, LspRange,
};
use lithe_lsp_transport::framing::{
    frame_message, parse_server_messages, FrameMessageRequest, ParseServerMessagesRequest,
};
use lithe_lsp_transport::process::{
    LspProcessHandle, LspProcessLauncher, LspProcessSpec, SystemProcessLauncher,
};
use lithe_plugin_language_servers::{
    RemoteSupport, DEFAULT_REQUEST_TIMEOUT_MILLISECONDS, DID_CHANGE_CONFIGURATION,
    MAX_REQUEST_TIMEOUT_MILLISECONDS,
};
use lithe_plugin_server_policy::{self as plugin_server, Admission, Arrival, PluginServerState};
use lithe_protocol::{CoreError, ErrorCode};
pub use logging::{resolve_engine_log_subject, ResolveEngineLogSubjectRequest};
use logging::{
    EngineLog, EngineLogInput, SessionLogIdentity, SessionRole, StderrFact, StderrLines,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_INITIALIZE_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_SHUTDOWN_TIMEOUT_MS: u64 = 2_000;
/// Upper bound of one build-provider request. A model request can load a
/// large build, so it may outlive the ordinary request timeout, but never
/// stays pending without a deadline.
const MAX_BUILD_PROVIDER_TIMEOUT_MS: u64 = 60 * 60_000;
/// Upper bound of one VCS-provider request. An update or check-in of a large
/// working copy over a slow link can take long, but never stays pending
/// without a deadline.
const MAX_VCS_PROVIDER_TIMEOUT_MS: u64 = 60 * 60_000;
/// Longest toolchain query a host may ask for; queries only read manager state.
const MAX_TOOLCHAIN_QUERY_TIMEOUT_MS: u64 = 60_000;
/// Bounds one provider request; the host can cancel sooner when a snapshot expires.
const MAX_KEYMAP_IMPORT_TIMEOUT_MS: u64 = 60_000;
const MONITOR_INTERVAL_MS: u64 = 10;
/// Shared plugin diagnostic budget per process session and monotonic minute.
const MAX_PLUGIN_DIAGNOSTICS_PER_MINUTE: u16 = 600;

static ENGINE: OnceLock<LspEngine> = OnceLock::new();

mod relay_hub;
use relay_hub::RelayHub;
mod function_provider;
pub use function_provider::function_provider_command;

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
/// Observable lifecycle of one managed language-server session.
pub enum LspLifecycleState {
    /// Session state exists, but the child process has not started.
    Created,
    /// The child process and its standard streams are being created.
    ProcessStarting,
    /// The process is running and the initialize handshake is pending.
    Initializing,
    /// Initialization completed and semantic requests may be sent.
    Ready,
    /// A graceful shutdown is pending or the process is being terminated.
    Stopping,
    /// The session ended normally and will produce no further events.
    Stopped,
    /// Startup, protocol handling, or the child process failed terminally.
    Failed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Validated process, workspace, initialization, and timeout settings for a server.
pub struct StartServerRequest {
    pub(crate) provider_id: String,
    /// Process role supplied by the host; old hosts retain language-server routing.
    #[serde(default)]
    pub role: SessionRole,
    /// Stable VCS identity supplied explicitly by the provider catalog, never parsed from providerID.
    #[serde(default)]
    pub log_subject: Option<String>,
    /// Plugin that owns the process, for plugin-provided servers and build
    /// providers. Only such a session can serve that plugin's functions;
    /// host-owned servers leave it out.
    #[serde(default)]
    pub plugin_id: Option<String>,
    /// Declarative `languageServer/start` attempt the host began; Core records its
    /// result. Absent for v1 hosts and SDK-owned backend processes.
    #[serde(default)]
    pub startup_operation: Option<AttemptHandoff>,
    /// Host services the plugin's permissions grant the session, as the
    /// launch plan lists them (`backend.hostServices`). Only a plugin-owned
    /// session may name any; Core forwards only these `lithe/host/*`
    /// requests to the host.
    #[serde(default)]
    pub host_services: Vec<String>,
    /// Options of the granted host services, such as the icons the host
    /// draws for editor actions. Only a plugin-owned session may name any.
    #[serde(default)]
    pub host_options: lithe_lsp_client::HostOptions,
    /// Provider IDs of the plugin's verified manifest. Only a plugin-owned
    /// session with some is offered `litheToolchains`.
    #[serde(default)]
    pub toolchain_providers: Vec<String>,
    /// Shortcut import providers declared by this plugin's verified manifest.
    #[serde(default)]
    pub keymap_import_providers: Vec<String>,
    /// What the plugin backend may do with this plugin-owned language server
    /// (plan API-25): the requests it may send, the gates they wait for, and
    /// the notifications it receives. Only a plugin-owned session has one;
    /// without it the backend cannot reach the server through the host.
    #[serde(default)]
    pub session_policy: Option<lithe_plugin_server_policy::SessionPolicy>,
    pub(crate) executable_path: String,
    #[serde(default)]
    pub(crate) arguments: Vec<String>,
    #[serde(default)]
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) root_uri: String,
    pub(crate) working_directory: String,
    #[serde(default)]
    pub(crate) initialization_options: Option<Value>,
    /// Settings a plugin-provided server reads through
    /// `workspace/configuration`, already expanded from the plugin's
    /// `workspaceConfiguration` template. Must be an object when present.
    #[serde(default)]
    pub workspace_configuration: Option<Value>,
    #[serde(default = "default_initialize_timeout")]
    pub(crate) initialize_timeout_milliseconds: u64,
    #[serde(default = "default_request_timeout")]
    pub(crate) request_timeout_milliseconds: u64,
    #[serde(default = "default_shutdown_timeout")]
    pub(crate) shutdown_timeout_milliseconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Identity and initial lifecycle state of a newly created session.
pub struct StartServerResponse {
    /// Catalog-derived channel for host diagnostic I/O; never derived from provider ID by hosts.
    pub channel: String,
    pub(crate) session_id: String,
    pub(crate) state: LspLifecycleState,
    pub(crate) process_id: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request targeting one existing server session.
pub struct SessionRequest {
    pub(crate) session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Complete document contents or incremental edits to open or update in a session.
pub struct SyncDocumentRequest {
    pub(crate) session_id: String,
    pub(crate) uri: String,
    pub(crate) language_id: String,
    /// Full document text. May be omitted when `content_changes` extends a
    /// document Core already holds.
    #[serde(default)]
    pub text: String,
    /// Edits in the order they happened, each range relative to the previous result.
    #[serde(default)]
    pub content_changes: Vec<LspDocumentContentChange>,
}

/// A host-confirmed disk save, carrying the exact saved text rather than newer edits.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveDocumentRequest {
    pub(crate) session_id: String,
    pub(crate) uri: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Core-owned document synchronization state returned after an editor update.
pub struct SyncDocumentResponse {
    /// Monotonic version used by all subsequent LSP requests and diagnostics.
    pub document_version: i64,
    /// `false` when the submitted text was already synchronized.
    pub changed: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Workspace file events forwarded through standard LSP watched-file semantics.
pub struct WorkspaceFilesChangedRequest {
    pub(crate) session_id: String,
    pub(crate) changes: Vec<WorkspaceFileChange>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One absolute document URI and the filesystem transition observed by a host.
pub struct WorkspaceFileChange {
    pub(crate) uri: String,
    pub(crate) kind: WorkspaceFileChangeKind,
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
/// LSP watched-file transition mapped to protocol event types 1, 2, and 3.
pub enum WorkspaceFileChangeKind {
    /// The file was created (event type 1).
    Created,
    /// The file was changed (event type 2).
    Changed,
    /// The file was deleted (event type 3).
    Deleted,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to remove a document from a session's synchronized state.
pub struct CloseDocumentRequest {
    pub(crate) session_id: String,
    pub(crate) uri: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
/// Semantic operation normalized across provider-specific LSP capabilities.
pub enum LspSemanticOperation {
    /// `textDocument/completion`.
    Completion,
    /// `textDocument/hover`.
    Hover,
    /// `textDocument/definition`.
    Definition,
    /// `textDocument/declaration`.
    Declaration,
    /// `textDocument/typeDefinition`.
    TypeDefinition,
    /// `textDocument/references`.
    References,
    /// `textDocument/implementation`.
    Implementation,
    /// `textDocument/rename`.
    Rename,
    /// `textDocument/formatting`.
    Formatting,
    /// `textDocument/rangeFormatting` over the request's `range`.
    RangeFormatting,
    /// `textDocument/documentSymbol`: the document outline.
    DocumentSymbols,
    /// `textDocument/codeAction`.
    CodeActions,
    /// `completionItem/resolve` for a previously returned completion item.
    ResolveCompletion,
    /// `codeAction/resolve` for a previously returned action.
    ResolveCodeAction,
    /// `workspace/executeCommand` using a server-provided command payload.
    ExecuteCommand,
    /// `textDocument/inlayHint`.
    InlayHints,
    /// `textDocument/foldingRange`.
    FoldingRanges,
    /// `textDocument/documentLink`: clickable ranges such as URLs and file references.
    DocumentLinks,
    /// `workspace/symbol`: workspace-wide symbol search for `query`; names no document.
    WorkspaceSymbols,
    /// Full `textDocument/semanticTokens/full` with its negotiated legend.
    SemanticTokens,
    /// `textDocument/codeLens`.
    CodeLens,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Inputs for one asynchronous semantic language-server operation.
pub struct SemanticRequest {
    pub(crate) session_id: String,
    #[serde(default)]
    pub(crate) operation_id: Option<String>,
    pub(crate) operation: LspSemanticOperation,
    #[serde(default)]
    pub(crate) uri: Option<String>,
    #[serde(default)]
    pub(crate) position: Option<LspPosition>,
    #[serde(default)]
    pub(crate) new_name: Option<String>,
    /// Search text for `workspaceSymbols`; absent means the empty query.
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub(crate) range: Option<LspRange>,
    #[serde(default)]
    pub(crate) diagnostics: Vec<LspClientDiagnostic>,
    /// LSP completion trigger context; validated by the shared request mapper.
    #[serde(default)]
    pub completion_context: Option<Value>,
    #[serde(default)]
    pub(crate) completion_item: Option<Value>,
    #[serde(default)]
    pub(crate) code_action: Option<Value>,
    #[serde(default)]
    pub(crate) command: Option<Value>,
    /// Optional LSP `FormattingOptions` (`tabSize`, `insertSpaces`, ...) for
    /// `formatting` and `rangeFormatting`; unset fields keep Core defaults.
    #[serde(default)]
    pub formatting_options: Option<Value>,
    /// For `references`: whether the answer lists the declaration itself
    /// (LSP `ReferenceContext.includeDeclaration`). Unset means `true`; the
    /// usages shown from a declaration (`lsp.declarationOrUsages`) pass
    /// `false` so the language server leaves the declaration out.
    #[serde(default)]
    pub include_declaration: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One build-provider request, forwarded as `build.modelPlan` or
/// `build.dependencyPlan` planned it. The result arrives as a
/// `requestCompleted` event whose `result.value` is the provider's reply,
/// which the host passes to Core's decoder unchanged.
pub struct BuildProviderRequest {
    pub(crate) session_id: String,
    #[serde(default)]
    pub(crate) operation_id: Option<String>,
    /// `lithe/build/model` or `lithe/build/dependencies`.
    pub method: String,
    /// The plan's `provider.params`, sent as is.
    pub params: Value,
    /// Deadline of this request, in milliseconds, capped at
    /// `MAX_BUILD_PROVIDER_TIMEOUT_MS`; the session's request timeout when absent.
    #[serde(default)]
    pub timeout_milliseconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One `toolchains/query`, addressed to a session that negotiated the provider
/// named in `params`. The result arrives as a `requestCompleted` event whose
/// `result.value` is `{ id, response }`, the complete untrusted JSON-RPC response,
/// which the host passes to `toolchains.decodeReply`.
pub struct ToolchainProviderRequest {
    pub(crate) session_id: String,
    #[serde(default)]
    pub(crate) operation_id: Option<String>,
    /// Query parameters; `toolchains.decodeReply` validates them with the reply.
    pub params: Value,
    /// Deadline of this request, capped at `MAX_TOOLCHAIN_QUERY_TIMEOUT_MS`.
    #[serde(default)]
    pub timeout_milliseconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One shortcut import operation against an opaque, host-owned snapshot.
pub struct KeymapImportProviderRequest {
    /// Plugin-owned session which negotiated the provider.
    pub session_id: String,
    /// Optional correlation identity for cancellation and completion events.
    #[serde(default)]
    pub operation_id: Option<String>,
    /// Plugin-local identity of the selected provider.
    #[serde(rename = "providerID")]
    pub provider_id: String,
    /// Host-owned snapshot reference, scoped to this import operation.
    #[serde(rename = "snapshotID")]
    pub snapshot_id: String,
    /// Requested deadline in milliseconds, bounded by the runtime.
    #[serde(default)]
    pub timeout_milliseconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One VCS-provider request. Core plans the method and params from the
/// operation and its arguments, so the host never writes a raw method. The
/// result arrives as a `requestCompleted` event whose `result.value` is the
/// provider's reply, which the host passes to `vcs.decodeReply`; a failure
/// carries a `vcs.*` code.
pub struct VcsProviderRequest {
    pub(crate) session_id: String,
    #[serde(default)]
    pub(crate) operation_id: Option<String>,
    /// The VCS the host launched the session for; a session that negotiated
    /// another VCS is refused.
    pub vcs: String,
    /// Operation name, such as `status` or `checkin`.
    pub operation: String,
    /// Native absolute path of the working-copy root.
    pub root: String,
    /// Absolute path of the VCS client the host resolved, when the provider
    /// declares one.
    #[serde(default)]
    pub tool: Option<String>,
    /// Operation arguments; see `crate::vcs` for each operation's shape.
    #[serde(default)]
    pub arguments: Value,
    /// Deadline of this request, in milliseconds, capped at
    /// `MAX_VCS_PROVIDER_TIMEOUT_MS`; the session's request timeout when absent.
    #[serde(default)]
    pub timeout_milliseconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// The host's answer to one `hostRequest` event: the outcome of a host
/// service a plugin backend asked for. Exactly one of `result` and `error`.
pub struct PluginHostResponse {
    pub(crate) session_id: String,
    /// The `hostRequestID` of the `hostRequest` event being answered,
    /// spelled as the event spells it.
    #[serde(rename = "hostRequestID")]
    pub host_request_id: String,
    /// The service's result; `null` for a write or delete.
    #[serde(default)]
    pub result: Option<Value>,
    /// The host could not perform the service. Its message reaches the
    /// backend, bounded. Its code passes only when it is one a host may
    /// report (`plugin_function::host_failure`); otherwise it is
    /// `plugin.host.failed`.
    #[serde(default)]
    pub error: Option<PluginHostFailure>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Why the host could not perform a forwarded host-service request.
pub struct PluginHostFailure {
    pub(crate) message: String,
    /// Stable `plugin.host.*` code of the failure.
    #[serde(default)]
    pub code: Option<String>,
    /// Structured facts a backend may act on, such as the current version
    /// of a `plugin.host.versionConflict`. It reaches the backend as
    /// `error.data.details` when it is an object within
    /// `plugin_function::MAX_HOST_DETAILS_BYTES`; otherwise it is dropped and
    /// the code and message still arrive.
    #[serde(default)]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One notification from the host to a plugin backend, such as a
/// subscription event or a changed set of workspace contexts. Core admits
/// only the methods of `plugin_function::HOST_NOTIFICATIONS`.
pub struct PluginHostNotification {
    pub(crate) session_id: String,
    pub(crate) method: String,
    pub(crate) params: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Whether a host notification was written to the backend. `false` means
/// the backend negotiated no plugin-function protocol version.
pub struct PluginHostNotificationResponse {
    pub(crate) delivered: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One plugin-function invocation. The result arrives as a `requestCompleted`
/// event whose `result.value` is the function's result; failures carry a
/// stable `plugin.function.*` code. Progress arrives as `requestProgress`.
pub struct PluginFunctionRequest {
    pub(crate) session_id: String,
    #[serde(default)]
    pub(crate) operation_id: Option<String>,
    pub(crate) function: lithe_plugin_rpc::FunctionReference,
    /// Serializable function input; must be a JSON object.
    #[serde(default = "empty_object")]
    pub params: Value,
    /// Deadline of this call, in milliseconds, capped at one hour; the
    /// session's request timeout when absent.
    #[serde(default)]
    pub timeout_milliseconds: Option<u64>,
    /// Workspace context `{ contextID, generation }` the call belongs to.
    /// It reaches the backend as `params.context`.
    #[serde(default)]
    pub context: Option<Value>,
}

fn empty_object() -> Value {
    json!({})
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One request a plugin backend sent to its own language server through the
/// host (plan API-25). The reply arrives as a `requestCompleted` event whose
/// `result.value` is the server's result; failures carry a `plugin.host.*`
/// code as `error.code`.
pub struct PluginServerRequest {
    /// The plugin-owned language-server session the host resolved.
    pub session_id: String,
    #[serde(default)]
    pub(crate) operation_id: Option<String>,
    /// The plugin and server the backend named. Core checks that the
    /// session really is that contribution, so a stale or mistaken
    /// resolution can never reach another plugin's server.
    pub plugin_id: String,
    #[serde(rename = "serverID")]
    pub(crate) server_id: String,
    /// An LSP request method the plugin declared in `requestMethods`.
    pub method: String,
    #[serde(default = "empty_object")]
    pub(crate) params: Value,
    /// Deadline in milliseconds, 60000 when absent and at most 300000.
    #[serde(default)]
    pub timeout_milliseconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One notification a plugin backend sent to its own language server.
pub struct PluginServerNotify {
    pub(crate) session_id: String,
    pub(crate) plugin_id: String,
    #[serde(rename = "serverID")]
    pub(crate) server_id: String,
    pub(crate) method: String,
    #[serde(default = "empty_object")]
    pub(crate) params: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Whether the notification reached the server. `false` means the server
/// was not initialized yet and will receive the stored configuration with
/// its `initialized` notification instead.
pub struct PluginServerNotifyResponse {
    pub(crate) delivered: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Correlation identifier used to receive or cancel an asynchronous result.
pub struct OperationResponse {
    pub(crate) operation_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to cancel a pending operation in one session.
pub struct CancelOperationRequest {
    pub(crate) session_id: String,
    pub(crate) operation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Ordered events drained from a session since the previous poll.
pub struct PollEventsResponse {
    pub(crate) events: Vec<LspRuntimeEvent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request that waits until queued events exist or the timeout elapses.
pub struct WaitEventsRequest {
    pub(crate) session_id: String,
    /// Upper bound for blocking on the session event channel, in milliseconds.
    #[serde(default = "default_wait_events_timeout")]
    pub timeout_milliseconds: u64,
}

fn default_wait_events_timeout() -> u64 {
    30_000
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Sequenced lifecycle, diagnostic, result, or log event from a server session.
pub struct LspRuntimeEvent {
    /// Event discriminator such as `stateChanged`, `requestCompleted`, or `diagnostics`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Core-derived catalog channel, present for structured log facts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Plugin that owns this process; omitted for host-owned sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// Explicit process role, independent of the provider's opaque ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<SessionRole>,
    /// Stable catalog identity; existing v1 fields remain available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// Catalog disk fields, validated and redacted by Core.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<BTreeMap<String, Value>>,
    /// Canonical catalog severity for v2; independent of the preserved v1 level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_level: Option<lithe_diagnostics::record::LogLevel>,
    pub(crate) sequence: u64,
    pub(crate) provider_id: String,
    pub(crate) session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<LspLifecycleState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostics: Option<Vec<LspClientDiagnostic>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<LspRuntimeError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) capabilities: Option<Vec<String>>,
    /// On `featuresChanged` while `completion` is offered: the session's
    /// `{ triggerCharacters, allCommitCharacters }`, from `completionProvider`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_options: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) server_info: Option<LspServerInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Server identity reported by the LSP initialize response.
pub struct LspServerInfo {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Structured runtime failure with session and protocol-stage context.
pub struct LspRuntimeError {
    pub(crate) code: String,
    pub(crate) provider_id: String,
    pub(crate) session_id: String,
    /// Lifecycle or protocol phase in which the error occurred.
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) document_uri: Option<String>,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) underlying_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) process_exit_code: Option<i32>,
    /// Plugin-defined code of a failed plugin function (`error.data.code`),
    /// passed through for the plugin's own UI; Lithe never branches on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_error_code: Option<String>,
    /// Structured facts of a failed plugin-server request, such as the
    /// server's own error `code` and `message`, passed to the backend as the
    /// failure's details. Only `lsp.pluginServerRequest` sets it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

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
enum PendingKind {
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
struct PendingRequest {
    kind: PendingKind,
    operation_id: Option<String>,
    method: String,
    document_uri: Option<String>,
    /// Document version observed when a presentation request was allocated.
    document_version: Option<i64>,
    created_at: Instant,
    deadline: Instant,
}

/// Links one host operation to the exact backend request and function it owns.
struct PendingFunctionCall {
    function_id: String,
    /// Also serves as the opaque wire operation ID; request IDs never repeat in a session.
    request_id: String,
}

struct SessionState {
    lifecycle: LspLifecycleState,
    client: LspClientState,
    pending: BTreeMap<String, PendingRequest>,
    request_by_operation: BTreeMap<String, String>,
    /// Exact function invocation owned by each active host operation. Every
    /// terminal path removes its entry before that host operation ID can be reused.
    function_by_operation: BTreeMap<String, PendingFunctionCall>,
    /// Symbolic-link facts about the open documents, rebuilt lazily from
    /// `client.open_documents`.
    linked_document_paths: LinkedDocumentPaths,
    /// Latest workspace event per URI observed before protocol initialization.
    /// The engine owns and drains this queue exactly once after `initialized`.
    pending_workspace_file_changes: BTreeMap<String, WorkspaceFileChangeKind>,
    events: VecDeque<LspRuntimeEvent>,
    next_sequence: u64,
    initialize_deadline: Option<Instant>,
    shutdown_deadline: Option<Instant>,
    request_timeout: Duration,
    shutdown_timeout: Duration,
    terminal_event_emitted: bool,
    /// Deadline cancellations awaiting the outbound worker, never written by the monitor.
    deadline_cancellations: Vec<String>,
    /// Bounds an outbound maintenance write independently of caller deadlines.
    maintenance_write_deadline: Option<Instant>,
    /// Policy and waiting requests of a plugin-owned language server; `None`
    /// for every other session.
    plugin_server: Option<PluginServerState>,
    /// Bounds one write the function relay's lane makes to this session, so a
    /// backend that stops reading its stdin cannot hold the lane forever.
    relay_write_deadline: Option<Instant>,
    /// Backend requests for a host service awaiting `lsp.hostResponse`, by
    /// the opaque ID the host sees, with the backend's own JSON-RPC ID.
    host_requests: BTreeMap<String, Value>,
    next_host_request_id: u64,
    /// Trace is opt-in and serialized with protocol writes through outbound_order.
    detailed_diagnostics: bool,
    /// Desired trace setting retained until the initialize handshake permits notifications.
    requested_diagnostics: bool,
    /// Malformed plugin diagnostics are summarized once per process session.
    rejected_plugin_diagnostics: bool,
    /// Sender-side ingress budget uses the existing process clock, never a timer.
    plugin_diagnostics_window: Instant,
    plugin_diagnostics_count: u16,
}

struct RuntimeSession {
    /// Declarative plugin start attempt shared with `start_server`.
    startup_attempt: AttemptSlot,
    id: String,
    provider_id: String,
    /// Plugin that owns the process, if any; see `StartServerRequest::plugin_id`.
    plugin_id: Option<String>,
    log_identity: SessionLogIdentity,
    /// Fixed workspace host, inherited by protocol workers; never a global selection.
    workspace_host: Option<lithe_workspace::Host>,
    /// Plugin-declared settings answered to `workspace/configuration`.
    /// Replaced as a whole by `lsp.updateWorkspaceConfiguration`; never held
    /// across another lock.
    plugin_configuration: Mutex<Option<Arc<Value>>>,
    /// Workspace URI the server was initialized with; scopes indexing
    /// exclusions and other workspace-wide checks to this session's root.
    root_uri: String,
    /// Serializes policy eligibility with document/file-event publication and
    /// policy withdrawal. Acquire before outbound_order; Git scans stay outside
    /// the outbound and registry locks.
    indexing_roots: Mutex<Vec<String>>,
    indexing_revision: std::sync::atomic::AtomicU64,
    /// Serializes protocol-state commits with complete outbound message batches.
    /// Callers acquire this before `state` whenever an action can write to the
    /// server, preserving the same order in memory and on the wire.
    outbound_order: Mutex<()>,
    state: Mutex<SessionState>,
    event_signal: Condvar,
    process: Arc<dyn LspProcessHandle>,
    active: AtomicBool,
    /// The engine's cross-plugin function relay; see `engine_function_relay`.
    relay: Arc<RelayHub>,
}

/// The engine is a process-owning singleton in production, but it holds no
/// global state of its own beyond the session registry, so tests construct
/// private instances with a scripted launcher.
pub(crate) struct LspEngine {
    next_session_id: AtomicU64,
    next_operation_id: AtomicU64,
    /// Shared with the relay's worker, which reaches sessions by ID.
    sessions: Arc<Mutex<BTreeMap<String, Arc<RuntimeSession>>>>,
    /// Cross-plugin function calls; every session of this engine uses it.
    relay: Arc<RelayHub>,
    /// Failed wire starts have no returned identity. Retain their cleanup
    /// responsibility here and retry disposal before another process starts.
    failed_starts: Mutex<BTreeSet<String>>,
    launcher: Arc<dyn LspProcessLauncher>,
}

fn default_initialize_timeout() -> u64 {
    DEFAULT_INITIALIZE_TIMEOUT_MS
}

fn default_request_timeout() -> u64 {
    DEFAULT_REQUEST_TIMEOUT_MS
}

fn default_shutdown_timeout() -> u64 {
    DEFAULT_SHUTDOWN_TIMEOUT_MS
}

/// Rejects plugin configuration that is not a JSON object, the only shape
/// `workspace/configuration` sections can be looked up in.
fn checked_workspace_configuration(
    configuration: Option<Value>,
) -> Result<Option<Value>, CoreError> {
    match configuration {
        None | Some(Value::Null) => Ok(None),
        Some(configuration @ Value::Object(_)) => Ok(Some(configuration)),
        Some(_) => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "workspaceConfiguration must be a JSON object.",
        )),
    }
}

/// Answers one `workspace/configuration` item from plugin configuration.
///
/// A section names a dotted path (`python.analysis`); an exact top-level key
/// wins over the path so servers that use dotted keys still match. A missing
/// section answers `null`, which tells the server to use its own default.
fn configuration_section(configuration: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|section| !section.is_empty()) else {
        return configuration.clone();
    };
    if let Some(value) = configuration.get(section) {
        return value.clone();
    }
    section
        .split('.')
        .try_fold(configuration, |value, key| value.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `workspace/didChangeConfiguration` carrying the whole plugin configuration.
fn plugin_configuration_notification(configuration: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "workspace/didChangeConfiguration",
        "params": { "settings": configuration }
    })
    .to_string()
}

/// Replaces the plugin configuration of a running plugin-provided session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWorkspaceConfigurationRequest {
    pub(crate) session_id: String,
    pub(crate) configuration: Value,
}

/// Whether the change reached the server now or waits for `initialized`.
#[derive(Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkspaceConfigurationResponse {
    pub(crate) sent: bool,
}

/// Applies a plugin setting with `effect: immediate` without a restart.
///
/// The stored configuration answers later `workspace/configuration` requests;
/// an initialized server is also told through `workspace/didChangeConfiguration`
/// so servers that pull configuration only on that signal refresh too.
pub fn update_workspace_configuration(
    request: UpdateWorkspaceConfigurationRequest,
) -> Result<UpdateWorkspaceConfigurationResponse, CoreError> {
    let configuration =
        checked_workspace_configuration(Some(request.configuration))?.unwrap_or_else(|| json!({}));
    engine()
        .session(&request.session_id)?
        .update_plugin_configuration(configuration)
}

/// Withdraw excluded working copies and update supported provider import configuration.
pub fn indexing_changed(root: &Path) -> Result<(), CoreError> {
    let sessions = engine()
        .sessions
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "LSP registry lock was poisoned"))?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    apply_indexing_to_sessions(root, sessions)
}

fn apply_indexing_to_sessions(
    root: &Path,
    sessions: Vec<Arc<RuntimeSession>>,
) -> Result<(), CoreError> {
    let root_uri = url::Url::from_directory_path(root).ok();
    let canonical_root = root_uri
        .as_ref()
        .and_then(|uri| canonical_workspace_root(uri.as_str()));
    let sessions = sessions
        .into_iter()
        .filter(|session| {
            canonical_workspace_root(&session.root_uri) == canonical_root
                && session.active.load(Ordering::Acquire)
        })
        .collect::<Vec<_>>();
    if sessions.is_empty() {
        return Ok(());
    }
    let prepared = lithe_project::indexing::prepare_provider_exclusions(root)?;
    let roots = lithe_project::indexing::excluded_roots(root);
    let revision = prepared.as_ref().map_or(0, |policy| policy.revision);
    if let Some(policy) = &prepared {
        policy.check()?;
    }
    for session in sessions {
        if let Some(policy) = &prepared {
            policy.check()?;
        }
        if ensure_not_terminal(session.lock_state()?.lifecycle).is_err() {
            continue;
        }
        if canonical_workspace_root(&session.root_uri) != canonical_root {
            continue;
        }
        let mut previous = session.lock_indexing_roots()?;
        if *previous == roots && session.indexing_revision.load(Ordering::Acquire) == revision {
            continue;
        }
        let documents = session
            .lock_state()?
            .client
            .open_documents
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for uri in documents {
            if lithe_project::indexing::uri_is_excluded(&session.root_uri, &uri)? {
                session.close_document(&uri)?;
            }
        }
        let diagnostic_uris = session
            .lock_state()?
            .client
            .diagnostics
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for uri in diagnostic_uris {
            if lithe_project::indexing::uri_is_excluded(&session.root_uri, &uri)? {
                let mut state = session.lock_state()?;
                state.client.diagnostics.remove(&uri);
                state.client.diagnostic_versions.remove(&uri);
                push_diagnostics_event(&session, &mut state, &uri, None, Vec::new());
            }
        }
        let pending_uris = session
            .lock_state()?
            .pending_workspace_file_changes
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for uri in pending_uris {
            if lithe_project::indexing::uri_is_excluded(&session.root_uri, &uri)? {
                session
                    .lock_state()?
                    .pending_workspace_file_changes
                    .remove(&uri);
            }
        }
        // Exclusions are folded into the configuration answered through
        // `workspace/configuration`, so an initialized server is told to pull
        // the changed sections again instead of keeping its old import scope.
        {
            let order = session.lock_outbound_order()?;
            if session.lock_state()?.client.initialized {
                if let Some(configuration) = session.plugin_configuration() {
                    session.send_messages_or_fail(
                        &order,
                        vec![plugin_configuration_notification(&configuration)],
                        "indexingChanged",
                    )?;
                }
            }
        }
        *previous = roots.clone();
        session.indexing_revision.store(revision, Ordering::Release);
    }
    Ok(())
}

/// Starts a managed language-server process and begins LSP initialization.
pub fn start_server(request: StartServerRequest) -> Result<StartServerResponse, CoreError> {
    engine().start_server(request)
}

/// Native host ownership hook; it does not add an application command.
pub fn start_server_owned(
    request: StartServerRequest,
    registered: impl FnOnce(&str),
) -> Result<StartServerResponse, CoreError> {
    engine().start_server_owned(request, registered)
}

/// Performs a bounded graceful shutdown while retaining the session record.
pub fn stop_server(request: SessionRequest) -> Result<(), CoreError> {
    engine().session(&request.session_id)?.stop()
}

impl RuntimeSession {
    /// Plugin configuration the server currently sees, if any.
    fn plugin_configuration(&self) -> Option<Arc<Value>> {
        let base = self
            .plugin_configuration
            .lock()
            .map(|configuration| configuration.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        crate::scan_settings::configuration(&self.provider_id, &self.root_uri, base.as_deref())
            .map(Arc::new)
    }

    fn update_plugin_configuration(
        &self,
        configuration: Value,
    ) -> Result<UpdateWorkspaceConfigurationResponse, CoreError> {
        if self.plugin_id.is_none() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Only plugin-provided language servers accept plugin configuration.",
            ));
        }
        let configuration = Arc::new(configuration);
        let outbound_order = self.lock_outbound_order()?;
        let initialized = {
            let state = self.lock_state()?;
            if matches!(
                state.lifecycle,
                LspLifecycleState::Stopping
                    | LspLifecycleState::Stopped
                    | LspLifecycleState::Failed
            ) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language session is no longer running.",
                ));
            }
            // Under `outbound_order`, an uninitialized client sends the stored
            // configuration with its `initialized` notification instead.
            state.client.initialized
        };
        *self
            .plugin_configuration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(configuration.clone());
        let messages = if initialized {
            vec![plugin_configuration_notification(
                self.plugin_configuration()
                    .as_deref()
                    .unwrap_or(&configuration),
            )]
        } else {
            Vec::new()
        };
        self.send_messages_or_fail(&outbound_order, messages, "workspaceConfigurationUpdate")?;
        Ok(UpdateWorkspaceConfigurationResponse { sent: initialized })
    }
}

/// Opens or replaces the synchronized contents of one document.
pub fn sync_document(request: SyncDocumentRequest) -> Result<SyncDocumentResponse, CoreError> {
    engine()
        .session(&request.session_id)?
        .sync_document(request)
}

/// Notifies an interested server after a successful native save. Returns false
/// for unopened documents, unsupported saves, or a save superseded by new edits.
pub fn save_document(request: SaveDocumentRequest) -> Result<bool, CoreError> {
    engine()
        .session(&request.session_id)?
        .save_document(request)
}

/// Forwards external source and build-configuration changes without restarting
/// the server or invalidating its durable workspace state.
pub fn workspace_files_changed(request: WorkspaceFilesChangedRequest) -> Result<(), CoreError> {
    engine()
        .session(&request.session_id)?
        .workspace_files_changed(request.changes)
}

/// Notifies the server that a synchronized document has closed.
pub fn close_document(request: CloseDocumentRequest) -> Result<(), CoreError> {
    engine()
        .session(&request.session_id)?
        .close_document(&request.uri)
}

/// Queues a semantic request and returns its operation identifier immediately.
pub fn semantic_request(request: SemanticRequest) -> Result<OperationResponse, CoreError> {
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| engine().next_operation_id());
    engine()
        .session(&request.session_id)?
        .request(request, operation_id.clone())?;
    Ok(OperationResponse { operation_id })
}

/// Sends one build-provider request to a session that negotiated the protocol.
pub fn build_provider_request(
    request: BuildProviderRequest,
) -> Result<OperationResponse, CoreError> {
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| engine().next_operation_id());
    engine()
        .session(&request.session_id)?
        .build_request(request, operation_id.clone())?;
    Ok(OperationResponse { operation_id })
}

/// Sends one `toolchains/query` to a session that negotiated its provider.
pub fn toolchain_provider_request(
    request: ToolchainProviderRequest,
) -> Result<OperationResponse, CoreError> {
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| engine().next_operation_id());
    engine()
        .session(&request.session_id)?
        .toolchain_request(request, operation_id.clone())?;
    Ok(OperationResponse { operation_id })
}

/// Sends a shortcut import request only to a negotiated plugin provider.
pub fn keymap_import_provider_request(
    request: KeymapImportProviderRequest,
) -> Result<OperationResponse, CoreError> {
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| engine().next_operation_id());
    engine()
        .session(&request.session_id)?
        .keymap_import_request(request, operation_id.clone())?;
    Ok(OperationResponse { operation_id })
}

/// Sends one VCS-provider request to a session that negotiated the protocol.
pub fn vcs_provider_request(request: VcsProviderRequest) -> Result<OperationResponse, CoreError> {
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| engine().next_operation_id());
    engine()
        .session(&request.session_id)?
        .vcs_request(request, operation_id.clone())?;
    Ok(OperationResponse { operation_id })
}

/// Answers one `hostRequest` event of a plugin backend with the outcome of
/// the host service it asked for.
pub fn plugin_host_response(request: PluginHostResponse) -> Result<(), CoreError> {
    engine()
        .session(&request.session_id)?
        .host_response(request)
}

/// Sends one host notification to a plugin backend.
pub fn plugin_host_notify(
    request: PluginHostNotification,
) -> Result<PluginHostNotificationResponse, CoreError> {
    let delivered = engine()
        .session(&request.session_id)?
        .host_notify(request)?;
    Ok(PluginHostNotificationResponse { delivered })
}

/// Invokes one plugin function on a session of the function's plugin.
pub fn plugin_function_request(
    request: PluginFunctionRequest,
) -> Result<OperationResponse, CoreError> {
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| engine().next_operation_id());
    engine()
        .session(&request.session_id)?
        .function_request(request, operation_id.clone(), None)?;
    Ok(OperationResponse { operation_id })
}

/// Sends one request of a plugin backend to the plugin's own language server.
pub fn plugin_server_request(request: PluginServerRequest) -> Result<OperationResponse, CoreError> {
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| engine().next_operation_id());
    engine()
        .session(&request.session_id)?
        .plugin_server_request(request, operation_id.clone())?;
    Ok(OperationResponse { operation_id })
}

/// Sends one notification of a plugin backend to the plugin's own language server.
pub fn plugin_server_notify(
    request: PluginServerNotify,
) -> Result<PluginServerNotifyResponse, CoreError> {
    let delivered = engine()
        .session(&request.session_id)?
        .plugin_server_notify(request)?;
    Ok(PluginServerNotifyResponse { delivered })
}

/// Replaces the host's view of the enabled plugins and live workspace
/// contexts that the cross-plugin function relay checks calls against. Calls
/// in flight that the new view forbids end with an error.
pub fn configure_function_relay(
    snapshot: lithe_plugin_function_relay::RelaySnapshot,
) -> Result<(), CoreError> {
    engine().relay.configure(snapshot)
}

/// Cancels a pending semantic request in both client state and the server.
pub fn cancel_operation(request: CancelOperationRequest) -> Result<(), CoreError> {
    engine()
        .session(&request.session_id)?
        .cancel_operation(&request.operation_id)
}

/// Host-owned toggle for one session's upstream trace diagnostics.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetDetailedDiagnosticsRequest {
    /// Opaque session returned by startServer.
    pub session_id: String,
    /// True sends verbose tracing, false sends off; default sessions start disabled.
    pub enabled: bool,
}

/// Updates the trace gate only after the corresponding ordered write succeeds.
pub fn set_detailed_diagnostics(request: SetDetailedDiagnosticsRequest) -> Result<(), CoreError> {
    engine()
        .session(&request.session_id)?
        .set_detailed_diagnostics(request.enabled)
}

/// Drains all currently queued events in deterministic sequence order.
pub fn poll_events(request: SessionRequest) -> Result<PollEventsResponse, CoreError> {
    let events = engine().session(&request.session_id)?.poll_events()?;
    Ok(PollEventsResponse { events })
}

/// Waits until queued events exist or the supplied timeout elapses.
pub fn wait_events(request: WaitEventsRequest) -> Result<PollEventsResponse, CoreError> {
    let events = engine()
        .session(&request.session_id)?
        .wait_events(Duration::from_millis(request.timeout_milliseconds))?;
    Ok(PollEventsResponse { events })
}

/// Stops a session if necessary and removes all state owned by it.
pub fn destroy_server(request: SessionRequest) -> Result<(), CoreError> {
    engine().destroy(&request.session_id)
}

fn engine() -> &'static LspEngine {
    ENGINE.get_or_init(LspEngine::new)
}

impl LspEngine {
    fn new() -> Self {
        Self::with_launcher(Arc::new(SystemProcessLauncher))
    }

    fn with_launcher(launcher: Arc<dyn LspProcessLauncher>) -> Self {
        let sessions = Arc::new(Mutex::new(BTreeMap::new()));
        Self {
            next_session_id: AtomicU64::new(1),
            next_operation_id: AtomicU64::new(1),
            relay: RelayHub::start(&sessions),
            sessions,
            failed_starts: Mutex::new(BTreeSet::new()),
            launcher,
        }
    }

    fn next_operation_id(&self) -> String {
        format!(
            "lsp-operation-{}",
            self.next_operation_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn start_server(&self, request: StartServerRequest) -> Result<StartServerResponse, CoreError> {
        let mut registered = None;
        let result = self.start_server_owned(request, |id| registered = Some(id.to_owned()));
        if result.is_err() {
            if let Some(id) = registered {
                if self.lock_sessions()?.contains_key(&id) {
                    self.lock_failed_starts()?.insert(id);
                    // Preserve the original startup error. Incomplete cleanup
                    // stays owned; an explicit startup retry reaps it before
                    // any replacement process can be launched.
                    // log-exempt: Same failure already recorded: this cleanup follows the startup failure returned below and traced by this request's Core failure; residue stays in failed_starts and the next start reports ProcessFailed.
                    let _ = self.cleanup_failed_start();
                }
            }
        }
        result
    }

    fn lock_failed_starts(&self) -> Result<MutexGuard<'_, BTreeSet<String>>, CoreError> {
        self.failed_starts.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Failed startup cleanup lock was poisoned.",
            )
        })
    }

    fn cleanup_failed_start(&self) -> Result<(), CoreError> {
        // Serialize cleanup attempts, but never hold the session registry or
        // protocol-state lock across process disposal. Each retry owns at most
        // one two-second disposal; a backlog must not make one call unbounded.
        let mut failed = self.lock_failed_starts()?;
        if let Some(id) = failed.first().cloned() {
            if self.lock_sessions()?.contains_key(&id) {
                self.destroy(&id)?;
            }
            failed.remove(&id);
        }
        if !failed.is_empty() {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Previous language-server startup cleanup is still pending. Retry startup to continue cleanup.",
            ));
        }
        Ok(())
    }

    fn start_server_owned(
        &self,
        request: StartServerRequest,
        registered: impl FnOnce(&str),
    ) -> Result<StartServerResponse, CoreError> {
        let attempt = startup_attempt(&request);
        let result = self.start_server_attempt(request, &attempt, registered);
        // A session that already reached a terminal result took the attempt;
        // otherwise the process never started. Cancellation is not a result.
        match &result {
            Err(error) if !matches!(error.code, ErrorCode::Cancelled) => {
                plugin_operations::finish(&attempt, Some(AttemptStage::Launch));
            }
            Err(_) => plugin_operations::abandon(&attempt),
            Ok(_) => {}
        }
        result
    }

    fn start_server_attempt(
        &self,
        request: StartServerRequest,
        attempt: &AttemptSlot,
        registered: impl FnOnce(&str),
    ) -> Result<StartServerResponse, CoreError> {
        validate_start_request(&request)?;
        self.cleanup_failed_start()?;
        if lithe_workspace::is_remote()
            && request
                .session_policy
                .as_ref()
                .is_some_and(|policy| policy.remote == RemoteSupport::Unsupported)
        {
            return Err(CoreError::new(
                ErrorCode::NotSupported,
                "The plugin's language server does not support remote workspaces",
            ));
        }
        // Initial provider settings must include native Git decisions even when
        // no session or inventory warm-up has run. Never scan under registry locks.
        let indexing_root = canonical_workspace_path(&request.root_uri);
        let indexing_context = if let Some(root) = &indexing_root {
            lithe_project::indexing::prepare_provider_exclusions(root)?
        } else {
            None
        };
        let check_indexing = || {
            if let Some(context) = &indexing_context {
                return context.check();
            }
            if indexing_root.as_ref().is_some_and(|root| {
                lithe_project::indexing::active_provider_context(root).is_some()
            }) {
                return Err(CoreError::new(
                    ErrorCode::Cancelled,
                    "Indexing policy changed",
                ));
            }
            Ok(())
        };
        let session_id = format!(
            "lsp-session-{}",
            self.next_session_id.fetch_add(1, Ordering::Relaxed)
        );
        let workspace_root = PathBuf::from(&request.working_directory);
        let initialize_timeout = Duration::from_millis(request.initialize_timeout_milliseconds);
        let request_timeout = Duration::from_millis(request.request_timeout_milliseconds);
        let shutdown_timeout = Duration::from_millis(request.shutdown_timeout_milliseconds);
        let workspace_configuration =
            checked_workspace_configuration(request.workspace_configuration)?;
        let initialize = client_initialize_for_plugin(
            ClientInitializeRequest {
                state: LspClientState {
                    offered_toolchain_providers: if request.plugin_id.is_some() {
                        request.toolchain_providers.iter().cloned().collect()
                    } else {
                        Default::default()
                    },
                    offered_keymap_import_providers: if request.plugin_id.is_some() {
                        request.keymap_import_providers.iter().cloned().collect()
                    } else {
                        Default::default()
                    },
                    ..LspClientState::default()
                },
                root_uri: request.root_uri.clone(),
                process_id: (!lithe_workspace::is_remote()).then_some(std::process::id() as i64),
                initialization_options: request.initialization_options,
            },
            request.plugin_id.is_some(),
        )?;
        let process = self.launcher.launch(LspProcessSpec {
            executable: PathBuf::from(&request.executable_path),
            arguments: request.arguments,
            working_directory: workspace_root,
            environment: request.environment,
        })?;

        let request_id = (initialize.state.next_request_id - 1).to_string();
        let now = Instant::now();
        let process_id = process.handle.process_id();
        let plugin_server = request.session_policy.map(PluginServerState::new);
        let session = Arc::new(RuntimeSession {
            startup_attempt: Arc::clone(attempt),
            id: session_id.clone(),
            log_identity: SessionLogIdentity::new(
                request.role,
                &request.provider_id,
                request.plugin_id.as_deref(),
                request.log_subject.as_deref(),
            ),
            provider_id: request.provider_id,
            plugin_id: request.plugin_id,
            workspace_host: lithe_workspace::current(),
            plugin_configuration: Mutex::new(workspace_configuration.map(Arc::new)),
            root_uri: request.root_uri,
            indexing_roots: Mutex::default(),
            indexing_revision: std::sync::atomic::AtomicU64::new(0),
            outbound_order: Mutex::new(()),
            state: Mutex::new(SessionState {
                lifecycle: LspLifecycleState::Created,
                client: LspClientState {
                    host_services: request.host_services.into_iter().collect(),
                    host_options: request.host_options,
                    ..initialize.state
                },
                pending: BTreeMap::from([(
                    request_id,
                    PendingRequest {
                        kind: PendingKind::Initialize,
                        operation_id: None,
                        method: "initialize".to_string(),
                        document_uri: None,
                        document_version: None,
                        created_at: now,
                        deadline: now + initialize_timeout,
                    },
                )]),
                request_by_operation: BTreeMap::new(),
                function_by_operation: BTreeMap::new(),
                linked_document_paths: LinkedDocumentPaths::default(),
                pending_workspace_file_changes: BTreeMap::new(),
                events: VecDeque::new(),
                next_sequence: 1,
                initialize_deadline: Some(now + initialize_timeout),
                shutdown_deadline: None,
                request_timeout,
                shutdown_timeout,
                terminal_event_emitted: false,
                deadline_cancellations: Vec::new(),
                maintenance_write_deadline: None,
                plugin_server,
                relay_write_deadline: None,
                host_requests: BTreeMap::new(),
                next_host_request_id: 1,
                detailed_diagnostics: false,
                requested_diagnostics: false,
                rejected_plugin_diagnostics: false,
                plugin_diagnostics_window: Instant::now(),
                plugin_diagnostics_count: 0,
            }),
            event_signal: Condvar::new(),
            process: process.handle,
            active: AtomicBool::new(true),
            relay: Arc::clone(&self.relay),
        });
        session.transition(LspLifecycleState::Created, None)?;
        session.transition(LspLifecycleState::ProcessStarting, None)?;
        session.transition(LspLifecycleState::Initializing, None)?;
        session.log(
            "info",
            EngineLog::ProcessStarted,
            "Language server process started",
            None,
        );

        // Register before checking the prepared generation. A policy change
        // either retires that generation or sees this session for its update.
        self.lock_sessions()?
            .insert(session_id.clone(), session.clone());
        if let Err(error) = check_indexing() {
            // A retired start is cancelled, not a handshake failure. Abandon
            // the attempt before `fail` or the process exit can record it.
            plugin_operations::abandon(attempt);
            session.fail(
                "indexingChanged",
                "initialize",
                "Indexing policy changed during language-server startup.",
                Some(core_error_detail(&error)),
                None,
            );
            session.active.store(false, Ordering::Release);
            session.kill_process();
            // No native identity has been published yet. Forget the session
            // only after disposal proves that its process tree has exited.
            if session.process.dispose(Duration::from_secs(2)).is_ok() {
                self.lock_sessions()?.remove(&session_id);
            } else {
                registered(&session_id);
            }
            return Err(error);
        }
        // Native owners learn the identity before initialize writes can fail.
        // They keep their package barrier until destroy verifies disposal.
        registered(&session_id);
        session.spawn_readers(process.output, process.errors);
        session.spawn_monitor();
        session.spawn_outbound_maintenance();
        let outbound_order = session.lock_outbound_order()?;
        if let Err(error) = session.send_messages(&outbound_order, initialize.messages) {
            // startServer fails here, so the attempt failed to launch.
            plugin_operations::finish(attempt, Some(AttemptStage::Launch));
            session.fail(
                "transportFailed",
                "initialize",
                "Could not write the initialize request.",
                Some(core_error_detail(&error)),
                None,
            );
            session.kill_process();
            return Err(error);
        }

        if let Err(error) = check_indexing() {
            // The readers already run; abandon before `fail` and the kill so
            // neither the failure nor the observed exit records the cancel.
            plugin_operations::abandon(attempt);
            session.fail(
                "indexingChanged",
                "initialize",
                "Indexing policy changed during language-server startup.",
                Some(core_error_detail(&error)),
                None,
            );
            session.active.store(false, Ordering::Release);
            session.kill_process();
            // The native owner already holds this identity. Keep it until
            // destroy verifies disposal and returns a positive receipt.
            return Err(error);
        }

        Ok(StartServerResponse {
            channel: session.log_identity.channel(),
            session_id,
            state: LspLifecycleState::Initializing,
            process_id,
        })
    }

    fn session(&self, session_id: &str) -> Result<Arc<RuntimeSession>, CoreError> {
        self.lock_sessions()?
            .get(session_id)
            .cloned()
            .ok_or_else(|| unknown_session(session_id))
    }

    fn destroy(&self, session_id: &str) -> Result<(), CoreError> {
        let session = self.session(session_id)?;
        let lifecycle = session.lock_state()?.lifecycle;
        if !matches!(
            lifecycle,
            LspLifecycleState::Stopped | LspLifecycleState::Failed
        ) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "A running language-server session cannot be destroyed.",
            ));
        }
        session.process.dispose(Duration::from_secs(2))?;
        self.lock_sessions()?.remove(session_id);
        Ok(())
    }

    fn lock_sessions(
        &self,
    ) -> Result<MutexGuard<'_, BTreeMap<String, Arc<RuntimeSession>>>, CoreError> {
        self.sessions.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Language-server session registry lock was poisoned.",
            )
        })
    }
}

impl RuntimeSession {
    fn save_document(&self, request: SaveDocumentRequest) -> Result<bool, CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            if state.lifecycle != LspLifecycleState::Ready {
                return Ok(false);
            }
            let Some(include_text) = state.client.text_document_save_include_text else {
                return Ok(false);
            };
            let Some(document) = state.client.open_documents.get(&request.uri) else {
                return Ok(false);
            };
            // A server may reload its working copy from the saved text. A
            // delayed save must not discard a newer editor revision already
            // synchronized.
            if *document.text != request.text {
                return Ok(false);
            }
            let mut params = json!({"textDocument": {"uri": request.uri}});
            if include_text {
                params["text"] = json!(request.text);
            }
            json!({"jsonrpc": "2.0", "method": "textDocument/didSave", "params": params})
                .to_string()
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "saveDocument")?;
        Ok(true)
    }

    fn workspace_files_changed(&self, changes: Vec<WorkspaceFileChange>) -> Result<(), CoreError> {
        if changes.is_empty() {
            return Ok(());
        }
        let _indexing_order = self.lock_indexing_roots()?;
        let mut normalized = BTreeMap::new();
        for change in changes {
            if lithe_project::indexing::uri_is_excluded(&self.root_uri, &change.uri)? {
                continue;
            }
            if !change.uri.contains("://") || change.uri.contains('\0') {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Workspace file changes require absolute document URIs.",
                )
                .with_details(change.uri));
            }
            // The last event for a URI represents its final filesystem state
            // and prevents a watcher burst from causing redundant imports.
            normalized.insert(change.uri, change.kind);
        }
        if normalized.is_empty() {
            return Ok(());
        }
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let mut state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            if !state.client.initialized {
                state.pending_workspace_file_changes.extend(normalized);
                return Ok(());
            }
            workspace_file_changes_notification(normalized)
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "workspaceFilesChanged")
    }

    fn sync_document(
        &self,
        request: SyncDocumentRequest,
    ) -> Result<SyncDocumentResponse, CoreError> {
        self.sync_document_after_policy_check(request, || {})
    }

    fn sync_document_after_policy_check(
        &self,
        request: SyncDocumentRequest,
        after_policy_check: impl FnOnce(),
    ) -> Result<SyncDocumentResponse, CoreError> {
        let _indexing_order = self.lock_indexing_roots()?;
        if lithe_project::indexing::uri_is_excluded(&self.root_uri, &request.uri)? {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Document is excluded from indexing",
            ));
        }
        after_policy_check();
        let uri = request.uri;
        let mut messages = Vec::new();
        let mut stale_cancellations = Vec::new();
        let document_version;
        let outbound_order = self.lock_outbound_order()?;
        {
            let mut state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            if let Some(document) = state.client.open_documents.get(&uri) {
                // Hosts commonly publish both a ranged edit and the resulting
                // full text. Repeating that full text must not create another
                // server reconciliation cycle or advance the semantic version.
                if request.content_changes.is_empty() && *document.text == *request.text {
                    return Ok(SyncDocumentResponse {
                        document_version: document.version.max(1),
                        changed: false,
                    });
                }
            }
            if state.client.initialized {
                let response = if state
                    .client
                    .open_documents
                    .get(&uri)
                    .is_some_and(|document| document.version > 0)
                {
                    client_change_document(ClientChangeDocumentRequest {
                        state: state.client.clone(),
                        uri: uri.clone(),
                        text: request.text,
                        content_changes: request.content_changes,
                    })?
                } else {
                    let text = unopened_document_text(
                        state.client.open_documents.get(&uri),
                        request.text,
                        &request.content_changes,
                    )?;
                    client_open_document(ClientOpenDocumentRequest {
                        state: state.client.clone(),
                        uri: uri.clone(),
                        language_id: request.language_id,
                        text,
                    })?
                };
                state.client = response.state;
                messages = response.messages;
                document_version = state
                    .client
                    .open_documents
                    .get(&uri)
                    .map(|document| document.version)
                    .unwrap_or_default();
                stale_cancellations =
                    cancel_stale_document_requests_locked(self, &mut state, &uri, document_version);
            } else {
                // Version zero means the semantic document exists in the Rust
                // store but has not yet been opened on the server. The latest
                // sync wins until initialize completes.
                let text = unopened_document_text(
                    state.client.open_documents.get(&uri),
                    request.text,
                    &request.content_changes,
                )?;
                state.client.diagnostics.remove(&uri);
                state.client.diagnostic_versions.remove(&uri);
                state.client.open_documents.insert(
                    uri.clone(),
                    LspClientDocument {
                        uri,
                        language_id: request.language_id,
                        version: 0,
                        text: text.into(),
                    },
                );
                // Version zero remains an internal "not opened on the wire"
                // sentinel. Consumers receive version one, which is the exact
                // version the latest queued text will have after initialize.
                document_version = 1;
            }
        }
        messages.extend(stale_cancellations);
        self.send_messages_or_fail(&outbound_order, messages, "documentSync")?;
        Ok(SyncDocumentResponse {
            document_version,
            changed: true,
        })
    }

    fn close_document(&self, uri: &str) -> Result<(), CoreError> {
        let mut messages = Vec::new();
        let cleared;
        let outbound_order = self.lock_outbound_order()?;
        {
            let mut state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            let Some(document) = state.client.open_documents.get(uri).cloned() else {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Cannot close a document that is not owned by the language-server session.",
                ));
            };
            cleared = state.client.diagnostics.contains_key(uri);
            if document.version == 0 {
                state.client.open_documents.remove(uri);
                state.client.diagnostics.remove(uri);
                state.client.diagnostic_versions.remove(uri);
            } else {
                let response = client_close_document(ClientCloseDocumentRequest {
                    state: state.client.clone(),
                    uri: uri.to_string(),
                })?;
                state.client = response.state;
                messages = response.messages;
            }
            if cleared {
                push_diagnostics_event(self, &mut state, uri, None, Vec::new());
            }
        }
        self.send_messages_or_fail(&outbound_order, messages, "documentClose")
    }

    fn request(&self, request: SemanticRequest, operation_id: String) -> Result<(), CoreError> {
        let document_version = request.uri.as_deref().and_then(|uri| {
            self.lock_state().ok().and_then(|state| {
                state
                    .client
                    .open_documents
                    .get(uri)
                    .map(|document| document.version.max(1))
            })
        });
        self.request_with_kind(
            request,
            operation_id,
            PendingKind::Feature,
            document_version,
        )
    }

    /// Writes a build-provider request. It needs no open document, so it is
    /// not a semantic feature request; only the negotiated protocol's methods
    /// are accepted, so the host cannot use it as a generic JSON-RPC tunnel.
    fn build_request(
        &self,
        request: BuildProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        if !lithe_lsp_client::build_provider::is_build_method(&request.method) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "This is not a build-provider request.",
            )
            .with_details(request.method));
        }
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) =
            {
                let mut state = self.lock_state()?;
                if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Language server is not ready.",
                    ));
                }
                if state.request_by_operation.contains_key(&operation_id) {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The language-server operation ID is already pending.",
                    ));
                }
                if !state.client.server_capabilities.iter().any(|feature| {
                    feature == lithe_lsp_client::build_provider::BUILD_PROVIDER_FEATURE
                }) {
                    return Err(CoreError::new(
                        ErrorCode::NotSupported,
                        "The process did not agree on a build-provider protocol version.",
                    )
                    .with_details(lithe_lsp_client::build_provider::BUILD_PROVIDER_FEATURE));
                }
                if request.params.get("launchOptions").is_some()
                    && request.method != lithe_lsp_client::build_provider::PREPARE_RUN_METHOD
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Launch options apply only to run preparation.",
                    ));
                }
                if (request.params.get("selectorKind").is_some()
                    || request.params.get("selectors").is_some())
                    && request.method != lithe_lsp_client::build_provider::TEST_METHOD
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Opaque selectors require a provider test request.",
                    ));
                }
                if request.params.get("testResults").is_some()
                    && (request.method != lithe_lsp_client::build_provider::TEST_METHOD
                        || request.params["testResults"] != json!(true))
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Inline test reports require a provider test request.",
                    ));
                }
                let mut params = request.params;
                if lithe_lsp_client::build_provider::is_progress_method(&request.method) {
                    let object = params.as_object_mut().ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "Build task parameters must be an object.",
                        )
                    })?;
                    object.insert(
                        "operationID".to_string(),
                        json!(state.client.next_request_id.to_string()),
                    );
                }
                let timeout = request
                    .timeout_milliseconds
                    .map(|milliseconds| {
                        Duration::from_millis(milliseconds.clamp(1, MAX_BUILD_PROVIDER_TIMEOUT_MS))
                    })
                    .unwrap_or(state.request_timeout);
                let response = allocate_raw_request(state.client.clone(), &request.method, params)?;
                let request_id = (response.state.next_request_id - 1).to_string();
                let now = Instant::now();
                state.client = response.state;
                state.pending.insert(
                    request_id.clone(),
                    PendingRequest {
                        kind: PendingKind::Feature,
                        operation_id: Some(operation_id.clone()),
                        method: request.method,
                        document_uri: None,
                        document_version: None,
                        created_at: now,
                        deadline: now + timeout,
                    },
                );
                state
                    .request_by_operation
                    .insert(operation_id, request_id.clone());
                (response.messages, request_id)
            };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    fn keymap_import_request(
        &self,
        request: KeymapImportProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        // A snapshot reference is an opaque token, never a filesystem path.
        // The future host service must separately enforce its session ownership.
        if request.snapshot_id.is_empty()
            || request.snapshot_id.len() > 128
            || !request
                .snapshot_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid import snapshot reference.",
            ));
        }
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Plugin backend is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The operation ID is already pending.",
                ));
            }
            if !state
                .client
                .keymap_import_providers
                .contains(&request.provider_id)
            {
                return Err(CoreError::new(
                    ErrorCode::NotSupported,
                    "The plugin did not negotiate this import provider.",
                )
                .with_details("plugin.keymapImport.providerUnavailable"));
            }
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(milliseconds.clamp(1, MAX_KEYMAP_IMPORT_TIMEOUT_MS))
                })
                .unwrap_or(
                    state
                        .request_timeout
                        .min(Duration::from_millis(MAX_KEYMAP_IMPORT_TIMEOUT_MS)),
                );
            let response = allocate_raw_request(
                state.client.clone(),
                lithe_lsp_client::keymap_provider::IMPORT_METHOD,
                json!({"providerID": request.provider_id, "snapshotID": request.snapshot_id}),
            )?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: lithe_lsp_client::keymap_provider::IMPORT_METHOD.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Writes a VCS-provider request planned by `lithe_vcs::plan`, which
    /// checks the operation's arguments and negotiated capability.
    fn toolchain_request(
        &self,
        request: ToolchainProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let provider = request.params["providerID"].as_str().unwrap_or_default();
            if !state
                .client
                .toolchain_providers
                .iter()
                .any(|negotiated| negotiated == provider)
            {
                return Err(CoreError::new(
                    ErrorCode::NotSupported,
                    "The process did not negotiate this toolchain provider.",
                )
                .with_details("plugin.toolchain.providerUnavailable"));
            }
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(milliseconds.clamp(1, MAX_TOOLCHAIN_QUERY_TIMEOUT_MS))
                })
                .unwrap_or(state.request_timeout);
            let response = allocate_raw_request(
                state.client.clone(),
                lithe_lsp_client::toolchain_provider::QUERY_METHOD,
                request.params,
            )?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: lithe_lsp_client::toolchain_provider::QUERY_METHOD.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    fn vcs_request(
        &self,
        request: VcsProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        let operation = lithe_vcs::Operation::parse(&request.operation).ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Unknown VCS operation")
                .with_details(request.operation.clone())
        })?;
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let negotiation = state
                .client
                .vcs_provider
                .as_ref()
                .filter(|negotiation| negotiation.vcs == request.vcs)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::NotSupported,
                        format!("The process is not a negotiated {} provider.", request.vcs),
                    )
                    .with_details("vcs.provider.unavailable")
                })?;
            let planned = lithe_vcs::plan(
                operation,
                &request.root,
                request.tool.as_deref(),
                request.arguments,
                &negotiation.capabilities,
            )?;
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(milliseconds.clamp(1, MAX_VCS_PROVIDER_TIMEOUT_MS))
                })
                .unwrap_or(state.request_timeout);
            let response =
                allocate_raw_request(state.client.clone(), planned.method, planned.params)?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: planned.method.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Writes a `lithe/function/invoke` request. The plan comes from
    /// `plugin_function::plan_invocation`, so plugin identity, negotiation,
    /// and registration are checked before anything reaches the process.
    ///
    /// `caller` is the plugin whose call the relay forwards, and `None` for a
    /// call the host starts. Only the relay passes `Some`, so a host cannot
    /// name a plugin as the caller and a plugin cannot choose its own name.
    fn function_request(
        &self,
        request: PluginFunctionRequest,
        operation_id: String,
        caller: Option<&str>,
    ) -> Result<(), CoreError> {
        if caller.is_none() && relay_hub::is_relay_operation_id(&operation_id) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "This language-server operation ID is reserved for relayed plugin calls.",
            ));
        }
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let negotiated = state
                .client
                .server_capabilities
                .iter()
                .any(|feature| feature == lithe_plugin_rpc::FUNCTIONS_FEATURE);
            // Progress echoes this per-request wire ID, never the reusable host ID.
            let wire_operation_id = state.client.next_request_id.to_string();
            let params = lithe_plugin_rpc::plan_invocation(
                &lithe_plugin_rpc::FunctionSession {
                    plugin_id: self.plugin_id.as_deref(),
                    negotiated,
                    functions: &state.client.plugin_functions,
                },
                &request.function,
                &wire_operation_id,
                &request.params,
            )?;
            let mut params = params;
            lithe_plugin_rpc::attach_context(&mut params, request.context.as_ref())?;
            lithe_plugin_rpc::attach_caller(&mut params, caller);
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(
                        milliseconds.clamp(1, lithe_plugin_rpc::MAX_FUNCTION_TIMEOUT_MS),
                    )
                })
                .unwrap_or(state.request_timeout);
            let response = allocate_raw_request(
                state.client.clone(),
                lithe_plugin_rpc::INVOKE_METHOD,
                params,
            )?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: lithe_plugin_rpc::INVOKE_METHOD.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id.clone(), request_id.clone());
            state.function_by_operation.insert(
                operation_id,
                PendingFunctionCall {
                    function_id: request.function.function_id,
                    request_id: request_id.clone(),
                },
            );
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Sends the backend the host's answer to one forwarded host-service
    /// request. An unknown ID, including one already answered or of a session
    /// that restarted, is refused so a late answer never reaches another
    /// request.
    fn host_response(&self, response: PluginHostResponse) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let mut state = self.lock_state()?;
            let backend_id = state
                .host_requests
                .remove(&response.host_request_id)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "No host-service request with this ID is waiting for an answer.",
                    )
                })?;
            match (response.result, response.error) {
                (_, Some(failure)) => {
                    let (code, rpc_code) = lithe_plugin_rpc::host_failure(failure.code.as_deref());
                    let mut data = json!({ "code": code });
                    if let Some(details) =
                        lithe_plugin_rpc::bounded_host_details(failure.details.as_ref())
                    {
                        data["details"] = details;
                    }
                    json_rpc_error_with_data(
                        &backend_id,
                        rpc_code,
                        &lithe_plugin_rpc::bounded_host_message(&failure.message),
                        data,
                    )?
                }
                (result, None) => {
                    let result = result.unwrap_or(Value::Null);
                    if result.to_string().len() > lithe_plugin_rpc::MAX_RESULT_BYTES {
                        json_rpc_error_with_code(
                            &backend_id,
                            -32014,
                            "Host result exceeds the byte limit.",
                            lithe_plugin_rpc::codes::HOST_LIMIT_EXCEEDED,
                        )?
                    } else {
                        json_rpc_result(&backend_id, result)?
                    }
                }
            }
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "hostResponse")
    }

    /// Writes one host notification to a ready plugin-owned backend.
    /// Returns `false`, writing nothing, when the backend negotiated no
    /// plugin-function protocol version.
    fn host_notify(&self, notification: PluginHostNotification) -> Result<bool, CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let state = self.lock_state()?;
            if self.plugin_id.is_none() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Only a plugin process receives host notifications.",
                ));
            }
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            let delivered = lithe_plugin_rpc::host_notification(
                &notification.method,
                &notification.params,
                state.client.plugin_function_version,
                &state.client.host_services,
            )?;
            if !delivered {
                return Ok(false);
            }
            json_rpc_notification(&notification.method, notification.params)?
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "hostNotify")?;
        Ok(true)
    }

    /// Admits a backend's request to its language server: it is sent now,
    /// held behind a gate, or joined to a command that already runs.
    fn plugin_server_request(
        &self,
        request: PluginServerRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let server = plugin_server_target(
                &mut state,
                self.plugin_id.as_deref(),
                &request.plugin_id,
                &request.server_id,
            )?;
            let now = Instant::now();
            let deadline = now + plugin_server_timeout(request.timeout_milliseconds);
            let admission = server.admit_request(Arrival {
                operation_id: &operation_id,
                method: &request.method,
                params: &request.params,
                now,
                deadline,
            })?;
            if admission != Admission::Send {
                // Held and joined requests are written later, or never.
                return Ok(());
            }
            match register_plugin_server_request(
                &mut state,
                &operation_id,
                &request.method,
                request.params,
                now,
                deadline,
            ) {
                Ok(registered) => registered,
                Err(error) => {
                    if let Some(server) = state.plugin_server.as_mut() {
                        server.finish(&operation_id);
                    }
                    return Err(error);
                }
            }
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Sends a backend's notification to its language server. Returns whether
    /// it was sent now.
    fn plugin_server_notify(&self, request: PluginServerNotify) -> Result<bool, CoreError> {
        {
            let mut state = self.lock_state()?;
            let server = plugin_server_target(
                &mut state,
                self.plugin_id.as_deref(),
                &request.plugin_id,
                &request.server_id,
            )?;
            server.admit_notification(&request.method, &request.params)?;
        }
        if request.method == DID_CHANGE_CONFIGURATION {
            // The settings also answer the server's own `workspace/configuration`
            // requests, so they go through the plugin configuration.
            let settings = request
                .params
                .get("settings")
                .filter(|settings| settings.is_object())
                .cloned()
                .ok_or_else(|| {
                    CoreError::new(ErrorCode::InvalidRequest, "settings must be an object.")
                        .with_details(lithe_plugin_rpc::codes::HOST_INVALID_PARAMS)
                })?;
            return Ok(self.update_plugin_configuration(settings)?.sent);
        }
        let message = json_rpc_notification(&request.method, request.params)?;
        let outbound_order = self.lock_outbound_order()?;
        self.send_messages_or_fail(&outbound_order, vec![message], "pluginServerNotify")?;
        Ok(true)
    }

    fn request_with_kind(
        &self,
        request: SemanticRequest,
        operation_id: String,
        requested_kind: PendingKind,
        requested_document_version: Option<i64>,
    ) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let uri = request.uri.clone();
            if let (Some(uri), Some(expected_version)) =
                (uri.as_deref(), requested_document_version)
            {
                let current_version = state
                    .client
                    .open_documents
                    .get(uri)
                    .map(|document| document.version.max(1))
                    .ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "The document is not synchronized with the language server.",
                        )
                    })?;
                if current_version != expected_version {
                    return Err(CoreError::new(
                        ErrorCode::Cancelled,
                        "The document changed before the language-server request started.",
                    )
                    .with_details(format!(
                        "expectedVersion={expected_version}, currentVersion={current_version}"
                    )));
                }
            }
            let method = semantic_method(request.operation);
            let required_capability = semantic_capability(request.operation);
            if let Some(capability) = required_capability {
                if !state
                    .client
                    .server_capabilities
                    .iter()
                    .any(|candidate| candidate == capability)
                {
                    return Err(CoreError::new(
                        ErrorCode::NotSupported,
                        "The language server did not advertise this capability.",
                    )
                    .with_details(capability));
                }
            }

            let pending_kind = requested_kind;
            let response = match request.operation {
                LspSemanticOperation::WorkspaceSymbols => {
                    let query = request.query.clone().unwrap_or_default();
                    allocate_raw_request(state.client.clone(), method, json!({ "query": query }))?
                }
                LspSemanticOperation::ExecuteCommand => {
                    let command = request.command.ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "This language-server request requires a command.",
                        )
                    })?;
                    allocate_raw_request(state.client.clone(), method, command)?
                }
                _ => {
                    let uri = uri.clone().ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "This language-server operation requires a document URI.",
                        )
                    })?;
                    let document_is_open = state
                        .client
                        .open_documents
                        .get(&uri)
                        .is_some_and(|document| document.version > 0);
                    if !document_is_open {
                        return Err(CoreError::new(
                            ErrorCode::InvalidRequest,
                            "The document is not open in the language server.",
                        ));
                    }
                    let feature_request = ClientFeatureRequest {
                        state: state.client.clone(),
                        uri,
                        method: method.to_string(),
                        position: request.position,
                        new_name: request.new_name,
                        query: request.query,
                        range: request.range,
                        diagnostics: request.diagnostics,
                        completion_context: request.completion_context,
                        completion_item: request.completion_item,
                        code_action: request.code_action,
                        command: request.command,
                        formatting_options: request.formatting_options,
                        include_declaration: request.include_declaration,
                    };
                    client_feature_request(feature_request)?
                }
            };
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            let pending = PendingRequest {
                kind: pending_kind,
                operation_id: Some(operation_id.clone()),
                method: method.to_string(),
                document_uri: uri.clone(),
                document_version: requested_document_version.or_else(|| {
                    uri.as_deref()
                        .and_then(|uri| state.client.open_documents.get(uri))
                        .map(|document| document.version)
                }),
                created_at: now,
                deadline: now + state.request_timeout,
            };
            state.client = response.state;
            state.pending.insert(request_id.clone(), pending);
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Writes an allocated request. A write failure completes the request
    /// with an error and fails the session, because the server's view of the
    /// conversation is no longer known.
    fn send_request_or_fail(
        &self,
        outbound_order: &MutexGuard<'_, ()>,
        messages: Vec<String>,
        request_id: &str,
    ) -> Result<(), CoreError> {
        if let Err(error) = self.send_messages(outbound_order, messages) {
            self.complete_request_with_error(
                request_id,
                "transportFailed",
                "request",
                "Could not write the language-server request.",
                Some(core_error_detail(&error)),
                None,
            );
            self.fail(
                "transportFailed",
                "request",
                "Language-server stdin failed.",
                Some(core_error_detail(&error)),
                None,
            );
            self.kill_process();
            return Err(error);
        }
        Ok(())
    }

    fn cancel_operation(&self, operation_id: &str) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let request_id = {
            let mut state = self.lock_state()?;
            if state
                .plugin_server
                .as_ref()
                .is_some_and(|server| server.is_unsent(operation_id))
            {
                cancel_unsent_plugin_server_request(self, &mut state, operation_id);
                return Ok(());
            }
            let request_id = state
                .request_by_operation
                .remove(operation_id)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Unknown pending language-server operation.",
                    )
                })?;
            let pending = state.pending.remove(&request_id).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Unknown pending language-server request.",
                )
            })?;
            state.client.pending_requests.remove(&request_id);
            state.client.pending_semantic_legends.remove(&request_id);
            state.function_by_operation.remove(operation_id);
            let error = runtime_error(
                self,
                "requestCancelled",
                "request",
                Some(&pending.method),
                pending.document_uri.as_deref(),
                "Language-server request was cancelled.",
                None,
                None,
            );
            push_request_event(
                self,
                &mut state,
                operation_id,
                &pending.method,
                None,
                Some(error),
            );
            request_id
        };
        let cancellation = json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": request_id }
        })
        .to_string();
        self.send_messages_or_fail(&outbound_order, vec![cancellation], "requestCancel")
    }

    fn stop(&self) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, force_kill) = {
            let mut state = self.lock_state()?;
            if matches!(
                state.lifecycle,
                LspLifecycleState::Stopped | LspLifecycleState::Failed
            ) {
                return Ok(());
            }
            if state.lifecycle == LspLifecycleState::Stopping {
                return Ok(());
            }
            fail_feature_requests(
                self,
                &mut state,
                "requestCancelled",
                "stop",
                "Language-server session is stopping.",
                None,
            );
            clear_function_registry(self, &mut state);
            clear_runtime_diagnostics(self, &mut state);
            transition_locked(self, &mut state, LspLifecycleState::Stopping, None);
            plugin_operations::abandon(&self.startup_attempt);
            state.initialize_deadline = None;
            state.shutdown_deadline = Some(Instant::now() + state.shutdown_timeout);
            if state.client.initialized {
                let response = client_shutdown(ClientShutdownRequest {
                    state: state.client.clone(),
                })?;
                let request_id = (response.state.next_request_id - 1).to_string();
                let now = Instant::now();
                let shutdown_timeout = state.shutdown_timeout;
                state.pending.insert(
                    request_id,
                    PendingRequest {
                        kind: PendingKind::Shutdown,
                        operation_id: None,
                        method: "shutdown".to_string(),
                        document_uri: None,
                        document_version: None,
                        created_at: now,
                        deadline: now + shutdown_timeout,
                    },
                );
                state.client = response.state;
                (response.messages, false)
            } else {
                state.client.pending_requests.clear();
                state.pending.clear();
                (Vec::new(), true)
            }
        };
        self.send_messages_or_fail(&outbound_order, messages, "shutdown")?;
        if force_kill {
            self.kill_process();
        }
        Ok(())
    }

    fn poll_events(&self) -> Result<Vec<LspRuntimeEvent>, CoreError> {
        let mut state = self.lock_state()?;
        Ok(state.events.drain(..).collect())
    }

    fn wait_events(&self, timeout: Duration) -> Result<Vec<LspRuntimeEvent>, CoreError> {
        let mut state = self.lock_state()?;
        let deadline = Instant::now() + timeout;
        loop {
            lithe_protocol::cancellation::check()?;
            if !state.events.is_empty() {
                return Ok(state.events.drain(..).collect());
            }
            if matches!(
                state.lifecycle,
                LspLifecycleState::Stopped | LspLifecycleState::Failed
            ) {
                // Returning Ok([]) here would let frontend pumps spin on Core IPC
                // forever after the terminal stateChanged event was drained.
                let details = match state.lifecycle {
                    LspLifecycleState::Failed => "sessionFailed",
                    _ => "sessionStopped",
                };
                return Err(CoreError::new(
                    ErrorCode::ProcessFailed,
                    "Language-server session is no longer running.",
                )
                .with_details(details));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(Vec::new());
            }
            let (guard, wait_result) =
                self.event_signal
                    .wait_timeout(state, remaining)
                    .map_err(|_| {
                        CoreError::new(
                            ErrorCode::Unknown,
                            "Language-server event wait lock was poisoned.",
                        )
                    })?;
            state = guard;
            if wait_result.timed_out() && state.events.is_empty() {
                return Ok(Vec::new());
            }
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> Result<EngineSnapshot, CoreError> {
        let state = self.lock_state()?;
        Ok(EngineSnapshot {
            session_id: self.id.clone(),
            provider_id: self.provider_id.clone(),
            root_uri: self.root_uri.clone(),
            state: state.lifecycle,
            initialized: state.client.initialized,
            open_documents: state.client.open_documents.clone(),
            pending_operation_ids: {
                let mut ids: Vec<String> = state.request_by_operation.keys().cloned().collect();
                ids.sort();
                ids
            },
            diagnostic_versions: state.client.diagnostic_versions.clone(),
        })
    }

    fn spawn_readers(
        self: &Arc<Self>,
        mut stdout: Box<dyn Read + Send>,
        mut stderr: Box<dyn Read + Send>,
    ) {
        let output_session = self.clone();
        thread::spawn(move || {
            let _host = lithe_workspace::enter(output_session.workspace_host.clone());
            let mut frame_buffer = Vec::new();
            let mut chunk = vec![0_u8; 8 * 1024];
            while output_session.active.load(Ordering::Acquire) {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        match parse_server_messages(ParseServerMessagesRequest {
                            buffer: std::mem::take(&mut frame_buffer),
                            chunk: chunk[..count].to_vec(),
                        }) {
                            Ok(parsed) => {
                                frame_buffer = parsed.buffer;
                                for message in parsed.messages {
                                    if let Err(error) =
                                        output_session.handle_server_message(message)
                                    {
                                        output_session.fail(
                                            "invalidServerMessage",
                                            "transport",
                                            "Language server sent an invalid message.",
                                            Some(core_error_detail(&error)),
                                            None,
                                        );
                                        output_session.kill_process();
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                output_session.fail(
                                    "transportFailed",
                                    "transport",
                                    "Language-server stdout framing failed.",
                                    Some(core_error_detail(&error)),
                                    None,
                                );
                                output_session.kill_process();
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        output_session.fail(
                            "transportFailed",
                            "transport",
                            "Could not read language-server stdout.",
                            Some(error.to_string()),
                            None,
                        );
                        output_session.kill_process();
                        return;
                    }
                }
            }
        });

        let error_session = self.clone();
        thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            let mut lines = StderrLines::default();
            let started = Instant::now();
            let emit = |fact| match fact {
                StderrFact::Line {
                    diagnostic,
                    truncated,
                } => error_session.log(
                    "debug",
                    EngineLog::StderrReceived
                        .with_fields(json!({"diagnostic": diagnostic, "truncated": truncated})),
                    "Language-server stderr",
                    Some(diagnostic),
                ),
                StderrFact::Dropped(count) => error_session.log(
                    "warning",
                    EngineLog::Dropped.with_fields(json!({"count": count})),
                    "Engine stderr lines dropped",
                    Some(format!("count={count}")),
                ),
            };
            while error_session.active.load(Ordering::Acquire) {
                match stderr.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        lines.feed(&chunk[..count], started.elapsed().as_millis() as u64, emit)
                    }
                    Err(error) => {
                        error_session.log_issue(
                            "warning",
                            "stderr",
                            "stderrReadFailed",
                            EngineLog::StderrReadFailed,
                            "Could not read language-server stderr",
                            Some(error.to_string()),
                        );
                        break;
                    }
                }
            }
            lines.finish(started.elapsed().as_millis() as u64, emit);
        });
    }

    fn spawn_monitor(self: &Arc<Self>) {
        let session = self.clone();
        thread::spawn(move || {
            while session.active.load(Ordering::Acquire) {
                if let Some(exit_code) = session.process.exit_status() {
                    session.handle_process_exit(exit_code);
                    break;
                }
                session.expire_deadlines();
                thread::sleep(Duration::from_millis(MONITOR_INTERVAL_MS));
            }
        });
    }

    fn handle_server_message(&self, message: String) -> Result<(), CoreError> {
        self.handle_server_message_after_policy_check(message, || {})
    }

    fn handle_server_message_after_policy_check(
        &self,
        message: String,
        after_policy_check: impl FnOnce(),
    ) -> Result<(), CoreError> {
        let value: Value = serde_json::from_str(&message).map_err(|error| {
            CoreError::new(ErrorCode::ParseFailed, "Invalid LSP server JSON message.")
                .with_details(error.to_string())
        })?;
        // Diagnostics and readiness flushes must share the same policy boundary
        // as host document commits and exclusion withdrawal.
        let _indexing_order = self.lock_indexing_roots()?;
        let outbound_order = self.lock_outbound_order()?;
        let response_id = if value.get("method").is_none() {
            lsp_value_id(value.get("id"))
        } else {
            None
        };
        let (known_pending, pending_before, old_capabilities, old_completion) = {
            let state = self.lock_state()?;
            let pending = response_id
                .as_ref()
                .and_then(|id| state.pending.get(id).cloned());
            (
                response_id
                    .as_ref()
                    .is_none_or(|id| state.client.pending_requests.contains_key(id)),
                pending,
                state.client.server_capabilities.clone(),
                lithe_lsp_client::completion_options(&state.client),
            )
        };
        // A response whose request has timed out, been cancelled, or belongs
        // to an older session is intentionally ignored.
        if response_id.is_some() && !known_pending {
            self.log(
                "info",
                EngineLog::ResponseSkipped.with_fields(json!(response_id)),
                "Ignored a late language-server response",
                response_id,
            );
            return Ok(());
        }
        // Only a remote workspace rewrites the message; a local one keeps the
        // value parsed above instead of parsing the same text again.
        let value = match &self.workspace_host {
            Some(host) => {
                let mapped = lithe_lsp_transport::remote_process::map_message(
                    host,
                    &message,
                    false,
                    pending_before
                        .as_ref()
                        .map(|pending| pending.method.as_str()),
                )?;
                serde_json::from_str(&mapped).map_err(|error| {
                    CoreError::new(
                        ErrorCode::ParseFailed,
                        "Invalid mapped LSP server JSON message.",
                    )
                    .with_details(error.to_string())
                })?
            }
            None => value,
        };
        let method = value.get("method").and_then(Value::as_str);
        if method == Some("textDocument/publishDiagnostics") {
            if let Some(uri) = value.pointer("/params/uri").and_then(Value::as_str) {
                if lithe_project::indexing::uri_is_excluded(&self.root_uri, uri).unwrap_or(true) {
                    return Ok(());
                }
            }
        }
        after_policy_check();
        if method == Some("workspace/configuration") {
            if let Some(response) = self.provider_configuration_response(&value)? {
                return self.send_messages_or_fail(
                    &outbound_order,
                    vec![response],
                    "serverRequest",
                );
            }
        }

        let reduced = {
            let mut state = self.lock_state()?;
            let redirected = if method == Some("textDocument/publishDiagnostics") {
                let state = &mut *state;
                diagnostics_addressed_to_open_document(
                    &state.client.open_documents,
                    &mut state.linked_document_paths,
                    &value,
                )
            } else {
                None
            };
            client_apply_server_message_for_plugin(
                state.client.clone(),
                redirected.as_ref().unwrap_or(&value),
                self.plugin_id.is_some(),
            )?
        };
        let mut outbound = reduced.messages;
        let mut flush_documents = false;
        let mut ready_server_info = None;
        let mut fail_initialize: Option<(String, Option<String>)> = None;
        {
            let mut state = self.lock_state()?;
            state.client = reduced.state;
            if let Some(request_id) = response_id.as_ref() {
                state.client.pending_requests.remove(request_id);
                state.client.pending_semantic_legends.remove(request_id);
                if let Some(pending) = state.pending.remove(request_id) {
                    if let Some(operation_id) = &pending.operation_id {
                        state.request_by_operation.remove(operation_id);
                        state.function_by_operation.remove(operation_id);
                    }
                }
            }

            match pending_before.as_ref().map(|pending| pending.kind) {
                Some(PendingKind::Initialize) => {
                    let server_error = value.get("error").map(Value::to_string);
                    if server_error.is_some() || !state.client.initialized {
                        state.initialize_deadline = None;
                        fail_initialize = Some((
                            if server_error.is_some() {
                                "initializeFailed".to_string()
                            } else {
                                "invalidServerMessage".to_string()
                            },
                            server_error,
                        ));
                    } else {
                        plugin_operations::finish(&self.startup_attempt, None);
                        state.initialize_deadline = None;
                        ready_server_info = parse_server_info(&value);
                        flush_documents = true;
                    }
                }
                Some(PendingKind::Feature)
                    if pending_before.as_ref().is_some_and(|pending| {
                        pending.method == lithe_plugin_rpc::INVOKE_METHOD
                    }) =>
                {
                    if let Some(operation_id) = pending_before
                        .as_ref()
                        .and_then(|pending| pending.operation_id.as_ref())
                    {
                        let (result, error) = match lithe_plugin_rpc::invocation_outcome(&value) {
                            Ok(result) => (Some(result), None),
                            Err(failure) => (None, Some(function_runtime_error(self, failure))),
                        };
                        push_request_event(
                            self,
                            &mut state,
                            operation_id,
                            lithe_plugin_rpc::INVOKE_METHOD,
                            result,
                            error,
                        );
                    }
                }
                Some(PendingKind::Feature)
                    if pending_before.as_ref().is_some_and(|pending| {
                        lithe_lsp_client::build_provider::is_execution_method(&pending.method)
                    }) =>
                {
                    if let Some(pending) = pending_before.as_ref() {
                        if let Some(operation_id) = &pending.operation_id {
                            let (result, error) =
                                match lithe_lsp_client::build_provider::execution_outcome(&value) {
                                    Ok(result) => (Some(result), None),
                                    Err((code, message)) => (
                                        None,
                                        Some(runtime_error(
                                            self,
                                            code,
                                            "request",
                                            Some(&pending.method),
                                            None,
                                            &message,
                                            None,
                                            None,
                                        )),
                                    ),
                                };
                            push_request_event(
                                self,
                                &mut state,
                                operation_id,
                                &pending.method,
                                result,
                                error,
                            );
                        }
                    }
                }
                Some(PendingKind::Feature)
                    if pending_before
                        .as_ref()
                        .is_some_and(|pending| lithe_vcs::is_vcs_method(&pending.method)) =>
                {
                    if let Some(pending) = pending_before.as_ref() {
                        if let Some(operation_id) = &pending.operation_id {
                            let (result, error) =
                                match lithe_lsp_client::vcs_provider::reply_outcome(&value) {
                                    Ok(result) => (Some(result), None),
                                    Err(failure) => (
                                        None,
                                        Some(runtime_error(
                                            self,
                                            failure.code,
                                            "request",
                                            Some(&pending.method),
                                            None,
                                            &failure.message,
                                            None,
                                            None,
                                        )),
                                    ),
                                };
                            push_request_event(
                                self,
                                &mut state,
                                operation_id,
                                &pending.method,
                                result,
                                error,
                            );
                        }
                    }
                }
                Some(PendingKind::Feature)
                    if pending_before.as_ref().is_some_and(|pending| {
                        pending.method == lithe_lsp_client::keymap_provider::IMPORT_METHOD
                    }) =>
                {
                    if let Some(pending) = pending_before.as_ref() {
                        if let Some(operation_id) = &pending.operation_id {
                            let (result, error) = if value.to_string().len()
                                > lithe_lsp_client::keymap_provider::MAX_REPLY_BYTES
                            {
                                (
                                    None,
                                    Some(runtime_error(
                                        self,
                                        "plugin.keymapImport.replyTooLarge",
                                        "request",
                                        Some(&pending.method),
                                        None,
                                        "The shortcut import provider reply is too large.",
                                        None,
                                        None,
                                    )),
                                )
                            } else {
                                (
                                    Some(
                                        json!({"value": {"id": value.get("id"), "response": value}}),
                                    ),
                                    None,
                                )
                            };
                            push_request_event(
                                self,
                                &mut state,
                                operation_id,
                                &pending.method,
                                result,
                                error,
                            );
                        }
                    }
                }
                Some(PendingKind::Feature)
                    if pending_before.as_ref().is_some_and(|pending| {
                        pending.method == lithe_lsp_client::toolchain_provider::QUERY_METHOD
                    }) =>
                {
                    if let Some(pending) = pending_before.as_ref() {
                        if let Some(operation_id) = &pending.operation_id {
                            let result =
                                lithe_lsp_client::toolchain_provider::reply_outcome(&value);
                            push_request_event(
                                self,
                                &mut state,
                                operation_id,
                                &pending.method,
                                Some(result),
                                None,
                            );
                        }
                    }
                }
                Some(PendingKind::Feature) => {
                    if let Some(pending) = pending_before.as_ref() {
                        if let Some(operation_id) = &pending.operation_id {
                            let event = reduced
                                .events
                                .iter()
                                .find(|event| event.request_id.as_ref() == response_id.as_ref());
                            let error =
                                event.and_then(|event| event.error.as_ref()).map(|detail| {
                                    runtime_error(
                                        self,
                                        "serverError",
                                        "request",
                                        Some(&pending.method),
                                        pending.document_uri.as_deref(),
                                        "Language server returned an error.",
                                        Some(detail),
                                        None,
                                    )
                                });
                            let result = event.and_then(|event| event.result.clone());
                            // A plugin-owned server marks locations that are not
                            // files read-only; answers from other servers pass through.
                            let result = if state.plugin_server.is_some() {
                                plugin_server::normalize_navigation_result(result)
                            } else {
                                result
                            };
                            push_request_event(
                                self,
                                &mut state,
                                operation_id,
                                &pending.method,
                                result,
                                error,
                            );
                        }
                    }
                }
                Some(PendingKind::PluginServer) => {
                    if let Some(pending) = pending_before.as_ref() {
                        if let Some(operation_id) = &pending.operation_id {
                            let (result, error) =
                                plugin_server_answer(self, &pending.method, &value);
                            push_request_event(
                                self,
                                &mut state,
                                operation_id,
                                &pending.method,
                                result,
                                error,
                            );
                        }
                    }
                }
                Some(PendingKind::Shutdown) => {
                    // The reducer emits `exit` only after the shutdown response.
                    state.shutdown_deadline = Some(Instant::now() + state.shutdown_timeout);
                }
                None => {}
            }

            for event in reduced.events {
                if event.kind == "notification" {
                    outbound.extend(observe_plugin_server_notification(
                        self,
                        &mut state,
                        event.method.as_deref(),
                        event.result.as_ref(),
                    ));
                }
                if event.kind == "semanticTokensRefresh" {
                    push_semantic_tokens_refresh_event(self, &mut state);
                } else if event.kind == "functionsChanged" {
                    let result = event.result.unwrap_or_default();
                    let removed: Vec<String> = result
                        .get("removed")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect();
                    outbound.extend(revoke_function_calls_locked(self, &mut state, &removed));
                    push_functions_event(self, &mut state, result);
                    relay_hub::observe_functions_changed(self);
                } else if event.kind == "functionRelayRequest" {
                    outbound.extend(self.relay.answer_request(
                        self,
                        &state,
                        event.method.as_deref().unwrap_or_default(),
                        &event.result.unwrap_or_default(),
                    )?);
                } else if event.kind == "functionProgress" {
                    push_function_progress_locked(self, &mut state, event.result);
                } else if event.kind == "buildProgress" {
                    push_build_progress_locked(self, &mut state, event.result);
                } else if event.kind == "hostRequest" {
                    outbound.extend(forward_host_request_locked(
                        self,
                        &mut state,
                        event.method.unwrap_or_default(),
                        event.result.unwrap_or_default(),
                    )?);
                } else if event.kind == lithe_lsp_client::vcs_provider::NEGOTIATED_EVENT {
                    push_vcs_provider_event(self, &mut state, event.result.unwrap_or_default());
                } else if event.kind == lithe_lsp_client::toolchain_provider::NEGOTIATED_EVENT {
                    push_negotiated_provider_event(
                        self,
                        &mut state,
                        event.result.unwrap_or_default(),
                        "toolchainProviders",
                    );
                } else if event.kind == lithe_lsp_client::keymap_provider::NEGOTIATED_EVENT {
                    push_negotiated_provider_event(
                        self,
                        &mut state,
                        event.result.unwrap_or_default(),
                        "keymapImportProviders",
                    );
                } else if event.kind == "diagnostics" {
                    if let Some(uri) = event.uri.as_deref() {
                        push_diagnostics_event(
                            self,
                            &mut state,
                            uri,
                            event.version,
                            event.diagnostics.unwrap_or_default(),
                        );
                    }
                } else if event.kind == "notification"
                    && event.method.as_deref() == Some(lithe_lsp_client::plugin_logging::METHOD)
                {
                    receive_plugin_diagnostic(self, &mut state, event.result.as_ref());
                } else if event.kind == "notification"
                    && self.plugin_id.is_some()
                    && event.method.as_deref() == Some("$/cancelRequest")
                    && cancel_host_request_locked(
                        self,
                        &mut state,
                        event.result.as_ref(),
                        &mut outbound,
                    )?
                {
                    // The backend withdrew one of its own host requests.
                } else if event.kind == "notification" {
                    let method = event.method.as_deref().unwrap_or("unknown");
                    let params = event.result.as_ref().unwrap_or(&Value::Null);
                    if method != "$/logTrace" || state.detailed_diagnostics {
                        let (identity, fields) = EngineLog::notification(method, params);
                        push_log_event(
                            self,
                            &mut state,
                            "info",
                            identity.with_fields(fields),
                            method,
                            event.result.map(|value| value.to_string()),
                        );
                    }
                }
            }
            if (state.client.server_capabilities != old_capabilities
                || lithe_lsp_client::completion_options(&state.client) != old_completion)
                && state.lifecycle == LspLifecycleState::Ready
            {
                let capabilities = state.client.server_capabilities.clone();
                push_features_event(self, &mut state, capabilities);
            }
        }

        if let Some((code, detail)) = fail_initialize {
            self.fail(
                &code,
                "initialize",
                "Language-server initialization failed.",
                detail,
                None,
            );
            self.kill_process();
            return Ok(());
        }
        if flush_documents {
            if let Some(configuration) = self.plugin_configuration() {
                outbound.push(plugin_configuration_notification(&configuration));
            }
            outbound.extend(self.flush_queued_documents()?);
            let diagnostics_enabled = self.lock_state()?.requested_diagnostics;
            if diagnostics_enabled {
                // LSP forbids setTrace before the initialize response. This batch
                // already starts with initialized, under the same outbound lock.
                outbound.push(trace_notification(true));
            }
            self.send_messages_or_fail(&outbound_order, outbound, "serverResponse")?;
            let mut state = self.lock_state()?;
            state.detailed_diagnostics = diagnostics_enabled;
            if let Some(info) = ready_server_info {
                push_server_info_event(self, &mut state, info);
            }
            push_log_event(
                self,
                &mut state,
                "info",
                EngineLog::ProtocolInitialized,
                "Language service protocol initialized",
                None,
            );
            transition_locked(self, &mut state, LspLifecycleState::Ready, None);
            let capabilities = state.client.server_capabilities.clone();
            push_features_event(self, &mut state, capabilities);
            return Ok(());
        }
        self.send_messages_or_fail(&outbound_order, outbound, "serverResponse")?;
        Ok(())
    }

    fn provider_configuration_response(
        &self,
        message: &Value,
    ) -> Result<Option<String>, CoreError> {
        let Some(id) = message.get("id") else {
            return Ok(None);
        };
        let Some(configuration) = self.plugin_configuration() else {
            return Ok(None);
        };
        // Each item asks for one section; `scopeUri` is ignored because a
        // session carries one configuration for its whole workspace.
        let values: Vec<Value> = message
            .get("params")
            .and_then(|params| params.get("items"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                configuration_section(&configuration, item.get("section").and_then(Value::as_str))
            })
            .collect();
        Ok(Some(
            serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": values
            }))
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::Unknown,
                    "Could not encode the provider configuration response.",
                )
                .with_details(error.to_string())
            })?,
        ))
    }

    fn flush_queued_documents(&self) -> Result<Vec<String>, CoreError> {
        let queued = {
            let state = self.lock_state()?;
            state
                .pending_workspace_file_changes
                .keys()
                .cloned()
                .chain(
                    state
                        .client
                        .open_documents
                        .values()
                        .filter(|document| document.version == 0)
                        .map(|document| document.uri.clone()),
                )
                .collect::<BTreeSet<_>>()
        };
        for uri in queued {
            if lithe_project::indexing::uri_is_excluded(&self.root_uri, &uri)? {
                let mut state = self.lock_state()?;
                state.pending_workspace_file_changes.remove(&uri);
                if state
                    .client
                    .open_documents
                    .get(&uri)
                    .is_some_and(|document| document.version == 0)
                {
                    state.client.open_documents.remove(&uri);
                }
            }
        }
        let mut state = self.lock_state()?;
        flush_queued_documents_locked(&mut state)
    }

    /// One session-owned writer sends deadline cancellations off the monitor thread.
    /// The monitor remains free to terminate a stalled write and expire other callers.
    fn spawn_outbound_maintenance(self: &Arc<Self>) {
        let session = self.clone();
        thread::spawn(move || {
            while session.active.load(Ordering::Acquire) {
                session.pump_outbound_maintenance();
                thread::sleep(Duration::from_millis(MONITOR_INTERVAL_MS));
            }
        });
    }

    fn pump_outbound_maintenance(&self) {
        if !self
            .lock_state()
            .is_ok_and(|state| !state.deadline_cancellations.is_empty())
        {
            return;
        }
        let Ok(outbound_order) = self.outbound_order.try_lock() else {
            return;
        };
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        let messages: Vec<String> = std::mem::take(&mut state.deadline_cancellations)
            .into_iter()
            .map(|id| {
                json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": id } })
                    .to_string()
            })
            .collect();
        if messages.is_empty() {
            return;
        }
        state.maintenance_write_deadline = Some(Instant::now() + state.request_timeout);
        drop(state);
        // Process termination by the monitor releases a blocked native stdin write.
        // log-exempt: Same failure already recorded: send_messages_or_fail transitions the session and emits its transport failure.
        let _ = self.send_messages_or_fail(&outbound_order, messages, "outboundMaintenance");
        if let Ok(mut state) = self.lock_state() {
            state.maintenance_write_deadline = None;
        }
    }

    fn expire_deadlines(&self) {
        self.expire_deadlines_at(Instant::now());
    }

    fn expire_deadlines_at(&self, now: Instant) {
        // Relayed calls that no callee runs yet have no session deadline.
        self.relay.expire(now);
        let mut cancellations = Vec::new();
        let mut initialize_timeout = false;
        let mut shutdown_timeout = false;
        let mut maintenance_write_timeout = false;
        let mut relay_write_timeout = false;
        if let Ok(mut state) = self.lock_state() {
            maintenance_write_timeout = state
                .maintenance_write_deadline
                .is_some_and(|deadline| now >= deadline);
            if maintenance_write_timeout {
                state.maintenance_write_deadline = None;
            }
            relay_write_timeout = state
                .relay_write_deadline
                .is_some_and(|deadline| now >= deadline);
            if relay_write_timeout {
                state.relay_write_deadline = None;
            }
            if state.lifecycle == LspLifecycleState::Initializing
                && state
                    .initialize_deadline
                    .is_some_and(|deadline| now >= deadline)
            {
                state.initialize_deadline = None;
                initialize_timeout = true;
                // Decided under the state lock, like the initialize response,
                // so exactly one of the two records the attempt.
                plugin_operations::finish(&self.startup_attempt, Some(AttemptStage::Timeout));
            }
            let expired: Vec<_> = state
                .pending
                .iter()
                .filter(|(_, pending)| {
                    matches!(
                        pending.kind,
                        PendingKind::Feature | PendingKind::PluginServer
                    ) && now >= pending.deadline
                })
                .map(|(id, _)| id.clone())
                .collect();
            for request_id in expired {
                let Some(pending) = state.pending.remove(&request_id) else {
                    continue;
                };
                state.client.pending_requests.remove(&request_id);
                state.client.pending_semantic_legends.remove(&request_id);
                if let Some(operation_id) = pending.operation_id.as_deref() {
                    state.request_by_operation.remove(operation_id);
                    state.function_by_operation.remove(operation_id);
                    let elapsed = now.saturating_duration_since(pending.created_at);
                    let error = runtime_error(
                        self,
                        "requestTimeout",
                        "request",
                        Some(&pending.method),
                        pending.document_uri.as_deref(),
                        "Language-server request timed out.",
                        Some(&format!("elapsedMilliseconds={}", elapsed.as_millis())),
                        None,
                    );
                    push_request_event(
                        self,
                        &mut state,
                        operation_id,
                        &pending.method,
                        None,
                        Some(error),
                    );
                }
                cancellations.push(request_id);
            }
            let held_expired = state
                .plugin_server
                .as_ref()
                .map(|server| server.expired_held(now))
                .unwrap_or_default();
            for waiter in held_expired {
                // Nothing was sent, so there is nothing to cancel on the wire.
                let error = runtime_error(
                    self,
                    "requestTimeout",
                    "request",
                    Some(&waiter.method),
                    None,
                    "Language-server request timed out waiting for the server to become ready.",
                    None,
                    None,
                );
                push_request_event(
                    self,
                    &mut state,
                    &waiter.operation_id,
                    &waiter.method,
                    None,
                    Some(error),
                );
            }
            if state.lifecycle == LspLifecycleState::Stopping
                && state
                    .shutdown_deadline
                    .is_some_and(|deadline| now >= deadline)
            {
                state.shutdown_deadline = None;
                shutdown_timeout = true;
                push_issue_log_event(
                    self,
                    &mut state,
                    "warning",
                    "shutdown",
                    "shutdownTimeout",
                    EngineLog::ShutdownTimedOut,
                    "Language-server shutdown timed out; forcing termination",
                    None,
                );
            }
        }
        if maintenance_write_timeout {
            self.fail(
                "transportFailed",
                "outboundMaintenance",
                "Language-server stdin write timed out.",
                None,
                None,
            );
            self.kill_process();
        } else if relay_write_timeout {
            // Killing the process releases the relay lane's blocked stdin write.
            self.fail(
                "transportFailed",
                "relayWrite",
                "Language-server stdin write timed out.",
                None,
                None,
            );
            self.kill_process();
        } else if initialize_timeout {
            // Timeout termination must not wait behind a blocked stdin write;
            // killing the process is what releases that write.
            self.fail(
                "initializeTimeout",
                "initialize",
                "Language-server initialization timed out.",
                None,
                None,
            );
            self.kill_process();
        } else if shutdown_timeout {
            self.kill_process();
        }
        if !cancellations.is_empty() {
            if let Ok(mut state) = self.lock_state() {
                if !matches!(
                    state.lifecycle,
                    LspLifecycleState::Stopped | LspLifecycleState::Failed
                ) {
                    state.deadline_cancellations.extend(cancellations);
                }
            }
        }
    }

    fn handle_process_exit(&self, exit_code: Option<i32>) {
        self.active.store(false, Ordering::Release);
        self.process.close_input();
        if let Ok(mut state) = self.lock_state() {
            let was_stopping = state.lifecycle == LspLifecycleState::Stopping;
            if !was_stopping {
                plugin_operations::finish(&self.startup_attempt, Some(AttemptStage::Exit));
            }
            if !state.terminal_event_emitted {
                let error = (!was_stopping).then(|| {
                    runtime_error(
                        self,
                        "serverExited",
                        "process",
                        None,
                        None,
                        "Language-server process exited.",
                        None,
                        exit_code,
                    )
                });
                fail_feature_requests(
                    self,
                    &mut state,
                    "serverExited",
                    "process",
                    "Language-server process exited before the request completed.",
                    exit_code,
                );
                clear_runtime_state(self, &mut state);
                transition_locked(
                    self,
                    &mut state,
                    if was_stopping {
                        LspLifecycleState::Stopped
                    } else {
                        LspLifecycleState::Failed
                    },
                    error,
                );
                state.terminal_event_emitted = true;
            }
        }
    }

    fn complete_request_with_error(
        &self,
        request_id: &str,
        code: &str,
        stage: &str,
        message: &str,
        underlying: Option<String>,
        exit_code: Option<i32>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            let Some(pending) = state.pending.remove(request_id) else {
                return;
            };
            state.client.pending_requests.remove(request_id);
            state.client.pending_semantic_legends.remove(request_id);
            if let Some(operation_id) = pending.operation_id.as_deref() {
                state.request_by_operation.remove(operation_id);
                state.function_by_operation.remove(operation_id);
                let error = runtime_error(
                    self,
                    code,
                    stage,
                    Some(&pending.method),
                    pending.document_uri.as_deref(),
                    message,
                    underlying.as_deref(),
                    exit_code,
                );
                push_request_event(
                    self,
                    &mut state,
                    operation_id,
                    &pending.method,
                    None,
                    Some(error),
                );
            }
        }
    }

    fn fail(
        &self,
        code: &str,
        stage: &str,
        message: &str,
        underlying: Option<String>,
        exit_code: Option<i32>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            if state.terminal_event_emitted {
                return;
            }
            // Timeouts were already taken by their caller; every other terminal
            // failure before `initialize` succeeded is a handshake failure.
            plugin_operations::finish(&self.startup_attempt, Some(AttemptStage::Initialize));
            fail_feature_requests(self, &mut state, code, stage, message, exit_code);
            clear_runtime_state(self, &mut state);
            let error = runtime_error(
                self,
                code,
                stage,
                None,
                None,
                message,
                underlying.as_deref(),
                exit_code,
            );
            transition_locked(self, &mut state, LspLifecycleState::Failed, Some(error));
            state.terminal_event_emitted = true;
        }
    }

    fn transition(
        &self,
        lifecycle: LspLifecycleState,
        error: Option<LspRuntimeError>,
    ) -> Result<(), CoreError> {
        let mut state = self.lock_state()?;
        transition_locked(self, &mut state, lifecycle, error);
        Ok(())
    }

    fn set_detailed_diagnostics(&self, enabled: bool) -> Result<(), CoreError> {
        let order = self.lock_outbound_order()?;
        let mut state = self.lock_state()?;
        let lifecycle = state.lifecycle;
        if matches!(
            lifecycle,
            LspLifecycleState::Stopped | LspLifecycleState::Failed | LspLifecycleState::Stopping
        ) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "The session is no longer running.",
            ));
        }
        if !state.client.initialized {
            state.requested_diagnostics = enabled;
            return Ok(());
        }
        drop(state);
        self.send_messages_or_fail(&order, vec![trace_notification(enabled)], "setTrace")?;
        let mut state = self.lock_state()?;
        state.requested_diagnostics = enabled;
        state.detailed_diagnostics = enabled;
        Ok(())
    }

    fn log(
        &self,
        level: &str,
        identity: impl Into<EngineLogInput>,
        message: &str,
        detail: Option<String>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            push_log_event(self, &mut state, level, identity, message, detail);
        }
    }

    fn log_issue(
        &self,
        level: &str,
        stage: &str,
        code: &str,
        identity: impl Into<EngineLogInput>,
        message: &str,
        detail: Option<String>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            push_issue_log_event(
                self, &mut state, level, stage, code, identity, message, detail,
            );
        }
    }

    fn send_messages_or_fail(
        &self,
        outbound_order: &MutexGuard<'_, ()>,
        messages: Vec<String>,
        stage: &str,
    ) -> Result<(), CoreError> {
        if messages.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.send_messages(outbound_order, messages) {
            self.fail(
                "transportFailed",
                stage,
                "Could not write to language-server stdin.",
                Some(core_error_detail(&error)),
                None,
            );
            self.kill_process();
            return Err(error);
        }
        Ok(())
    }

    fn send_messages(
        &self,
        _outbound_order: &MutexGuard<'_, ()>,
        messages: Vec<String>,
    ) -> Result<(), CoreError> {
        for message in messages {
            let message = match &self.workspace_host {
                Some(host) => {
                    lithe_lsp_transport::remote_process::map_message(host, &message, true, None)?
                }
                None => message,
            };
            let frame = frame_message(FrameMessageRequest { message })?.frame;
            self.process.write_input(frame.as_bytes())?;
        }
        Ok(())
    }

    fn kill_process(&self) {
        self.process.terminate();
        if let Some(error) = self.process.take_termination_failure() {
            // The session owns process recovery; WARN reports cleanup degradation.
            // Reuse the role routing and runtime envelope without adding a v1 log row.
            if let Ok(mut state) = self.lock_state() {
                let category = format!("{:?}", error.kind());
                let error = runtime_error(
                    self,
                    "processCleanupFailed",
                    "cleanup",
                    None,
                    None,
                    "Engine process cleanup failed",
                    None,
                    None,
                );
                let input = EngineLog::ProcessCleanupFailed.with_fields(json!({}));
                let (id, mut fields) =
                    input
                        .identity
                        .fact(&self.log_identity, &input.fields, Some(&error));
                fields.insert("error_code".into(), json!(category));
                fields.insert("error_domain".into(), json!("io"));
                push_cleanup_fact(self, &mut state, id, fields, error);
            }
        }
    }

    fn lock_outbound_order(&self) -> Result<MutexGuard<'_, ()>, CoreError> {
        self.outbound_order.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Language-server outbound ordering lock was poisoned.",
            )
        })
    }

    fn lock_indexing_roots(&self) -> Result<MutexGuard<'_, Vec<String>>, CoreError> {
        self.indexing_roots
            .lock()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "LSP indexing lock was poisoned"))
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, SessionState>, CoreError> {
        self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Language-server session state lock was poisoned.",
            )
        })
    }
}

fn trace_notification(enabled: bool) -> String {
    json!({"jsonrpc":"2.0", "method":"$/setTrace", "params":{"value": if enabled { "verbose" } else { "off" }}}).to_string()
}

fn validate_start_request(request: &StartServerRequest) -> Result<(), CoreError> {
    if request.provider_id.trim().is_empty() {
        return Err(invalid_field("providerId"));
    }
    // A host-owned server is no plugin's backend, and a service Core does not
    // know could never be forwarded.
    if !request.host_services.is_empty()
        && (request.plugin_id.is_none()
            || request
                .host_services
                .iter()
                .any(|service| !lithe_plugin_rpc::is_host_grant(service)))
    {
        return Err(invalid_field("hostServices"));
    }
    if let Some(policy) = &request.session_policy {
        policy.validate(request.plugin_id.as_deref())?;
    }
    if !request.host_options.is_empty()
        && (request.plugin_id.is_none()
            || request
                .host_options
                .editor
                .as_ref()
                .is_some_and(|editor| editor.validate().is_err()))
    {
        return Err(invalid_field("hostOptions"));
    }
    if request.executable_path.trim().is_empty()
        || request.executable_path.contains('\0')
        || request.working_directory.trim().is_empty()
        || request.working_directory.contains('\0')
    {
        return Err(invalid_field("executablePath/workingDirectory"));
    }
    if !request.root_uri.contains("://") || request.root_uri.contains('\0') {
        return Err(invalid_field("rootUri"));
    }
    Ok(())
}

fn invalid_field(field: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid language-server start request.",
    )
    .with_details(field)
}

fn core_error_detail(error: &CoreError) -> String {
    match error.details.as_deref() {
        Some(details) if !details.is_empty() => format!("{} ({details})", error.message),
        _ => error.message.clone(),
    }
}

fn unknown_session(session_id: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Unknown language-server session.",
    )
    .with_details(session_id)
}

fn ensure_not_terminal(lifecycle: LspLifecycleState) -> Result<(), CoreError> {
    if matches!(
        lifecycle,
        LspLifecycleState::Stopping | LspLifecycleState::Stopped | LspLifecycleState::Failed
    ) {
        Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Language-server session is not accepting document changes.",
        ))
    } else {
        Ok(())
    }
}

/// The file a `file:` URI resolves to once symbolic links are followed, or
/// `None` for other schemes and for files that do not exist.
fn resolved_file_of_uri(uri: &str) -> Option<PathBuf> {
    let path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
    lithe_workspace::fs::canonicalize(path).ok()
}

/// Which open documents the host reached through a symbolic link.
///
/// Only such a document can be the target of a diagnostics rewrite, so the
/// rest are remembered as examined and never resolved again while open.
#[derive(Default)]
struct LinkedDocumentPaths {
    /// Open documents whose file has already been resolved.
    examined: BTreeSet<String>,
    /// Resolved file of each examined document that differs from its own
    /// path, keyed by the document's URI.
    resolved: BTreeMap<String, PathBuf>,
}

impl LinkedDocumentPaths {
    /// Drops closed documents and resolves each newly opened one once.
    fn refresh(&mut self, open_documents: &BTreeMap<String, LspClientDocument>) {
        self.examined.retain(|uri| open_documents.contains_key(uri));
        self.resolved
            .retain(|uri, _| open_documents.contains_key(uri));
        for uri in open_documents.keys() {
            if self.examined.contains(uri) {
                continue;
            }
            let Some(resolved) = resolved_file_of_uri(uri) else {
                // Not on disk yet (an unsaved document); look again next time.
                continue;
            };
            self.examined.insert(uri.clone());
            let own = url::Url::parse(uri)
                .ok()
                .and_then(|url| url.to_file_path().ok());
            if own.as_ref() != Some(&resolved) {
                self.resolved.insert(uri.clone(), resolved);
            }
        }
    }
}

/// Re-addresses published diagnostics to the open document they describe.
///
/// A server reports a file at its resolved location while the host may have
/// opened it through a symbolic link (a server may answer `/private/tmp/...`
/// for a document opened as `/tmp/...` on macOS). The client reducer matches URIs
/// textually and would drop such diagnostics, so when no open document carries
/// the published URI this returns the message rewritten to the URI of the open
/// document that resolves to the same file. Returns `None` when the message
/// needs no change.
///
/// Servers publish for many files that are not open, so this runs on a hot
/// path. Only documents opened through a link can be rewrite targets, and
/// `linked` remembers those, which makes the common case free of file-system
/// calls and the linked case a single `canonicalize` of the published file.
fn diagnostics_addressed_to_open_document(
    open_documents: &BTreeMap<String, LspClientDocument>,
    linked: &mut LinkedDocumentPaths,
    message: &Value,
) -> Option<Value> {
    let published = message.get("params")?.get("uri")?.as_str()?;
    if open_documents.contains_key(published) {
        return None;
    }
    linked.refresh(open_documents);
    if linked.resolved.is_empty() {
        return None;
    }
    let target = resolved_file_of_uri(published)?;
    let document_uri = linked
        .resolved
        .iter()
        .find(|(_, resolved)| **resolved == target)
        .map(|(uri, _)| uri.clone())?;
    let mut message = message.clone();
    message["params"]["uri"] = Value::String(document_uri);
    Some(message)
}

/// The workspace directory with symbolic links resolved, when it exists.
///
/// Resolving links touches the file system, so it happens here in the engine
/// rather than in the pure normalizer that receives the result.
fn canonical_workspace_path(root_uri: &str) -> Option<PathBuf> {
    let path = url::Url::parse(root_uri).ok()?.to_file_path().ok()?;
    // Core policy identity uses the native canonical path, including Windows'
    // verbatim prefix. A protocol/display path is not a policy registry key.
    lithe_workspace::fs::canonicalize(path).ok()
}

fn canonical_workspace_root(root_uri: &str) -> Option<String> {
    let canonical = canonical_workspace_path(root_uri)?;
    let canonical = canonical.to_string_lossy().replace('\\', "/");
    // Windows canonical paths carry a verbatim `\\?\` prefix that language servers never report.
    Some(
        canonical
            .strip_prefix("//?/UNC/")
            .map(|rest| format!("//{rest}"))
            .or_else(|| canonical.strip_prefix("//?/").map(str::to_string))
            .unwrap_or(canonical),
    )
}

fn semantic_method(operation: LspSemanticOperation) -> &'static str {
    match operation {
        LspSemanticOperation::Completion => "textDocument/completion",
        LspSemanticOperation::Hover => "textDocument/hover",
        LspSemanticOperation::Definition => "textDocument/definition",
        LspSemanticOperation::Declaration => "textDocument/declaration",
        LspSemanticOperation::TypeDefinition => "textDocument/typeDefinition",
        LspSemanticOperation::References => "textDocument/references",
        LspSemanticOperation::Implementation => "textDocument/implementation",
        LspSemanticOperation::Rename => "textDocument/rename",
        LspSemanticOperation::Formatting => "textDocument/formatting",
        LspSemanticOperation::RangeFormatting => "textDocument/rangeFormatting",
        LspSemanticOperation::DocumentSymbols => "textDocument/documentSymbol",
        LspSemanticOperation::CodeActions => "textDocument/codeAction",
        LspSemanticOperation::ResolveCompletion => "completionItem/resolve",
        LspSemanticOperation::ResolveCodeAction => "codeAction/resolve",
        LspSemanticOperation::ExecuteCommand => "workspace/executeCommand",
        LspSemanticOperation::InlayHints => "textDocument/inlayHint",
        LspSemanticOperation::FoldingRanges => "textDocument/foldingRange",
        LspSemanticOperation::DocumentLinks => "textDocument/documentLink",
        LspSemanticOperation::WorkspaceSymbols => "workspace/symbol",
        LspSemanticOperation::SemanticTokens => "textDocument/semanticTokens/full",
        LspSemanticOperation::CodeLens => "textDocument/codeLens",
    }
}

fn semantic_capability(operation: LspSemanticOperation) -> Option<&'static str> {
    match operation {
        LspSemanticOperation::Completion => Some("completion"),
        LspSemanticOperation::Hover => Some("hover"),
        LspSemanticOperation::Definition => Some("definition"),
        LspSemanticOperation::Declaration => Some("declaration"),
        LspSemanticOperation::TypeDefinition => Some("typeDefinition"),
        LspSemanticOperation::References => Some("references"),
        LspSemanticOperation::Implementation => Some("implementation"),
        LspSemanticOperation::Rename => Some("rename"),
        LspSemanticOperation::Formatting => Some("formatting"),
        LspSemanticOperation::RangeFormatting => Some("rangeFormatting"),
        LspSemanticOperation::DocumentSymbols => Some("documentSymbols"),
        LspSemanticOperation::CodeActions => Some("codeActions"),
        LspSemanticOperation::ResolveCompletion => Some("completionResolve"),
        LspSemanticOperation::ResolveCodeAction => Some("codeActionResolve"),
        LspSemanticOperation::ExecuteCommand => Some("executeCommand"),
        LspSemanticOperation::InlayHints => Some("inlayHints"),
        LspSemanticOperation::FoldingRanges => Some("foldingRanges"),
        LspSemanticOperation::DocumentLinks => Some("documentLinks"),
        LspSemanticOperation::WorkspaceSymbols => Some("workspaceSymbols"),
        LspSemanticOperation::SemanticTokens => Some("semanticTokens"),
        LspSemanticOperation::CodeLens => Some("codeLens"),
    }
}

fn allocate_raw_request(
    mut state: LspClientState,
    method: &str,
    params: Value,
) -> Result<lithe_lsp_client::LspClientResponse, CoreError> {
    let id = state.next_request_id.to_string();
    state.next_request_id += 1;
    state
        .pending_requests
        .insert(id.clone(), method.to_string());
    let message = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    }))
    .map_err(|error| {
        CoreError::new(
            ErrorCode::Unknown,
            "Could not encode a language-server request.",
        )
        .with_details(error.to_string())
    })?;
    Ok(lithe_lsp_client::LspClientResponse {
        state,
        messages: vec![message],
        events: Vec::new(),
    })
}

fn lsp_value_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_server_info(message: &Value) -> Option<LspServerInfo> {
    let info = message.get("result")?.get("serverInfo")?;
    Some(LspServerInfo {
        name: info.get("name")?.as_str()?.to_string(),
        version: info
            .get("version")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

fn enqueue_runtime_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    mut event: LspRuntimeEvent,
) {
    if event.event.is_some() {
        let (id, fields) = (
            event.event.take().expect("source catalog identity"),
            event.fields.take().expect("source catalog fields"),
        );
        let level = if id == "log.dropped" {
            Some(lithe_diagnostics::record::LogLevel::Warn)
        } else {
            lithe_diagnostics::catalog::event_by_id(&id).map(|entry| entry.level)
        };
        // In-memory v1 detail is retained for old hosts. The v2 fields always pass
        // the same normalization and catalog routing boundary as persisted facts.
        // A fact that fails that boundary keeps only its v1 fields; the runtime
        // event itself is never dropped and Core never panics on source data.
        if let Some(level) = level {
            let mut fact = lithe_diagnostics::record::StructuredLogEvent {
                ts: String::new(),
                level,
                seq: 0,
                event: id.clone(),
                op: None,
                msg: String::new(),
                fields,
            };
            if let Ok(channel) = lithe_diagnostics::record::prepare_catalog_event(
                &mut fact,
                &json!({"channel": session.log_identity.channel()}),
            ) {
                event.channel = Some(channel);
                event.plugin_id = session.plugin_id.clone();
                event.role = Some(session.log_identity.role);
                event.event = Some(id);
                event.fields = Some(fact.fields);
                event.catalog_level = Some(fact.level);
            }
        }
    }
    state.events.push_back(event);
    session.event_signal.notify_all();
}

// Cleanup failure is a log row, never a state change: hosts take the latest
// `stateChanged` as the session state, so a stateless one would erase `failed`.
fn push_cleanup_fact(
    session: &RuntimeSession,
    state: &mut SessionState,
    id: String,
    fields: std::collections::BTreeMap<String, Value>,
    error: LspRuntimeError,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "log".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: Some(id),
            fields: Some(fields),
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: None,
            error: Some(error),
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: Some("warning".to_string()),
            message: Some("Engine process cleanup failed".to_string()),
            detail: None,
        },
    );
}

// Only a declarative language server carries a host attempt. SDK backends report
// their own operations through `lithe/log`, so a stray handoff on another role is
// ignored rather than rejecting the start. The handoff names its plugin, since a
// plugin API 1 server starts with no session plugin; a session another plugin
// owns never takes the attempt, so no fact lands on a plugin that did not run.
fn startup_attempt(request: &StartServerRequest) -> AttemptSlot {
    match &request.startup_operation {
        Some(handoff)
            if request.role == SessionRole::LanguageServer
                && request
                    .plugin_id
                    .as_ref()
                    .is_none_or(|owner| *owner == handoff.plugin_id) =>
        {
            Attempt::slot(&LANGUAGE_SERVER_START, handoff.clone())
        }
        _ => Attempt::none(),
    }
}

fn transition_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    lifecycle: LspLifecycleState,
    error: Option<LspRuntimeError>,
) {
    state.lifecycle = lifecycle;
    relay_hub::observe_lifecycle(session, lifecycle);
    let (id, fields) = EngineLog::SessionUpdated.fact(
        &session.log_identity,
        &json!({"state": lifecycle}),
        error.as_ref(),
    );
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "stateChanged".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: Some(id),
            fields: Some(fields),
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: Some(lifecycle),
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: None,
            error,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

fn push_request_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    operation_id: &str,
    method: &str,
    result: Option<Value>,
    error: Option<LspRuntimeError>,
) {
    if state
        .plugin_server
        .as_ref()
        .is_some_and(|server| server.owns(operation_id))
    {
        complete_plugin_server_request(session, state, operation_id, method, result, error);
        return;
    }
    // No host operation waits for a relayed call: the relay answers the
    // calling plugin, so the event never reaches the host.
    if relay_hub::is_relayed_operation(operation_id, method) {
        session.relay.callee_finished(
            &session.id,
            operation_id,
            relay_hub::callee_outcome(result, error),
        );
        return;
    }
    enqueue_request_completed(session, state, operation_id, method, result, error);
}

/// Ends a request the backend sent to its plugin-owned language server. This
/// is the only place where the session policy releases such an operation, so
/// every way a request can end (answer, cancel, timeout, session failure)
/// leaves the policy consistent. Requests that joined a running command
/// finish with the same result or error.
fn complete_plugin_server_request(
    session: &RuntimeSession,
    state: &mut SessionState,
    operation_id: &str,
    method: &str,
    result: Option<Value>,
    mut error: Option<LspRuntimeError>,
) {
    if let Some(error) = error.as_mut() {
        error.code = plugin_server::host_error_code(&error.code).to_string();
    }
    let joined = state
        .plugin_server
        .as_mut()
        .map(|server| server.finish(operation_id))
        .unwrap_or_default();
    for waiter in joined {
        enqueue_request_completed(
            session,
            state,
            &waiter.operation_id,
            &waiter.method,
            result.clone(),
            error.clone(),
        );
    }
    enqueue_request_completed(session, state, operation_id, method, result, error);
}

fn enqueue_request_completed(
    session: &RuntimeSession,
    state: &mut SessionState,
    operation_id: &str,
    method: &str,
    result: Option<Value>,
    error: Option<LspRuntimeError>,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "requestCompleted".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: Some(operation_id.to_string()),
            method: Some(method.to_string()),
            uri: None,
            version: None,
            diagnostics: None,
            result,
            error,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

/// Resolves a function mapping only while both pending tables still own its exact request.
fn pending_function_request<'a>(
    state: &'a SessionState,
    operation_id: &str,
    call: &PendingFunctionCall,
) -> Option<&'a PendingRequest> {
    if state.request_by_operation.get(operation_id) != Some(&call.request_id)
        || state
            .client
            .pending_requests
            .get(&call.request_id)
            .map(String::as_str)
            != Some(lithe_plugin_rpc::INVOKE_METHOD)
    {
        return None;
    }
    state.pending.get(&call.request_id).filter(|pending| {
        pending.kind == PendingKind::Feature
            && pending.method == lithe_plugin_rpc::INVOKE_METHOD
            && pending.operation_id.as_deref() == Some(operation_id)
    })
}

/// Ends every running call of the withdrawn functions with
/// `plugin.function.revoked` and returns the `$/cancelRequest` messages
/// telling the backend to stop. A reply that still arrives is late and is
/// ignored like any reply to a request that is no longer pending.
fn revoke_function_calls_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    removed: &[String],
) -> Vec<String> {
    let revoked: Vec<String> = state
        .function_by_operation
        .iter()
        .filter(|(operation, call)| {
            removed.contains(&call.function_id)
                && pending_function_request(state, operation, call).is_some()
        })
        .map(|(operation, _)| operation.clone())
        .collect();
    let mut cancellations = Vec::new();
    for operation_id in revoked {
        state.function_by_operation.remove(&operation_id);
        let Some(request_id) = state.request_by_operation.remove(&operation_id) else {
            continue;
        };
        state.pending.remove(&request_id);
        state.client.pending_requests.remove(&request_id);
        let failure = lithe_plugin_rpc::FunctionFailure {
            code: lithe_plugin_rpc::codes::REVOKED,
            message: "The plugin withdrew this function while it was running.".to_string(),
            plugin_error_code: None,
        };
        let error = function_runtime_error(session, failure);
        push_request_event(
            session,
            state,
            &operation_id,
            lithe_plugin_rpc::INVOKE_METHOD,
            None,
            Some(error),
        );
        cancellations.push(
            json!({
                "jsonrpc": "2.0",
                "method": "$/cancelRequest",
                "params": { "id": request_id }
            })
            .to_string(),
        );
    }
    cancellations
}

/// A wire request ID identifies one exact build operation in this session.
/// Cancellation/completion removes it from pending, so late progress is inert.
fn push_build_progress_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    progress: Option<Value>,
) {
    let Some(mut progress) = progress else {
        return;
    };
    let Some(id) = progress.get("operationID").and_then(Value::as_str) else {
        return;
    };
    let Some(pending) = state.pending.get(id) else {
        return;
    };
    if !lithe_lsp_client::build_provider::is_progress_method(&pending.method) {
        return;
    }
    let Some(operation_id) = pending.operation_id.clone() else {
        return;
    };
    progress["operationID"] = json!(operation_id);
    let method = pending.method.clone();
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "requestProgress".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: Some(operation_id),
            method: Some(method),
            uri: None,
            version: None,
            diagnostics: None,
            result: Some(progress),
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

/// Publishes a progress report of a call that is still running. Reports for
/// an unknown, finished, cancelled, or revoked operation are dropped, so a
/// late report never revives an ended call in the host.
fn push_function_progress_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    progress: Option<Value>,
) {
    let Some(mut progress) = progress else {
        return;
    };
    let Some(wire_operation_id) = progress.get("operationID").and_then(Value::as_str) else {
        return;
    };
    let Some(operation_id) = state
        .function_by_operation
        .iter()
        .find_map(|(operation, call)| {
            (call.request_id == wire_operation_id
                && pending_function_request(state, operation, call).is_some())
            .then(|| operation.clone())
        })
    else {
        return;
    };
    progress["operationID"] = json!(operation_id);
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "requestProgress".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: Some(operation_id),
            method: Some(lithe_plugin_rpc::INVOKE_METHOD.to_string()),
            uri: None,
            version: None,
            diagnostics: None,
            result: Some(progress),
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

/// Publishes the VCS and capabilities a provider negotiated, so the host can
/// check the VCS against the manifest entry and decide which actions to show.
fn push_vcs_provider_event(session: &RuntimeSession, state: &mut SessionState, result: Value) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "vcsProvider".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: Some(result),
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

/// Publishes the providers a backend selected, so the host queries only those.
fn push_negotiated_provider_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    result: Value,
    kind: &str,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: kind.to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: Some(result),
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

/// Handles a backend's `$/cancelRequest` for one of its own pending host
/// requests: the backend gets `plugin.host.cancelled` at once, so the
/// pending slot frees without waiting for the host, and the host gets a
/// `hostRequestCancelled` event naming the `hostRequestID` so it can stop
/// the work. A later answer from the host for that ID is refused like any
/// unknown ID. Returns false when the notification names no pending host
/// request, so it is handled like any other notification.
fn cancel_host_request_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    params: Option<&Value>,
    outbound: &mut Vec<String>,
) -> Result<bool, CoreError> {
    if state.client.plugin_function_version.is_none() {
        return Ok(false);
    }
    let Some(backend_id) = params.and_then(|params| params.get("id")) else {
        return Ok(false);
    };
    let Some(host_request_id) = state
        .host_requests
        .iter()
        .find(|(_, pending)| *pending == backend_id)
        .map(|(host_request_id, _)| host_request_id.clone())
    else {
        // Not a host request: it may name a call the backend relayed to
        // another plugin.
        return Ok(session.relay.cancel_from_caller(&session.id, backend_id));
    };
    let backend_id = state
        .host_requests
        .remove(&host_request_id)
        .unwrap_or(Value::Null);
    outbound.push(json_rpc_error_with_code(
        &backend_id,
        -32800,
        "The plugin process cancelled this host-service request.",
        lithe_plugin_rpc::codes::HOST_CANCELLED,
    )?);
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "hostRequestCancelled".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: Some(json!({ "hostRequestID": host_request_id })),
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
    Ok(true)
}

/// Publishes the session's registered functions after a registry change.
/// Publishes a backend's well-formed host-service request as `hostRequest`,
/// or answers it at once when the session already has the most requests the
/// host may owe it. Returns the messages to send to the backend.
fn forward_host_request_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    method: String,
    request: Value,
) -> Result<Vec<String>, CoreError> {
    let backend_id = request.get("id").cloned().unwrap_or(Value::Null);
    if state.host_requests.len() >= lithe_plugin_rpc::MAX_PENDING_HOST_REQUESTS {
        let code = lithe_plugin_rpc::codes::HOST_LIMIT_EXCEEDED;
        return Ok(vec![json_rpc_error_with_code(
            &backend_id,
            lithe_plugin_rpc::host_rpc_code(code),
            "Too many host-service requests are waiting for the host.",
            code,
        )?]);
    }
    let host_request_id = format!("host-{}", state.next_host_request_id);
    state.next_host_request_id += 1;
    state
        .host_requests
        .insert(host_request_id.clone(), backend_id);
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "hostRequest".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: Some(method),
            uri: None,
            version: None,
            diagnostics: None,
            result: Some(json!({
                "hostRequestID": host_request_id,
                "service": request.get("service").cloned().unwrap_or(Value::Null),
                "params": request.get("params").cloned().unwrap_or(Value::Null),
            })),
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
    Ok(Vec::new())
}

fn push_functions_event(session: &RuntimeSession, state: &mut SessionState, result: Value) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "functionsChanged".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: Some(result),
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

fn function_runtime_error(
    session: &RuntimeSession,
    failure: lithe_plugin_rpc::FunctionFailure,
) -> LspRuntimeError {
    let mut error = runtime_error(
        session,
        failure.code,
        "request",
        Some(lithe_plugin_rpc::INVOKE_METHOD),
        None,
        &failure.message,
        None,
        None,
    );
    error.plugin_error_code = failure.plugin_error_code;
    error
}

/// The plugin-owned server a call targets, after checking that the session
/// is that contribution and can take requests now.
fn plugin_server_target<'a>(
    state: &'a mut SessionState,
    session_plugin_id: Option<&str>,
    plugin_id: &str,
    server_id: &str,
) -> Result<&'a mut PluginServerState, CoreError> {
    let ready = state.lifecycle == LspLifecycleState::Ready && state.client.initialized;
    let server = state
        .plugin_server
        .as_mut()
        .ok_or_else(plugin_server::foreign_contribution)?;
    server.check_owner(session_plugin_id, plugin_id, server_id)?;
    if !ready {
        return Err(plugin_server::unavailable());
    }
    Ok(server)
}

fn plugin_server_timeout(requested: Option<u64>) -> Duration {
    let milliseconds = requested
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MILLISECONDS)
        .clamp(1, MAX_REQUEST_TIMEOUT_MILLISECONDS);
    Duration::from_millis(milliseconds)
}

/// Allocates the JSON-RPC request of an admitted operation and records it as
/// pending. Returns the messages to write and the request ID.
fn register_plugin_server_request(
    state: &mut SessionState,
    operation_id: &str,
    method: &str,
    params: Value,
    created_at: Instant,
    deadline: Instant,
) -> Result<(Vec<String>, String), CoreError> {
    let response = allocate_raw_request(state.client.clone(), method, params)?;
    let request_id = (response.state.next_request_id - 1).to_string();
    state.client = response.state;
    state.pending.insert(
        request_id.clone(),
        PendingRequest {
            kind: PendingKind::PluginServer,
            operation_id: Some(operation_id.to_string()),
            method: method.to_string(),
            document_uri: None,
            document_version: None,
            created_at,
            deadline,
        },
    );
    state
        .request_by_operation
        .insert(operation_id.to_string(), request_id.clone());
    Ok((response.messages, request_id))
}

/// Applies a notification the server sent to a plugin-owned session: it may
/// open gates, and the backend may have asked to receive it. Returns the
/// messages of the requests that waited for those gates.
fn observe_plugin_server_notification(
    session: &RuntimeSession,
    state: &mut SessionState,
    method: Option<&str>,
    params: Option<&Value>,
) -> Vec<String> {
    let (Some(method), Some(server)) = (method, state.plugin_server.as_mut()) else {
        return Vec::new();
    };
    let observation = server.observe(method, params.unwrap_or(&Value::Null));
    if let Some(forwarded) = observation.forward {
        push_negotiated_provider_event(session, state, forwarded, "pluginServerNotification");
    }
    observation
        .released
        .into_iter()
        .flat_map(|held| release_held_plugin_server_request(session, state, held))
        .collect()
}

/// Sends a request whose gates opened. It keeps the deadline it was
/// admitted with, so the time spent waiting counts against its timeout.
fn release_held_plugin_server_request(
    session: &RuntimeSession,
    state: &mut SessionState,
    held: plugin_server::HeldRequest,
) -> Vec<String> {
    let registered = register_plugin_server_request(
        state,
        &held.operation_id,
        &held.method,
        held.params,
        held.created_at,
        held.deadline,
    );
    match registered {
        Ok((messages, _)) => messages,
        Err(error) => {
            let failure = runtime_error(
                session,
                "transportFailed",
                "request",
                Some(&held.method),
                None,
                "Could not encode the language-server request.",
                Some(&error.message),
                None,
            );
            push_request_event(
                session,
                state,
                &held.operation_id,
                &held.method,
                None,
                Some(failure),
            );
            Vec::new()
        }
    }
}

/// Cancels a plugin-server request that never reached the server, because it
/// waits behind a gate or joined a running command. Nothing is written.
fn cancel_unsent_plugin_server_request(
    session: &RuntimeSession,
    state: &mut SessionState,
    operation_id: &str,
) {
    let method = state
        .plugin_server
        .as_ref()
        .and_then(|server| server.method_of(operation_id))
        .unwrap_or_default()
        .to_string();
    let error = runtime_error(
        session,
        "requestCancelled",
        "request",
        Some(&method),
        None,
        "Language-server request was cancelled.",
        None,
        None,
    );
    push_request_event(session, state, operation_id, &method, None, Some(error));
}

/// What the backend receives for a plugin-owned server's reply: the result
/// untouched, so no Lithe normalizer reinterprets the server's own protocol,
/// or the server's error code and message as details of
/// `plugin.host.languageServerError`.
fn plugin_server_answer(
    session: &RuntimeSession,
    method: &str,
    reply: &Value,
) -> (Option<Value>, Option<LspRuntimeError>) {
    use lithe_plugin_rpc::{bounded_host_message, codes, MAX_RESULT_BYTES};
    let failure = |code: &str, message: &str| {
        runtime_error(
            session,
            code,
            "request",
            Some(method),
            None,
            message,
            None,
            None,
        )
    };
    if let Some(upstream) = reply.get("error") {
        let mut error = failure(
            codes::HOST_LANGUAGE_SERVER_ERROR,
            "The language server returned an error.",
        );
        let message = upstream
            .get("message")
            .and_then(Value::as_str)
            .map(bounded_host_message);
        error.details = Some(json!({ "code": upstream.get("code"), "message": message }));
        return (None, Some(error));
    }
    let result = reply.get("result").cloned().unwrap_or(Value::Null);
    let size = serde_json::to_vec(&result).map_or(usize::MAX, |bytes| bytes.len());
    if size > MAX_RESULT_BYTES {
        let error = failure(
            codes::HOST_LIMIT_EXCEEDED,
            "The language server's result is too large.",
        );
        return (None, Some(error));
    }
    (Some(json!({ "value": result })), None)
}

fn cancel_stale_document_requests_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    uri: &str,
    document_version: i64,
) -> Vec<String> {
    let stale_ids: Vec<String> = state
        .pending
        .iter()
        .filter(|(_, pending)| {
            pending.document_uri.as_deref() == Some(uri)
                && pending
                    .document_version
                    .is_some_and(|version| version < document_version)
                && is_stale_sensitive_method(&pending.method)
        })
        .map(|(request_id, _)| request_id.clone())
        .collect();
    let mut messages = Vec::new();
    for request_id in stale_ids {
        let Some(pending) = state.pending.remove(&request_id) else {
            continue;
        };
        state.client.pending_requests.remove(&request_id);
        state.client.pending_semantic_legends.remove(&request_id);
        if let Some(operation_id) = pending.operation_id.as_deref() {
            state.request_by_operation.remove(operation_id);
            state.function_by_operation.remove(operation_id);
            let error = runtime_error(
                session,
                "staleDocumentVersion",
                "request",
                Some(&pending.method),
                Some(uri),
                "The document changed before the language-server result arrived.",
                Some("A newer document version superseded this request."),
                None,
            );
            push_request_event(
                session,
                state,
                operation_id,
                &pending.method,
                None,
                Some(error),
            );
        }
        messages.push(
            json!({
                "jsonrpc": "2.0",
                "method": "$/cancelRequest",
                "params": { "id": request_id }
            })
            .to_string(),
        );
    }
    messages
}

fn is_stale_sensitive_method(method: &str) -> bool {
    matches!(
        method,
        "textDocument/completion"
            | "textDocument/hover"
            | "textDocument/inlayHint"
            | "textDocument/foldingRange"
            | "textDocument/codeLens"
            | "codeLens/resolve"
            | "textDocument/implementation"
    )
}

fn push_diagnostics_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    uri: &str,
    version: Option<i64>,
    diagnostics: Vec<LspClientDiagnostic>,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "diagnostics".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: Some(uri.to_string()),
            version,
            diagnostics: Some(diagnostics),
            result: None,
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

fn push_semantic_tokens_refresh_event(session: &RuntimeSession, state: &mut SessionState) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "semanticTokensRefresh".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: None,
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

fn push_features_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    capabilities: Vec<String>,
) {
    let completion_options = lithe_lsp_client::completion_options(&state.client);
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "featuresChanged".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: None,
            error: None,
            capabilities: Some(capabilities),
            completion_options,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

fn push_server_info_event(session: &RuntimeSession, state: &mut SessionState, info: LspServerInfo) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "serverInfoChanged".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: None,
            fields: None,
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: None,
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: Some(info),
            level: None,
            message: None,
            detail: None,
        },
    );
}

/// Accepts only negotiated scalar plugin facts. Persistence and overload accounting
/// reuse the bounded Core handoff instead of copying protocol params into v1 detail.
fn receive_plugin_diagnostic(
    session: &RuntimeSession,
    state: &mut SessionState,
    params: Option<&Value>,
) {
    let Some(plugin) = session.plugin_id.as_deref() else {
        return;
    };
    let plugin_identity = SessionLogIdentity::new(
        SessionRole::Backend,
        &session.provider_id,
        Some(plugin),
        None,
    );
    let plugin = plugin_identity.segment();
    let valid = state
        .client
        .server_capabilities
        .iter()
        .any(|name| name == lithe_lsp_client::plugin_logging::FEATURE)
        .then(|| {
            lithe_lsp_client::plugin_logging::decode(
                plugin,
                &state.client.plugin_functions,
                params.unwrap_or(&Value::Null),
            )
        })
        .flatten();
    let Some(fact) = valid else {
        if !state.rejected_plugin_diagnostics {
            state.rejected_plugin_diagnostics = true;
            if lithe_diagnostics::core_events::record(
                "plugins.diagnostics.rejected",
                BTreeMap::from([
                    ("plugin".into(), json!(plugin)),
                    ("code".into(), json!("invalid_diagnostic")),
                ]),
                None,
            )
            .is_err()
            {
                lithe_diagnostics::core_events::note_dropped();
            }
        }
        return;
    };
    if state.plugin_diagnostics_window.elapsed() >= Duration::from_secs(60) {
        state.plugin_diagnostics_window = Instant::now();
        state.plugin_diagnostics_count = 0;
    }
    if state.plugin_diagnostics_count >= MAX_PLUGIN_DIAGNOSTICS_PER_MINUTE {
        lithe_diagnostics::core_events::note_dropped();
        return;
    }
    state.plugin_diagnostics_count += 1;
    // SDK contexts carry wire request IDs, not reusable UI operation IDs. Only
    // a live request owned by this session may extend the host's correlation.
    let operation = fact.wire_operation.as_deref().and_then(|wire| {
        state
            .pending
            .get(wire)
            .and_then(|pending| pending.operation_id.clone())
    });
    if lithe_diagnostics::core_events::record(fact.event, fact.fields, operation).is_err() {
        lithe_diagnostics::core_events::note_dropped();
    }
}

fn push_log_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    level: &str,
    identity: impl Into<EngineLogInput>,
    message: &str,
    detail: Option<String>,
) {
    push_classified_log_event(session, state, level, identity, message, detail, None);
}

fn push_issue_log_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    level: &str,
    stage: &str,
    code: &str,
    identity: impl Into<EngineLogInput>,
    message: &str,
    detail: Option<String>,
) {
    let error = runtime_error(session, code, stage, None, None, message, None, None);
    push_classified_log_event(
        session,
        state,
        level,
        identity,
        message,
        detail,
        Some(error),
    );
}

fn push_classified_log_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    level: &str,
    identity: impl Into<EngineLogInput>,
    message: &str,
    detail: Option<String>,
    error: Option<LspRuntimeError>,
) {
    let input = identity.into();
    let (event_id, fields) =
        input
            .identity
            .fact(&session.log_identity, &input.fields, error.as_ref());
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "log".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: Some(event_id),
            fields: Some(fields),
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: None,
            operation_id: None,
            method: None,
            uri: None,
            version: None,
            diagnostics: None,
            result: None,
            error,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: Some(level.to_string()),
            message: Some(message.to_string()),
            detail: detail.filter(|value| !value.is_empty()),
        },
    );
}

fn take_sequence(state: &mut SessionState) -> u64 {
    let sequence = state.next_sequence;
    state.next_sequence += 1;
    sequence
}

fn runtime_error(
    session: &RuntimeSession,
    code: &str,
    stage: &str,
    method: Option<&str>,
    document_uri: Option<&str>,
    message: &str,
    underlying: Option<&str>,
    process_exit_code: Option<i32>,
) -> LspRuntimeError {
    LspRuntimeError {
        code: code.to_string(),
        provider_id: session.provider_id.clone(),
        session_id: session.id.clone(),
        stage: stage.to_string(),
        method: method.map(ToString::to_string),
        document_uri: document_uri.map(ToString::to_string),
        message: message.to_string(),
        underlying_message: underlying.map(ToString::to_string),
        process_exit_code,
        plugin_error_code: None,
        details: None,
    }
}

fn fail_feature_requests(
    session: &RuntimeSession,
    state: &mut SessionState,
    code: &str,
    stage: &str,
    message: &str,
    exit_code: Option<i32>,
) {
    let pending: Vec<_> = state
        .pending
        .iter()
        .filter(|(_, pending)| {
            matches!(
                pending.kind,
                PendingKind::Feature | PendingKind::PluginServer
            )
        })
        .map(|(request_id, pending)| (request_id.clone(), pending.clone()))
        .collect();
    for (request_id, pending) in pending {
        state.pending.remove(&request_id);
        state.client.pending_requests.remove(&request_id);
        state.client.pending_semantic_legends.remove(&request_id);
        if let Some(operation_id) = pending.operation_id.as_deref() {
            state.request_by_operation.remove(operation_id);
            state.function_by_operation.remove(operation_id);
            let error = runtime_error(
                session,
                code,
                stage,
                Some(&pending.method),
                pending.document_uri.as_deref(),
                message,
                None,
                exit_code,
            );
            push_request_event(
                session,
                state,
                operation_id,
                &pending.method,
                None,
                Some(error),
            );
        }
    }
    // Requests held behind a gate have no pending entry, so they end here.
    let held = state
        .plugin_server
        .as_ref()
        .map(|server| server.all_held())
        .unwrap_or_default();
    for waiter in held {
        let error = runtime_error(
            session,
            code,
            stage,
            Some(&waiter.method),
            None,
            message,
            None,
            exit_code,
        );
        push_request_event(
            session,
            state,
            &waiter.operation_id,
            &waiter.method,
            None,
            Some(error),
        );
    }
}

fn clear_runtime_diagnostics(session: &RuntimeSession, state: &mut SessionState) {
    let diagnostics: Vec<_> = state.client.diagnostics.keys().cloned().collect();
    state.client.diagnostics.clear();
    state.client.diagnostic_versions.clear();
    for uri in diagnostics {
        push_diagnostics_event(session, state, &uri, None, Vec::new());
    }
}

/// Withdraws the session registry when shutdown or process failure ends its lifetime.
fn clear_function_registry(session: &RuntimeSession, state: &mut SessionState) {
    let removed: Vec<_> = std::mem::take(&mut state.client.plugin_functions)
        .into_iter()
        .collect();
    if !removed.is_empty() {
        push_functions_event(
            session,
            state,
            json!({ "functions": [], "removed": removed }),
        );
    }
}

fn clear_runtime_state(session: &RuntimeSession, state: &mut SessionState) {
    clear_runtime_diagnostics(session, state);
    state.client.initialized = false;
    state.client.text_document_save_include_text = None;
    state.client.shutdown_requested = false;
    state.client.server_capabilities.clear();
    state.client.completion_providers.clear();
    state.client.open_documents.clear();
    state.client.pending_requests.clear();
    state.pending.clear();
    state.request_by_operation.clear();
    state.function_by_operation.clear();
    clear_function_registry(session, state);
    state.client.vcs_provider = None;
    state.client.toolchain_providers.clear();
    state.client.keymap_import_providers.clear();
    state.linked_document_paths = LinkedDocumentPaths::default();
    state.deadline_cancellations.clear();
    state.maintenance_write_deadline = None;
    state.relay_write_deadline = None;
    state.pending_workspace_file_changes.clear();
    state.initialize_deadline = None;
    state.shutdown_deadline = None;
    push_features_event(session, state, Vec::new());
}

/// Resolves the text of a document that has not been opened on the wire yet.
///
/// Hosts may send only ranged edits, which then extend the text Core retained.
/// Edits for a document Core never received cannot be resolved, so the host
/// resends the full text.
fn unopened_document_text(
    stored: Option<&LspClientDocument>,
    text: String,
    changes: &[LspDocumentContentChange],
) -> Result<String, CoreError> {
    if !text.is_empty() || changes.is_empty() {
        return Ok(text);
    }
    let Some(stored) = stored else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Cannot apply ranged edits to a document Core has not received.",
        )
        .with_details("documentNotSynchronized"));
    };
    lithe_text::apply_content_changes_sequential(&stored.text, changes)
}

fn flush_queued_documents_locked(state: &mut SessionState) -> Result<Vec<String>, CoreError> {
    let queued: Vec<_> = state
        .client
        .open_documents
        .values()
        .filter(|document| document.version == 0)
        .cloned()
        .collect();
    let mut messages = Vec::new();
    for document in queued {
        let response = client_open_document(ClientOpenDocumentRequest {
            state: state.client.clone(),
            uri: document.uri,
            language_id: document.language_id,
            text: document.text.to_string(),
        })?;
        state.client = response.state;
        messages.extend(response.messages);
    }
    if !state.pending_workspace_file_changes.is_empty() {
        let changes = std::mem::take(&mut state.pending_workspace_file_changes);
        messages.push(workspace_file_changes_notification(changes));
    }
    Ok(messages)
}

fn workspace_file_changes_notification(
    changes: BTreeMap<String, WorkspaceFileChangeKind>,
) -> String {
    let changes: Vec<_> = changes
        .into_iter()
        .map(|(uri, kind)| {
            let event_type = match kind {
                WorkspaceFileChangeKind::Created => 1,
                WorkspaceFileChangeKind::Changed => 2,
                WorkspaceFileChangeKind::Deleted => 3,
            };
            json!({ "uri": uri, "type": event_type })
        })
        .collect();
    json!({
        "jsonrpc": "2.0",
        "method": "workspace/didChangeWatchedFiles",
        "params": { "changes": changes }
    })
    .to_string()
}

#[cfg(test)]
mod function_relay_tests;

#[cfg(test)]
mod tests;
