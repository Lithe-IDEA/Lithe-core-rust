//! Breakpoints of every kind: host requests, the sets sent to the adapter, muting and the
//! verified results.

use super::*;

/// Stores a deterministic breakpoint set and sends it after DAP initialization.
pub fn set_breakpoints(
    mut request: SetBreakpointsRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    validate_path(&request.source_path, "sourcePath")?;
    for breakpoint in &request.breakpoints {
        if breakpoint.line < 1 || breakpoint.column.is_some_and(|column| column < 1) {
            return Err(invalid_request(
                "Debug breakpoint line and column values must be one-based.",
            ));
        }
    }
    request.breakpoints.sort_by_key(|breakpoint| {
        (
            breakpoint.line,
            breakpoint.column.unwrap_or(0),
            breakpoint.enabled,
            breakpoint.condition.clone().unwrap_or_default(),
            breakpoint.hit_condition.clone().unwrap_or_default(),
            breakpoint.log_message.clone().unwrap_or_default(),
        )
    });
    request.breakpoints.dedup();
    with_session(&request.session_id, |session| {
        session
            .resolved_source_breakpoints
            .retain(|_, point| point.source_path.as_deref() != Some(request.source_path.as_str()));
        session
            .breakpoints
            .insert(request.source_path.clone(), request.breakpoints);
        if session.did_receive_initialized {
            session.send_breakpoints(&request.source_path)?;
        }
        Ok(session.take_update())
    })
}

/// Stores deterministic exception filters and sends them after DAP initialization.
pub fn set_exception_breakpoints(
    mut request: SetExceptionBreakpointsRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    for breakpoint in &mut request.breakpoints {
        breakpoint.filter = breakpoint.filter.trim().to_string();
        if breakpoint.filter.is_empty() {
            return Err(invalid_request(
                "Debug exception breakpoint filters cannot be empty.",
            ));
        }
        breakpoint.condition = breakpoint
            .condition
            .take()
            .map(|condition| condition.trim().to_string())
            .filter(|condition| !condition.is_empty());
    }
    request.breakpoints.sort_by(|left, right| {
        (&left.filter, left.enabled, &left.condition).cmp(&(
            &right.filter,
            right.enabled,
            &right.condition,
        ))
    });
    request
        .breakpoints
        .dedup_by(|left, right| left.filter == right.filter);
    with_session(&request.session_id, |session| {
        session.exception_breakpoints = request.breakpoints;
        session.did_configure_exception_breakpoints = true;
        if session.did_receive_initialized {
            session.send_exception_breakpoints()?;
        }
        Ok(session.take_update())
    })
}

/// Stores deterministic function breakpoints and sends them when supported.
pub fn set_function_breakpoints(
    mut request: SetFunctionBreakpointsRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    for breakpoint in &mut request.breakpoints {
        breakpoint.name = breakpoint.name.trim().to_string();
        if breakpoint.name.is_empty() {
            return Err(invalid_request(
                "Debug function breakpoint names cannot be empty.",
            ));
        }
        breakpoint.condition = normalize_optional_text(breakpoint.condition.take());
        breakpoint.hit_condition = normalize_optional_text(breakpoint.hit_condition.take());
    }
    request.breakpoints.sort_by(|left, right| {
        (
            &left.name,
            left.enabled,
            &left.condition,
            &left.hit_condition,
        )
            .cmp(&(
                &right.name,
                right.enabled,
                &right.condition,
                &right.hit_condition,
            ))
    });
    request
        .breakpoints
        .dedup_by(|left, right| left.name == right.name);
    with_session(&request.session_id, |session| {
        session.function_breakpoints = request.breakpoints;
        if session.did_receive_initialized && session.capabilities.supports_function_breakpoints {
            session.send_function_breakpoints()?;
        }
        Ok(session.take_update())
    })
}

/// Stores deterministic instruction breakpoints and sends them when supported.
///
/// References are opaque address strings: they are only trimmed and bounded, never parsed,
/// so a 64-bit address survives untouched and no CPU-specific logic enters the Core.
pub fn set_instruction_breakpoints(
    mut request: SetInstructionBreakpointsRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    for breakpoint in &mut request.breakpoints {
        breakpoint.instruction_reference =
            validated_memory_reference(&breakpoint.instruction_reference)?;
        breakpoint.condition = normalize_optional_text(breakpoint.condition.take());
        breakpoint.hit_condition = normalize_optional_text(breakpoint.hit_condition.take());
    }
    request.breakpoints.sort_by(|left, right| {
        (
            &left.instruction_reference,
            left.offset,
            left.enabled,
            &left.condition,
            &left.hit_condition,
        )
            .cmp(&(
                &right.instruction_reference,
                right.offset,
                right.enabled,
                &right.condition,
                &right.hit_condition,
            ))
    });
    request.breakpoints.dedup_by(|left, right| {
        left.instruction_reference == right.instruction_reference && left.offset == right.offset
    });
    with_session(&request.session_id, |session| {
        session.instruction_breakpoints = request.breakpoints;
        if session.did_receive_initialized && session.capabilities.supports_instruction_breakpoints
        {
            session.send_instruction_breakpoints()?;
        }
        Ok(session.take_update())
    })
}

/// Resolves one adapter-owned data breakpoint identity for the selected variable.
pub fn data_breakpoint_info(
    mut request: DataBreakpointInfoRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.operation_id, "operationId")?;
    request.name = request.name.trim().to_string();
    if request.name.is_empty() {
        return Err(invalid_request(
            "Debug data breakpoint info requires a variable name.",
        ));
    }
    if request
        .variables_reference
        .is_some_and(|reference| reference < 1)
    {
        return Err(invalid_request(
            "Debug variablesReference must be positive.",
        ));
    }
    if request.frame_id.is_some_and(|frame_id| frame_id < 0) {
        return Err(invalid_request("Debug frameId cannot be negative."));
    }
    if request.variables_reference.is_none() && request.frame_id.is_none() {
        return Err(invalid_request(
            "Debug data breakpoint info requires a variable reference or frame.",
        ));
    }
    with_session(&request.session_id, |session| {
        if !session.capabilities.supports_data_breakpoints {
            return Err(invalid_request(
                "The debug adapter does not support data breakpoints.",
            ));
        }
        let mut arguments = Map::new();
        arguments.insert("name".to_string(), json!(request.name));
        insert_option(
            &mut arguments,
            "variablesReference",
            request.variables_reference,
        );
        insert_option(&mut arguments, "frameId", request.frame_id);
        session.send_request(
            "dataBreakpointInfo",
            Value::Object(arguments),
            PendingRequest::DataBreakpointInfo {
                operation_id: request.operation_id,
            },
        )?;
        Ok(session.take_update())
    })
}

/// Stores deterministic adapter-resolved data breakpoints and sends them when supported.
pub fn set_data_breakpoints(
    mut request: SetDataBreakpointsRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    for breakpoint in &mut request.breakpoints {
        breakpoint.data_id = breakpoint.data_id.trim().to_string();
        if breakpoint.data_id.is_empty() {
            return Err(invalid_request(
                "Debug data breakpoint identifiers cannot be empty.",
            ));
        }
        breakpoint.label = normalize_optional_text(breakpoint.label.take());
        breakpoint.access_type = normalize_optional_text(breakpoint.access_type.take());
        breakpoint.condition = normalize_optional_text(breakpoint.condition.take());
        breakpoint.hit_condition = normalize_optional_text(breakpoint.hit_condition.take());
    }
    request.breakpoints.sort_by(|left, right| {
        (
            &left.data_id,
            &left.access_type,
            left.enabled,
            &left.condition,
            &left.hit_condition,
        )
            .cmp(&(
                &right.data_id,
                &right.access_type,
                right.enabled,
                &right.condition,
                &right.hit_condition,
            ))
    });
    request.breakpoints.dedup_by(|left, right| {
        left.data_id == right.data_id && left.access_type == right.access_type
    });
    with_session(&request.session_id, |session| {
        session.data_breakpoints = request.breakpoints;
        if session.did_receive_initialized && session.capabilities.supports_data_breakpoints {
            session.send_data_breakpoints()?;
        }
        Ok(session.take_update())
    })
}

/// Mutes or unmutes every breakpoint of one session (`debug.setBreakpointsMuted`).
///
/// Muting keeps the stored source, exception, function, instruction and data breakpoints
/// and sends each set empty; unmuting sends them as stored again. Only a set holding an
/// enabled breakpoint is resent, since muting does not change what any other set sends.
/// Before DAP initialization the state is only stored and applies to the first sets sent.
pub fn set_breakpoints_muted(
    request: SetBreakpointsMutedRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    with_session(&request.session_id, |session| {
        if session.breakpoints_muted != request.muted {
            session.breakpoints_muted = request.muted;
            if session.did_receive_initialized {
                session.resend_enabled_breakpoints()?;
            }
        }
        Ok(session.take_update())
    })
}

impl DebugSession {
    /// Whether a stored breakpoint with this enabled state goes to the adapter.
    fn sends(&self, enabled: bool) -> bool {
        enabled && !self.breakpoints_muted
    }

    /// Sends again every set holding an enabled breakpoint, after muting changed what
    /// those sets send. An adapter is never sent a kind it did not declare.
    fn resend_enabled_breakpoints(&mut self) -> Result<(), CoreError> {
        if self
            .exception_breakpoints
            .iter()
            .any(|breakpoint| breakpoint.enabled)
        {
            self.send_exception_breakpoints()?;
        }
        if self.capabilities.supports_function_breakpoints
            && self
                .function_breakpoints
                .iter()
                .any(|breakpoint| breakpoint.enabled)
        {
            self.send_function_breakpoints()?;
        }
        if self.capabilities.supports_instruction_breakpoints
            && self
                .instruction_breakpoints
                .iter()
                .any(|breakpoint| breakpoint.enabled)
        {
            self.send_instruction_breakpoints()?;
        }
        if self.capabilities.supports_data_breakpoints
            && self
                .data_breakpoints
                .iter()
                .any(|breakpoint| breakpoint.enabled)
        {
            self.send_data_breakpoints()?;
        }
        let sources: Vec<String> = self
            .breakpoints
            .iter()
            .filter(|(_, points)| points.iter().any(|breakpoint| breakpoint.enabled))
            .map(|(path, _)| path.clone())
            .collect();
        for source in sources {
            self.send_breakpoints(&source)?;
        }
        Ok(())
    }

    pub(crate) fn send_breakpoints(&mut self, source_path: &str) -> Result<(), CoreError> {
        let generation = self
            .breakpoint_generations
            .entry(source_path.to_string())
            .or_default();
        *generation += 1;
        let generation = *generation;
        let requested = self
            .breakpoints
            .get(source_path)
            .cloned()
            .unwrap_or_default();
        let active: Vec<SourceBreakpoint> = requested
            .iter()
            .filter(|breakpoint| self.sends(breakpoint.enabled))
            .cloned()
            .collect();
        let breakpoints: Vec<Value> = active
            .iter()
            .map(|breakpoint| {
                let mut value = Map::new();
                value.insert("line".to_string(), json!(breakpoint.line));
                insert_option(&mut value, "column", breakpoint.column);
                insert_nonempty(&mut value, "condition", breakpoint.condition.as_deref());
                insert_nonempty(
                    &mut value,
                    "hitCondition",
                    breakpoint.hit_condition.as_deref(),
                );
                insert_nonempty(&mut value, "logMessage", breakpoint.log_message.as_deref());
                Value::Object(value)
            })
            .collect();
        let source_name = source_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(source_path);
        self.send_request(
            "setBreakpoints",
            json!({
                "source": {"name": source_name, "path": source_path},
                "breakpoints": breakpoints,
                "sourceModified": false
            }),
            PendingRequest::SetBreakpoints {
                source_path: source_path.to_string(),
                requested: active,
                generation,
            },
        )
    }

    pub(crate) fn send_exception_breakpoints(&mut self) -> Result<(), CoreError> {
        let active: Vec<&ExceptionBreakpoint> = self
            .exception_breakpoints
            .iter()
            .filter(|breakpoint| self.sends(breakpoint.enabled))
            .collect();
        let filters: Vec<&str> = active
            .iter()
            .map(|breakpoint| breakpoint.filter.as_str())
            .collect();
        let filter_options: Vec<Value> = if self.capabilities.supports_exception_filter_options {
            active
                .iter()
                .filter_map(|breakpoint| {
                    breakpoint.condition.as_ref().map(|condition| {
                        json!({
                            "filterId": breakpoint.filter,
                            "condition": condition
                        })
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut arguments = Map::new();
        arguments.insert("filters".to_string(), json!(filters));
        if !filter_options.is_empty() {
            arguments.insert("filterOptions".to_string(), Value::Array(filter_options));
        }
        self.send_request(
            "setExceptionBreakpoints",
            Value::Object(arguments),
            PendingRequest::SetExceptionBreakpoints,
        )
    }

    pub(crate) fn send_function_breakpoints(&mut self) -> Result<(), CoreError> {
        let requested: Vec<FunctionBreakpoint> = self
            .function_breakpoints
            .iter()
            .filter(|breakpoint| self.sends(breakpoint.enabled))
            .cloned()
            .collect();
        let breakpoints: Vec<Value> = requested
            .iter()
            .map(|breakpoint| {
                let mut value = Map::new();
                value.insert("name".to_string(), json!(breakpoint.name));
                insert_nonempty(&mut value, "condition", breakpoint.condition.as_deref());
                insert_nonempty(
                    &mut value,
                    "hitCondition",
                    breakpoint.hit_condition.as_deref(),
                );
                Value::Object(value)
            })
            .collect();
        self.send_request(
            "setFunctionBreakpoints",
            json!({"breakpoints": breakpoints}),
            PendingRequest::SetFunctionBreakpoints { requested },
        )
    }

    pub(crate) fn send_instruction_breakpoints(&mut self) -> Result<(), CoreError> {
        let requested: Vec<InstructionBreakpoint> = self
            .instruction_breakpoints
            .iter()
            .filter(|breakpoint| self.sends(breakpoint.enabled))
            .cloned()
            .collect();
        let breakpoints: Vec<Value> = requested
            .iter()
            .map(|breakpoint| {
                let mut value = Map::new();
                value.insert(
                    "instructionReference".to_string(),
                    json!(breakpoint.instruction_reference),
                );
                insert_option(&mut value, "offset", breakpoint.offset);
                insert_nonempty(&mut value, "condition", breakpoint.condition.as_deref());
                insert_nonempty(
                    &mut value,
                    "hitCondition",
                    breakpoint.hit_condition.as_deref(),
                );
                Value::Object(value)
            })
            .collect();
        self.send_request(
            "setInstructionBreakpoints",
            json!({"breakpoints": breakpoints}),
            PendingRequest::SetInstructionBreakpoints { requested },
        )
    }

    pub(crate) fn send_data_breakpoints(&mut self) -> Result<(), CoreError> {
        let requested: Vec<DataBreakpoint> = self
            .data_breakpoints
            .iter()
            .filter(|breakpoint| self.sends(breakpoint.enabled))
            .cloned()
            .collect();
        let breakpoints: Vec<Value> = requested
            .iter()
            .map(|breakpoint| {
                let mut value = Map::new();
                value.insert("dataId".to_string(), json!(breakpoint.data_id));
                insert_nonempty(&mut value, "accessType", breakpoint.access_type.as_deref());
                insert_nonempty(&mut value, "condition", breakpoint.condition.as_deref());
                insert_nonempty(
                    &mut value,
                    "hitCondition",
                    breakpoint.hit_condition.as_deref(),
                );
                Value::Object(value)
            })
            .collect();
        self.send_request(
            "setDataBreakpoints",
            json!({"breakpoints": breakpoints}),
            PendingRequest::SetDataBreakpoints { requested },
        )
    }
}

impl DebugSession {
    pub(crate) fn emit_breakpoint_results(
        &mut self,
        body: &Value,
        source_path: &str,
        requested: &[SourceBreakpoint],
    ) {
        let values = body
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, value) in values.iter().enumerate() {
            let fallback = requested.get(index).map(|breakpoint| breakpoint.line);
            let mut breakpoint =
                parse_breakpoint(value, None, Some(source_path), fallback.unwrap_or(0));
            breakpoint.requested_line = fallback;
            if value.get("id").and_then(Value::as_i64).is_some() {
                self.resolved_source_breakpoints
                    .insert(breakpoint.id, breakpoint.clone());
            }
            self.emit(DebugEventBody::Breakpoint { breakpoint });
        }
    }

    pub(crate) fn emit_function_breakpoint_results(
        &mut self,
        body: &Value,
        requested: &[FunctionBreakpoint],
    ) {
        let values = body
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, value) in values.iter().enumerate() {
            let function_name = requested
                .get(index)
                .map(|breakpoint| breakpoint.name.as_str());
            self.emit(DebugEventBody::Breakpoint {
                breakpoint: parse_breakpoint(value, function_name, None, 0),
            });
        }
    }

    pub(crate) fn emit_instruction_breakpoint_results(
        &mut self,
        body: &Value,
        requested: &[InstructionBreakpoint],
    ) {
        let values = body
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, value) in values.iter().enumerate() {
            let mut breakpoint = parse_breakpoint(value, None, None, 0);
            // Adapters may omit the reference; the request at the same index owns it.
            if breakpoint.instruction_reference.is_none() {
                breakpoint.instruction_reference = requested
                    .get(index)
                    .map(|item| item.instruction_reference.clone());
            }
            self.emit(DebugEventBody::Breakpoint { breakpoint });
        }
    }

    pub(crate) fn emit_data_breakpoint_results(
        &mut self,
        body: &Value,
        requested: &[DataBreakpoint],
    ) {
        let values = body
            .get("breakpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, value) in values.iter().enumerate() {
            let mut breakpoint = parse_breakpoint(value, None, None, 0);
            breakpoint.data_id = requested.get(index).map(|item| item.data_id.clone());
            self.emit(DebugEventBody::Breakpoint { breakpoint });
        }
    }
}

pub(crate) fn parse_breakpoint(
    value: &Value,
    function_name: Option<&str>,
    source_path: Option<&str>,
    fallback_line: i64,
) -> DebugBreakpoint {
    DebugBreakpoint {
        id: value.get("id").and_then(Value::as_i64).unwrap_or(0),
        verified: value
            .get("verified")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        message: string_field(value, "message"),
        function_name: function_name.map(str::to_string),
        data_id: None,
        instruction_reference: string_field(value, "instructionReference"),
        requested_line: None,
        source_path: value
            .get("source")
            .and_then(|source| source.get("path"))
            .and_then(Value::as_str)
            .or(source_path)
            .map(str::to_string),
        line: value
            .get("line")
            .and_then(Value::as_i64)
            .or((fallback_line > 0).then_some(fallback_line)),
        column: value.get("column").and_then(Value::as_i64),
    }
}

pub(crate) fn parse_exception_breakpoint_filter(
    value: &Value,
) -> Option<DebugExceptionBreakpointFilter> {
    let filter = value.get("filter")?.as_str()?.trim();
    let label = value.get("label")?.as_str()?.trim();
    if filter.is_empty() || label.is_empty() {
        return None;
    }
    Some(DebugExceptionBreakpointFilter {
        filter: filter.to_string(),
        label: label.to_string(),
        description: string_field(value, "description"),
        default: bool_field(value, "default"),
        supports_condition: bool_field(value, "supportsCondition"),
        condition_description: string_field(value, "conditionDescription"),
    })
}
