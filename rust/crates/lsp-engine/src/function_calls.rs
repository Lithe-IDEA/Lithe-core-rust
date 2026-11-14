//! Plugin function calls: pending call lookup, progress, the published function registry
//! and revocation.

use super::*;

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
pub(crate) fn revoke_function_calls_locked(
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

/// Publishes a progress report of a call that is still running. Reports for
/// an unknown, finished, cancelled, or revoked operation are dropped, so a
/// late report never revives an ended call in the host.
pub(crate) fn push_function_progress_locked(
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

pub(crate) fn push_functions_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    result: Value,
) {
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

pub(crate) fn function_runtime_error(
    session: &RuntimeSession,
    failure: lithe_plugin_rpc::FunctionFailure,
) -> LspRuntimeError {
    let mut error = runtime_error(
        session,
        failure.code,
        "request",
        &failure.message,
        FailureContext::request(lithe_plugin_rpc::INVOKE_METHOD, None),
    );
    error.plugin_error_code = failure.plugin_error_code;
    error
}

/// Withdraws the session registry when shutdown or process failure ends its lifetime.
pub(crate) fn clear_function_registry(session: &RuntimeSession, state: &mut SessionState) {
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
