//! Applying messages the language server sends.

use super::*;

/// Reduces a server message for host-owned stateless client tests.
#[cfg(test)]
pub fn client_apply_server_message(
    request: ClientApplyServerMessageRequest,
) -> Result<LspClientResponse, CoreError> {
    let message: Value = serde_json::from_str(&request.message).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP server JSON message")
            .with_details(error.to_string())
    })?;
    client_apply_server_message_for_plugin(request.state, &message, false)
}

/// Applies function protocol messages only for sessions with a runtime-owned plugin identity.
///
/// Takes the message already parsed: the engine parses each server message once,
/// outside the session state lock, and reduces it while holding that lock.
pub fn client_apply_server_message_for_plugin(
    mut state: LspClientState,
    message: &Value,
    plugin_owned: bool,
) -> Result<LspClientResponse, CoreError> {
    let mut responses = Vec::new();
    let mut events = Vec::new();

    if let Some(method) = message.get("method").and_then(Value::as_str) {
        let request_id = message.get("id");
        match method {
            "textDocument/publishDiagnostics" => {
                if let Some(params) = message.get("params") {
                    let uri = params
                        .get("uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    validate_uri(uri)?;
                    // Servers may re-encode the URIs they echo back (VS Code's
                    // URI library writes `d%3A` for a Windows drive while this
                    // host opened the document as `d:`), so a raw map lookup
                    // silently drops every published diagnostic on Windows.
                    // Match through the normalized form and publish under the
                    // document's own canonical URI.
                    let document = open_document_for_published_uri(&state.open_documents, uri);
                    let Some(document) = document else {
                        return Ok(client_response(state, responses, events));
                    };
                    let document_uri = document.uri.clone();
                    let version = params.get("version").and_then(Value::as_i64);
                    if version.is_some_and(|version| version != document.version) {
                        return Ok(client_response(state, responses, events));
                    }
                    let diagnostics = parse_diagnostics(params.get("diagnostics"));
                    state
                        .diagnostics
                        .insert(document_uri.clone(), diagnostics.clone());
                    if let Some(version) = version {
                        state
                            .diagnostic_versions
                            .insert(document_uri.clone(), version);
                    } else {
                        state.diagnostic_versions.remove(&document_uri);
                    }
                    events.push(LspClientEvent {
                        kind: "diagnostics".to_string(),
                        request_id: None,
                        method: None,
                        uri: Some(document_uri),
                        version,
                        diagnostics: Some(diagnostics),
                        result: None,
                        error: None,
                    });
                }
            }
            "client/registerCapability" => {
                apply_dynamic_registration(&mut state, message);
                if let Some(id) = request_id {
                    responses.push(json_rpc_result(id, Value::Null)?);
                }
            }
            "client/unregisterCapability" => {
                apply_dynamic_unregistration(&mut state, message);
                if let Some(id) = request_id {
                    responses.push(json_rpc_result(id, Value::Null)?);
                }
            }
            lithe_plugin_rpc::REGISTER_METHOD | lithe_plugin_rpc::UNREGISTER_METHOD
                if plugin_owned
                    && !state.shutdown_requested
                    && state
                        .server_capabilities
                        .iter()
                        .any(|feature| feature == lithe_plugin_rpc::FUNCTIONS_FEATURE) =>
            {
                let params = message.get("params");
                let outcome = if method == lithe_plugin_rpc::REGISTER_METHOD {
                    lithe_plugin_rpc::apply_registration(&state.plugin_functions, params)
                        .map(|registry| (registry, Vec::new()))
                } else {
                    lithe_plugin_rpc::apply_unregistration(&state.plugin_functions, params)
                };
                match outcome {
                    Ok((registry, removed)) => {
                        state.plugin_functions = registry;
                        if let Some(id) = request_id {
                            responses.push(json_rpc_result(id, Value::Null)?);
                        }
                        events.push(LspClientEvent {
                            kind: "functionsChanged".to_string(),
                            request_id: None,
                            method: Some(method.to_string()),
                            uri: None,
                            version: None,
                            diagnostics: None,
                            result: Some(json!({
                                "functions": state.plugin_functions,
                                "removed": removed
                            })),
                            error: None,
                        });
                    }
                    Err(reason) => {
                        if let Some(id) = request_id {
                            responses.push(json_rpc_error_with_code(
                                id,
                                -32602,
                                reason,
                                lithe_plugin_rpc::codes::INVALID_REGISTRATION,
                            )?);
                        }
                    }
                }
            }
            crate::build_provider::PROGRESS_METHOD
                if !state.shutdown_requested
                    && state.server_capabilities.iter().any(|feature| {
                        feature == crate::build_provider::BUILD_PROVIDER_FEATURE
                    }) =>
            {
                if let Some(result) = crate::build_provider::progress(message.get("params")) {
                    events.push(LspClientEvent {
                        kind: "buildProgress".to_string(),
                        request_id: None,
                        method: Some(method.to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: Some(result),
                        error: None,
                    });
                }
            }
            lithe_plugin_rpc::PROGRESS_METHOD
                if plugin_owned
                    && !state.shutdown_requested
                    && state
                        .server_capabilities
                        .iter()
                        .any(|feature| feature == lithe_plugin_rpc::FUNCTIONS_FEATURE) =>
            {
                // Malformed reports are dropped here; reports for an operation
                // that is no longer running are dropped by the engine.
                if let Some(progress) = lithe_plugin_rpc::parse_progress(message.get("params")) {
                    let mut result = json!({ "operationID": progress.operation_id });
                    if let Some(text) = progress.message {
                        result["message"] = json!(text);
                    }
                    if let Some(percentage) = progress.percentage {
                        result["percentage"] = json!(percentage);
                    }
                    events.push(LspClientEvent {
                        kind: "functionProgress".to_string(),
                        request_id: None,
                        method: Some(method.to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: Some(result),
                        error: None,
                    });
                }
            }
            "workspace/semanticTokens/refresh" => {
                if let Some(id) = request_id {
                    responses.push(json_rpc_result(id, Value::Null)?);
                }
                events.push(LspClientEvent {
                    kind: "semanticTokensRefresh".to_string(),
                    request_id: None,
                    method: Some(method.to_string()),
                    uri: None,
                    version: None,
                    diagnostics: None,
                    result: None,
                    error: None,
                });
            }
            "workspace/configuration" => {
                if let Some(id) = request_id {
                    let item_count = message
                        .get("params")
                        .and_then(|params| params.get("items"))
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    responses.push(json_rpc_result(
                        id,
                        Value::Array(vec![Value::Null; item_count]),
                    )?);
                }
            }
            "workspace/workspaceFolders" | "window/workDoneProgress/create" => {
                if let Some(id) = request_id {
                    responses.push(json_rpc_result(id, Value::Null)?);
                }
            }
            _ if plugin_owned && request_id.is_some() && method.starts_with("lithe/host/") => {
                let id = request_id.expect("guarded above");
                match lithe_plugin_rpc::host_request(
                    method,
                    message.get("params"),
                    state.plugin_function_version,
                    &state.host_services,
                ) {
                    // The engine answers once the host has; the request
                    // carries the backend's own ID back to it.
                    Ok(lithe_plugin_rpc::HostRoute::Answer(mut result)) => {
                        // getCapabilities also reports the options of the
                        // granted host services.
                        if let (lithe_plugin_rpc::GET_CAPABILITIES, Some(editor)) =
                            (method, &state.host_options.editor)
                        {
                            editor.report(&mut result, &state.host_services);
                        }
                        responses.push(json_rpc_result(id, result)?)
                    }
                    Ok(lithe_plugin_rpc::HostRoute::Forward(service)) => {
                        events.push(LspClientEvent {
                            kind: "hostRequest".to_string(),
                            request_id: None,
                            method: Some(method.to_string()),
                            uri: None,
                            version: None,
                            diagnostics: None,
                            result: Some(json!({
                                "id": id,
                                "service": service,
                                "params": lithe_plugin_rpc::forwarded_host_params(
                                    method,
                                    message.get("params"),
                                ),
                            })),
                            error: None,
                        })
                    }
                    // Another plugin's function: the engine's relay checks the
                    // call against the plugins' manifests and answers the
                    // backend itself, so the reducer only names the request.
                    Ok(lithe_plugin_rpc::HostRoute::Relay) => events.push(LspClientEvent {
                        kind: "functionRelayRequest".to_string(),
                        request_id: None,
                        method: Some(method.to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: Some(json!({
                            "id": id,
                            "params": message.get("params").cloned().unwrap_or(Value::Null),
                        })),
                        error: None,
                    }),
                    Err(refusal) => {
                        let mut data = json!({ "code": refusal.code });
                        if let Some(details) = refusal.details {
                            data["details"] = details;
                        }
                        responses.push(json_rpc_error_with_data(
                            id,
                            refusal.rpc_code,
                            refusal.message,
                            data,
                        )?)
                    }
                }
            }
            _ => {
                if let Some(id) = request_id {
                    responses.push(match lithe_plugin_rpc::refused_backend_request(method) {
                        Some(code) => json_rpc_error_with_code(
                            id,
                            -32601,
                            "The host does not serve this request to a plugin process.",
                            code,
                        )?,
                        None => json_rpc_error(id, -32601, "Method not found")?,
                    });
                } else {
                    events.push(LspClientEvent {
                        kind: "notification".to_string(),
                        request_id: None,
                        method: Some(method.to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: message.get("params").cloned(),
                        error: None,
                    });
                }
            }
        }
    } else if let Some(id) = lsp_message_id(message) {
        let pending = state.pending_requests.remove(&id);
        if pending.as_deref() == Some("initialize") {
            if let Some(result) = message
                .get("result")
                .and_then(Value::as_object)
                .filter(|result| result.get("capabilities").is_some_and(Value::is_object))
            {
                state.server_capabilities = feature_names_from_capabilities(
                    result.get("capabilities").unwrap_or(&Value::Null),
                );
                if plugin_owned
                    && crate::plugin_logging::negotiated(
                        result.get("capabilities").unwrap_or(&Value::Null),
                    )
                {
                    state
                        .server_capabilities
                        .push(crate::plugin_logging::FEATURE.into());
                }
                let functions = plugin_owned
                    .then(|| {
                        lithe_plugin_rpc::negotiate(
                            result.get("capabilities").unwrap_or(&Value::Null),
                        )
                    })
                    .flatten();
                if let Some((version, functions)) = functions {
                    state.plugin_functions = functions;
                    state.plugin_function_version = Some(version);
                    events.push(LspClientEvent {
                        kind: "functionsChanged".to_string(),
                        request_id: None,
                        method: Some("initialize".to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: Some(json!({ "functions": state.plugin_functions, "removed": [] })),
                        error: None,
                    });
                } else {
                    state.plugin_functions.clear();
                    state.plugin_function_version = None;
                    state
                        .server_capabilities
                        .retain(|feature| feature != lithe_plugin_rpc::FUNCTIONS_FEATURE);
                }
                // Only a plugin's own process may act as a VCS provider; a
                // host-owned server that happens to answer is ignored.
                state.vcs_provider = plugin_owned
                    .then(|| {
                        crate::vcs_provider::negotiate(
                            result.get("capabilities").unwrap_or(&Value::Null),
                        )
                    })
                    .flatten();
                match &state.vcs_provider {
                    Some(negotiation) => events.push(LspClientEvent {
                        kind: crate::vcs_provider::NEGOTIATED_EVENT.to_string(),
                        request_id: None,
                        method: Some("initialize".to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: Some(json!(negotiation)),
                        error: None,
                    }),
                    None => state
                        .server_capabilities
                        .retain(|feature| feature != crate::vcs_provider::VCS_PROVIDER_FEATURE),
                }
                state.toolchain_providers = plugin_owned
                    .then(|| {
                        crate::toolchain_provider::negotiate(
                            result.get("capabilities").unwrap_or(&Value::Null),
                            &state.offered_toolchain_providers,
                        )
                    })
                    .flatten()
                    .unwrap_or_default();
                if !state.toolchain_providers.is_empty() {
                    events.push(LspClientEvent {
                        kind: crate::toolchain_provider::NEGOTIATED_EVENT.to_string(),
                        request_id: None,
                        method: Some("initialize".to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: Some(json!({ "providers": state.toolchain_providers })),
                        error: None,
                    });
                }
                state.keymap_import_providers = plugin_owned
                    .then(|| {
                        crate::keymap_provider::negotiate(
                            result.get("capabilities").unwrap_or(&Value::Null),
                            &state.offered_keymap_import_providers,
                        )
                    })
                    .flatten()
                    .unwrap_or_default();
                if !state.keymap_import_providers.is_empty() {
                    events.push(LspClientEvent {
                        kind: crate::keymap_provider::NEGOTIATED_EVENT.to_string(),
                        request_id: None,
                        method: Some("initialize".to_string()),
                        uri: None,
                        version: None,
                        diagnostics: None,
                        result: Some(json!({ "providers": state.keymap_import_providers })),
                        error: None,
                    });
                }
                state.text_document_sync =
                    text_document_sync_kind(result.get("capabilities").unwrap_or(&Value::Null));
                state.text_document_save_include_text = match result
                    .get("capabilities")
                    .and_then(|value| value.pointer("/textDocumentSync/save"))
                {
                    Some(Value::Bool(true)) => Some(false),
                    Some(Value::Object(options)) => Some(
                        options
                            .get("includeText")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    ),
                    _ => None,
                };
                if let Some(options) = result
                    .get("capabilities")
                    .and_then(|c| c.get("completionProvider"))
                    .filter(|options| options.is_object())
                {
                    state
                        .completion_providers
                        .insert(String::new(), options.clone());
                }
                if let Some(options) = result
                    .get("capabilities")
                    .and_then(|c| c.get("semanticTokensProvider"))
                {
                    if semantic_token_legend(options).is_some() {
                        state
                            .semantic_token_providers
                            .insert(String::new(), options.clone());
                    }
                }
                state.initialized = true;
                responses.push(json_rpc_notification("initialized", json!({}))?);
            }
        }
        if pending.as_deref() == Some("shutdown") {
            state.initialized = false;
            state.shutdown_requested = false;
            state.server_capabilities.clear();
            state.semantic_token_providers.clear();
            state.completion_providers.clear();
            state.text_document_sync = LspTextDocumentSyncKind::Full;
            state.open_documents.clear();
            state.diagnostics.clear();
            state.diagnostic_versions.clear();
            state.plugin_functions.clear();
            state.plugin_function_version = None;
            state.vcs_provider = None;
            responses.push(json_rpc_message_without_params(None, "exit")?);
        }
        let result = if pending.as_deref() == Some("textDocument/semanticTokens/full") {
            semantic_token_result(
                state.pending_semantic_legends.remove(&id).as_ref(),
                message.get("result"),
            )
        } else {
            lsp_feature_result_for_method(pending.as_deref(), message.get("result"))
        };
        events.push(LspClientEvent {
            kind: if message.get("error").is_some() {
                "error".to_string()
            } else {
                "response".to_string()
            },
            request_id: Some(id),
            method: pending,
            uri: None,
            version: None,
            diagnostics: None,
            result,
            error: message.get("error").map(|value| value.to_string()),
        });
    }

    Ok(client_response(state, responses, events))
}

/// Resolves a server-published document URI against the client's open
/// documents, tolerating URI re-encoding differences.
///
/// Language servers frequently normalize the URIs a client sends them: VS
/// Code's URI library (used by Intelephense, typescript-language-server, and
/// others) percent-encodes the Windows drive colon (`d:` becomes `d%3A`) and
/// lowercases the drive letter. A raw map lookup therefore misses even though
/// the document is open. Comparison preserves URI component boundaries and
/// folds the Windows drive letter while keeping Unix paths case-sensitive.
fn open_document_for_published_uri<'a>(
    documents: &'a std::collections::BTreeMap<String, LspClientDocument>,
    uri: &str,
) -> Option<&'a LspClientDocument> {
    if let Some(document) = documents.get(uri) {
        return Some(document);
    }
    let normalized = normalize_uri_for_comparison(uri);
    documents
        .iter()
        .find(|(key, _)| normalize_uri_for_comparison(key) == normalized)
        .map(|(_, document)| document)
}
