//! `runConfig.*` commands: run configurations and their launch plans.

use crate::protocol::{CoreError, CoreResponse, ErrorCode, RunConfigCommand};
/// The manifest type every domain command validates plugin manifests with.
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
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
        RunConfigCommand::ConfigurationTypes => respond(
            id,
            with_run_types(
                payload,
                "Invalid configuration types request",
                true,
                |mut request: crate::execution::ConfigurationTypesRequest, run_types| {
                    request.run_types = run_types;
                    crate::execution::configuration_types(request)
                },
            ),
        ),
        RunConfigCommand::EditorForm => respond(
            id,
            with_run_types(
                payload,
                "Invalid run configuration form request",
                true,
                |mut request: crate::execution::EditorFormRequest, run_types| {
                    request.run_types = run_types;
                    crate::execution::editor_form(request)
                },
            ),
        ),
        RunConfigCommand::SaveConfiguration => respond(
            id,
            with_run_types(
                payload,
                "Invalid run configuration save request",
                false,
                |mut request: crate::execution::SaveConfigurationRequest, run_types| {
                    request.run_types = run_types;
                    crate::execution::save_configuration(request)
                },
            ),
        ),
        RunConfigCommand::BeforeLaunch => {
            match serde_json::from_value::<crate::execution::BeforeLaunchRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration before launch request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::before_launch)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::DeleteConfiguration => {
            match serde_json::from_value::<crate::execution::DeleteConfigurationRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration delete request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::delete_configuration)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::CopyConfiguration => {
            match serde_json::from_value::<crate::execution::CopyConfigurationRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration copy request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::copy_configuration)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RunConfigCommand::Arrange => {
            match serde_json::from_value::<crate::execution::ArrangeRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration arrange request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::execution::arrange)
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

fn respond(id: Option<String>, result: Result<Value, CoreError>) -> CoreResponse {
    match result {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, error),
    }
}

/// Runs an editor command that offers the run configuration types installed
/// plugins declare. The payload's `runTypePlugins` (`{language, plugins:
/// [{manifest, localizations}]}`) is taken out and checked by the build
/// crate; the rest is the command's request. With `report`, the answer
/// carries the plugins' translation reports as `localizations`, so the host
/// can read the files a report asks for and call again.
fn with_run_types<T: serde::de::DeserializeOwned>(
    mut payload: Value,
    invalid: &'static str,
    report: bool,
    run: impl FnOnce(T, Vec<crate::execution::DeclaredRunType>) -> Result<Value, CoreError>,
) -> Result<Value, CoreError> {
    let invalid = |error: serde_json::Error| {
        CoreError::new(ErrorCode::InvalidRequest, invalid).with_details(error.to_string())
    };
    let plugins = match payload
        .as_object_mut()
        .and_then(|object| object.remove("runTypePlugins"))
    {
        Some(plugins) => {
            serde_json::from_value::<crate::build::RunTypePlugins>(plugins).map_err(invalid)?
        }
        None => crate::build::RunTypePlugins::default(),
    };
    let request = serde_json::from_value::<T>(payload).map_err(invalid)?;
    let (run_types, reports) = crate::build::declared_run_types::<Manifest>(&plugins)?;
    let mut data = run(request, run_types)?;
    if report && !reports.is_empty() {
        data["localizations"] = Value::Array(reports);
    }
    Ok(data)
}
