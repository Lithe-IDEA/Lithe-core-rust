//! Deterministic application services shared by the macOS and Windows hosts.

use lithe_build as build;
use lithe_debug as debug;
/// Run configurations and launch environments, kept in their own crate.
pub use lithe_execution as execution;
mod lsp;
use lithe_project as project;
use lithe_project_generators as project_generators;
mod protocol;
mod runtime;
/// Shared toolchain policy, independent of plugin transport and native discovery.
pub use lithe_toolchain as toolchain;
use lithe_vcs as vcs;

pub use protocol::{
    CoreCommand, CoreError, CoreEvent, CoreRequest, CoreResponse, ErrorCode, ResponseData,
};

/// The `ai.*` commands, kept in their own crate (`rust/crates/ai`); re-exported so the former
/// `lithe_core::ai` paths keep resolving.
pub use lithe_ai as ai;

/// The cancellation scope of one request, for a host crate that answers some
/// commands itself and still has to honour `cancel_operation` and timeouts the
/// way Core's own dispatcher does.
pub use protocol::cancellation::{check as check_cancellation, Scope as CancellationScope};

/// Executes one versioned application command and returns a JSON response.
pub fn execute_json(request: &str) -> String {
    runtime::execute_json(request)
}

/// Starts the existing LSP engine with a native resource owner attached before
/// initialization. The callback only records the ID; it must not call Core.
/// Unlike a wire request, this also preserves ownership when the first write
/// fails and the ordinary start command returns an error.
pub fn start_lsp_owned(
    mut payload: serde_json::Value,
    registered: impl FnOnce(&str),
) -> Result<serde_json::Value, CoreError> {
    lsp::resolve_request_paths("lsp.startServer", &mut payload)?;
    let request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid language-server launch.")
            .with_details(error.to_string())
    })?;
    let response = lsp::interface::start_server_owned(request, registered)?;
    serde_json::to_value(response).map_err(|error| {
        CoreError::new(
            ErrorCode::Unknown,
            "Could not encode language-server launch.",
        )
        .with_details(error.to_string())
    })
}

/// Requests cooperative cancellation of an active operation.
///
/// Native Rust hosts use this entry point while the Swift and C++ clients keep
/// using the stable `lithe_core_cancel` C ABI.
pub fn cancel_operation(operation_id: &str) -> bool {
    protocol::cancellation::cancel(operation_id)
}

#[cfg(test)]
mod tests;

/// Executes a request with Git diagnostics and search snapshots.
/// The observer is scoped to this call; callbacks complete before the final JSON returns.
pub fn execute_json_with_events(
    request: &str,
    sink: std::sync::Arc<dyn Fn(&str) + Send + Sync>,
) -> String {
    lithe_request_context::events::with_sink(sink, || {
        let response = execute_json(request);
        lithe_request_context::events::request_finished(&response);
        response
    })
}

/// Runs the native AskPass helper before either host initializes its user interface.
pub fn git_askpass_main(prompt: &str) -> i32 {
    crate::runtime::helper_failures::install();
    lithe_git_host::authentication::helper_main(prompt)
}

/// Shared classification for ordinary project watcher events after root resolution.
pub use project::is_workspace_event_relevant;

/// Shared, IO-free classification for coalesced native Git watcher events.
pub use lithe_git::{
    classify_git_metadata_change, GitMetadataChangeKind, GitMetadataEventClassification,
    GitMetadataWatchContext,
};

/// Connects an authorized native window to the local IDE broker's request notifications.
/// The host retains the callback until close; receivers claim IDs through `ideHost.control`
/// before invoking application actions. This adapter boundary has no UI dependencies.
pub fn attach_ide_request_dispatcher(
    host_id: &str,
    owner: &str,
    dispatcher: std::sync::Arc<dyn Fn(String) -> Result<(), String> + Send + Sync>,
) -> Result<(), String> {
    crate::runtime::helper_failures::install();
    lithe_ide_host::attach_dispatcher(host_id, owner, dispatcher)
}
