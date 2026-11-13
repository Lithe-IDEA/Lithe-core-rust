//! Serializable client state and wire models for the generic LSP implementation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use lithe_text::{
    LspDocumentContentChange, LspPosition, LspPositionResponse, LspRange, LspRangeResponse,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Normalized inlay hint independent of provider-specific extensions.
pub struct LspInlayHintResponse {
    pub(crate) position: LspPositionResponse,
    pub(crate) label: String,
    /// Numeric LSP `InlayHintKind`, when supplied by the server.
    pub kind: Option<i64>,
    pub(crate) tooltip: Option<String>,
    pub(crate) padding_left: bool,
    pub(crate) padding_right: bool,
    pub(crate) text_edits: Vec<Value>,
    pub(crate) data: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Normalized folding range using UTF-16 columns when supplied by the server.
pub struct LspFoldingRangeResponse {
    pub(crate) start_line: i64,
    pub(crate) start_utf16_column: Option<i64>,
    pub(crate) end_line: i64,
    pub(crate) end_utf16_column: Option<i64>,
    /// Server-provided LSP fold category such as `comment`, `imports`, or `region`.
    pub kind: Option<String>,
    pub(crate) collapsed_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Normalized workspace symbol from either `SymbolInformation` or `WorkspaceSymbol`.
pub struct LspWorkspaceSymbolResponse {
    pub(crate) name: String,
    /// Numeric LSP `SymbolKind`, when supplied by the server.
    pub kind: Option<i64>,
    pub(crate) container_name: Option<String>,
    pub(crate) uri: String,
    /// Null when the server returned only a URI and expects `workspaceSymbol/resolve`.
    pub range: Option<LspRangeResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Normalized document link; `target` is absent when the server defers it to resolve.
pub struct LspDocumentLinkResponse {
    pub(crate) range: LspRangeResponse,
    pub(crate) target: Option<String>,
    pub(crate) tooltip: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Normalized CodeLens payload awaiting an optional resolve operation.
pub struct LspCodeLensResponse {
    pub(crate) range: LspRangeResponse,
    pub(crate) command: Option<Value>,
    pub(crate) data: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
/// LSP `TextDocumentSyncKind` advertised by the server.
pub enum LspTextDocumentSyncKind {
    /// The server does not want document change notifications.
    None,
    /// The server expects complete document text on every change.
    #[default]
    Full,
    /// The server accepts range-based incremental `didChange` payloads.
    Incremental,
}

/// Options a host declares for the services it serves one plugin session,
/// from `lsp.startServer.hostOptions`. Core reports them to the backend
/// through `lithe/host/getCapabilities`, only for granted methods.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostOptions {
    /// Icons, source bound and provider types of the editor service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<lithe_plugin_editor::actions::EditorHostOptions>,
}

impl HostOptions {
    /// Whether the host declared no options.
    pub fn is_empty(&self) -> bool {
        self.editor.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Pure client protocol state carried between JSON command invocations.
pub struct LspClientState {
    /// ID the next outbound request takes.
    #[serde(default = "default_next_request_id")]
    pub next_request_id: u64,
    /// Whether the `initialize` response arrived.
    #[serde(default)]
    pub initialized: bool,
    /// Whether `shutdown` was sent; no new requests follow it.
    #[serde(default)]
    pub shutdown_requested: bool,
    /// Feature names the server's capabilities advertise.
    #[serde(default)]
    pub server_capabilities: Vec<String>,
    /// `completionProvider` options by registration id: the static capability
    /// under the empty id, dynamic registrations under their own ids. Only the
    /// trigger and commit characters are read from them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub completion_providers: BTreeMap<String, Value>,
    /// Full semantic-token registrations, including the server-owned legends.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub semantic_token_providers: BTreeMap<String, Value>,
    #[serde(default)]
    /// Server `TextDocumentSyncKind`, used to choose full or incremental `didChange`.
    pub text_document_sync: LspTextDocumentSyncKind,
    /// `None` means saves were not requested; `Some` is the server's includeText flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_document_save_include_text: Option<bool>,
    /// Documents synchronized with the server, by URI.
    #[serde(default)]
    pub open_documents: BTreeMap<String, LspClientDocument>,
    /// Method of every request awaiting a response, by request ID.
    #[serde(default)]
    pub pending_requests: BTreeMap<String, String>,
    /// Legend captured when a semantic request is sent; registrations may change
    /// before the response arrives.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_semantic_legends: BTreeMap<String, Value>,
    /// Latest published diagnostics, by document URI.
    #[serde(default)]
    pub diagnostics: BTreeMap<String, Vec<LspClientDiagnostic>>,
    /// Document version each published diagnostic set belongs to,
    /// when the server named one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub diagnostic_versions: BTreeMap<String, i64>,
    /// Plugin functions the backend registered, once `litheFunctions` was
    /// negotiated; empty otherwise.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub plugin_functions: BTreeSet<String>,
    /// The `litheFunctions` version the backend chose; `None` when the
    /// protocol was not negotiated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_function_version: Option<u64>,
    /// Host services the plugin's permissions grant this session, from
    /// `lsp.startServer.hostServices`. Empty for host-owned sessions.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub host_services: BTreeSet<String>,
    /// Options the host declared for its granted services, from
    /// `lsp.startServer.hostOptions`.
    #[serde(default, skip_serializing_if = "HostOptions::is_empty")]
    pub host_options: HostOptions,
    /// The VCS and capabilities a plugin process answered under
    /// `litheVcs`; `None` when the protocol was not negotiated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs_provider: Option<super::vcs_provider::VcsNegotiation>,
    /// Manifest providers `initialize` offers under `litheToolchains`, from
    /// `lsp.startServer.toolchainProviders`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub offered_toolchain_providers: BTreeSet<String>,
    /// The offered providers the backend selected; empty until negotiated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolchain_providers: Vec<String>,
    /// Manifest providers offered under `litheKeymapImport` for this session.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub offered_keymap_import_providers: BTreeSet<String>,
    /// The shortcut import providers selected by the backend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keymap_import_providers: Vec<String>,
}

impl Default for LspClientState {
    fn default() -> Self {
        Self {
            next_request_id: default_next_request_id(),
            initialized: false,
            shutdown_requested: false,
            server_capabilities: Vec::new(),
            completion_providers: BTreeMap::new(),
            semantic_token_providers: BTreeMap::new(),
            text_document_sync: LspTextDocumentSyncKind::Full,
            text_document_save_include_text: None,
            open_documents: BTreeMap::new(),
            pending_requests: BTreeMap::new(),
            pending_semantic_legends: BTreeMap::new(),
            diagnostics: BTreeMap::new(),
            diagnostic_versions: BTreeMap::new(),
            plugin_functions: BTreeSet::new(),
            plugin_function_version: None,
            host_services: BTreeSet::new(),
            host_options: HostOptions::default(),
            vcs_provider: None,
            offered_toolchain_providers: BTreeSet::new(),
            toolchain_providers: Vec::new(),
            offered_keymap_import_providers: BTreeSet::new(),
            keymap_import_providers: Vec::new(),
        }
    }
}

fn default_next_request_id() -> u64 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Document version and contents currently synchronized with the server.
pub struct LspClientDocument {
    /// Document URI as the host opened it.
    pub uri: String,
    /// LSP language identifier.
    pub language_id: String,
    /// Version sent with the last `didOpen` or `didChange`.
    pub version: i64,
    /// Shared so that copying the client state, which every request, document
    /// change, and server message does while holding the session locks, does
    /// not copy the text of every open document. Serializes as a plain string.
    pub text: Arc<str>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Diagnostic normalized to fields supported by every frontend.
pub struct LspClientDiagnostic {
    pub(crate) range: LspRangeResponse,
    pub(crate) severity: Option<i64>,
    /// Diagnostic text as the server reported it.
    pub message: String,
    pub(crate) source: Option<String>,
    pub(crate) code: Option<String>,
    #[serde(default)]
    pub(crate) tags: Vec<i64>,
    #[serde(default)]
    pub(crate) related_information: Vec<LspClientDiagnosticRelatedInformation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Related diagnostic message and its source location.
pub struct LspClientDiagnosticRelatedInformation {
    pub(crate) location: LspClientDiagnosticLocation,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// URI and range referenced by related diagnostic information.
pub struct LspClientDiagnosticLocation {
    pub(crate) uri: String,
    pub(crate) range: LspRangeResponse,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Inputs for constructing the LSP initialize request and initial client state.
pub struct ClientInitializeRequest {
    /// Client state to start from.
    #[serde(default)]
    pub state: LspClientState,
    /// Workspace root URI.
    pub root_uri: String,
    /// Host process ID; `None` sends `null`.
    #[serde(default)]
    pub process_id: Option<i64>,
    /// Server-specific `initializationOptions`.
    #[serde(default)]
    pub initialization_options: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Inputs for opening a complete document in pure client state.
pub struct ClientOpenDocumentRequest {
    /// Current client state.
    #[serde(default)]
    pub state: LspClientState,
    /// Document URI.
    pub uri: String,
    /// LSP language identifier.
    pub language_id: String,
    /// Complete document text.
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Inputs for updating a synchronized document's contents.
pub struct ClientChangeDocumentRequest {
    /// Current client state.
    #[serde(default)]
    pub state: LspClientState,
    /// URI of an open document.
    pub uri: String,
    /// Complete text after the change. Optional when `content_changes` is set.
    #[serde(default)]
    pub text: String,
    /// Edits in the order they happened, as LSP `didChange` applies them.
    #[serde(default)]
    pub content_changes: Vec<LspDocumentContentChange>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Inputs for closing a document in pure client state.
pub struct ClientCloseDocumentRequest {
    /// Current client state.
    #[serde(default)]
    pub state: LspClientState,
    /// URI of an open document.
    pub uri: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Inputs for the LSP shutdown handshake.
pub struct ClientShutdownRequest {
    /// Current client state.
    #[serde(default)]
    pub state: LspClientState,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Generic feature request translated into one provider-neutral JSON-RPC call.
pub struct ClientFeatureRequest {
    /// Current client state.
    #[serde(default)]
    pub state: LspClientState,
    /// Document the request is about; empty for `workspace/symbol`.
    pub uri: String,
    /// LSP method to call.
    pub method: String,
    /// Cursor position for position-based methods.
    #[serde(default)]
    pub position: Option<LspPosition>,
    /// Search text for `workspace/symbol`; absent means the empty query.
    #[serde(default)]
    pub query: Option<String>,
    /// New symbol name for `rename`.
    #[serde(default)]
    pub new_name: Option<String>,
    /// Selection for range-based methods.
    #[serde(default)]
    pub range: Option<LspRange>,
    /// Diagnostics in `range`, sent as the `codeAction` context.
    #[serde(default)]
    pub diagnostics: Vec<LspClientDiagnostic>,
    /// LSP completion trigger context; validated by the shared request mapper.
    #[serde(default)]
    pub completion_context: Option<Value>,
    /// Item to complete for `completionItem/resolve`.
    #[serde(default)]
    pub completion_item: Option<Value>,
    /// Action to complete for `codeAction/resolve`.
    #[serde(default)]
    pub code_action: Option<Value>,
    /// Command to run for `workspace/executeCommand`.
    #[serde(default)]
    pub command: Option<Value>,
    /// Caller `FormattingOptions` for `formatting` and `rangeFormatting`.
    #[serde(default)]
    pub formatting_options: Option<Value>,
    /// `textDocument/references` `context.includeDeclaration`; unset means `true`.
    #[serde(default)]
    pub include_declaration: Option<bool>,
}

#[cfg(test)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Server JSON-RPC message to reduce into the current client state. Only the
/// stateless reducer tests use it; the engine passes the parsed message.
pub struct ClientApplyServerMessageRequest {
    #[serde(default)]
    pub state: LspClientState,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Updated client state, outbound messages, and host-facing events.
pub struct LspClientResponse {
    /// Client state after the step.
    pub state: LspClientState,
    /// Encoded JSON-RPC messages to write to the server, in order.
    pub messages: Vec<String>,
    /// Events for the host, in order.
    pub events: Vec<LspClientEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Normalized event produced while reducing a server message.
pub struct LspClientEvent {
    /// Pure-client event category: `diagnostics`, `notification`, `response`,
    /// `error`, `functionsChanged`, or `functionProgress`.
    pub kind: String,
    /// Request a `response` or `error` event answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Method of the notification or the answered request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Document a `diagnostics` event is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Document version the diagnostics belong to, when the server named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// Published diagnostics of a `diagnostics` event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<LspClientDiagnostic>>,
    /// Result or notification parameters, shaped into the host model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message of an `error` event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
