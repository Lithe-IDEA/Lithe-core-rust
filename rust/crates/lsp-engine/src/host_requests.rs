//! Requests a plugin backend sends to the host: forwarding and cancellation.

use super::*;

/// Handles a backend's `$/cancelRequest` for one of its own pending host
/// requests: the backend gets `plugin.host.cancelled` at once, so the
/// pending slot frees without waiting for the host, and the host gets a
/// `hostRequestCancelled` event naming the `hostRequestID` so it can stop
/// the work. A later answer from the host for that ID is refused like any
/// unknown ID. Returns false when the notification names no pending host
/// request, so it is handled like any other notification.
pub(crate) fn cancel_host_request_locked(
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
pub(crate) fn forward_host_request_locked(
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
