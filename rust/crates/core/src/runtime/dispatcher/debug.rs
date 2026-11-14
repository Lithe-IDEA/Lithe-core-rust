//! `debug.*` commands: debug adapters, transports, sessions, breakpoints and inspection.

use super::{plugin_response, reducer_response};
use crate::protocol::{CoreError, CoreResponse, DebugCommand, ErrorCode};
/// The manifest type every domain command validates plugin manifests with.
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
use serde_json::{json, Value};

/// Runs one `debug.*` command.
pub(super) fn dispatch(command: DebugCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match command {
        DebugCommand::ValidateAdapterEndpoint => {
            match crate::debug::validate_adapter_endpoint(payload) {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::TransportPolicy => match crate::debug::transport_policy(payload) {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        DebugCommand::TransportHandle => match crate::debug::handle_transport_event(payload) {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        DebugCommand::TransportRelease => match crate::debug::release_transport(payload) {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        DebugCommand::AdaptersStartArguments => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::debug_start_arguments_command,
        ),
        DebugCommand::AdaptersExecutablePlan => plugin_response(
            id,
            payload,
            lithe_plugin_manifest::debug_executable_plan_command,
        ),
        DebugCommand::StartOrContinue => match crate::debug::start_or_continue(payload) {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        DebugCommand::DecodeBackendConfiguration => plugin_response(
            id,
            payload,
            crate::debug::decode_backend_configuration_command::<Manifest>,
        ),
        DebugCommand::CreateSession => {
            match serde_json::from_value::<crate::debug::CreateSessionRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug create-session request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::create_session)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug session update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::Launch => {
            match serde_json::from_value::<crate::debug::LaunchRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid debug launch request")
                        .with_details(error.to_string())
                })
                .and_then(crate::debug::launch)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug launch update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::SteppingFilters => {
            match serde_json::from_value::<crate::debug::DebugSteppingFiltersRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug stepping-filters request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::stepping_filters)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug stepping filters should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::PlanVariablePage => reducer_response(
            id,
            payload,
            "Invalid debug variable page plan request",
            crate::debug::plan_variable_page,
        ),
        DebugCommand::ReduceVariablePage => reducer_response(
            id,
            payload,
            "Invalid debug variable page reduction request",
            crate::debug::reduce_variable_page,
        ),
        DebugCommand::PlanDisassemblyWindow => reducer_response(
            id,
            payload,
            "Invalid debug disassembly window request",
            crate::debug::plan_disassembly_window,
        ),
        DebugCommand::ReduceDisassemblyPage => reducer_response(
            id,
            payload,
            "Invalid debug disassembly page reduction request",
            crate::debug::reduce_disassembly_page,
        ),
        DebugCommand::PlanMemoryWindow => reducer_response(
            id,
            payload,
            "Invalid debug memory window request",
            crate::debug::plan_memory_window,
        ),
        DebugCommand::LocateMemoryWindow => reducer_response(
            id,
            payload,
            "Invalid debug memory window location request",
            crate::debug::locate_memory_window,
        ),
        DebugCommand::SetBreakpointsMuted => reducer_response(
            id,
            payload,
            "Invalid debug set-breakpoints-muted request",
            crate::debug::set_breakpoints_muted,
        ),
        DebugCommand::RelocateBreakpoints => {
            match serde_json::from_value::<crate::debug::RelocateBreakpointsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug relocate-breakpoints request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::relocate_breakpoints)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug breakpoint relocation should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::SetBreakpoints => {
            match serde_json::from_value::<crate::debug::SetBreakpointsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug set-breakpoints request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::set_breakpoints)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug breakpoint update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::SetExceptionBreakpoints => {
            match serde_json::from_value::<crate::debug::SetExceptionBreakpointsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug set-exception-breakpoints request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::set_exception_breakpoints)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Debug exception breakpoint update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::SetFunctionBreakpoints => {
            match serde_json::from_value::<crate::debug::SetFunctionBreakpointsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug set-function-breakpoints request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::set_function_breakpoints)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Debug function breakpoint update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::SetInstructionBreakpoints => {
            match serde_json::from_value::<crate::debug::SetInstructionBreakpointsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug set-instruction-breakpoints request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::set_instruction_breakpoints)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Debug instruction breakpoint update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::DataBreakpointInfo => {
            match serde_json::from_value::<crate::debug::DataBreakpointInfoRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug data-breakpoint-info request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::data_breakpoint_info)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Debug data breakpoint info update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::SetDataBreakpoints => {
            match serde_json::from_value::<crate::debug::SetDataBreakpointsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug set-data-breakpoints request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::set_data_breakpoints)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug data breakpoint update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::SetVariable => {
            match serde_json::from_value::<crate::debug::SetVariableRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug set-variable request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::set_variable)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug variable update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::CancelOperation => {
            match serde_json::from_value::<crate::debug::CancelOperationRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug cancel-operation request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::cancel_operation)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug cancellation update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::Execute => {
            match serde_json::from_value::<crate::debug::ExecuteRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid debug execute request")
                        .with_details(error.to_string())
                })
                .and_then(crate::debug::execute)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug execution update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::Inspect => {
            match serde_json::from_value::<crate::debug::InspectRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid debug inspect request")
                        .with_details(error.to_string())
                })
                .and_then(crate::debug::inspect)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug inspection update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::Receive => {
            match serde_json::from_value::<crate::debug::ReceiveRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid debug receive request")
                        .with_details(error.to_string())
                })
                .and_then(crate::debug::receive)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug receive update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::RunInTerminalResponse => {
            match serde_json::from_value::<crate::debug::DebugRunInTerminalResponseRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug run-in-terminal response",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::run_in_terminal_response)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Debug run-in-terminal response update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::StartDebuggingResponse => {
            match serde_json::from_value::<crate::debug::DebugStartDebuggingResponseRequest>(
                payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid debug start-debugging response",
                )
                .with_details(error.to_string())
            })
            .and_then(crate::debug::start_debugging_response)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Debug start-debugging response update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::Disconnect => {
            match serde_json::from_value::<crate::debug::DisconnectRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug disconnect request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::disconnect)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Debug disconnect update should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DebugCommand::DestroySession => {
            match serde_json::from_value::<crate::debug::SessionRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid debug destroy-session request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::debug::destroy_session)
            {
                Ok(destroyed_children) => CoreResponse::success(
                    id,
                    json!({"destroyed": true, "destroyedChildSessionIds": destroyed_children}),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    }
}
