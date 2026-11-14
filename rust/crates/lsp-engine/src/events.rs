//! The session event queue: request completions, diagnostics, features, server info,
//! logs, failures and state clearing.

use super::*;

pub(crate) fn enqueue_runtime_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    mut event: LspRuntimeEvent,
) {
    if event.event.is_some() {
        let (id, fields) = (
            event.event.take().expect("source catalog identity"),
            event.fields.take().expect("source catalog fields"),
        );
        let level = if id == "log.dropped" {
            Some(lithe_diagnostics::record::LogLevel::Warn)
        } else {
            lithe_diagnostics::catalog::event_by_id(&id).map(|entry| entry.level)
        };
        // In-memory v1 detail is retained for old hosts. The v2 fields always pass
        // the same normalization and catalog routing boundary as persisted facts.
        // A fact that fails that boundary keeps only its v1 fields; the runtime
        // event itself is never dropped and Core never panics on source data.
        if let Some(level) = level {
            let mut fact = lithe_diagnostics::record::StructuredLogEvent {
                ts: String::new(),
                level,
                seq: 0,
                event: id.clone(),
                op: None,
                msg: String::new(),
                fields,
            };
            if let Ok(channel) = lithe_diagnostics::record::prepare_catalog_event(
                &mut fact,
                &json!({"channel": session.log_identity.channel()}),
            ) {
                event.channel = Some(channel);
                event.plugin_id = session.plugin_id.clone();
                event.role = Some(session.log_identity.role);
                event.event = Some(id);
                event.fields = Some(fact.fields);
                event.catalog_level = Some(fact.level);
            }
        }
    }
    state.events.push_back(event);
    session.event_signal.notify_all();
}

pub(crate) fn push_request_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    operation_id: &str,
    method: &str,
    result: Option<Value>,
    error: Option<LspRuntimeError>,
) {
    if state
        .plugin_server
        .as_ref()
        .is_some_and(|server| server.owns(operation_id))
    {
        complete_plugin_server_request(session, state, operation_id, method, result, error);
        return;
    }
    // No host operation waits for a relayed call: the relay answers the
    // calling plugin, so the event never reaches the host.
    if relay_hub::is_relayed_operation(operation_id, method) {
        session.relay.callee_finished(
            &session.id,
            operation_id,
            relay_hub::callee_outcome(result, error),
        );
        return;
    }
    enqueue_request_completed(session, state, operation_id, method, result, error);
}

pub(crate) fn enqueue_request_completed(
    session: &RuntimeSession,
    state: &mut SessionState,
    operation_id: &str,
    method: &str,
    result: Option<Value>,
    error: Option<LspRuntimeError>,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "requestCompleted".to_string(),
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
            operation_id: Some(operation_id.to_string()),
            method: Some(method.to_string()),
            uri: None,
            version: None,
            diagnostics: None,
            result,
            error,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

pub(crate) fn push_diagnostics_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    uri: &str,
    version: Option<i64>,
    diagnostics: Vec<LspClientDiagnostic>,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "diagnostics".to_string(),
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
            uri: Some(uri.to_string()),
            version,
            diagnostics: Some(diagnostics),
            result: None,
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

pub(crate) fn push_semantic_tokens_refresh_event(
    session: &RuntimeSession,
    state: &mut SessionState,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "semanticTokensRefresh".to_string(),
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
            result: None,
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

pub(crate) fn push_features_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    capabilities: Vec<String>,
) {
    let completion_options = lithe_lsp_client::completion_options(&state.client);
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "featuresChanged".to_string(),
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
            result: None,
            error: None,
            capabilities: Some(capabilities),
            completion_options,
            server_info: None,
            level: None,
            message: None,
            detail: None,
        },
    );
}

pub(crate) fn push_server_info_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    info: LspServerInfo,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "serverInfoChanged".to_string(),
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
            result: None,
            error: None,
            capabilities: None,
            completion_options: None,
            server_info: Some(info),
            level: None,
            message: None,
            detail: None,
        },
    );
}

/// Accepts only negotiated scalar plugin facts. Persistence and overload accounting
/// reuse the bounded Core handoff instead of copying protocol params into v1 detail.
pub(crate) fn receive_plugin_diagnostic(
    session: &RuntimeSession,
    state: &mut SessionState,
    params: Option<&Value>,
) {
    let Some(plugin) = session.plugin_id.as_deref() else {
        return;
    };
    let plugin_identity = SessionLogIdentity::new(
        SessionRole::Backend,
        &session.provider_id,
        Some(plugin),
        None,
    );
    let plugin = plugin_identity.segment();
    let valid = state
        .client
        .server_capabilities
        .iter()
        .any(|name| name == lithe_lsp_client::plugin_logging::FEATURE)
        .then(|| {
            lithe_lsp_client::plugin_logging::decode(
                plugin,
                &state.client.plugin_functions,
                params.unwrap_or(&Value::Null),
            )
        })
        .flatten();
    let Some(fact) = valid else {
        if !state.rejected_plugin_diagnostics {
            state.rejected_plugin_diagnostics = true;
            if lithe_diagnostics::core_events::record(
                "plugins.diagnostics.rejected",
                BTreeMap::from([
                    ("plugin".into(), json!(plugin)),
                    ("code".into(), json!("invalid_diagnostic")),
                ]),
                None,
            )
            .is_err()
            {
                lithe_diagnostics::core_events::note_dropped();
            }
        }
        return;
    };
    if state.plugin_diagnostics_window.elapsed() >= Duration::from_secs(60) {
        state.plugin_diagnostics_window = Instant::now();
        state.plugin_diagnostics_count = 0;
    }
    if state.plugin_diagnostics_count >= MAX_PLUGIN_DIAGNOSTICS_PER_MINUTE {
        lithe_diagnostics::core_events::note_dropped();
        return;
    }
    state.plugin_diagnostics_count += 1;
    // SDK contexts carry wire request IDs, not reusable UI operation IDs. Only
    // a live request owned by this session may extend the host's correlation.
    let operation = fact.wire_operation.as_deref().and_then(|wire| {
        state
            .pending
            .get(wire)
            .and_then(|pending| pending.operation_id.clone())
    });
    if lithe_diagnostics::core_events::record(fact.event, fact.fields, operation).is_err() {
        lithe_diagnostics::core_events::note_dropped();
    }
}

pub(crate) fn push_log_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    level: &str,
    identity: impl Into<EngineLogInput>,
    message: &str,
    detail: Option<String>,
) {
    push_classified_log_event(session, state, level, identity, message, detail, None);
}

pub(crate) fn push_classified_log_event(
    session: &RuntimeSession,
    state: &mut SessionState,
    level: &str,
    identity: impl Into<EngineLogInput>,
    message: &str,
    detail: Option<String>,
    error: Option<LspRuntimeError>,
) {
    let input = identity.into();
    let (event_id, fields) =
        input
            .identity
            .fact(&session.log_identity, &input.fields, error.as_ref());
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "log".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: Some(event_id),
            fields: Some(fields),
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
            result: None,
            error,
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: Some(level.to_string()),
            message: Some(message.to_string()),
            detail: detail.filter(|value| !value.is_empty()),
        },
    );
}

pub(crate) fn take_sequence(state: &mut SessionState) -> u64 {
    let sequence = state.next_sequence;
    state.next_sequence += 1;
    sequence
}

/// What a runtime failure concerns beyond its code, stage and message. The
/// default is a failure of the session itself, outside any request.
#[derive(Clone, Copy, Default)]
pub(crate) struct FailureContext<'a> {
    method: Option<&'a str>,
    document_uri: Option<&'a str>,
    underlying: Option<&'a str>,
    process_exit_code: Option<i32>,
}

impl<'a> FailureContext<'a> {
    /// A failure of one request, and of the document it names, if any.
    pub(crate) fn request(method: &'a str, document_uri: Option<&'a str>) -> Self {
        Self {
            method: Some(method),
            document_uri,
            ..Self::default()
        }
    }

    /// The lower-level message behind the failure.
    pub(crate) fn with_underlying(self, underlying: Option<&'a str>) -> Self {
        Self { underlying, ..self }
    }

    /// The exit code of the process whose end caused the failure.
    pub(crate) fn with_exit_code(self, process_exit_code: Option<i32>) -> Self {
        Self {
            process_exit_code,
            ..self
        }
    }
}

pub(crate) fn runtime_error(
    session: &RuntimeSession,
    code: &str,
    stage: &str,
    message: &str,
    context: FailureContext<'_>,
) -> LspRuntimeError {
    LspRuntimeError {
        code: code.to_string(),
        provider_id: session.provider_id.clone(),
        session_id: session.id.clone(),
        stage: stage.to_string(),
        method: context.method.map(ToString::to_string),
        document_uri: context.document_uri.map(ToString::to_string),
        message: message.to_string(),
        underlying_message: context.underlying.map(ToString::to_string),
        process_exit_code: context.process_exit_code,
        plugin_error_code: None,
        details: None,
    }
}

pub(crate) fn fail_feature_requests(
    session: &RuntimeSession,
    state: &mut SessionState,
    code: &str,
    stage: &str,
    message: &str,
    exit_code: Option<i32>,
) {
    let pending: Vec<_> = state
        .pending
        .iter()
        .filter(|(_, pending)| {
            matches!(
                pending.kind,
                PendingKind::Feature | PendingKind::PluginServer
            )
        })
        .map(|(request_id, pending)| (request_id.clone(), pending.clone()))
        .collect();
    for (request_id, pending) in pending {
        state.pending.remove(&request_id);
        state.client.pending_requests.remove(&request_id);
        state.client.pending_semantic_legends.remove(&request_id);
        if let Some(operation_id) = pending.operation_id.as_deref() {
            state.request_by_operation.remove(operation_id);
            state.function_by_operation.remove(operation_id);
            let error = runtime_error(
                session,
                code,
                stage,
                message,
                FailureContext::request(&pending.method, pending.document_uri.as_deref())
                    .with_exit_code(exit_code),
            );
            push_request_event(
                session,
                state,
                operation_id,
                &pending.method,
                None,
                Some(error),
            );
        }
    }
    // Requests held behind a gate have no pending entry, so they end here.
    let held = state
        .plugin_server
        .as_ref()
        .map(|server| server.all_held())
        .unwrap_or_default();
    for waiter in held {
        let error = runtime_error(
            session,
            code,
            stage,
            message,
            FailureContext::request(&waiter.method, None).with_exit_code(exit_code),
        );
        push_request_event(
            session,
            state,
            &waiter.operation_id,
            &waiter.method,
            None,
            Some(error),
        );
    }
}

pub(crate) fn clear_runtime_diagnostics(session: &RuntimeSession, state: &mut SessionState) {
    let diagnostics: Vec<_> = state.client.diagnostics.keys().cloned().collect();
    state.client.diagnostics.clear();
    state.client.diagnostic_versions.clear();
    for uri in diagnostics {
        push_diagnostics_event(session, state, &uri, None, Vec::new());
    }
}

pub(crate) fn clear_runtime_state(session: &RuntimeSession, state: &mut SessionState) {
    clear_runtime_diagnostics(session, state);
    state.client.initialized = false;
    state.client.text_document_save_include_text = None;
    state.client.shutdown_requested = false;
    state.client.server_capabilities.clear();
    state.client.completion_providers.clear();
    state.client.open_documents.clear();
    state.client.pending_requests.clear();
    state.pending.clear();
    state.request_by_operation.clear();
    state.function_by_operation.clear();
    clear_function_registry(session, state);
    state.client.vcs_provider = None;
    state.client.toolchain_providers.clear();
    state.client.keymap_import_providers.clear();
    state.linked_document_paths = LinkedDocumentPaths::default();
    state.deadline_cancellations.clear();
    state.maintenance_write_deadline = None;
    state.relay_write_deadline = None;
    state.pending_workspace_file_changes.clear();
    state.initialize_deadline = None;
    state.shutdown_deadline = None;
    push_features_event(session, state, Vec::new());
}
