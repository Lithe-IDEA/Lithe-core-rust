//! Messages read from the server and the backend: responses, server requests,
//! notifications and configuration pulls.

use super::*;

impl RuntimeSession {
    pub(crate) fn handle_server_message(&self, message: String) -> Result<(), CoreError> {
        self.handle_server_message_after_policy_check(message, || {})
    }

    pub(crate) fn handle_server_message_after_policy_check(
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
                                            &message,
                                            FailureContext::request(&pending.method, None),
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
                                            &failure.message,
                                            FailureContext::request(&pending.method, None),
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
                                        "The shortcut import provider reply is too large.",
                                        FailureContext::request(&pending.method, None),
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
                                        "Language server returned an error.",
                                        FailureContext::request(
                                            &pending.method,
                                            pending.document_uri.as_deref(),
                                        )
                                        .with_underlying(Some(detail)),
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
