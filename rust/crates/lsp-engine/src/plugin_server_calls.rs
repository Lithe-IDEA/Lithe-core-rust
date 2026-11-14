//! Requests and notifications a plugin backend sends to the plugin's own language server:
//! targets, admission gates, held requests and answers.

use super::*;

/// Ends a request the backend sent to its plugin-owned language server. This
/// is the only place where the session policy releases such an operation, so
/// every way a request can end (answer, cancel, timeout, session failure)
/// leaves the policy consistent. Requests that joined a running command
/// finish with the same result or error.
pub(crate) fn complete_plugin_server_request(
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

/// The plugin-owned server a call targets, after checking that the session
/// is that contribution and can take requests now.
pub(crate) fn plugin_server_target<'a>(
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

pub(crate) fn plugin_server_timeout(requested: Option<u64>) -> Duration {
    let milliseconds = requested
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MILLISECONDS)
        .clamp(1, MAX_REQUEST_TIMEOUT_MILLISECONDS);
    Duration::from_millis(milliseconds)
}

/// Allocates the JSON-RPC request of an admitted operation and records it as
/// pending. Returns the messages to write and the request ID.
pub(crate) fn register_plugin_server_request(
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
pub(crate) fn observe_plugin_server_notification(
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
                "Could not encode the language-server request.",
                FailureContext::request(&held.method, None).with_underlying(Some(&error.message)),
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
pub(crate) fn cancel_unsent_plugin_server_request(
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
        "Language-server request was cancelled.",
        FailureContext::request(&method, None),
    );
    push_request_event(session, state, operation_id, &method, None, Some(error));
}

/// What the backend receives for a plugin-owned server's reply: the result
/// untouched, so no Lithe normalizer reinterprets the server's own protocol,
/// or the server's error code and message as details of
/// `plugin.host.languageServerError`.
pub(crate) fn plugin_server_answer(
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
            message,
            FailureContext::request(method, None),
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
