//! Events of negotiated providers: build progress, VCS descriptions and the provider selection.

use super::*;

/// A wire request ID identifies one exact build operation in this session.
/// Cancellation/completion removes it from pending, so late progress is inert.
pub(crate) fn push_build_progress_locked(
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

/// Publishes the VCS and capabilities a provider negotiated, so the host can
/// check the VCS against the manifest entry and decide which actions to show.
pub(crate) fn push_vcs_provider_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    result: Value,
) {
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
pub(crate) fn push_negotiated_provider_event(
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
