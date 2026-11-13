//! `lsp.*` commands: language server sessions, documents, edits, requests and events.

use super::plugin_response;
use crate::protocol::{CoreError, CoreResponse, ErrorCode, LspCommand};
use serde_json::{json, Value};

/// Runs one `lsp.*` command.
pub(super) fn dispatch(command: LspCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match command {
        LspCommand::ApplyTextEdits => {
            match serde_json::from_value::<lithe_text::ApplyTextEditsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP text edit request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_text::apply_text_edits)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP text edit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::DocumentIdentity => plugin_response(
            id,
            payload,
            lithe_lsp_policy::document_identity::resolve_command,
        ),
        LspCommand::PlanWorkspaceEdit => plugin_response(
            id,
            payload,
            lithe_lsp_policy::workspace_edit_plan::plan_command,
        ),
        LspCommand::ClassifyNavigationTarget => plugin_response(
            id,
            payload,
            lithe_lsp_policy::navigation_target::classify_command,
        ),
        LspCommand::DeclarationOrUsages => plugin_response(
            id,
            payload,
            lithe_lsp_policy::declaration_or_usages::decide_command,
        ),
        LspCommand::PlainSnippet => {
            match serde_json::from_value::<lithe_text::PlainSnippetRequest>(payload).map_err(
                |error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP snippet request")
                        .with_details(error.to_string())
                },
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_text::plain_snippet(request))
                        .expect("LSP snippet response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::BuiltinCompletions => {
            match serde_json::from_value::<lithe_text::BuiltinRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP completion request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_text::builtin_completions)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP completion response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::BuiltinHover => {
            match serde_json::from_value::<lithe_text::BuiltinRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP hover request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_text::builtin_hover)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP hover response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::BuiltinNavigation => {
            match serde_json::from_value::<lithe_text::BuiltinNavigationRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP navigation request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_text::builtin_navigation)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP navigation response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::MergeCompletions => plugin_response(
            id,
            payload,
            lithe_lsp_policy::completion_merge::merge_command,
        ),
        LspCommand::ActivationPolicy => {
            plugin_response(id, payload, lithe_lsp_policy::activation::policy_command)
        }
        LspCommand::ProblemsQuery => {
            plugin_response(id, payload, lithe_lsp_policy::problems::query_command)
        }
        LspCommand::StartServer => {
            match serde_json::from_value::<lithe_lsp_engine::StartServerRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP start-server request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::start_server)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP start-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::StopServer => {
            match serde_json::from_value::<lithe_lsp_engine::SessionRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP stop-server request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::stop_server)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP stop-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::UpdateWorkspaceConfiguration => {
            match serde_json::from_value::<lithe_lsp_engine::UpdateWorkspaceConfigurationRequest>(
                payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid workspace configuration update request",
                )
                .with_details(error.to_string())
            })
            .and_then(lithe_lsp_engine::update_workspace_configuration)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("workspace configuration update response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::SaveDocument => {
            match serde_json::from_value::<lithe_lsp_engine::SaveDocumentRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP save-document request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::save_document)
            {
                Ok(sent) => CoreResponse::success(id, json!({"sent": sent})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::DocumentUri => {
            plugin_response(id, payload, lithe_document_uri::document_uri_command)
        }
        LspCommand::SyncDocument => {
            let uri = payload
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            match serde_json::from_value::<lithe_lsp_engine::SyncDocumentRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP sync-document request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::sync_document)
            {
                Ok(data) => {
                    let mut encoded = serde_json::to_value(data)
                        .expect("LSP sync-document response should encode");
                    encoded["uri"] = json!(uri);
                    CoreResponse::success(id, encoded)
                }
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::WorkspaceFilesChanged => {
            match serde_json::from_value::<lithe_lsp_engine::WorkspaceFilesChangedRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP workspace-files-changed request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::workspace_files_changed)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("LSP workspace-files-changed response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::CloseDocument => {
            match serde_json::from_value::<lithe_lsp_engine::CloseDocumentRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP close-document request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::close_document)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP close-document response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::Request => {
            match serde_json::from_value::<lithe_lsp_engine::SemanticRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid semantic LSP request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::semantic_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Semantic LSP response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::BuildRequest => {
            match serde_json::from_value::<lithe_lsp_engine::BuildProviderRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid build-provider request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::build_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Build-provider response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::ToolchainRequest => {
            match serde_json::from_value::<lithe_lsp_engine::ToolchainProviderRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid toolchain-provider request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::toolchain_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Toolchain response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::KeymapImportRequest => {
            match serde_json::from_value::<lithe_lsp_engine::KeymapImportProviderRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid shortcut import request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::keymap_import_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Shortcut import response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::VcsRequest => {
            match serde_json::from_value::<lithe_lsp_engine::VcsProviderRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid VCS-provider request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::vcs_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("VCS-provider response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::FunctionProvider => {
            plugin_response(id, payload, lithe_lsp_engine::function_provider_command)
        }
        LspCommand::FunctionRequest => {
            match serde_json::from_value::<lithe_lsp_engine::PluginFunctionRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid plugin-function request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::plugin_function_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Plugin-function response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::ConfigureFunctionRelay => {
            match serde_json::from_value::<lithe_plugin_function_relay::RelaySnapshot>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid function relay snapshot")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::configure_function_relay)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::HostResponse => {
            match serde_json::from_value::<lithe_lsp_engine::PluginHostResponse>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid host-service response")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::plugin_host_response)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::HostNotify => {
            match serde_json::from_value::<lithe_lsp_engine::PluginHostNotification>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid host notification")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::plugin_host_notify)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Host notification response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::PluginServerRequest => {
            match serde_json::from_value::<lithe_lsp_engine::PluginServerRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid plugin language-server request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::plugin_server_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Plugin language-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::PluginServerNotify => {
            match serde_json::from_value::<lithe_lsp_engine::PluginServerNotify>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid plugin language-server notification",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::plugin_server_notify)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Plugin language-server notification response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::CancelOperation => {
            match serde_json::from_value::<lithe_lsp_engine::CancelOperationRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP cancellation request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::cancel_operation)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP cancellation response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::PollEvents => {
            match serde_json::from_value::<lithe_lsp_engine::SessionRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP poll-events request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::poll_events)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP poll-events response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::SetDetailedDiagnostics => {
            match serde_json::from_value::<lithe_lsp_engine::SetDetailedDiagnosticsRequest>(payload)
                .map_err(|_| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid detailed diagnostics request",
                    )
                })
                .and_then(lithe_lsp_engine::set_detailed_diagnostics)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::WaitEvents => {
            match serde_json::from_value::<lithe_lsp_engine::WaitEventsRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP wait-events request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::wait_events)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP wait-events response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        LspCommand::DestroyServer => {
            match serde_json::from_value::<lithe_lsp_engine::SessionRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP destroy-server request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_lsp_engine::destroy_server)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP destroy-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    }
}
