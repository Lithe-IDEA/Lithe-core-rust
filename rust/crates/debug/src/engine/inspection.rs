//! Inspection: threads, stacks, scopes, variables, evaluation, memory, disassembly and
//! completions, and their normalized results.

use super::*;

/// Longest opaque memory or instruction reference the Core forwards to an adapter.
const MAXIMUM_MEMORY_REFERENCE_LENGTH: usize = 256;

/// Largest `readMemory` request, so one call cannot ask an adapter for unbounded bytes.
const MAXIMUM_READ_MEMORY_BYTES: i64 = 65_536;

/// Largest `disassemble` window, enough for a scrolling view without flooding the transport.
const MAXIMUM_DISASSEMBLE_INSTRUCTIONS: i64 = 1_000;

/// Queues one capability-gated variable mutation while execution is paused.
pub fn set_variable(mut request: SetVariableRequest) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.operation_id, "operationId")?;
    if request.variables_reference < 1 {
        return Err(invalid_request(
            "Debug variablesReference must be positive.",
        ));
    }
    request.name = request.name.trim().to_string();
    if request.name.is_empty() {
        return Err(invalid_request(
            "Debug setVariable requires a variable name.",
        ));
    }
    with_session(&request.session_id, |session| {
        if session.state != DebugSessionState::Paused {
            return Err(invalid_request(
                "Variable mutation requires a paused debug session.",
            ));
        }
        if !session.capabilities.supports_set_variable {
            return Err(invalid_request(
                "The debug adapter does not support variable mutation.",
            ));
        }
        session.send_request(
            "setVariable",
            json!({
                "variablesReference": request.variables_reference,
                "name": request.name,
                "value": request.value
            }),
            PendingRequest::SetVariable {
                operation_id: request.operation_id,
                name: request.name,
            },
        )?;
        Ok(session.take_update())
    })
}

/// Queues one typed thread, stack, scope, variable, or evaluation request.
pub fn inspect(request: InspectRequest) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.operation_id, "operationId")?;
    let arguments = inspect_arguments(&request)?;
    with_session(&request.session_id, |session| {
        if !matches!(
            session.state,
            DebugSessionState::Running | DebugSessionState::Paused
        ) {
            return Err(invalid_request(
                "Debugger inspection requires a running or paused session.",
            ));
        }
        if request.kind == DebugInspectKind::RedefineClasses
            && session.features.hot_code_replace.is_none()
        {
            return Err(invalid_request(
                "The debug adapter does not declare hot code replacement.",
            ));
        }
        if request.kind == DebugInspectKind::StepInTargets
            && !session.capabilities.supports_step_in_targets_request
        {
            return Err(invalid_request(
                "The debug adapter does not support smart step into.",
            ));
        }
        if request.kind == DebugInspectKind::GotoTargets
            && !session.capabilities.supports_goto_targets_request
        {
            return Err(invalid_request(
                "The debug adapter does not support run to cursor.",
            ));
        }
        if request.kind == DebugInspectKind::Completions
            && !session.capabilities.supports_completions_request
        {
            return Err(invalid_request(
                "The debug adapter does not support console completion.",
            ));
        }
        if request.kind == DebugInspectKind::Disassemble {
            if session.state != DebugSessionState::Paused {
                return Err(invalid_request(
                    "Disassembly requires a paused debug session.",
                ));
            }
            if !session.capabilities.supports_disassemble_request {
                return Err(invalid_request(
                    "The debug adapter does not support disassembly.",
                ));
            }
        }
        if request.kind == DebugInspectKind::ReadMemory {
            if session.state != DebugSessionState::Paused {
                return Err(invalid_request(
                    "Reading memory requires a paused debug session.",
                ));
            }
            if !session.capabilities.supports_read_memory_request {
                return Err(invalid_request(
                    "The debug adapter does not support reading memory.",
                ));
            }
        }
        if request.kind == DebugInspectKind::ExceptionInfo {
            if session.state != DebugSessionState::Paused {
                return Err(invalid_request(
                    "Exception information requires a paused debug session.",
                ));
            }
            if !session.capabilities.supports_exception_info_request {
                return Err(invalid_request(
                    "The debug adapter does not support exception information.",
                ));
            }
        }
        let command = match (request.kind, &session.features.hot_code_replace) {
            (DebugInspectKind::RedefineClasses, Some(feature)) => feature.request.clone(),
            (kind, _) => kind.command().to_string(),
        };
        session.send_request(
            &command,
            Value::Object(arguments),
            PendingRequest::Inspect {
                operation_id: request.operation_id,
                kind: request.kind,
                thread_id: request.thread_id,
                frame_id: request.frame_id,
            },
        )?;
        Ok(session.take_update())
    })
}

impl DebugSession {
    /// Emits the v2 projection of a stack trace or scopes reply beside the v1 result.
    pub(crate) fn emit_inspection_projection(
        &mut self,
        kind: DebugInspectKind,
        thread_id: Option<i64>,
        frame_id: Option<i64>,
        body: &Value,
    ) {
        let projection = match (kind, thread_id, frame_id) {
            (DebugInspectKind::StackTrace, Some(thread_id), _) => stack_trace_projection(
                &self.id,
                self.stop_generation,
                thread_id,
                body,
                &self.root_path,
                |name, path, hint| {
                    stack_frame_matches_filters(
                        name,
                        path,
                        hint,
                        &self.stepping_filters,
                        &self.root_path,
                    )
                },
            ),
            (DebugInspectKind::Scopes, _, Some(frame_id)) => scopes_projection(
                &self.id,
                self.stop_generation,
                frame_id,
                body,
                &self.root_path,
            ),
            _ => None,
        };
        if let Some(projection) = projection {
            self.emit(DebugEventBody::Projection { projection });
        }
    }
}

pub(crate) fn inspect_arguments(request: &InspectRequest) -> Result<Map<String, Value>, CoreError> {
    // `readMemory` reuses `count` for its byte length; every other kind rejects paging fields.
    let paging_allowed = request.kind == DebugInspectKind::Variables;
    let count_allowed = paging_allowed || request.kind == DebugInspectKind::ReadMemory;
    if (!paging_allowed && (request.variable_filter.is_some() || request.start.is_some()))
        || (!count_allowed && request.count.is_some())
    {
        return Err(invalid_request(
            "Debug variable paging is only valid for variables inspection.",
        ));
    }
    let is_memory_kind = matches!(
        request.kind,
        DebugInspectKind::Disassemble | DebugInspectKind::ReadMemory
    );
    if !is_memory_kind
        && (request.memory_reference.is_some()
            || request.offset.is_some()
            || request.instruction_offset.is_some()
            || request.instruction_count.is_some()
            || request.resolve_symbols.is_some())
    {
        return Err(invalid_request(
            "Debug memory fields are only valid for disassemble or readMemory.",
        ));
    }
    let mut arguments = Map::new();
    match request.kind {
        DebugInspectKind::Threads | DebugInspectKind::RedefineClasses => {}
        DebugInspectKind::StackTrace => {
            arguments.insert(
                "threadId".to_string(),
                json!(required_positive(request.thread_id, "threadId")?),
            );
        }
        DebugInspectKind::Scopes => {
            arguments.insert(
                "frameId".to_string(),
                json!(required_nonnegative(request.frame_id, "frameId")?),
            );
        }
        DebugInspectKind::Variables => {
            arguments.insert(
                "variablesReference".to_string(),
                json!(required_positive(
                    request.variables_reference,
                    "variablesReference"
                )?),
            );
            if let Some(filter) = request.variable_filter {
                arguments.insert("filter".to_string(), json!(filter.argument()));
            }
            if let Some(start) = request.start {
                arguments.insert(
                    "start".to_string(),
                    json!(required_nonnegative(Some(start), "start")?),
                );
            }
            if let Some(count) = request.count {
                arguments.insert(
                    "count".to_string(),
                    json!(required_positive(Some(count), "count")?),
                );
            }
        }
        DebugInspectKind::Evaluate => {
            let expression = request.expression.as_deref().unwrap_or_default().trim();
            if expression.is_empty() {
                return Err(invalid_request("Debug evaluate requires an expression."));
            }
            arguments.insert("expression".to_string(), json!(expression));
            let context = request.context.as_deref().unwrap_or("watch");
            if !matches!(
                context,
                "watch" | "hover" | "repl" | "clipboard" | "variables"
            ) {
                return Err(invalid_request(
                    "Debug evaluate context must be watch, hover, repl, clipboard or variables.",
                ));
            }
            arguments.insert("context".to_string(), json!(context));
            if let Some(frame_id) = request.frame_id {
                if frame_id < 0 {
                    return Err(invalid_request("Debug frameId cannot be negative."));
                }
                arguments.insert("frameId".to_string(), json!(frame_id));
            }
        }
        DebugInspectKind::ExceptionInfo => {
            arguments.insert(
                "threadId".to_string(),
                json!(required_positive(request.thread_id, "threadId")?),
            );
        }
        DebugInspectKind::StepInTargets => {
            arguments.insert(
                "frameId".to_string(),
                json!(required_nonnegative(request.frame_id, "frameId")?),
            );
        }
        DebugInspectKind::GotoTargets => {
            let source_path = request.source_path.as_deref().unwrap_or_default().trim();
            if source_path.is_empty() {
                return Err(invalid_request("Debug gotoTargets requires a source path."));
            }
            arguments.insert("source".to_string(), json!({"path": source_path}));
            arguments.insert(
                "line".to_string(),
                json!(required_positive(request.line, "line")?),
            );
            if let Some(column) = request.column {
                if column < 1 {
                    return Err(invalid_request("Debug column must be positive."));
                }
                arguments.insert("column".to_string(), json!(column));
            }
        }
        DebugInspectKind::Disassemble => {
            let reference = request.memory_reference.as_deref().unwrap_or_default();
            arguments.insert(
                "memoryReference".to_string(),
                json!(validated_memory_reference(reference)?),
            );
            insert_option(&mut arguments, "offset", request.offset);
            insert_option(
                &mut arguments,
                "instructionOffset",
                request.instruction_offset,
            );
            let count = required_positive(request.instruction_count, "instructionCount")?;
            if count > MAXIMUM_DISASSEMBLE_INSTRUCTIONS {
                return Err(invalid_request(
                    "Debug instructionCount cannot exceed 1000 instructions.",
                ));
            }
            arguments.insert("instructionCount".to_string(), json!(count));
            arguments.insert(
                "resolveSymbols".to_string(),
                json!(request.resolve_symbols.unwrap_or(true)),
            );
        }
        DebugInspectKind::ReadMemory => {
            let reference = request.memory_reference.as_deref().unwrap_or_default();
            arguments.insert(
                "memoryReference".to_string(),
                json!(validated_memory_reference(reference)?),
            );
            insert_option(&mut arguments, "offset", request.offset);
            let count = required_positive(request.count, "count")?;
            if count > MAXIMUM_READ_MEMORY_BYTES {
                return Err(invalid_request(
                    "Debug readMemory count cannot exceed 65536 bytes.",
                ));
            }
            arguments.insert("count".to_string(), json!(count));
        }
        DebugInspectKind::Completions => {
            // Whitespace is significant: `break ` and `break` complete differently.
            let text = request.expression.as_deref().unwrap_or_default();
            if text.is_empty() {
                return Err(invalid_request("Debug completions require text."));
            }
            arguments.insert("text".to_string(), json!(text));
            // The caret defaults to the end of the text. DAP columns count UTF-16
            // units from 1, matching the `columnsStartAt1` the session initialized with.
            let end = text.encode_utf16().count() as i64 + 1;
            let column = request.column.unwrap_or(end);
            if column < 1 || column > end {
                return Err(invalid_request(
                    "Debug completion column must lie within the text.",
                ));
            }
            arguments.insert("column".to_string(), json!(column));
            if let Some(frame_id) = request.frame_id {
                if frame_id < 0 {
                    return Err(invalid_request("Debug frameId cannot be negative."));
                }
                arguments.insert("frameId".to_string(), json!(frame_id));
            }
        }
    }
    Ok(arguments)
}

pub(crate) fn normalize_inspection(
    kind: DebugInspectKind,
    body: &Value,
    stepping_filters: &DebugSteppingFilters,
    root_path: &str,
) -> Result<DebugOperationResult, CoreError> {
    match kind {
        DebugInspectKind::Threads => Ok(DebugOperationResult::Threads {
            threads: required_array(body, "threads")?
                .iter()
                .filter_map(parse_thread)
                .collect(),
        }),
        DebugInspectKind::StackTrace => Ok(DebugOperationResult::StackTrace {
            stack_frames: required_array(body, "stackFrames")?
                .iter()
                .filter_map(|value| parse_stack_frame(value, stepping_filters, root_path))
                .collect(),
        }),
        DebugInspectKind::Scopes => Ok(DebugOperationResult::Scopes {
            scopes: required_array(body, "scopes")?
                .iter()
                .filter_map(parse_scope)
                .collect(),
        }),
        DebugInspectKind::Variables => Ok(DebugOperationResult::Variables {
            variables: required_array(body, "variables")?
                .iter()
                .filter_map(parse_variable)
                .collect(),
        }),
        DebugInspectKind::Evaluate => Ok(DebugOperationResult::Evaluate {
            variable: DebugVariable {
                name: body
                    .get("evaluateName")
                    .and_then(Value::as_str)
                    .unwrap_or("Expression")
                    .to_string(),
                value: required_str(body, "result")?.to_string(),
                r#type: string_field(body, "type"),
                evaluate_name: string_field(body, "evaluateName"),
                variables_reference: body
                    .get("variablesReference")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                named_variables: nonnegative_count_field(body, "namedVariables"),
                indexed_variables: nonnegative_count_field(body, "indexedVariables"),
            },
        }),
        DebugInspectKind::ExceptionInfo => Ok(DebugOperationResult::ExceptionInfo {
            exception_info: DebugExceptionInfo {
                exception_id: required_str(body, "exceptionId")?.to_string(),
                description: string_field(body, "description"),
                break_mode: required_str(body, "breakMode")?.to_string(),
                details: body.get("details").and_then(parse_exception_details),
            },
        }),
        DebugInspectKind::RedefineClasses => {
            // An adapter may return DAP success with an application-level error.
            if let Some(message) = body
                .get("errorMessage")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                return Err(invalid_request(message));
            }
            let mut changed_classes = required_array(body, "changedClasses")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid_request("Invalid changed class name."))
                })
                .collect::<Result<Vec<_>, _>>()?;
            changed_classes.sort();
            changed_classes.dedup();
            Ok(DebugOperationResult::RedefineClasses { changed_classes })
        }
        DebugInspectKind::StepInTargets => Ok(DebugOperationResult::StepInTargets {
            targets: required_array(body, "targets")?
                .iter()
                .filter_map(parse_step_in_target)
                .collect(),
        }),
        DebugInspectKind::GotoTargets => Ok(DebugOperationResult::GotoTargets {
            targets: required_array(body, "targets")?
                .iter()
                .filter_map(parse_goto_target)
                .collect(),
        }),
        DebugInspectKind::Completions => Ok(DebugOperationResult::Completions {
            items: required_array(body, "targets")?
                .iter()
                .filter_map(parse_completion_item)
                .collect(),
        }),
        DebugInspectKind::Disassemble => Ok(DebugOperationResult::Disassemble {
            instructions: required_array(body, "instructions")?
                .iter()
                .filter_map(parse_disassembled_instruction)
                .collect(),
        }),
        DebugInspectKind::ReadMemory => {
            // Memory that is entirely unreadable comes back without `data`.
            let data_base64 = string_field(body, "data").unwrap_or_default();
            if BASE64.decode(&data_base64).is_err() {
                return Err(CoreError::new(
                    ErrorCode::ParseFailed,
                    "DAP readMemory data was not valid base64.",
                ));
            }
            Ok(DebugOperationResult::ReadMemory {
                address: required_str(body, "address")?.to_string(),
                unreadable_bytes: nonnegative_count_field(body, "unreadableBytes"),
                data_base64,
            })
        }
    }
}

/// Rejects empty, oversized or multi-line references; the value itself stays opaque.
pub(crate) fn validated_memory_reference(reference: &str) -> Result<String, CoreError> {
    let reference = reference.trim();
    if reference.is_empty()
        || reference.chars().count() > MAXIMUM_MEMORY_REFERENCE_LENGTH
        || reference.chars().any(char::is_control)
    {
        return Err(invalid_request(
            "Debug memory references must be short single-line adapter references.",
        ));
    }
    Ok(reference.to_string())
}

fn parse_disassembled_instruction(value: &Value) -> Option<DebugDisassembledInstruction> {
    let address = value.get("address")?.as_str()?;
    let instruction = value.get("instruction")?.as_str()?;
    if address.is_empty() {
        return None;
    }
    Some(DebugDisassembledInstruction {
        address: address.to_string(),
        instruction_bytes: string_field(value, "instructionBytes").filter(|text| !text.is_empty()),
        instruction: instruction.to_string(),
        symbol: string_field(value, "symbol").filter(|text| !text.is_empty()),
        source_path: value
            .get("location")
            .and_then(|source| source.get("path"))
            .and_then(Value::as_str)
            .map(str::to_string),
        line: value
            .get("line")
            .and_then(Value::as_i64)
            .filter(|line| *line > 0),
    })
}

fn parse_exception_details(value: &Value) -> Option<DebugExceptionDetails> {
    value.as_object()?;
    Some(DebugExceptionDetails {
        message: string_field(value, "message"),
        type_name: string_field(value, "typeName"),
        full_type_name: string_field(value, "fullTypeName"),
        evaluate_name: string_field(value, "evaluateName"),
        stack_trace: string_field(value, "stackTrace"),
        inner_exceptions: value
            .get("innerException")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(parse_exception_details)
            .collect(),
    })
}

fn parse_step_in_target(value: &Value) -> Option<DebugStepInTarget> {
    Some(DebugStepInTarget {
        id: value.get("id")?.as_i64()?,
        label: value.get("label")?.as_str()?.to_string(),
        line: value.get("line").and_then(Value::as_i64),
        column: value.get("column").and_then(Value::as_i64),
        end_line: value.get("endLine").and_then(Value::as_i64),
        end_column: value.get("endColumn").and_then(Value::as_i64),
    })
}

fn parse_goto_target(value: &Value) -> Option<DebugGotoTarget> {
    Some(DebugGotoTarget {
        id: value.get("id")?.as_i64()?,
        label: value.get("label")?.as_str()?.to_string(),
        line: value.get("line")?.as_i64()?,
        column: value.get("column").and_then(Value::as_i64),
        end_line: value.get("endLine").and_then(Value::as_i64),
        end_column: value.get("endColumn").and_then(Value::as_i64),
        instruction_pointer_reference: string_field(value, "instructionPointerReference"),
    })
}

fn parse_completion_item(value: &Value) -> Option<DebugCompletionItem> {
    let label = value.get("label")?.as_str()?;
    if label.is_empty() {
        return None;
    }
    // The wire range counts from 1 with the session's `columnsStartAt1`; clients get 0.
    let offset = |field: &str| {
        value
            .get(field)
            .and_then(Value::as_i64)
            .filter(|number| *number >= 0)
    };
    Some(DebugCompletionItem {
        label: label.to_string(),
        text: string_field(value, "text")
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| label.to_string()),
        detail: string_field(value, "detail"),
        kind: string_field(value, "type"),
        start: offset("start").map(|start| (start - 1).max(0)),
        length: offset("length"),
    })
}

fn parse_thread(value: &Value) -> Option<DebugThread> {
    Some(DebugThread {
        id: value.get("id")?.as_i64()?,
        name: value.get("name")?.as_str()?.to_string(),
    })
}

fn parse_stack_frame(
    value: &Value,
    stepping_filters: &DebugSteppingFilters,
    root_path: &str,
) -> Option<DebugStackFrame> {
    let name = value.get("name")?.as_str()?.to_string();
    let source_path = value
        .get("source")
        .and_then(|source| source.get("path"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let source_reference = value
        .get("source")
        .and_then(|source| source.get("sourceReference"))
        .and_then(Value::as_i64)
        .filter(|reference| *reference > 0);
    let presentation_hint = value.get("presentationHint").and_then(Value::as_str);
    // A frame in code without debug info has no file; the Core must not invent line 1 of
    // nothing, so the UI can show the function and its instruction address instead.
    let has_source = source_path.is_some() || source_reference.is_some();
    Some(DebugStackFrame {
        id: value.get("id")?.as_i64()?,
        is_filtered: stack_frame_matches_filters(
            &name,
            source_path.as_deref(),
            presentation_hint,
            stepping_filters,
            root_path,
        ),
        name,
        source_path,
        source_reference,
        line: value
            .get("line")
            .and_then(Value::as_i64)
            .unwrap_or(if has_source { 1 } else { 0 }),
        column: value.get("column").and_then(Value::as_i64).unwrap_or(1),
        instruction_pointer_reference: string_field(value, "instructionPointerReference")
            .filter(|reference| !reference.is_empty()),
    })
}

fn parse_scope(value: &Value) -> Option<DebugScope> {
    Some(DebugScope {
        name: value.get("name")?.as_str()?.to_string(),
        variables_reference: value.get("variablesReference")?.as_i64()?,
        expensive: value
            .get("expensive")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        named_variables: nonnegative_count_field(value, "namedVariables"),
        indexed_variables: nonnegative_count_field(value, "indexedVariables"),
        presentation_hint: string_field(value, "presentationHint").filter(|hint| !hint.is_empty()),
    })
}

fn parse_variable(value: &Value) -> Option<DebugVariable> {
    Some(DebugVariable {
        name: value.get("name")?.as_str()?.to_string(),
        value: value.get("value")?.as_str()?.to_string(),
        r#type: string_field(value, "type"),
        evaluate_name: string_field(value, "evaluateName"),
        variables_reference: value
            .get("variablesReference")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        named_variables: nonnegative_count_field(value, "namedVariables"),
        indexed_variables: nonnegative_count_field(value, "indexedVariables"),
    })
}

pub(crate) fn nonnegative_count_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0).max(0)
}
