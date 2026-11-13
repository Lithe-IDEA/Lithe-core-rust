//! Session lifecycle: reader, monitor and maintenance threads, deadlines, process exit,
//! failure, shutdown and ordered writes.

use super::*;

impl RuntimeSession {
    pub(crate) fn stop(&self) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, force_kill) = {
            let mut state = self.lock_state()?;
            if matches!(
                state.lifecycle,
                LspLifecycleState::Stopped | LspLifecycleState::Failed
            ) {
                return Ok(());
            }
            if state.lifecycle == LspLifecycleState::Stopping {
                return Ok(());
            }
            fail_feature_requests(
                self,
                &mut state,
                "requestCancelled",
                "stop",
                "Language-server session is stopping.",
                None,
            );
            clear_function_registry(self, &mut state);
            clear_runtime_diagnostics(self, &mut state);
            transition_locked(self, &mut state, LspLifecycleState::Stopping, None);
            plugin_operations::abandon(&self.startup_attempt);
            state.initialize_deadline = None;
            state.shutdown_deadline = Some(Instant::now() + state.shutdown_timeout);
            if state.client.initialized {
                let response = client_shutdown(ClientShutdownRequest {
                    state: state.client.clone(),
                })?;
                let request_id = (response.state.next_request_id - 1).to_string();
                let now = Instant::now();
                let shutdown_timeout = state.shutdown_timeout;
                state.pending.insert(
                    request_id,
                    PendingRequest {
                        kind: PendingKind::Shutdown,
                        operation_id: None,
                        method: "shutdown".to_string(),
                        document_uri: None,
                        document_version: None,
                        created_at: now,
                        deadline: now + shutdown_timeout,
                    },
                );
                state.client = response.state;
                (response.messages, false)
            } else {
                state.client.pending_requests.clear();
                state.pending.clear();
                (Vec::new(), true)
            }
        };
        self.send_messages_or_fail(&outbound_order, messages, "shutdown")?;
        if force_kill {
            self.kill_process();
        }
        Ok(())
    }

    pub(crate) fn poll_events(&self) -> Result<Vec<LspRuntimeEvent>, CoreError> {
        let mut state = self.lock_state()?;
        Ok(state.events.drain(..).collect())
    }

    pub(crate) fn wait_events(&self, timeout: Duration) -> Result<Vec<LspRuntimeEvent>, CoreError> {
        let mut state = self.lock_state()?;
        let deadline = Instant::now() + timeout;
        loop {
            lithe_protocol::cancellation::check()?;
            if !state.events.is_empty() {
                return Ok(state.events.drain(..).collect());
            }
            if matches!(
                state.lifecycle,
                LspLifecycleState::Stopped | LspLifecycleState::Failed
            ) {
                // Returning Ok([]) here would let frontend pumps spin on Core IPC
                // forever after the terminal stateChanged event was drained.
                let details = match state.lifecycle {
                    LspLifecycleState::Failed => "sessionFailed",
                    _ => "sessionStopped",
                };
                return Err(CoreError::new(
                    ErrorCode::ProcessFailed,
                    "Language-server session is no longer running.",
                )
                .with_details(details));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(Vec::new());
            }
            let (guard, wait_result) =
                self.event_signal
                    .wait_timeout(state, remaining)
                    .map_err(|_| {
                        CoreError::new(
                            ErrorCode::Unknown,
                            "Language-server event wait lock was poisoned.",
                        )
                    })?;
            state = guard;
            if wait_result.timed_out() && state.events.is_empty() {
                return Ok(Vec::new());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Result<EngineSnapshot, CoreError> {
        let state = self.lock_state()?;
        Ok(EngineSnapshot {
            session_id: self.id.clone(),
            provider_id: self.provider_id.clone(),
            root_uri: self.root_uri.clone(),
            state: state.lifecycle,
            initialized: state.client.initialized,
            open_documents: state.client.open_documents.clone(),
            pending_operation_ids: {
                let mut ids: Vec<String> = state.request_by_operation.keys().cloned().collect();
                ids.sort();
                ids
            },
            diagnostic_versions: state.client.diagnostic_versions.clone(),
        })
    }

    pub(crate) fn spawn_readers(
        self: &Arc<Self>,
        mut stdout: Box<dyn Read + Send>,
        mut stderr: Box<dyn Read + Send>,
    ) {
        let output_session = self.clone();
        thread::spawn(move || {
            let _host = lithe_workspace::enter(output_session.workspace_host.clone());
            let mut frame_buffer = Vec::new();
            let mut chunk = vec![0_u8; 8 * 1024];
            while output_session.active.load(Ordering::Acquire) {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        match parse_server_messages(ParseServerMessagesRequest {
                            buffer: std::mem::take(&mut frame_buffer),
                            chunk: chunk[..count].to_vec(),
                        }) {
                            Ok(parsed) => {
                                frame_buffer = parsed.buffer;
                                for message in parsed.messages {
                                    if let Err(error) =
                                        output_session.handle_server_message(message)
                                    {
                                        output_session.fail(
                                            "invalidServerMessage",
                                            "transport",
                                            "Language server sent an invalid message.",
                                            Some(core_error_detail(&error)),
                                            None,
                                        );
                                        output_session.kill_process();
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                output_session.fail(
                                    "transportFailed",
                                    "transport",
                                    "Language-server stdout framing failed.",
                                    Some(core_error_detail(&error)),
                                    None,
                                );
                                output_session.kill_process();
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        output_session.fail(
                            "transportFailed",
                            "transport",
                            "Could not read language-server stdout.",
                            Some(error.to_string()),
                            None,
                        );
                        output_session.kill_process();
                        return;
                    }
                }
            }
        });

        let error_session = self.clone();
        thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            let mut lines = StderrLines::default();
            let started = Instant::now();
            let emit = |fact| match fact {
                StderrFact::Line {
                    diagnostic,
                    truncated,
                } => error_session.log(
                    "debug",
                    EngineLog::StderrReceived
                        .with_fields(json!({"diagnostic": diagnostic, "truncated": truncated})),
                    "Language-server stderr",
                    Some(diagnostic),
                ),
                StderrFact::Dropped(count) => error_session.log(
                    "warning",
                    EngineLog::Dropped.with_fields(json!({"count": count})),
                    "Engine stderr lines dropped",
                    Some(format!("count={count}")),
                ),
            };
            while error_session.active.load(Ordering::Acquire) {
                match stderr.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        lines.feed(&chunk[..count], started.elapsed().as_millis() as u64, emit)
                    }
                    Err(error) => {
                        error_session.log_issue(
                            "warning",
                            "stderr",
                            "stderrReadFailed",
                            EngineLog::StderrReadFailed,
                            "Could not read language-server stderr",
                            Some(error.to_string()),
                        );
                        break;
                    }
                }
            }
            lines.finish(started.elapsed().as_millis() as u64, emit);
        });
    }

    pub(crate) fn spawn_monitor(self: &Arc<Self>) {
        let session = self.clone();
        thread::spawn(move || {
            while session.active.load(Ordering::Acquire) {
                if let Some(exit_code) = session.process.exit_status() {
                    session.handle_process_exit(exit_code);
                    break;
                }
                session.expire_deadlines();
                thread::sleep(Duration::from_millis(MONITOR_INTERVAL_MS));
            }
        });
    }
}

impl RuntimeSession {
    /// One session-owned writer sends deadline cancellations off the monitor thread.
    /// The monitor remains free to terminate a stalled write and expire other callers.
    pub(crate) fn spawn_outbound_maintenance(self: &Arc<Self>) {
        let session = self.clone();
        thread::spawn(move || {
            while session.active.load(Ordering::Acquire) {
                session.pump_outbound_maintenance();
                thread::sleep(Duration::from_millis(MONITOR_INTERVAL_MS));
            }
        });
    }

    fn pump_outbound_maintenance(&self) {
        if !self
            .lock_state()
            .is_ok_and(|state| !state.deadline_cancellations.is_empty())
        {
            return;
        }
        let Ok(outbound_order) = self.outbound_order.try_lock() else {
            return;
        };
        let Ok(mut state) = self.lock_state() else {
            return;
        };
        let messages: Vec<String> = std::mem::take(&mut state.deadline_cancellations)
            .into_iter()
            .map(|id| {
                json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": id } })
                    .to_string()
            })
            .collect();
        if messages.is_empty() {
            return;
        }
        state.maintenance_write_deadline = Some(Instant::now() + state.request_timeout);
        drop(state);
        // Process termination by the monitor releases a blocked native stdin write.
        // log-exempt: Same failure already recorded: send_messages_or_fail transitions the session and emits its transport failure.
        let _ = self.send_messages_or_fail(&outbound_order, messages, "outboundMaintenance");
        if let Ok(mut state) = self.lock_state() {
            state.maintenance_write_deadline = None;
        }
    }

    fn expire_deadlines(&self) {
        self.expire_deadlines_at(Instant::now());
    }

    pub(crate) fn expire_deadlines_at(&self, now: Instant) {
        // Relayed calls that no callee runs yet have no session deadline.
        self.relay.expire(now);
        let mut cancellations = Vec::new();
        let mut initialize_timeout = false;
        let mut shutdown_timeout = false;
        let mut maintenance_write_timeout = false;
        let mut relay_write_timeout = false;
        if let Ok(mut state) = self.lock_state() {
            maintenance_write_timeout = state
                .maintenance_write_deadline
                .is_some_and(|deadline| now >= deadline);
            if maintenance_write_timeout {
                state.maintenance_write_deadline = None;
            }
            relay_write_timeout = state
                .relay_write_deadline
                .is_some_and(|deadline| now >= deadline);
            if relay_write_timeout {
                state.relay_write_deadline = None;
            }
            if state.lifecycle == LspLifecycleState::Initializing
                && state
                    .initialize_deadline
                    .is_some_and(|deadline| now >= deadline)
            {
                state.initialize_deadline = None;
                initialize_timeout = true;
                // Decided under the state lock, like the initialize response,
                // so exactly one of the two records the attempt.
                plugin_operations::finish(&self.startup_attempt, Some(AttemptStage::Timeout));
            }
            let expired: Vec<_> = state
                .pending
                .iter()
                .filter(|(_, pending)| {
                    matches!(
                        pending.kind,
                        PendingKind::Feature | PendingKind::PluginServer
                    ) && now >= pending.deadline
                })
                .map(|(id, _)| id.clone())
                .collect();
            for request_id in expired {
                let Some(pending) = state.pending.remove(&request_id) else {
                    continue;
                };
                state.client.pending_requests.remove(&request_id);
                state.client.pending_semantic_legends.remove(&request_id);
                if let Some(operation_id) = pending.operation_id.as_deref() {
                    state.request_by_operation.remove(operation_id);
                    state.function_by_operation.remove(operation_id);
                    let elapsed = now.saturating_duration_since(pending.created_at);
                    let error = runtime_error(
                        self,
                        "requestTimeout",
                        "request",
                        "Language-server request timed out.",
                        FailureContext::request(&pending.method, pending.document_uri.as_deref())
                            .with_underlying(Some(&format!(
                                "elapsedMilliseconds={}",
                                elapsed.as_millis()
                            ))),
                    );
                    push_request_event(
                        self,
                        &mut state,
                        operation_id,
                        &pending.method,
                        None,
                        Some(error),
                    );
                }
                cancellations.push(request_id);
            }
            let held_expired = state
                .plugin_server
                .as_ref()
                .map(|server| server.expired_held(now))
                .unwrap_or_default();
            for waiter in held_expired {
                // Nothing was sent, so there is nothing to cancel on the wire.
                let error = runtime_error(
                    self,
                    "requestTimeout",
                    "request",
                    "Language-server request timed out waiting for the server to become ready.",
                    FailureContext::request(&waiter.method, None),
                );
                push_request_event(
                    self,
                    &mut state,
                    &waiter.operation_id,
                    &waiter.method,
                    None,
                    Some(error),
                );
            }
            if state.lifecycle == LspLifecycleState::Stopping
                && state
                    .shutdown_deadline
                    .is_some_and(|deadline| now >= deadline)
            {
                state.shutdown_deadline = None;
                shutdown_timeout = true;
                let message = "Language-server shutdown timed out; forcing termination";
                let error = runtime_error(
                    self,
                    "shutdownTimeout",
                    "shutdown",
                    message,
                    FailureContext::default(),
                );
                push_classified_log_event(
                    self,
                    &mut state,
                    "warning",
                    EngineLog::ShutdownTimedOut,
                    message,
                    None,
                    Some(error),
                );
            }
        }
        if maintenance_write_timeout {
            self.fail(
                "transportFailed",
                "outboundMaintenance",
                "Language-server stdin write timed out.",
                None,
                None,
            );
            self.kill_process();
        } else if relay_write_timeout {
            // Killing the process releases the relay lane's blocked stdin write.
            self.fail(
                "transportFailed",
                "relayWrite",
                "Language-server stdin write timed out.",
                None,
                None,
            );
            self.kill_process();
        } else if initialize_timeout {
            // Timeout termination must not wait behind a blocked stdin write;
            // killing the process is what releases that write.
            self.fail(
                "initializeTimeout",
                "initialize",
                "Language-server initialization timed out.",
                None,
                None,
            );
            self.kill_process();
        } else if shutdown_timeout {
            self.kill_process();
        }
        if !cancellations.is_empty() {
            if let Ok(mut state) = self.lock_state() {
                if !matches!(
                    state.lifecycle,
                    LspLifecycleState::Stopped | LspLifecycleState::Failed
                ) {
                    state.deadline_cancellations.extend(cancellations);
                }
            }
        }
    }

    fn handle_process_exit(&self, exit_code: Option<i32>) {
        self.active.store(false, Ordering::Release);
        self.process.close_input();
        if let Ok(mut state) = self.lock_state() {
            let was_stopping = state.lifecycle == LspLifecycleState::Stopping;
            if !was_stopping {
                plugin_operations::finish(&self.startup_attempt, Some(AttemptStage::Exit));
            }
            if !state.terminal_event_emitted {
                let error = (!was_stopping).then(|| {
                    runtime_error(
                        self,
                        "serverExited",
                        "process",
                        "Language-server process exited.",
                        FailureContext::default().with_exit_code(exit_code),
                    )
                });
                fail_feature_requests(
                    self,
                    &mut state,
                    "serverExited",
                    "process",
                    "Language-server process exited before the request completed.",
                    exit_code,
                );
                clear_runtime_state(self, &mut state);
                transition_locked(
                    self,
                    &mut state,
                    if was_stopping {
                        LspLifecycleState::Stopped
                    } else {
                        LspLifecycleState::Failed
                    },
                    error,
                );
                state.terminal_event_emitted = true;
            }
        }
    }

    pub(crate) fn complete_request_with_error(
        &self,
        request_id: &str,
        code: &str,
        stage: &str,
        message: &str,
        underlying: Option<String>,
        exit_code: Option<i32>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            let Some(pending) = state.pending.remove(request_id) else {
                return;
            };
            state.client.pending_requests.remove(request_id);
            state.client.pending_semantic_legends.remove(request_id);
            if let Some(operation_id) = pending.operation_id.as_deref() {
                state.request_by_operation.remove(operation_id);
                state.function_by_operation.remove(operation_id);
                let error = runtime_error(
                    self,
                    code,
                    stage,
                    message,
                    FailureContext::request(&pending.method, pending.document_uri.as_deref())
                        .with_underlying(underlying.as_deref())
                        .with_exit_code(exit_code),
                );
                push_request_event(
                    self,
                    &mut state,
                    operation_id,
                    &pending.method,
                    None,
                    Some(error),
                );
            }
        }
    }

    pub(crate) fn fail(
        &self,
        code: &str,
        stage: &str,
        message: &str,
        underlying: Option<String>,
        exit_code: Option<i32>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            if state.terminal_event_emitted {
                return;
            }
            // Timeouts were already taken by their caller; every other terminal
            // failure before `initialize` succeeded is a handshake failure.
            plugin_operations::finish(&self.startup_attempt, Some(AttemptStage::Initialize));
            fail_feature_requests(self, &mut state, code, stage, message, exit_code);
            clear_runtime_state(self, &mut state);
            let error = runtime_error(
                self,
                code,
                stage,
                message,
                FailureContext::default()
                    .with_underlying(underlying.as_deref())
                    .with_exit_code(exit_code),
            );
            transition_locked(self, &mut state, LspLifecycleState::Failed, Some(error));
            state.terminal_event_emitted = true;
        }
    }

    pub(crate) fn transition(
        &self,
        lifecycle: LspLifecycleState,
        error: Option<LspRuntimeError>,
    ) -> Result<(), CoreError> {
        let mut state = self.lock_state()?;
        transition_locked(self, &mut state, lifecycle, error);
        Ok(())
    }

    pub(crate) fn set_detailed_diagnostics(&self, enabled: bool) -> Result<(), CoreError> {
        let order = self.lock_outbound_order()?;
        let mut state = self.lock_state()?;
        let lifecycle = state.lifecycle;
        if matches!(
            lifecycle,
            LspLifecycleState::Stopped | LspLifecycleState::Failed | LspLifecycleState::Stopping
        ) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "The session is no longer running.",
            ));
        }
        if !state.client.initialized {
            state.requested_diagnostics = enabled;
            return Ok(());
        }
        drop(state);
        self.send_messages_or_fail(&order, vec![trace_notification(enabled)], "setTrace")?;
        let mut state = self.lock_state()?;
        state.requested_diagnostics = enabled;
        state.detailed_diagnostics = enabled;
        Ok(())
    }

    pub(crate) fn log(
        &self,
        level: &str,
        identity: impl Into<EngineLogInput>,
        message: &str,
        detail: Option<String>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            push_log_event(self, &mut state, level, identity, message, detail);
        }
    }

    pub(crate) fn log_issue(
        &self,
        level: &str,
        stage: &str,
        code: &str,
        identity: impl Into<EngineLogInput>,
        message: &str,
        detail: Option<String>,
    ) {
        if let Ok(mut state) = self.lock_state() {
            let error = runtime_error(self, code, stage, message, FailureContext::default());
            push_classified_log_event(
                self,
                &mut state,
                level,
                identity,
                message,
                detail,
                Some(error),
            );
        }
    }

    pub(crate) fn send_messages_or_fail(
        &self,
        outbound_order: &MutexGuard<'_, ()>,
        messages: Vec<String>,
        stage: &str,
    ) -> Result<(), CoreError> {
        if messages.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.send_messages(outbound_order, messages) {
            self.fail(
                "transportFailed",
                stage,
                "Could not write to language-server stdin.",
                Some(core_error_detail(&error)),
                None,
            );
            self.kill_process();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn send_messages(
        &self,
        _outbound_order: &MutexGuard<'_, ()>,
        messages: Vec<String>,
    ) -> Result<(), CoreError> {
        for message in messages {
            let message = match &self.workspace_host {
                Some(host) => {
                    lithe_lsp_transport::remote_process::map_message(host, &message, true, None)?
                }
                None => message,
            };
            let frame = frame_message(FrameMessageRequest { message })?.frame;
            self.process.write_input(frame.as_bytes())?;
        }
        Ok(())
    }

    pub(crate) fn kill_process(&self) {
        self.process.terminate();
        if let Some(error) = self.process.take_termination_failure() {
            // The session owns process recovery; WARN reports cleanup degradation.
            // Reuse the role routing and runtime envelope without adding a v1 log row.
            if let Ok(mut state) = self.lock_state() {
                let category = format!("{:?}", error.kind());
                let error = runtime_error(
                    self,
                    "processCleanupFailed",
                    "cleanup",
                    "Engine process cleanup failed",
                    FailureContext::default(),
                );
                let input = EngineLog::ProcessCleanupFailed.with_fields(json!({}));
                let (id, mut fields) =
                    input
                        .identity
                        .fact(&self.log_identity, &input.fields, Some(&error));
                fields.insert("error_code".into(), json!(category));
                fields.insert("error_domain".into(), json!("io"));
                push_cleanup_fact(self, &mut state, id, fields, error);
            }
        }
    }

    pub(crate) fn lock_outbound_order(&self) -> Result<MutexGuard<'_, ()>, CoreError> {
        self.outbound_order.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Language-server outbound ordering lock was poisoned.",
            )
        })
    }

    pub(crate) fn lock_indexing_roots(&self) -> Result<MutexGuard<'_, Vec<String>>, CoreError> {
        self.indexing_roots
            .lock()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "LSP indexing lock was poisoned"))
    }

    pub(crate) fn lock_state(&self) -> Result<MutexGuard<'_, SessionState>, CoreError> {
        self.state.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Language-server session state lock was poisoned.",
            )
        })
    }
}

pub(crate) fn trace_notification(enabled: bool) -> String {
    json!({"jsonrpc":"2.0", "method":"$/setTrace", "params":{"value": if enabled { "verbose" } else { "off" }}}).to_string()
}

// Cleanup failure is a log row, never a state change: hosts take the latest
// `stateChanged` as the session state, so a stateless one would erase `failed`.
pub(crate) fn push_cleanup_fact(
    session: &RuntimeSession,
    state: &mut SessionState,
    id: String,
    fields: std::collections::BTreeMap<String, Value>,
    error: LspRuntimeError,
) {
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "log".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: Some(id),
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
            error: Some(error),
            capabilities: None,
            completion_options: None,
            server_info: None,
            level: Some("warning".to_string()),
            message: Some("Engine process cleanup failed".to_string()),
            detail: None,
        },
    );
}

pub(crate) fn transition_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    lifecycle: LspLifecycleState,
    error: Option<LspRuntimeError>,
) {
    state.lifecycle = lifecycle;
    relay_hub::observe_lifecycle(session, lifecycle);
    let (id, fields) = EngineLog::SessionUpdated.fact(
        &session.log_identity,
        &json!({"state": lifecycle}),
        error.as_ref(),
    );
    let sequence = take_sequence(state);
    enqueue_runtime_event(
        session,
        state,
        LspRuntimeEvent {
            kind: "stateChanged".to_string(),
            channel: None,
            plugin_id: None,
            role: None,
            event: Some(id),
            fields: Some(fields),
            catalog_level: None,
            sequence,
            provider_id: session.provider_id.clone(),
            session_id: session.id.clone(),
            state: Some(lifecycle),
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
            level: None,
            message: None,
            detail: None,
        },
    );
}
