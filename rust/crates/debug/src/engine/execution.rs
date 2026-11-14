//! Execution control: continue, step, pause, restart, goto and smart step, gated on the
//! adapter's capabilities.

use super::*;

/// Queues one continue, pause, or stepping request.
pub fn execute(request: ExecuteRequest) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.operation_id, "operationId")?;
    with_session(&request.session_id, |session| {
        if !matches!(
            session.state,
            DebugSessionState::Running | DebugSessionState::Paused
        ) {
            return Err(invalid_request(
                "Execution control requires a running or paused debug session.",
            ));
        }
        if matches!(
            request.command,
            DebugExecutionCommand::Next
                | DebugExecutionCommand::StepIn
                | DebugExecutionCommand::StepOut
                | DebugExecutionCommand::StepBack
                | DebugExecutionCommand::Goto
        ) && session.state != DebugSessionState::Paused
        {
            return Err(invalid_request("Stepping requires a paused debug session."));
        }
        if request.command == DebugExecutionCommand::Pause
            && session.state != DebugSessionState::Running
        {
            return Err(invalid_request("Pause requires a running debug session."));
        }
        if request.command == DebugExecutionCommand::Continue
            && session.state != DebugSessionState::Paused
        {
            return Err(invalid_request("Continue requires a paused debug session."));
        }
        if request.single_thread
            && !session
                .capabilities
                .supports_single_thread_execution_requests
        {
            return Err(invalid_request(
                "The debug adapter does not support single-thread execution control.",
            ));
        }
        if request.command == DebugExecutionCommand::StepBack
            && !session.capabilities.supports_step_back
        {
            return Err(invalid_request(
                "The debug adapter does not support stepping backwards.",
            ));
        }
        if request.command == DebugExecutionCommand::Goto
            && !session.capabilities.supports_goto_targets_request
        {
            return Err(invalid_request(
                "The debug adapter does not support run to cursor.",
            ));
        }
        if request.command == DebugExecutionCommand::Restart
            && !session.capabilities.supports_restart_request
        {
            return Err(invalid_request(
                "The debug adapter does not support restart requests.",
            ));
        }
        if request.command == DebugExecutionCommand::Terminate
            && !session.capabilities.supports_terminate_request
        {
            return Err(invalid_request(
                "The debug adapter does not support terminate requests.",
            ));
        }
        if matches!(
            request.command,
            DebugExecutionCommand::Next
                | DebugExecutionCommand::StepIn
                | DebugExecutionCommand::StepOut
                | DebugExecutionCommand::StepBack
                | DebugExecutionCommand::Goto
        ) && request.thread_id.is_none()
        {
            return Err(invalid_request("Stepping requires a selected thread."));
        }
        if let Some(granularity) = request.granularity {
            if !matches!(
                request.command,
                DebugExecutionCommand::Next
                    | DebugExecutionCommand::StepIn
                    | DebugExecutionCommand::StepOut
                    | DebugExecutionCommand::StepBack
            ) {
                return Err(invalid_request(
                    "Debug granularity is only valid for next, stepIn, stepOut or stepBack.",
                ));
            }
            if !session.capabilities.supports_stepping_granularity {
                return Err(invalid_request(
                    "The debug adapter does not support choosing the stepping granularity.",
                )
                .with_details(granularity.argument().to_string()));
            }
        }
        let mut arguments = Map::new();
        if !matches!(
            request.command,
            DebugExecutionCommand::Restart | DebugExecutionCommand::Terminate
        ) {
            if let Some(thread_id) = request.thread_id {
                arguments.insert("threadId".to_string(), json!(thread_id));
            }
        }
        if request.command == DebugExecutionCommand::Goto {
            insert_option(
                &mut arguments,
                "targetId",
                Some(required_positive(request.target_id, "targetId")?),
            );
        } else if request.command == DebugExecutionCommand::StepIn {
            if let Some(target_id) = request.target_id {
                if target_id < 1 {
                    return Err(invalid_request("Debug targetId must be positive."));
                }
                arguments.insert("targetId".to_string(), json!(target_id));
            }
        } else if request.target_id.is_some() {
            return Err(invalid_request(
                "Debug targetId is only valid for stepIn or goto.",
            ));
        }
        if matches!(
            request.command,
            DebugExecutionCommand::Continue
                | DebugExecutionCommand::Next
                | DebugExecutionCommand::StepIn
                | DebugExecutionCommand::StepOut
                | DebugExecutionCommand::StepBack
                | DebugExecutionCommand::Goto
                | DebugExecutionCommand::Pause
        ) {
            arguments.insert(
                "singleThread".to_string(),
                Value::Bool(request.single_thread),
            );
        }
        if let Some(granularity) = request.granularity {
            arguments.insert("granularity".to_string(), json!(granularity.argument()));
        }
        session.send_request(
            request.command.command(),
            Value::Object(arguments),
            PendingRequest::Execute {
                operation_id: request.operation_id,
                command: request.command,
                single_thread: request.single_thread,
            },
        )?;
        Ok(session.take_update())
    })
}
