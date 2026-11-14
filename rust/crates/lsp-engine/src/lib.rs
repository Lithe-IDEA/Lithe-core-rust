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

// Cold language-server runtimes can spend more than ten seconds loading their
// bundles before answering initialize. This remains one bounded handshake,
// independent of request deadlines, with normal cancellation and disposal.
const DEFAULT_INITIALIZE_TIMEOUT_MS: u64 = 60_000;
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

mod configuration;
mod documents;
mod engine;
mod events;
mod function_calls;
mod host_requests;
mod lifecycle;
mod operations;
mod plugin_server_calls;
mod provider_events;
mod requests;
mod server_messages;
mod state;
mod validation;

use configuration::*;
use documents::*;
use engine::*;
use events::*;
use function_calls::*;
use host_requests::*;
use lifecycle::*;
use operations::*;
use plugin_server_calls::*;
use provider_events::*;
pub use requests::*;
use state::*;
use validation::*;

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

#[cfg(test)]
mod function_relay_tests;

#[cfg(test)]
mod tests;
