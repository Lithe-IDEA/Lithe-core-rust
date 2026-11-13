//! `runConfig.*` commands: run configurations and their launch plans.

use crate::protocol::{CoreError, CoreResponse, ErrorCode, RunConfigCommand};
use serde_json::Value;

/// Runs one `runConfig.*` command.
pub(super) fn dispatch(
    command: RunConfigCommand,
    id: Option<String>,
    payload: Value,
) -> CoreResponse {
    match command {
        RunConfigCommand::Inspect => {
            match serde_json::from_value::<crate::execution::InspectRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration inspect request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::inspect)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::ProjectToolchain => {
            match serde_json::from_value::<crate::execution::ProjectToolchainRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid project toolchain request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::project_toolchain)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::Generate => {
            match serde_json::from_value::<crate::execution::GenerateRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration generate request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::generate)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::ArgumentText => {
            match serde_json::from_value::<crate::execution::ArgumentTextRequest>(payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::json!(crate::execution::format_argument_text(request)),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid argument text request")
                        .with_details(error.to_string()),
                ),
            }
        }
        RunConfigCommand::Resolve => {
            match serde_json::from_value::<crate::execution::ResolveRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration resolve request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::resolve)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::DecideTitleAction => {
            match serde_json::from_value::<crate::execution::TitleActionRequest>(payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::execution::decide_title_action(request))
                        .expect("Title action decisions always encode"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid title action request")
                        .with_details(error.to_string()),
                ),
            }
        }
        RunConfigCommand::SelectContextConfiguration => {
            match serde_json::from_value::<crate::execution::ContextSelectionRequest>(payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::json!({"configurationId": crate::execution::select_context_configuration(request)}),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid context configuration request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        RunConfigCommand::UpdateOptions => {
            match serde_json::from_value::<crate::execution::UpdateOptionsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid run options request")
                        .with_details(error.to_string())
                })
                .and_then(crate::execution::update_options)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::UpdateToolPreferences => {
            match serde_json::from_value::<crate::execution::UpdateToolPreferencesRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid tool preferences request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::update_tool_preferences)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::SaveEditorChanges => {
            match serde_json::from_value::<crate::execution::UpdateOptionsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration editor request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::save_editor_changes)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::CreateUserConfiguration => {
            match serde_json::from_value::<crate::execution::CreateUserConfigurationRequest>(
                payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid user configuration request",
                )
                .with_details(error.to_string())
            })
            .and_then(crate::execution::create_user_configuration)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::ProviderLaunchInputs => {
            match serde_json::from_value::<crate::execution::ProviderLaunchInputsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid provider launch input request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::provider_launch_inputs)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::CreateLaunchPlan => {
            match serde_json::from_value::<crate::execution::LaunchPlanRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid launch plan request")
                        .with_details(error.to_string())
                })
                .and_then(crate::execution::create_launch_plan)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    }
}
