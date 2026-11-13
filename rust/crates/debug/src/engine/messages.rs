//! DAP traffic of one session: outgoing requests and responses, and the responses, events and
//! reverse requests the adapter sends.

use super::*;

impl DebugSession {
    pub(crate) fn send_request(
        &mut self,
        command: &str,
        arguments: Value,
        pending: PendingRequest,
    ) -> Result<(), CoreError> {
        let sequence = self.next_request_sequence;
        self.next_request_sequence += 1;
        let message = json!({
            "seq": sequence,
            "type": "request",
            "command": command,
            "arguments": arguments
        });
        self.outbound_frames.push(frame_message(&message)?);
        self.request_stop_generations
            .insert(sequence, self.stop_generation);
        self.pending_requests.insert(sequence, pending);
        Ok(())
    }

    pub(crate) fn send_response(
        &mut self,
        request_sequence: i64,
        command: &str,
        success: bool,
        message: Option<&str>,
    ) -> Result<(), CoreError> {
        self.send_response_with_body(request_sequence, command, success, message, None)
    }

    pub(crate) fn send_response_with_body(
        &mut self,
        request_sequence: i64,
        command: &str,
        success: bool,
        message: Option<&str>,
        body: Option<Value>,
    ) -> Result<(), CoreError> {
        let sequence = self.next_request_sequence;
        self.next_request_sequence += 1;
        let mut response = json!({
            "seq": sequence,
            "type": "response",
            "request_seq": request_sequence,
            "success": success,
            "command": command
        });
        if let Some(message) = message {
            response["message"] = Value::String(message.to_string());
        }
        if let Some(body) = body {
            response["body"] = body;
        }
        self.outbound_frames.push(frame_message(&response)?);
        Ok(())
    }

    pub(crate) fn handle_message(&mut self, message: Value) -> Result<(), CoreError> {
        match message.get("type").and_then(Value::as_str) {
            Some("response") => self.handle_response(&message),
            Some("event") => self.handle_event(&message),
            Some("request") => self.handle_server_request(&message),
            _ => Err(CoreError::new(
                ErrorCode::ParseFailed,
                "DAP message did not contain a supported type.",
            )),
        }
    }

    fn handle_response(&mut self, message: &Value) -> Result<(), CoreError> {
        let request_sequence = required_i64(message, "request_seq")?;
        let Some(pending) = self.pending_requests.remove(&request_sequence) else {
            return Ok(());
        };
        let issued_generation = self.request_stop_generations.remove(&request_sequence);
        let success = message
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !success {
            let command = pending.command().to_string();
            let detail = adapter_failure_detail(message)
                .unwrap_or_else(|| "The debug adapter rejected the request.".to_string());
            if let Some(operation_id) = pending.operation_id() {
                self.emit(DebugEventBody::OperationFailed {
                    operation_id: operation_id.to_string(),
                    command,
                    code: DebugOperationFailureCode::AdapterRejected,
                    message: detail,
                });
            }
            if matches!(
                pending,
                PendingRequest::Initialize | PendingRequest::Launch { .. }
            ) {
                self.transition(DebugSessionState::Failed);
            }
            return Ok(());
        }
        if pending.is_reference_bound() && issued_generation != Some(self.stop_generation) {
            if let Some(operation_id) = pending.operation_id() {
                self.emit(DebugEventBody::OperationFailed {
                    operation_id: operation_id.to_string(),
                    command: pending.command().to_string(),
                    code: DebugOperationFailureCode::StaleReference,
                    message: "The debuggee resumed or stopped again before the adapter replied."
                        .to_string(),
                });
            }
            return Ok(());
        }
        let body = message.get("body").cloned().unwrap_or_else(|| json!({}));
        match pending {
            PendingRequest::Initialize => {
                self.adapter_capabilities = body.as_object().cloned().unwrap_or_default();
                self.refresh_capabilities(false);
                if !self.did_configure_exception_breakpoints {
                    self.exception_breakpoints = self.default_exception_breakpoints();
                }
                self.supports_configuration_done = self.capabilities.supports_configuration_done;
                self.transition(DebugSessionState::Ready);
                if let Some((operation_id, configuration)) = self.pending_launch.take() {
                    self.perform_launch(operation_id, configuration)?;
                }
            }
            PendingRequest::Launch { operation_id } => {
                // Adapters may emit stopped or terminated before this acknowledgement.
                // A late response completes the operation, but cannot resume or resurrect it.
                if self.state == DebugSessionState::Launching {
                    self.transition(DebugSessionState::Running);
                }
                self.emit(DebugEventBody::OperationCompleted {
                    operation_id,
                    result: DebugOperationResult::Acknowledged {
                        command: "launch".to_string(),
                    },
                });
            }
            PendingRequest::SetBreakpoints {
                source_path,
                requested,
                generation,
            } => {
                if self.breakpoint_generations.get(&source_path) == Some(&generation) {
                    self.emit_breakpoint_results(&body, &source_path, &requested);
                }
            }
            PendingRequest::SetExceptionBreakpoints => {}
            PendingRequest::SetFunctionBreakpoints { requested } => {
                self.emit_function_breakpoint_results(&body, &requested)
            }
            PendingRequest::SetInstructionBreakpoints { requested } => {
                self.emit_instruction_breakpoint_results(&body, &requested)
            }
            PendingRequest::DataBreakpointInfo { operation_id } => {
                self.emit(DebugEventBody::OperationCompleted {
                    operation_id,
                    result: DebugOperationResult::DataBreakpointInfo {
                        data_id: string_field(&body, "dataId"),
                        description: body
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        access_types: body
                            .get("accessTypes")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect(),
                        can_persist: bool_field(&body, "canPersist"),
                    },
                });
            }
            PendingRequest::SetDataBreakpoints { requested } => {
                self.emit_data_breakpoint_results(&body, &requested)
            }
            PendingRequest::SetVariable { operation_id, name } => {
                self.emit(DebugEventBody::OperationCompleted {
                    operation_id,
                    result: DebugOperationResult::SetVariable {
                        variable: DebugVariable {
                            name,
                            value: required_str(&body, "value")?.to_string(),
                            r#type: string_field(&body, "type"),
                            evaluate_name: None,
                            variables_reference: body
                                .get("variablesReference")
                                .and_then(Value::as_i64)
                                .unwrap_or(0),
                            named_variables: nonnegative_count_field(&body, "namedVariables"),
                            indexed_variables: nonnegative_count_field(&body, "indexedVariables"),
                        },
                    },
                });
            }
            PendingRequest::Cancel => {}
            PendingRequest::ConfigurationDone => {}
            PendingRequest::Execute {
                operation_id,
                command,
                single_thread,
            } => {
                // Some adapters send `stopped` before the reply to `next`; a newer generation
                // means an event already advanced the state, so the reply must not undo it.
                if command != DebugExecutionCommand::Pause
                    && command != DebugExecutionCommand::Terminate
                    && !(single_thread && command == DebugExecutionCommand::Continue)
                    && issued_generation == Some(self.stop_generation)
                {
                    self.stop_generation += 1;
                    self.transition(DebugSessionState::Running);
                }
                self.emit(DebugEventBody::OperationCompleted {
                    operation_id,
                    result: DebugOperationResult::Acknowledged {
                        command: command.command().to_string(),
                    },
                });
            }
            PendingRequest::Inspect {
                operation_id,
                kind,
                thread_id,
                frame_id,
            } => {
                self.emit_inspection_projection(kind, thread_id, frame_id, &body);
                let result = match (kind, &self.features.hot_code_replace) {
                    (DebugInspectKind::RedefineClasses, Some(feature)) => {
                        declared_replacement(feature, &body)
                    }
                    _ => normalize_inspection(kind, &body, &self.stepping_filters, &self.root_path),
                };
                // HCR rejection and unreadable memory are operation failures, not a broken
                // transport/session.
                if matches!(
                    kind,
                    DebugInspectKind::RedefineClasses
                        | DebugInspectKind::Disassemble
                        | DebugInspectKind::ReadMemory
                ) {
                    match result {
                        Ok(result) => self.emit(DebugEventBody::OperationCompleted {
                            operation_id,
                            result,
                        }),
                        Err(error) => self.emit(DebugEventBody::OperationFailed {
                            operation_id,
                            command: kind.command().into(),
                            code: DebugOperationFailureCode::AdapterRejected,
                            message: error.message,
                        }),
                    }
                } else {
                    self.emit(DebugEventBody::OperationCompleted {
                        operation_id,
                        result: result?,
                    });
                }
            }
            PendingRequest::Disconnect => {}
        }
        Ok(())
    }

    fn handle_event(&mut self, message: &Value) -> Result<(), CoreError> {
        let event = required_str(message, "event")?;
        let body = message.get("body").cloned().unwrap_or_else(|| json!({}));
        match event {
            "initialized" => {
                self.did_receive_initialized = true;
                self.emit(DebugEventBody::Initialized);
                self.send_exception_breakpoints()?;
                if self.capabilities.supports_function_breakpoints {
                    self.send_function_breakpoints()?;
                }
                if self.capabilities.supports_instruction_breakpoints {
                    self.send_instruction_breakpoints()?;
                }
                if self.capabilities.supports_data_breakpoints {
                    self.send_data_breakpoints()?;
                }
                let sources: Vec<String> = self.breakpoints.keys().cloned().collect();
                for source in sources {
                    self.send_breakpoints(&source)?;
                }
                if self.supports_configuration_done {
                    self.send_request(
                        "configurationDone",
                        json!({}),
                        PendingRequest::ConfigurationDone,
                    )?;
                }
            }
            "output" => self.emit(DebugEventBody::Output {
                category: body
                    .get("category")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                output: body
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            "stopped" => {
                self.stop_generation += 1;
                self.transition(DebugSessionState::Paused);
                self.emit(DebugEventBody::Stopped {
                    reason: body
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("pause")
                        .to_string(),
                    thread_id: body.get("threadId").and_then(Value::as_i64),
                    description: body
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    stop_generation: self.stop_generation,
                });
            }
            "continued" => {
                // Resuming one thread leaves the frames and variables of the others valid.
                if body
                    .get("allThreadsContinued")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
                {
                    self.stop_generation += 1;
                    self.transition(DebugSessionState::Running);
                }
                self.emit(DebugEventBody::Continued {
                    thread_id: body.get("threadId").and_then(Value::as_i64),
                });
            }
            "terminated" => {
                self.stop_generation += 1;
                self.transition(DebugSessionState::Terminated);
                self.request_children_stop();
                self.emit(DebugEventBody::Terminated { exit_code: None });
            }
            "exited" => {
                self.stop_generation += 1;
                self.transition(DebugSessionState::Terminated);
                self.request_children_stop();
                self.emit(DebugEventBody::Terminated {
                    exit_code: body.get("exitCode").and_then(Value::as_i64),
                });
            }
            "capabilities" => {
                if let Some(updated) = body.get("capabilities").and_then(Value::as_object) {
                    self.adapter_capabilities.extend(updated.clone());
                    self.refresh_capabilities(true);
                }
            }
            "breakpoint" => {
                if let Some(value) = body.get("breakpoint") {
                    let mut breakpoint = parse_breakpoint(value, None, None, 0);
                    if let Some(previous) = self.resolved_source_breakpoints.get(&breakpoint.id) {
                        breakpoint.requested_line = previous.requested_line;
                        breakpoint.source_path = breakpoint
                            .source_path
                            .or_else(|| previous.source_path.clone());
                        breakpoint.line = breakpoint.line.or(previous.line);
                        breakpoint.column = breakpoint.column.or(previous.column);
                        self.resolved_source_breakpoints
                            .insert(breakpoint.id, breakpoint.clone());
                    }
                    self.emit(DebugEventBody::Breakpoint { breakpoint });
                }
            }
            declared
                if self
                    .features
                    .hot_code_replace
                    .as_ref()
                    .and_then(|feature| feature.event.as_deref())
                    == Some(declared) =>
            {
                // The declared progress event shows its message in the Debug Console.
                if let Some(message) = body
                    .get("message")
                    .and_then(Value::as_str)
                    .filter(|message| !message.is_empty())
                {
                    self.emit(DebugEventBody::Output {
                        category: Some("console".to_string()),
                        output: format!("{message}\n"),
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_server_request(&mut self, message: &Value) -> Result<(), CoreError> {
        let request_sequence = required_i64(message, "seq")?;
        let command = required_str(message, "command")?;
        if command == "runInTerminal" && self.supports_run_in_terminal_request {
            let arguments = message.get("arguments").unwrap_or(&Value::Null);
            match parse_run_in_terminal_request(arguments) {
                Ok(request) => {
                    let request_id = format!("runInTerminal-{request_sequence}");
                    if self
                        .pending_run_in_terminal_requests
                        .contains_key(&request_id)
                    {
                        // One DAP sequence can have only one response. Keep the
                        // original native launch pending instead of replacing
                        // it or starting a second process for a malformed retry.
                        return Ok(());
                    }
                    self.pending_run_in_terminal_requests
                        .insert(request_id.clone(), request_sequence);
                    self.emit(DebugEventBody::RunInTerminalRequested {
                        request_id,
                        request,
                    });
                    return Ok(());
                }
                Err(error) => {
                    return self.send_response(
                        request_sequence,
                        command,
                        false,
                        Some(&error.message),
                    );
                }
            }
        }
        if command == "startDebugging" && self.supports_start_debugging_request {
            let arguments = message.get("arguments").unwrap_or(&Value::Null);
            return match parse_start_debugging_request(arguments) {
                Ok(request) => {
                    let request_id = format!("startDebugging-{request_sequence}");
                    if self
                        .pending_start_debugging_requests
                        .contains_key(&request_id)
                    {
                        // A DAP sequence has one response; a duplicate must not
                        // open a second child session.
                        return Ok(());
                    }
                    self.pending_start_debugging_requests
                        .insert(request_id.clone(), request_sequence);
                    self.emit(DebugEventBody::StartDebuggingRequested {
                        request_id,
                        request,
                    });
                    Ok(())
                }
                Err(error) => {
                    self.send_response(request_sequence, command, false, Some(&error.message))
                }
            };
        }
        self.send_response(
            request_sequence,
            command,
            false,
            Some("This debug adapter request is not supported by Lithe."),
        )
    }

    pub(crate) fn fail_pending_start_debugging_requests(
        &mut self,
        message: &str,
    ) -> Result<(), CoreError> {
        let pending = std::mem::take(&mut self.pending_start_debugging_requests);
        for request_sequence in pending.into_values() {
            self.send_response(request_sequence, "startDebugging", false, Some(message))?;
        }
        Ok(())
    }

    pub(crate) fn fail_pending_run_in_terminal_requests(
        &mut self,
        message: &str,
    ) -> Result<(), CoreError> {
        let pending = std::mem::take(&mut self.pending_run_in_terminal_requests);
        for (_, request_sequence) in pending {
            self.send_response(request_sequence, "runInTerminal", false, Some(message))?;
        }
        Ok(())
    }
}

pub(crate) fn parse_capabilities(value: &Value) -> DebugCapabilities {
    DebugCapabilities {
        supports_configuration_done: bool_field(value, "supportsConfigurationDoneRequest"),
        supports_conditional_breakpoints: bool_field(value, "supportsConditionalBreakpoints"),
        supports_hit_conditional_breakpoints: bool_field(
            value,
            "supportsHitConditionalBreakpoints",
        ),
        supports_log_points: bool_field(value, "supportsLogPoints"),
        supports_function_breakpoints: bool_field(value, "supportsFunctionBreakpoints"),
        supports_data_breakpoints: bool_field(value, "supportsDataBreakpoints"),
        supports_exception_options: bool_field(value, "supportsExceptionOptions"),
        supports_exception_filter_options: bool_field(value, "supportsExceptionFilterOptions"),
        supports_set_variable: bool_field(value, "supportsSetVariable"),
        supports_cancel_request: bool_field(value, "supportsCancelRequest"),
        supports_single_thread_execution_requests: bool_field(
            value,
            "supportsSingleThreadExecutionRequests",
        ),
        supports_restart_request: bool_field(value, "supportsRestartRequest"),
        supports_terminate_request: bool_field(value, "supportsTerminateRequest"),
        supports_step_back: bool_field(value, "supportsStepBack"),
        supports_exception_info_request: bool_field(value, "supportsExceptionInfoRequest"),
        supports_step_in_targets_request: bool_field(value, "supportsStepInTargetsRequest"),
        supports_goto_targets_request: bool_field(value, "supportsGotoTargetsRequest"),
        supports_completions_request: bool_field(value, "supportsCompletionsRequest"),
        supports_disassemble_request: bool_field(value, "supportsDisassembleRequest"),
        supports_stepping_granularity: bool_field(value, "supportsSteppingGranularity"),
        supports_instruction_breakpoints: bool_field(value, "supportsInstructionBreakpoints"),
        supports_read_memory_request: bool_field(value, "supportsReadMemoryRequest"),
        exception_breakpoint_filters: value
            .get("exceptionBreakpointFilters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(parse_exception_breakpoint_filter)
            .collect(),
    }
}

fn parse_start_debugging_request(value: &Value) -> Result<DebugStartDebuggingRequest, CoreError> {
    let arguments = value
        .as_object()
        .ok_or_else(|| invalid_request("Debug startDebugging arguments must be a JSON object."))?;
    let mut configuration = arguments
        .get("configuration")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| invalid_request("Debug startDebugging configuration must be an object."))?;
    // DAP names the kind on the envelope; the configuration keeps the same word
    // for adapters that read it, but Lithe routes on the typed field.
    let kind = arguments
        .get("request")
        .or_else(|| configuration.get("request"))
        .and_then(Value::as_str);
    let request = match kind {
        Some("launch") => DebugRequestKind::Launch,
        Some("attach") => DebugRequestKind::Attach,
        _ => {
            return Err(invalid_request(
                "Debug startDebugging request must be launch or attach.",
            ))
        }
    };
    configuration.remove("request");
    Ok(DebugStartDebuggingRequest {
        request,
        configuration,
    })
}

fn parse_run_in_terminal_request(value: &Value) -> Result<DebugRunInTerminalRequest, CoreError> {
    const MAX_ARGUMENT_COUNT: usize = 4_096;
    const MAX_ENVIRONMENT_COUNT: usize = 4_096;

    let object = value
        .as_object()
        .ok_or_else(|| invalid_request("Debug runInTerminal arguments must be a JSON object."))?;
    let kind = match object.get("kind") {
        None => DebugRunInTerminalKind::Integrated,
        Some(Value::String(value)) if value == "integrated" => DebugRunInTerminalKind::Integrated,
        Some(Value::String(value)) if value == "external" => DebugRunInTerminalKind::External,
        _ => {
            return Err(invalid_request(
                "Debug runInTerminal kind must be integrated or external.",
            ))
        }
    };
    let title = match object.get("title") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => normalize_optional_text(Some(value.clone())),
        _ => {
            return Err(invalid_request(
                "Debug runInTerminal title must be a string.",
            ))
        }
    };
    let cwd = object
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_request("Debug runInTerminal cwd must be a string."))?
        .to_string();
    if cwd.contains('\0') {
        return Err(invalid_request(
            "Debug runInTerminal cwd contains an invalid null byte.",
        ));
    }
    let argument_values = object
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_request("Debug runInTerminal args must be an array."))?;
    if argument_values.is_empty() || argument_values.len() > MAX_ARGUMENT_COUNT {
        return Err(invalid_request(
            "Debug runInTerminal args must contain between 1 and 4096 items.",
        ));
    }
    let mut args = Vec::with_capacity(argument_values.len());
    for value in argument_values {
        let argument = value.as_str().ok_or_else(|| {
            invalid_request("Debug runInTerminal args must contain only strings.")
        })?;
        if argument.contains('\0') {
            return Err(invalid_request(
                "Debug runInTerminal args contain an invalid null byte.",
            ));
        }
        args.push(argument.to_string());
    }
    if args[0].trim().is_empty() {
        return Err(invalid_request(
            "Debug runInTerminal args must begin with an executable.",
        ));
    }

    let mut environment = Vec::new();
    if let Some(value) = object.get("env") {
        let values = value
            .as_object()
            .ok_or_else(|| invalid_request("Debug runInTerminal env must be a JSON object."))?;
        if values.len() > MAX_ENVIRONMENT_COUNT {
            return Err(invalid_request(
                "Debug runInTerminal env cannot exceed 4096 entries.",
            ));
        }
        let mut names: Vec<&String> = values.keys().collect();
        names.sort();
        for name in names {
            if name.is_empty() || name.contains(['=', '\0']) {
                return Err(invalid_request(
                    "Debug runInTerminal env contains an invalid variable name.",
                ));
            }
            let value = match &values[name] {
                Value::Null => None,
                Value::String(value) if !value.contains('\0') => Some(value.clone()),
                _ => {
                    return Err(invalid_request(
                        "Debug runInTerminal env values must be strings or null.",
                    ))
                }
            };
            environment.push(DebugRunInTerminalEnvironmentVariable {
                name: name.clone(),
                value,
            });
        }
    }
    let args_can_be_interpreted_by_shell = match object.get("argsCanBeInterpretedByShell") {
        None => false,
        Some(Value::Bool(value)) => *value,
        _ => {
            return Err(invalid_request(
                "Debug runInTerminal argsCanBeInterpretedByShell must be a boolean.",
            ))
        }
    };

    Ok(DebugRunInTerminalRequest {
        kind,
        title,
        cwd,
        args,
        environment,
        args_can_be_interpreted_by_shell,
    })
}

pub(crate) fn validate_dap_process_id(value: Option<i64>, field: &str) -> Result<(), CoreError> {
    if value.is_some_and(|value| !(1..=i32::MAX as i64).contains(&value)) {
        return Err(invalid_request(&format!(
            "Debug runInTerminal {field} must be between 1 and {}.",
            i32::MAX
        )));
    }
    Ok(())
}
