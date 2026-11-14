//! Request, response and event payloads of the `lsp.*` commands.

use super::*;

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

fn default_initialize_timeout() -> u64 {
    DEFAULT_INITIALIZE_TIMEOUT_MS
}

fn default_request_timeout() -> u64 {
    DEFAULT_REQUEST_TIMEOUT_MS
}

fn default_shutdown_timeout() -> u64 {
    DEFAULT_SHUTDOWN_TIMEOUT_MS
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

/// Host-owned toggle for one session's upstream trace diagnostics.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetDetailedDiagnosticsRequest {
    /// Opaque session returned by startServer.
    pub session_id: String,
    /// True sends verbose tracing, false sends off; default sessions start disabled.
    pub enabled: bool,
}
