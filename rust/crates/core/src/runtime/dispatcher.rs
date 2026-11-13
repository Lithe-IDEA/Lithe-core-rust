//! Validation and routing from versioned command names to their owning domains.

/// The manifest type every domain command validates plugin manifests with.
use crate::plugins::declarative::DeclarativePluginManifest as Manifest;
use crate::project::MarkdownRenderRequest;
use crate::project::{
    self, DirectoryListingRequest, DocumentLifecycleRequest, FileReadRequest, FileWriteRequest,
    ReplacementPreviewRequest, SearchIndexRequest, SearchIndexUpdateRequest, SearchRequest,
    WorkspaceSnapshotRequest,
};
use crate::project::{
    HistoryContentRequest, HistoryDeleteRequest, HistoryEntriesRequest,
    HistoryIgnorePreviewRequest, HistoryRecordRequest, HistoryRelocateRequest,
    HistoryRenameRequest, HistoryStorageRequest,
};
use crate::protocol::CoreResponse;
use crate::protocol::{CoreCommand, CoreRequest};
use crate::protocol::{CoreError, ErrorCode};
use lithe_diagnostics::{manifest::BuildManifestRequest, redaction::RedactTextRequest};
use lithe_git::{
    self as git, GitApplyRequest, GitBlameRequest, GitBranchNamePreviewRequest,
    GitCheckoutPreflightRequest, GitCommandRequest, GitCommitFilesRequest, GitCommitRequest,
    GitComparisonRequest, GitConflictMarkerRequest, GitDiffRequest, GitHistoryCursorCloseRequest,
    GitHistoryLocateRequest, GitHistoryPageRequest, GitHistoryRequest,
    GitHistoryRewritePreviewRequest, GitIntegrationPreflightRequest, GitLineHistoryRequest,
    GitOperationStateRequest, GitPullPreflightRequest, GitPullRequestContextRequest,
    GitPushPreviewRequest, GitRebaseControlRequest, GitRebasePreviewRequest,
    GitRebaseSessionRequest, GitRebaseStartRequest, GitReferencesRequest, GitRepositoryRootRequest,
    GitStashesRequest, GitStatusRequest, GitUntrackedDirectoryRequest, GitWatchContextRequest,
    GitWorktreesRequest, GitWriteRequest, PatchApplyRequest, PatchExportRequest,
    PatchPreviewRequest, WorkspaceRepositoriesRequest,
};
use lithe_github::{NormalizeResponseRequest, ParseRemoteRequest, RequestPlanRequest};
use serde_json::json;

pub fn execute_json(request: &str) -> String {
    execute_document_json(request, None)
}

/// Borrows an immutable document for one request; no text is retained after dispatch.
pub(crate) fn execute_document_json(request: &str, document: Option<&str>) -> String {
    crate::runtime::helper_failures::install();
    let response = execute(request, document);
    serde_json::to_string(&response).unwrap_or_else(|_| {
        serde_json::to_string(&CoreResponse::failure(
            None,
            CoreError::new(ErrorCode::Unknown, "Could not encode core response"),
        ))
        .expect("fallback response should encode")
    })
}

/// Decode an agent management payload and run `operation` on it.
/// Declarative plugin commands decode their own payloads so manifest content
/// problems surface as `rejected` outcomes rather than Core failures.
fn plugin_response(
    id: Option<String>,
    payload: serde_json::Value,
    operation: fn(serde_json::Value) -> Result<serde_json::Value, CoreError>,
) -> CoreResponse {
    match operation(payload) {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, error),
    }
}

/// Runs one `ai.*` command in `lithe-ai`.
fn ai_response(id: Option<String>, command: &str, payload: serde_json::Value) -> CoreResponse {
    match lithe_ai::execute(command, payload) {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, crate::protocol::ai_error(error)),
    }
}

/// Runs one rule in `lithe-core-rules`.
fn rules_response(id: Option<String>, command: &str, payload: serde_json::Value) -> CoreResponse {
    match lithe_core_rules::execute(command, payload) {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, crate::protocol::rule_error(error)),
    }
}

/// Decodes a typed plan/reduce payload, runs the pure `operation` on it, and
/// encodes its result; a payload that does not decode is `InvalidRequest`.
fn reducer_response<T: serde::de::DeserializeOwned, R: serde::Serialize>(
    id: Option<String>,
    payload: serde_json::Value,
    invalid_message: &str,
    operation: fn(T) -> Result<R, CoreError>,
) -> CoreResponse {
    match serde_json::from_value::<T>(payload)
        .map_err(|error| {
            CoreError::new(ErrorCode::InvalidRequest, invalid_message)
                .with_details(error.to_string())
        })
        .and_then(operation)
    {
        Ok(data) => CoreResponse::success(
            id,
            serde_json::to_value(data).expect("Reducer results always encode"),
        ),
        Err(error) => CoreResponse::failure(id, error),
    }
}

/// Host detailed-diagnostics state; `roots: None` enables DEBUG for every root.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreEventConfiguration {
    debug: bool,
    roots: Option<Vec<String>>,
}

// Domain failures retain their unique numeric logging codes inside the existing Core envelope.
fn log_v2_result<T: serde::Serialize>(
    result: Result<T, lithe_diagnostics::channel::LogError>,
) -> serde_json::Value {
    match result {
        Ok(value) => serde_json::json!({"result": value}),
        Err(error) => serde_json::json!({"error": error}),
    }
}

fn execute(request: &str, document: Option<&str>) -> CoreResponse {
    let parsed: CoreRequest = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => {
            return CoreResponse::failure(
                None,
                CoreError::new(ErrorCode::InvalidRequest, "Invalid JSON request")
                    .with_details(error.to_string()),
            );
        }
    };
    // The trace spans every scope below and records once, after the final response.
    let trace = crate::runtime::request_trace::Request::begin(&parsed);
    let response = execute_parsed(parsed, document);
    trace.finish(&response);
    response
}

fn execute_parsed(mut parsed: CoreRequest, document: Option<&str>) -> CoreResponse {
    let id = parsed.id.clone();
    let response_id = id.clone();
    let operation_id = parsed.operation_id.clone().or_else(|| id.clone());
    let _cancellation_scope = crate::protocol::cancellation::Scope::begin(
        operation_id.clone(),
        parsed.timeout_milliseconds,
    );
    lithe_request_context::events::request_started(operation_id.as_deref());
    if let Err(error) = crate::protocol::cancellation::check() {
        return CoreResponse::failure(id, error);
    }
    if parsed.remote_workspace.is_some() && !has_remote_workspace_adapter(&parsed.command) {
        return CoreResponse::failure(
            id,
            CoreError::new(
                ErrorCode::NotSupported,
                "This command has no remote workspace adapter",
            ),
        );
    }
    let _workspace = match lithe_workspace::Scope::begin(parsed.remote_workspace) {
        Ok(scope) => scope,
        Err(error) => return CoreResponse::failure(id, error),
    };
    let _git_policy = match lithe_git::execution_policy::Scope::begin(parsed.git_execution) {
        Ok(scope) => scope,
        Err(error) => return CoreResponse::failure(id, error),
    };
    let Some(command) = CoreCommand::parse(&parsed.command) else {
        return CoreResponse::failure(
            id,
            CoreError::new(ErrorCode::NotSupported, "Unsupported core command")
                .with_details(parsed.command),
        );
    };

    if let Err(error) = crate::lsp::resolve_request_paths(&parsed.command, &mut parsed.payload) {
        return CoreResponse::failure(id, error);
    }

    let preserve_workspace_outcome = matches!(command, CoreCommand::GitWorkspaceCommitStep);
    let response = match command {
        CoreCommand::IdeHostControl => {
            let action = parsed.payload["action"].as_str().unwrap_or("");
            match lithe_ide_host::execute(action, parsed.payload["arguments"].clone()) {
                Ok(data) => CoreResponse::success(id, data),
                Err(message) => {
                    CoreResponse::failure(id, CoreError::new(ErrorCode::InvalidRequest, message))
                }
            }
        }
        CoreCommand::Ping => CoreResponse::success(
            id,
            json!({
                "protocolVersion": 1,
                "coreVersion": env!("CARGO_PKG_VERSION")
            }),
        ),
        CoreCommand::AiCommitInputBudget => ai_response(id, "ai.commitInputBudget", parsed.payload),
        CoreCommand::AiPlanCommit => ai_response(id, "ai.planCommit", parsed.payload),
        CoreCommand::AiDecodeCommitMessage => {
            ai_response(id, "ai.decodeCommitMessage", parsed.payload)
        }
        CoreCommand::AiConfigurationSources => {
            ai_response(id, "ai.configuration.sources", parsed.payload)
        }
        CoreCommand::AiConfigurationParse => {
            ai_response(id, "ai.configuration.parse", parsed.payload)
        }
        CoreCommand::AiPullRequestPlan => ai_response(id, "ai.pullRequest.plan", parsed.payload),
        CoreCommand::AiPullRequestDecode => {
            ai_response(id, "ai.pullRequest.decode", parsed.payload)
        }
        CoreCommand::AiInlineEditPlan => ai_response(id, "ai.inlineEdit.plan", parsed.payload),
        CoreCommand::AiInlineEditDecode => ai_response(id, "ai.inlineEdit.decode", parsed.payload),
        CoreCommand::PluginsNativeUIValidateData => plugin_response(
            id,
            parsed.payload,
            crate::plugins::native_ui::validate_data_command,
        ),
        CoreCommand::PluginsWebOpen => plugin_response(
            id,
            parsed.payload,
            crate::plugins::web_content::open_command,
        ),
        CoreCommand::PluginsWebClose => plugin_response(
            id,
            parsed.payload,
            crate::plugins::web_content::close_command,
        ),
        CoreCommand::PluginsWebResource => plugin_response(
            id,
            parsed.payload,
            crate::plugins::web_content::resource_command,
        ),
        CoreCommand::PluginsWebDocument => plugin_response(
            id,
            parsed.payload,
            crate::plugins::web_content::document_command,
        ),
        CoreCommand::PluginsWebRequest => plugin_response(
            id,
            parsed.payload,
            crate::plugins::web_content::request_command,
        ),
        CoreCommand::PluginsWebReply => plugin_response(
            id,
            parsed.payload,
            crate::plugins::web_content::reply_command,
        ),
        CoreCommand::PluginsWebEvent => plugin_response(
            id,
            parsed.payload,
            crate::plugins::web_content::event_command,
        ),
        CoreCommand::PluginsDeclarativeValidate => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::validate_command,
        ),
        CoreCommand::PluginsDeclarativeRunPlan => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::run_plan_command,
        ),
        CoreCommand::PluginsDeclarativeDiscoverTests => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::discover_tests_command,
        ),
        CoreCommand::PluginsDeclarativeTestPlan => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::test_plan_command,
        ),
        CoreCommand::PluginsDeclarativeLanguageServerLaunchPlan => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::language_server_launch_plan_command,
        ),
        CoreCommand::PluginsLanguageServersDecodeLaunch => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::decode_language_server_launch_command,
        ),
        CoreCommand::PluginsLanguageServersStateRetention => plugin_response(
            id,
            parsed.payload,
            crate::plugins::language_server_sessions::state_retention_command,
        ),
        CoreCommand::PluginsContentProvidersDecode => plugin_response(
            id,
            parsed.payload,
            crate::plugins::content_providers::decode_content_command,
        ),
        CoreCommand::DebugValidateAdapterEndpoint => {
            match crate::debug::validate_adapter_endpoint(parsed.payload) {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DebugTransportPolicy => match crate::debug::transport_policy(parsed.payload) {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::DebugTransportHandle => {
            match crate::debug::handle_transport_event(parsed.payload) {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DebugTransportRelease => {
            match crate::debug::release_transport(parsed.payload) {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DebugAdaptersStartArguments => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::debug_start_arguments_command,
        ),
        CoreCommand::DebugAdaptersExecutablePlan => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::debug_executable_plan_command,
        ),
        CoreCommand::PluginsDeclarativeResolveSettings => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::resolve_settings_command,
        ),
        CoreCommand::PluginsInstalledSelect => plugin_response(
            id,
            parsed.payload,
            crate::plugins::installed::select_command,
        ),
        CoreCommand::PluginsSettingsCatalog => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::settings_catalog_command,
        ),
        CoreCommand::PluginsSettingsNavigation => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::settings_navigation_command,
        ),
        CoreCommand::PluginsSettingsDelivery => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::settings_delivery_command,
        ),
        CoreCommand::PluginsDeclarativeSettingsState => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::settings_state_command,
        ),
        CoreCommand::PluginsDeclarativeSettingsActionPlan => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::settings_action_plan_command,
        ),
        CoreCommand::PluginsDeclarativeEditSettings => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::edit_settings_command,
        ),
        CoreCommand::PluginsDeclarativeWorkspaceConfiguration => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::workspace_configuration_command,
        ),
        CoreCommand::PluginsDeclarativeDecodeGrammar => plugin_response(
            id,
            parsed.payload,
            crate::plugins::declarative::decode_grammar_command,
        ),
        CoreCommand::PluginsEditorActionsDecode => plugin_response(
            id,
            parsed.payload,
            crate::plugins::editor_actions::decode_command,
        ),
        CoreCommand::PluginsHostApiDescribe => plugin_response(
            id,
            parsed.payload,
            crate::plugins::host_api::describe_command,
        ),
        CoreCommand::PluginsHostApiAuthorize => plugin_response(
            id,
            parsed.payload,
            crate::plugins::host_api::authorize_command,
        ),
        CoreCommand::PluginsEditorProvidersDecode => plugin_response(
            id,
            parsed.payload,
            crate::plugins::editor_providers::decode_command,
        ),
        CoreCommand::KeymapDecodeImport => plugin_response(
            id,
            parsed.payload,
            crate::plugins::keymap_import::decode_command,
        ),
        CoreCommand::KeymapPlanImportSources => plugin_response(
            id,
            parsed.payload,
            crate::plugins::keymap_import::plan_sources_command,
        ),
        CoreCommand::KeymapPlanImportBindings => plugin_response(
            id,
            parsed.payload,
            crate::plugins::keymap_import::plan_bindings_command,
        ),
        CoreCommand::DebugStartOrContinue => {
            match crate::debug::start_or_continue(parsed.payload) {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::ToolchainsRegistry => plugin_response(id, parsed.payload, |payload| {
            crate::toolchain::registry_command(
                payload,
                crate::plugins::declarative::validated_manifest_json,
            )
        }),
        CoreCommand::ToolchainsDecodeReply => plugin_response(id, parsed.payload, |payload| {
            crate::toolchain::decode_reply_command(
                payload,
                crate::plugins::declarative::validated_manifest_json,
            )
        }),
        CoreCommand::ToolchainsFailureReply => {
            plugin_response(id, parsed.payload, crate::toolchain::failure_reply_command)
        }
        CoreCommand::ToolchainsResolvePins => {
            plugin_response(id, parsed.payload, crate::toolchain::resolve_pins_command)
        }
        CoreCommand::ToolchainsEditPins => {
            plugin_response(id, parsed.payload, crate::toolchain::edit_pins_command)
        }
        CoreCommand::ToolchainsPinStoragePlan => plugin_response(
            id,
            parsed.payload,
            crate::toolchain::pin_storage_plan_command,
        ),
        CoreCommand::ToolchainsDecodePinDocument => plugin_response(
            id,
            parsed.payload,
            crate::toolchain::decode_pin_document_command,
        ),
        CoreCommand::ToolchainsEditPinSources => plugin_response(
            id,
            parsed.payload,
            crate::toolchain::edit_pin_sources_command,
        ),
        CoreCommand::ToolchainsResolveConsumerPreferences => plugin_response(
            id,
            parsed.payload,
            crate::toolchain::resolve_consumer_preferences_command,
        ),
        CoreCommand::ToolchainsEditConsumerPreferences => plugin_response(
            id,
            parsed.payload,
            crate::toolchain::edit_consumer_preferences_command,
        ),
        CoreCommand::ToolchainsResolve => plugin_response(id, parsed.payload, |payload| {
            crate::toolchain::resolve_command(
                payload,
                crate::plugins::declarative::validated_manifest_json,
            )
        }),
        CoreCommand::ToolchainsCheckVersion => plugin_response(id, parsed.payload, |payload| {
            crate::toolchain::check_version_command(
                payload,
                crate::plugins::declarative::validated_manifest_json,
            )
        }),
        CoreCommand::ToolchainsDiscoveryPlan => plugin_response(id, parsed.payload, |payload| {
            crate::toolchain::discovery_plan_command(
                payload,
                crate::plugins::declarative::validated_manifest_json,
            )
        }),
        CoreCommand::ToolchainsCandidates => plugin_response(id, parsed.payload, |payload| {
            crate::toolchain::candidates_command(
                payload,
                crate::plugins::declarative::validated_manifest_json,
            )
        }),
        CoreCommand::ToolchainsInstantiateLaunch => {
            plugin_response(id, parsed.payload, |payload| {
                crate::toolchain::instantiate_launch_command(
                    payload,
                    crate::plugins::declarative::validated_manifest_json,
                )
            })
        }
        CoreCommand::RunDecodeStart => plugin_response(
            id,
            parsed.payload,
            crate::build::run_decode_start_command::<Manifest>,
        ),
        CoreCommand::DebugDecodeBackendConfiguration => plugin_response(
            id,
            parsed.payload,
            crate::debug::decode_backend_configuration_command::<Manifest>,
        ),
        CoreCommand::OutputLinksMatch => plugin_response(
            id,
            parsed.payload,
            crate::plugins::output_links::match_lines,
        ),
        CoreCommand::OutputLinksDecodeResolution => plugin_response(
            id,
            parsed.payload,
            crate::plugins::output_links::decode_resolution,
        ),
        CoreCommand::BuildDetect => plugin_response(
            id,
            parsed.payload,
            crate::build::build_detect_command::<Manifest>,
        ),
        CoreCommand::BuildProjectOptions => {
            plugin_response(id, parsed.payload, crate::build::project_options_command)
        }
        CoreCommand::BuildResolveStoredOptions => plugin_response(
            id,
            parsed.payload,
            crate::build::resolve_stored_options_command,
        ),
        CoreCommand::BuildEditStoredOption => plugin_response(
            id,
            parsed.payload,
            crate::build::build_edit_stored_option_command::<Manifest>,
        ),
        CoreCommand::BuildInvocationPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_invocation_plan_command::<Manifest>,
        ),
        CoreCommand::BuildCheckToolVersion => plugin_response(
            id,
            parsed.payload,
            crate::build::build_check_tool_version_command::<Manifest>,
        ),
        CoreCommand::BuildResolveOptions => plugin_response(
            id,
            parsed.payload,
            crate::build::build_resolve_options_command::<Manifest>,
        ),
        CoreCommand::BuildModelPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_model_plan_command::<Manifest>,
        ),
        CoreCommand::BuildTaskPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_task_plan_command::<Manifest>,
        ),
        CoreCommand::BuildPrepareDebugPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_prepare_debug_plan_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeDebugPreparation => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_debug_preparation_command::<Manifest>,
        ),
        CoreCommand::BuildResolveModelInputs => plugin_response(
            id,
            parsed.payload,
            crate::build::build_resolve_model_inputs_command::<Manifest>,
        ),
        CoreCommand::BuildConfigurationEditPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_configuration_edit_plan_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeConfigurationEdit => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_configuration_edit_command::<Manifest>,
        ),
        CoreCommand::BuildConfigurationPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_configuration_plan_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeConfiguration => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_configuration_command::<Manifest>,
        ),
        CoreCommand::BuildPrepareRunPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_prepare_run_plan_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeRunPreparation => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_run_preparation_command::<Manifest>,
        ),
        CoreCommand::BuildTestPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_test_plan_command::<Manifest>,
        ),
        CoreCommand::BuildTestDiscoveryPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_test_discovery_plan_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeTestDiscovery => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_test_discovery_command::<Manifest>,
        ),
        CoreCommand::BuildDependencyPlan => plugin_response(
            id,
            parsed.payload,
            crate::build::build_dependency_plan_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeModel => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_model_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeTestResults => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_test_results_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeDiagnostics => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_diagnostics_command::<Manifest>,
        ),
        CoreCommand::BuildDecodeDependencies => plugin_response(
            id,
            parsed.payload,
            crate::build::build_decode_dependencies_command::<Manifest>,
        ),
        CoreCommand::BuildSyncFingerprint => plugin_response(
            id,
            parsed.payload,
            crate::build::build_sync_fingerprint_command::<Manifest>,
        ),
        CoreCommand::BuildResolveTestRunOutcome => reducer_response(
            id,
            parsed.payload,
            "Invalid test run outcome request",
            crate::build::resolve_test_run_outcome,
        ),
        CoreCommand::BuildMergeTestOutcomes => reducer_response(
            id,
            parsed.payload,
            "Invalid test outcome merge request",
            crate::build::merge_test_outcomes,
        ),
        CoreCommand::VcsDetectRoots => plugin_response(
            id,
            parsed.payload,
            crate::vcs::vcs_detect_roots_command::<Manifest>,
        ),
        CoreCommand::VcsOwnerOf => {
            plugin_response(id, parsed.payload, crate::vcs::vcs_owner_of_command)
        }
        CoreCommand::VcsDecodeReply => {
            plugin_response(id, parsed.payload, crate::vcs::vcs_decode_reply_command)
        }
        CoreCommand::ProjectGeneratorCatalog => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_generator_catalog_command::<Manifest>,
        ),
        CoreCommand::ProjectGeneratorForm => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_generator_form_command::<Manifest>,
        ),
        CoreCommand::ProjectGenerationPlan => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_generation_plan_command::<Manifest>,
        ),
        CoreCommand::ProjectCheckName => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_check_name_command,
        ),
        CoreCommand::ProjectExtractArchive => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_extract_archive_command,
        ),
        CoreCommand::PluginsMarketplaceValidateIndex => plugin_response(
            id,
            parsed.payload,
            crate::plugins::marketplace::validate_index_command,
        ),
        CoreCommand::PluginsMarketplaceResolveSource => plugin_response(
            id,
            parsed.payload,
            crate::plugins::marketplace::resolve_source_command,
        ),
        CoreCommand::PluginsMarketplaceInstallReview => plugin_response(
            id,
            parsed.payload,
            crate::plugins::marketplace::install_review_command,
        ),
        CoreCommand::WorkspaceReleaseIndexing => {
            match serde_json::from_value::<project::indexing::ReleaseIndexingRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid indexing release")
                    .with_details(error.to_string())
            })
            .and_then(project::indexing::release)
            {
                Ok(released) => {
                    CoreResponse::success(id, serde_json::json!({"released": released}))
                }
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceConfigureIndexing => {
            match serde_json::from_value::<project::indexing::ConfigureIndexingRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid indexing policy")
                    .with_details(error.to_string())
            })
            .and_then(|request| project::indexing::configure(request, crate::lsp::indexing_changed))
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("indexing status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceRefreshIndexing => {
            match serde_json::from_value::<project::indexing::RefreshIndexingRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid indexing refresh")
                    .with_details(error.to_string())
            })
            .and_then(|request| {
                project::indexing::refresh_indexing(request, crate::lsp::indexing_changed)
            }) {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("indexing status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceSnapshot => {
            match serde_json::from_value::<WorkspaceSnapshotRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace snapshot request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::snapshot)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("snapshot should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceFilterChanges => {
            match serde_json::from_value::<crate::project::WorkspaceChangeFilterRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::json!(crate::project::filter_workspace_changes(request)),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid workspace event paths")
                        .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::WorkspaceListDirectories => {
            match serde_json::from_value::<DirectoryListingRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid directory listing request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::list_directories)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("directory listing should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceOrderEntries => plugin_response(
            id,
            parsed.payload,
            crate::project::file_name_order::order_entries_command,
        ),
        CoreCommand::WorkspaceRankNames => plugin_response(
            id,
            parsed.payload,
            crate::project::name_ranking::rank_names_command,
        ),
        CoreCommand::WorkspacePlanDuplicate => plugin_response(
            id,
            parsed.payload,
            crate::project::file_operations::plan_duplicate_command,
        ),
        CoreCommand::WorkspacePlanPaste => plugin_response(
            id,
            parsed.payload,
            crate::project::file_operations::plan_paste_command,
        ),
        CoreCommand::WorkspaceReduceDirectoryAction => plugin_response(
            id,
            parsed.payload,
            crate::project::directory_actions::reduce_command,
        ),
        CoreCommand::WorkspaceRelocateDirectoryRules => plugin_response(
            id,
            parsed.payload,
            crate::project::directory_actions::relocate_command,
        ),
        CoreCommand::WorkspaceRepositories => {
            match serde_json::from_value::<WorkspaceRepositoriesRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace repositories request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::workspace_repositories)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("workspace repositories should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceRecentProjectsReduce => plugin_response(
            id,
            parsed.payload,
            crate::project::recent_projects::reduce_command,
        ),
        CoreCommand::WorkspaceSearchIndexWarm => {
            match serde_json::from_value::<SearchIndexRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search index request")
                        .with_details(error.to_string())
                })
                .and_then(project::warm_search_index)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search index status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceSearchIndexUpdate => {
            match serde_json::from_value::<SearchIndexUpdateRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search index update")
                        .with_details(error.to_string())
                })
                .and_then(|request| {
                    project::update_search_index(request, crate::lsp::indexing_changed)
                }) {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search index status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceSearchIndexInvalidate => {
            match serde_json::from_value::<SearchIndexRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search index request")
                        .with_details(error.to_string())
                })
                .and_then(project::invalidate_search_index)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::SearchActions => rules_response(id, "search.actions", parsed.payload),
        CoreCommand::WorkspaceSearch => {
            match serde_json::from_value::<SearchRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search request")
                        .with_details(error.to_string())
                })
                .and_then(project::search)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceSearchEverywhere => {
            match serde_json::from_value::<project::SearchPageRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Search Everywhere request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::search_page)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search everywhere should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceReplacePreview => {
            match serde_json::from_value::<ReplacementPreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid replacement preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::replace_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("replacement preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceReplaceText => {
            match serde_json::from_value::<project::ReplacementTextRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid text replacement request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::replace_text)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::WorkspaceSearchTextBatch => {
            match serde_json::from_value::<project::SearchTextBatchRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid text search request")
                        .with_details(error.to_string())
                })
                .and_then(project::search_text_batch)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::FileRead => match serde_json::from_value::<FileReadRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid file read request")
                    .with_details(error.to_string())
            })
            .and_then(project::read_file)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("file response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::FileWrite => match serde_json::from_value::<FileWriteRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid file write request")
                    .with_details(error.to_string())
            })
            .and_then(project::write_file)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("file response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::TextDecode => {
            match serde_json::from_value::<project::TextDecodeRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid text decode request")
                        .with_details(error.to_string())
                })
                .and_then(project::decode_text_bytes)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("text response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::TextEncode => {
            match serde_json::from_value::<project::TextEncodeRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid text encode request")
                        .with_details(error.to_string())
                })
                .and_then(project::encode_text)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("text response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DocumentClassifyText => {
            match parsed
                .payload
                .get("text")
                .and_then(serde_json::Value::as_str)
            {
                Some(text) => CoreResponse::success(
                    id,
                    serde_json::json!({
                        "isPlainText": project::is_plain_text(text)
                    }),
                ),
                None => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Decoded document text is required",
                    ),
                ),
            }
        }
        CoreCommand::DocumentPlanSaveBatch => plugin_response(
            id,
            parsed.payload,
            project::document_save_batch::plan_command,
        ),
        CoreCommand::DocumentReduceSaveBatch => plugin_response(
            id,
            parsed.payload,
            project::document_save_batch::reduce_command,
        ),
        CoreCommand::DocumentLifecycle => {
            match serde_json::from_value::<DocumentLifecycleRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid document lifecycle request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::decide_document_lifecycle)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("document lifecycle decision should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryRecord => {
            match serde_json::from_value::<HistoryRecordRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history record request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::record)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryEntries => {
            match serde_json::from_value::<HistoryEntriesRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history entries request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::entries)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history entries should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryContent => {
            match serde_json::from_value::<HistoryContentRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history content request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::content)
            {
                Ok(data) => CoreResponse::success(id, serde_json::json!({"text": data})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryPlanEvent => {
            plugin_response(id, parsed.payload, crate::project::plan_event_command)
        }
        CoreCommand::HistoryRelocate => {
            match serde_json::from_value::<HistoryRelocateRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid history relocate request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::project::relocate)
            {
                Ok(()) => CoreResponse::success(id, serde_json::json!({"relocated": true})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryRename => {
            match serde_json::from_value::<HistoryRenameRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history rename request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::rename)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history entry should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryDelete => {
            match serde_json::from_value::<HistoryDeleteRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history delete request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::delete)
            {
                Ok(()) => CoreResponse::success(id, serde_json::json!({"deleted": true})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryIgnorePreview => {
            match serde_json::from_value::<HistoryIgnorePreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid history ignore preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::project::ignore_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history ignore preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryUsage => {
            match serde_json::from_value::<HistoryStorageRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history usage request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::usage)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history usage should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HistoryClear => {
            match serde_json::from_value::<HistoryStorageRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history clear request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::clear)
            {
                Ok(()) => CoreResponse::success(id, serde_json::json!({"cleared": true})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DiffCompareTexts => {
            plugin_response(id, parsed.payload, lithe_diff::compare_texts_command)
        }
        CoreCommand::ExecutionResolveSearchPath => {
            match serde_json::from_value::<crate::execution::SearchPathRequest>(parsed.payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::json!(crate::execution::resolve_search_path(request)),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search path request")
                        .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ExecutionResolveEnvironment => {
            match serde_json::from_value::<crate::execution::LaunchEnvironmentRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::execution::resolve_launch_environment(request))
                        .expect("environment encodes"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid launch environment request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ProcessLifecycleResolve => {
            match serde_json::from_value::<lithe_process::lifecycle::LifecycleRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_process::lifecycle::resolve(&request))
                        .expect("process lifecycle plan should encode"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid process lifecycle request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ProcessLifecycleInventory => {
            match serde_json::from_value::<lithe_process::inventory::InventoryRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_process::inventory::classify(&request))
                        .expect("process inventory should encode"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid process inventory request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ProcessLifecycleSettle => {
            match serde_json::from_value::<lithe_process::lifecycle::SettleRequest>(parsed.payload)
            {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_process::lifecycle::settle(&request))
                        .expect("process lifecycle settlement should encode"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid process lifecycle settlement request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::KeymapParseContext => {
            rules_response(id, "keymap.parseContext", parsed.payload)
        }
        CoreCommand::KeymapEvaluateContext => {
            rules_response(id, "keymap.evaluateContext", parsed.payload)
        }
        CoreCommand::KeymapAnalyzeConflicts => {
            rules_response(id, "keymap.analyzeConflicts", parsed.payload)
        }
        CoreCommand::KeymapModifierGestureStep => {
            rules_response(id, "keymap.modifierGestureStep", parsed.payload)
        }
        CoreCommand::EditorWorkbenchStateReduce => {
            rules_response(id, "editor.workbenchState.reduce", parsed.payload)
        }
        CoreCommand::WorkspaceClassifyVisibility => {
            rules_response(id, "workspace.classifyVisibility", parsed.payload)
        }
        CoreCommand::WorkspaceVisibilityDefaults => {
            rules_response(id, "workspace.visibilityDefaults", parsed.payload)
        }
        CoreCommand::WorkbenchChromeLayoutReduce => {
            rules_response(id, "workbench.chromeLayout.reduce", parsed.payload)
        }
        CoreCommand::TerminalParseLinkTarget => {
            let result =
                serde_json::from_value::<lithe_terminal::ParseLinkTargetsRequest>(parsed.payload)
                    .map_err(|_| {
                        CoreError::new(ErrorCode::InvalidRequest, "Invalid terminal link request")
                    })
                    .and_then(lithe_terminal::parse_link_targets);
            match result {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("terminal link result should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::MarkdownRender => {
            match serde_json::from_value::<MarkdownRenderRequest>(parsed.payload).map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Markdown render request")
                    .with_details(error.to_string())
            }) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::project::render(request))
                        .expect("Markdown render response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DebugCreateSession => {
            match serde_json::from_value::<crate::debug::CreateSessionRequest>(parsed.payload)
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
        CoreCommand::DebugLaunch => {
            match serde_json::from_value::<crate::debug::LaunchRequest>(parsed.payload)
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
        CoreCommand::DebugSteppingFilters => {
            match serde_json::from_value::<crate::debug::DebugSteppingFiltersRequest>(
                parsed.payload,
            )
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
        CoreCommand::DebugPlanVariablePage => reducer_response(
            id,
            parsed.payload,
            "Invalid debug variable page plan request",
            crate::debug::plan_variable_page,
        ),
        CoreCommand::DebugReduceVariablePage => reducer_response(
            id,
            parsed.payload,
            "Invalid debug variable page reduction request",
            crate::debug::reduce_variable_page,
        ),
        CoreCommand::DebugPlanDisassemblyWindow => reducer_response(
            id,
            parsed.payload,
            "Invalid debug disassembly window request",
            crate::debug::plan_disassembly_window,
        ),
        CoreCommand::DebugReduceDisassemblyPage => reducer_response(
            id,
            parsed.payload,
            "Invalid debug disassembly page reduction request",
            crate::debug::reduce_disassembly_page,
        ),
        CoreCommand::DebugPlanMemoryWindow => reducer_response(
            id,
            parsed.payload,
            "Invalid debug memory window request",
            crate::debug::plan_memory_window,
        ),
        CoreCommand::DebugLocateMemoryWindow => reducer_response(
            id,
            parsed.payload,
            "Invalid debug memory window location request",
            crate::debug::locate_memory_window,
        ),
        CoreCommand::DebugSetBreakpointsMuted => reducer_response(
            id,
            parsed.payload,
            "Invalid debug set-breakpoints-muted request",
            crate::debug::set_breakpoints_muted,
        ),
        CoreCommand::DebugRelocateBreakpoints => {
            match serde_json::from_value::<crate::debug::RelocateBreakpointsRequest>(parsed.payload)
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
        CoreCommand::DebugSetBreakpoints => {
            match serde_json::from_value::<crate::debug::SetBreakpointsRequest>(parsed.payload)
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
        CoreCommand::DebugSetExceptionBreakpoints => {
            match serde_json::from_value::<crate::debug::SetExceptionBreakpointsRequest>(
                parsed.payload,
            )
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
        CoreCommand::DebugSetFunctionBreakpoints => {
            match serde_json::from_value::<crate::debug::SetFunctionBreakpointsRequest>(
                parsed.payload,
            )
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
        CoreCommand::DebugSetInstructionBreakpoints => {
            match serde_json::from_value::<crate::debug::SetInstructionBreakpointsRequest>(
                parsed.payload,
            )
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
        CoreCommand::DebugDataBreakpointInfo => {
            match serde_json::from_value::<crate::debug::DataBreakpointInfoRequest>(parsed.payload)
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
        CoreCommand::DebugSetDataBreakpoints => {
            match serde_json::from_value::<crate::debug::SetDataBreakpointsRequest>(parsed.payload)
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
        CoreCommand::DebugSetVariable => {
            match serde_json::from_value::<crate::debug::SetVariableRequest>(parsed.payload)
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
        CoreCommand::DebugCancelOperation => {
            match serde_json::from_value::<crate::debug::CancelOperationRequest>(parsed.payload)
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
        CoreCommand::DebugExecute => {
            match serde_json::from_value::<crate::debug::ExecuteRequest>(parsed.payload)
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
        CoreCommand::DebugInspect => {
            match serde_json::from_value::<crate::debug::InspectRequest>(parsed.payload)
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
        CoreCommand::DebugReceive => {
            match serde_json::from_value::<crate::debug::ReceiveRequest>(parsed.payload)
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
        CoreCommand::DebugRunInTerminalResponse => {
            match serde_json::from_value::<crate::debug::DebugRunInTerminalResponseRequest>(
                parsed.payload,
            )
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
        CoreCommand::DebugStartDebuggingResponse => {
            match serde_json::from_value::<crate::debug::DebugStartDebuggingResponseRequest>(
                parsed.payload,
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
        CoreCommand::DebugDisconnect => {
            match serde_json::from_value::<crate::debug::DisconnectRequest>(parsed.payload)
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
        CoreCommand::DebugDestroySession => {
            match serde_json::from_value::<crate::debug::SessionRequest>(parsed.payload)
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
        CoreCommand::LspApplyTextEdits => {
            match serde_json::from_value::<crate::lsp::ApplyTextEditsRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP text edit request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::apply_text_edits)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP text edit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspDocumentIdentity => plugin_response(
            id,
            parsed.payload,
            lithe_lsp_policy::document_identity::resolve_command,
        ),
        CoreCommand::LspPlanWorkspaceEdit => plugin_response(
            id,
            parsed.payload,
            lithe_lsp_policy::workspace_edit_plan::plan_command,
        ),
        CoreCommand::LspClassifyNavigationTarget => plugin_response(
            id,
            parsed.payload,
            lithe_lsp_policy::navigation_target::classify_command,
        ),
        CoreCommand::LspDeclarationOrUsages => plugin_response(
            id,
            parsed.payload,
            lithe_lsp_policy::declaration_or_usages::decide_command,
        ),
        CoreCommand::EditorFind => match lithe_editor::find::command(parsed.payload, document) {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::EditorPreferencesResolve => plugin_response(
            id,
            parsed.payload,
            lithe_editor::preferences::resolve_command,
        ),
        CoreCommand::EditorRecentReduce => {
            plugin_response(id, parsed.payload, lithe_editor::recent::reduce_command)
        }
        CoreCommand::EditorRecoveryDecide => {
            plugin_response(id, parsed.payload, lithe_editor::recovery::decide_command)
        }
        CoreCommand::KeymapSchemesReduce => {
            rules_response(id, "keymap.schemes.reduce", parsed.payload)
        }
        CoreCommand::EditorPlanTextEdits => plugin_response(
            id,
            parsed.payload,
            lithe_editor::text_edit_plan::plan_command,
        ),
        CoreCommand::EditorParseNavigationInput => plugin_response(
            id,
            parsed.payload,
            lithe_editor::navigation_input::parse_command,
        ),
        CoreCommand::EditorNavigationHistoryReduce => plugin_response(
            id,
            parsed.payload,
            lithe_editor::navigation_history::reduce_command,
        ),
        CoreCommand::EditorLineEdit => {
            match serde_json::from_value::<lithe_editor::LineEditRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid editor line edit request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_editor::line_edit)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Editor line edit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::EditorLineCommentToken => {
            match serde_json::from_value::<lithe_editor::LineCommentTokenRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid editor line comment token request",
                    )
                    .with_details(error.to_string())
                })
                .map(lithe_editor::line_comment_token)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Comment token response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::NetworkProxyPolicy => {
            match serde_json::from_value::<lithe_network::NetworkProxyPolicyRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid network proxy policy request",
                    )
                    .with_details(error.to_string())
                })
                .map(lithe_network::proxy_policy)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Proxy policy response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspPlainSnippet => {
            match serde_json::from_value::<crate::lsp::PlainSnippetRequest>(parsed.payload).map_err(
                |error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP snippet request")
                        .with_details(error.to_string())
                },
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::lsp::plain_snippet(request))
                        .expect("LSP snippet response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspBuiltinCompletions => {
            match serde_json::from_value::<crate::lsp::BuiltinRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP completion request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::builtin_completions)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP completion response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspBuiltinHover => {
            match serde_json::from_value::<crate::lsp::BuiltinRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP hover request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::builtin_hover)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP hover response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspBuiltinNavigation => {
            match serde_json::from_value::<crate::lsp::BuiltinNavigationRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP navigation request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::builtin_navigation)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP navigation response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspMergeCompletions => plugin_response(
            id,
            parsed.payload,
            lithe_lsp_policy::completion_merge::merge_command,
        ),
        CoreCommand::LspActivationPolicy => plugin_response(
            id,
            parsed.payload,
            lithe_lsp_policy::activation::policy_command,
        ),
        CoreCommand::LspProblemsQuery => plugin_response(
            id,
            parsed.payload,
            lithe_lsp_policy::problems::query_command,
        ),
        CoreCommand::LspStartServer => {
            match serde_json::from_value::<crate::lsp::StartServerRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP start-server request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::start_server)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP start-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspStopServer => {
            match serde_json::from_value::<crate::lsp::SessionRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP stop-server request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::stop_server)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP stop-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspUpdateWorkspaceConfiguration => {
            match serde_json::from_value::<crate::lsp::UpdateWorkspaceConfigurationRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid workspace configuration update request",
                )
                .with_details(error.to_string())
            })
            .and_then(crate::lsp::update_workspace_configuration)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("workspace configuration update response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspSaveDocument => {
            match serde_json::from_value::<crate::lsp::SaveDocumentRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP save-document request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::save_document)
            {
                Ok(sent) => CoreResponse::success(id, json!({"sent": sent})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspDocumentUri => {
            plugin_response(id, parsed.payload, crate::lsp::document_uri_command)
        }
        CoreCommand::LspSyncDocument => {
            let uri = parsed
                .payload
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            match serde_json::from_value::<crate::lsp::SyncDocumentRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP sync-document request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::sync_document)
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
        CoreCommand::LspWorkspaceFilesChanged => {
            match serde_json::from_value::<crate::lsp::WorkspaceFilesChangedRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP workspace-files-changed request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::workspace_files_changed)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("LSP workspace-files-changed response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspCloseDocument => {
            match serde_json::from_value::<crate::lsp::CloseDocumentRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP close-document request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::close_document)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP close-document response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspRequest => {
            match serde_json::from_value::<crate::lsp::SemanticRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid semantic LSP request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::semantic_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Semantic LSP response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspBuildRequest => {
            match serde_json::from_value::<crate::lsp::BuildProviderRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid build-provider request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::build_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Build-provider response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspToolchainRequest => {
            match serde_json::from_value::<crate::lsp::ToolchainProviderRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid toolchain-provider request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::toolchain_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Toolchain response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspKeymapImportRequest => {
            match serde_json::from_value::<crate::lsp::KeymapImportProviderRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid shortcut import request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::keymap_import_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Shortcut import response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspVcsRequest => {
            match serde_json::from_value::<crate::lsp::VcsProviderRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid VCS-provider request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::vcs_provider_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("VCS-provider response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspFunctionProvider => {
            plugin_response(id, parsed.payload, crate::lsp::function_provider_command)
        }
        CoreCommand::LspFunctionRequest => {
            match serde_json::from_value::<crate::lsp::PluginFunctionRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid plugin-function request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::plugin_function_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Plugin-function response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspConfigureFunctionRelay => {
            match serde_json::from_value::<crate::lsp::RelaySnapshot>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid function relay snapshot")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::configure_function_relay)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspHostResponse => {
            match serde_json::from_value::<crate::lsp::PluginHostResponse>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid host-service response")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::plugin_host_response)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspHostNotify => {
            match serde_json::from_value::<crate::lsp::PluginHostNotification>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid host notification")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::plugin_host_notify)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Host notification response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspPluginServerRequest => {
            match serde_json::from_value::<crate::lsp::PluginServerRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid plugin language-server request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::plugin_server_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Plugin language-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspPluginServerNotify => {
            match serde_json::from_value::<crate::lsp::PluginServerNotify>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid plugin language-server notification",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::plugin_server_notify)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Plugin language-server notification response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspCancelOperation => {
            match serde_json::from_value::<crate::lsp::CancelOperationRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP cancellation request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::cancel_operation)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP cancellation response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspPollEvents => {
            match serde_json::from_value::<crate::lsp::SessionRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP poll-events request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::poll_events)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP poll-events response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspSetDetailedDiagnostics => {
            match serde_json::from_value::<crate::lsp::SetDetailedDiagnosticsRequest>(
                parsed.payload,
            )
            .map_err(|_| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid detailed diagnostics request",
                )
            })
            .and_then(crate::lsp::set_detailed_diagnostics)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspWaitEvents => {
            match serde_json::from_value::<crate::lsp::WaitEventsRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid LSP wait-events request")
                        .with_details(error.to_string())
                })
                .and_then(crate::lsp::wait_events)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP wait-events response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::LspDestroyServer => {
            match serde_json::from_value::<crate::lsp::SessionRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid LSP destroy-server request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::lsp::destroy_server)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("LSP destroy-server response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::RunConfigInspect => {
            match serde_json::from_value::<crate::execution::InspectRequest>(parsed.payload)
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
        CoreCommand::RunConfigProjectToolchain => {
            match serde_json::from_value::<crate::execution::ProjectToolchainRequest>(
                parsed.payload,
            )
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
        CoreCommand::RunConfigGenerate => {
            match serde_json::from_value::<crate::execution::GenerateRequest>(parsed.payload)
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
        CoreCommand::RunConfigArgumentText => {
            match serde_json::from_value::<crate::execution::ArgumentTextRequest>(parsed.payload) {
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
        CoreCommand::RunConfigResolve => {
            match serde_json::from_value::<crate::execution::ResolveRequest>(parsed.payload)
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
        CoreCommand::RunConfigDecideTitleAction => {
            match serde_json::from_value::<crate::execution::TitleActionRequest>(parsed.payload) {
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
        CoreCommand::RunConfigSelectContextConfiguration => {
            match serde_json::from_value::<crate::execution::ContextSelectionRequest>(
                parsed.payload,
            ) {
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
        CoreCommand::RunConfigUpdateOptions => {
            match serde_json::from_value::<crate::execution::UpdateOptionsRequest>(parsed.payload)
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
        CoreCommand::RunConfigUpdateToolPreferences => {
            match serde_json::from_value::<crate::execution::UpdateToolPreferencesRequest>(
                parsed.payload,
            )
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
        CoreCommand::RunConfigSaveEditorChanges => {
            match serde_json::from_value::<crate::execution::UpdateOptionsRequest>(parsed.payload)
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
        CoreCommand::RunConfigCreateUserConfiguration => {
            match serde_json::from_value::<crate::execution::CreateUserConfigurationRequest>(
                parsed.payload,
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
        CoreCommand::RunConfigProviderLaunchInputs => {
            match serde_json::from_value::<crate::execution::ProviderLaunchInputsRequest>(
                parsed.payload,
            )
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
        CoreCommand::RunConfigCreateLaunchPlan => {
            match serde_json::from_value::<crate::execution::LaunchPlanRequest>(parsed.payload)
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
        CoreCommand::GitWorkspaceCommitPrepare => {
            let result =
                serde_json::from_value::<git::workspace_commit::PrepareRequest>(parsed.payload)
                    .map_err(|error| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "Invalid workspace commit request",
                        )
                        .with_details(error.to_string())
                    })
                    .and_then(git::workspace_commit::prepare);
            match result {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Workspace commit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitWorkspaceCommitStep => {
            let result =
                serde_json::from_value::<git::workspace_commit::StepRequest>(parsed.payload)
                    .map_err(|error| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "Invalid workspace commit request",
                        )
                        .with_details(error.to_string())
                    })
                    .and_then(git::workspace_commit::step);
            match result {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Workspace commit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitCommitState => {
            match serde_json::from_value::<GitStatusRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid commit state request")
                        .with_details(error.to_string())
                })
                .and_then(git::commit_state)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Commit state should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitStatus => match serde_json::from_value::<GitStatusRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git status request")
                    .with_details(error.to_string())
            })
            .and_then(git::status)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::GitUntrackedDirectory => {
            match serde_json::from_value::<GitUntrackedDirectoryRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git untracked directory request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::untracked_directory)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git untracked directory should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitClassifyMetadataChanges => {
            plugin_response(id, parsed.payload, git::metadata_classification_command)
        }
        CoreCommand::GitWatchContext => {
            match serde_json::from_value::<GitWatchContextRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git watch context request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::watch_context)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git watch context should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }

        CoreCommand::GitWorktrees => {
            match serde_json::from_value::<GitWorktreesRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git worktrees request")
                        .with_details(error.to_string())
                })
                .and_then(git::worktrees)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git worktrees should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }

        CoreCommand::GitPullRequestContext => {
            match serde_json::from_value::<GitPullRequestContextRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git pull request context request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::pull_request_context)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git pull request context should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }

        CoreCommand::GitCommand => {
            match serde_json::from_value::<GitCommandRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git command request")
                        .with_details(error.to_string())
                })
                .and_then(git::command)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git command response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRepositoryRoot => {
            match serde_json::from_value::<GitRepositoryRootRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git repository root request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::discover_repository_root)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git repository root response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitWorkspaceStatusMerge => {
            match serde_json::from_value::<git::workspace_status::Request>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace Git status request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::workspace_status::plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git workspace status plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitResolveFileContext => {
            match serde_json::from_value::<git::file_context::Request>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git file context request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::file_context::resolve)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git file context should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRemotes
        | CoreCommand::GitTags
        | CoreCommand::GitParseCheckoutMetadata
        | CoreCommand::GitParseGitdirPointer => {
            let operation = match command {
                CoreCommand::GitRemotes => "remotes",
                CoreCommand::GitTags => "tags",
                CoreCommand::GitParseCheckoutMetadata => "checkout",
                CoreCommand::GitParseGitdirPointer => "gitdir",
                _ => unreachable!(),
            };
            match git::catalog::command(operation, parsed.payload) {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRebasePreview => {
            match serde_json::from_value::<GitRebasePreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase preview request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRebaseStart => {
            match serde_json::from_value::<GitRebaseStartRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase start request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_start)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase start should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRebaseSession => {
            match serde_json::from_value::<GitRebaseSessionRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase session request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_session)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase session should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRebaseControl => {
            match serde_json::from_value::<GitRebaseControlRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase control request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_control)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase control should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRepositorySetup => {
            match serde_json::from_value::<git::GitSetupRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git setup request")
                        .with_details(error.to_string())
                })
                .and_then(git::repository_setup)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git setup should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitInitialize => {
            match serde_json::from_value::<git::GitSetupRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git setup request")
                        .with_details(error.to_string())
                })
                .and_then(git::initialize_repository)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git setup should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitConfigureIdentity => {
            match serde_json::from_value::<git::GitConfigureIdentityRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git setup request")
                        .with_details(error.to_string())
                })
                .and_then(git::configure_identity)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git setup should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitPatchExport => {
            match serde_json::from_value::<PatchExportRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid patch export request")
                        .with_details(error.to_string())
                })
                .and_then(git::export_patch)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Patch export should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitPatchPreview => {
            match serde_json::from_value::<PatchPreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid patch preview request")
                        .with_details(error.to_string())
                })
                .and_then(git::preview_patch)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Patch preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitPatchApply => {
            match serde_json::from_value::<PatchApplyRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid patch apply request")
                        .with_details(error.to_string())
                })
                .and_then(git::apply_patch_reviewed)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Patch apply should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHistoryRewritePreview => {
            match serde_json::from_value::<GitHistoryRewritePreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid history rewrite preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::history_rewrite_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("History rewrite preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHistoryActionCapabilities => plugin_response(
            id,
            parsed.payload,
            lithe_git::history_action_capabilities_command,
        ),
        CoreCommand::GitRepositoryColorAssignments => plugin_response(
            id,
            parsed.payload,
            lithe_git::repository_color_assignments_command,
        ),
        CoreCommand::GitUpdatePlan => {
            plugin_response(id, parsed.payload, lithe_git::update_plan_command)
        }
        CoreCommand::GitHistoryWindow => {
            plugin_response(id, parsed.payload, lithe_git::history_window_command)
        }
        CoreCommand::GitGraphProjection => {
            plugin_response(id, parsed.payload, lithe_git::graph_projection_command)
        }
        CoreCommand::GitNormalizeChangeTargets => plugin_response(
            id,
            parsed.payload,
            lithe_git::normalize_change_targets_command,
        ),
        CoreCommand::GitRebasePlanPreview => {
            plugin_response(id, parsed.payload, lithe_git::rebase_plan_preview_command)
        }
        CoreCommand::GitReferenceTree => {
            plugin_response(id, parsed.payload, lithe_git::reference_tree_command)
        }
        CoreCommand::GitPrepareCommitSelection => plugin_response(
            id,
            parsed.payload,
            lithe_git::prepare_commit_selection_command,
        ),
        CoreCommand::GitShelf => plugin_response(id, parsed.payload, lithe_git::shelf_command),
        CoreCommand::GitCommitMessageContext => plugin_response(
            id,
            parsed.payload,
            lithe_git::commit_message_context_command,
        ),
        CoreCommand::GitChangelists => {
            plugin_response(id, parsed.payload, lithe_git::changelists_command)
        }
        CoreCommand::GitReconcileDeletedSelection => plugin_response(
            id,
            parsed.payload,
            lithe_git::reconcile_deleted_selection_command,
        ),
        CoreCommand::GitRollbackPrepare => {
            plugin_response(id, parsed.payload, lithe_git::rollback_prepare_command)
        }
        CoreCommand::GitParsePatch => {
            plugin_response(id, parsed.payload, lithe_git::parse_patch_command)
        }
        CoreCommand::GitHistoryQuery => {
            plugin_response(id, parsed.payload, lithe_git::history_query_command)
        }
        CoreCommand::GitRollbackPlan => {
            plugin_response(id, parsed.payload, lithe_git::rollback_plan_command)
        }
        CoreCommand::GitWrite => {
            match serde_json::from_value::<GitWriteRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git write request")
                        .with_details(error.to_string())
                })
                .and_then(git::write)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git write response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitAuthRespond => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Reply {
                request_id: String,
                answer: Option<String>,
            }
            match serde_json::from_value::<Reply>(parsed.payload) {
                Ok(reply) => CoreResponse::success(
                    id,
                    json!({ "accepted": lithe_git_host::authentication::respond(&reply.request_id, reply.answer) }),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git authentication response",
                    ),
                ),
            }
        }
        CoreCommand::GitExecutable => match git::configuration::locate_executable() {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::RemoteProbe => {
            match serde_json::from_value::<lithe_workspace::probe::ProbeRequest>(parsed.payload) {
                Ok(request) => match lithe_workspace::probe::probe(request) {
                    Ok(result) => CoreResponse::success(
                        id,
                        serde_json::to_value(result).expect("remote probe result should encode"),
                    ),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote probe request")
                        .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::RemoteConnect => {
            match serde_json::from_value::<lithe_workspace::sessions::ConnectRequest>(
                parsed.payload,
            ) {
                Ok(request) => match lithe_workspace::sessions::connect(request) {
                    Ok(result) => CoreResponse::success(
                        id,
                        serde_json::to_value(result).expect("remote connect result should encode"),
                    ),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote connect request")
                        .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::RemoteReadDir => {
            match serde_json::from_value::<lithe_workspace::sessions::ReadDirRequest>(
                parsed.payload,
            ) {
                Ok(request) => match lithe_workspace::sessions::read_dir(request) {
                    Ok(result) => CoreResponse::success(id, result),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote directory request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::RemoteReadFile => {
            match serde_json::from_value::<lithe_workspace::sessions::ReadFileRequest>(
                parsed.payload,
            ) {
                Ok(request) => match lithe_workspace::sessions::read_file(request) {
                    Ok(result) => CoreResponse::success(id, result),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote file request")
                        .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::RemoteWriteFile => {
            match serde_json::from_value::<lithe_workspace::sessions::WriteFileRequest>(
                parsed.payload,
            ) {
                Ok(request) => match lithe_workspace::sessions::write_file(request) {
                    Ok(result) => CoreResponse::success(id, result),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote file request")
                        .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::RemoteCaptureProcess => {
            match serde_json::from_value(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid host capture request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_workspace::host::capture_process)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::RemoteHostRequest
        | CoreCommand::RemoteRunProcess
        | CoreCommand::RemoteRunWatch
        | CoreCommand::RemoteRunTerminal => {
            let operation = match command {
                CoreCommand::RemoteHostRequest => lithe_workspace::host::request,
                CoreCommand::RemoteRunProcess => lithe_workspace::host::run_process,
                CoreCommand::RemoteRunWatch => lithe_workspace::host::run_watch,
                _ => lithe_workspace::host::run_terminal,
            };
            match serde_json::from_value(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid workspace host request")
                        .with_details(error.to_string())
                })
                .and_then(operation)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::RemoteUploadFile => {
            match serde_json::from_value(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid upload request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_workspace::upload::upload)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::RemoteHostInfo => {
            match parsed
                .payload
                .get("sessionId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "Missing host session"))
                .and_then(lithe_workspace::upload::host_info)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::RemoteDisconnect => {
            match serde_json::from_value::<lithe_workspace::sessions::DisconnectRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    json!({ "closed": lithe_workspace::sessions::disconnect(request) }),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote disconnect request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::RemoteCloseAll => CoreResponse::success(
            id,
            json!({ "closed": lithe_workspace::sessions::close_all() }),
        ),
        CoreCommand::RemoteAuthRespond => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Reply {
                request_id: String,
                answer: Option<String>,
            }
            match serde_json::from_value::<Reply>(parsed.payload) {
                Ok(reply) => CoreResponse::success(
                    id,
                    json!({ "accepted": lithe_workspace::probe::respond(&reply.request_id, reply.answer) }),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote authentication response",
                    ),
                ),
            }
        }
        CoreCommand::GitExecutionInspect | CoreCommand::GitExecutionConfigure => {
            match git::configuration::dispatch(
                parsed.payload,
                matches!(command, CoreCommand::GitExecutionConfigure),
            ) {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitRemoteUrl => match git::configuration::remote_url(parsed.payload) {
            Ok(value) => CoreResponse::success(id, value),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::GitConsolePresentation => {
            match serde_json::from_value::<git::console::Request>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git console snapshot")
                        .with_details(error.to_string())
                })
                .and_then(git::console::present)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Console presentation should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitFetchPlan => {
            match serde_json::from_value::<git::GitFetchPlanRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Fetch plan request")
                        .with_details(error.to_string())
                })
                .and_then(git::fetch_plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Fetch plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitDiff => match serde_json::from_value::<GitDiffRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git diff request")
                    .with_details(error.to_string())
            })
            .and_then(git::diff)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git diff response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::GitPlanDiffBlocks => {
            plugin_response(id, parsed.payload, lithe_git::plan_diff_blocks_command)
        }
        CoreCommand::GitApplyDiffBlock => {
            plugin_response(id, parsed.payload, lithe_git::apply_diff_block_command)
        }
        CoreCommand::GitStageGutterChange => {
            plugin_response(id, parsed.payload, lithe_git::stage_gutter_change_command)
        }
        CoreCommand::GitApply => match serde_json::from_value::<GitApplyRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git apply request")
                    .with_details(error.to_string())
            })
            .and_then(git::apply)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git apply response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::GitHistory => {
            match serde_json::from_value::<GitHistoryRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git history request")
                        .with_details(error.to_string())
                })
                .and_then(git::history)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git history response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitReferences => {
            match serde_json::from_value::<GitReferencesRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git references request")
                        .with_details(error.to_string())
                })
                .and_then(git::references)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git references response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHistoryPage => {
            match serde_json::from_value::<GitHistoryPageRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git history page request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::history_page)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git history page response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHistoryCursorClose => {
            match serde_json::from_value::<GitHistoryCursorCloseRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git history cursor close request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::close_history_cursor)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git history cursor close response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHistoryLocate => {
            match serde_json::from_value::<GitHistoryLocateRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git history locate request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::history_locate)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git history locate response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitBranchNamePreview => {
            match serde_json::from_value::<GitBranchNamePreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git branch name preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::branch_name_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git branch name preview response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitPushPreview => {
            match serde_json::from_value::<GitPushPreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git push preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::push_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git push preview response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitCommit => match serde_json::from_value::<GitCommitRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git commit request")
                    .with_details(error.to_string())
            })
            .and_then(git::commit)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git commit response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::GitCommitFiles => {
            match serde_json::from_value::<GitCommitFilesRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git commit files request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::commit_files)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git commit files response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitComparison => {
            match serde_json::from_value::<GitComparisonRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git comparison request")
                        .with_details(error.to_string())
                })
                .and_then(git::comparison)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git comparison response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitStashes => {
            match serde_json::from_value::<GitStashesRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git stashes request")
                        .with_details(error.to_string())
                })
                .and_then(git::stashes)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git stashes response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitCheckoutPreflight => {
            match serde_json::from_value::<GitCheckoutPreflightRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git checkout preflight request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::checkout_preflight)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git checkout preflight response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitPullPreflight => {
            match serde_json::from_value::<GitPullPreflightRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git pull preflight request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::pull_preflight)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git pull preflight response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitConflictMarkers => {
            match serde_json::from_value::<GitConflictMarkerRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git conflict marker request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::conflict_marker_paths)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git conflict marker response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitIntegrationPreflight => {
            match serde_json::from_value::<GitIntegrationPreflightRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git integration preflight request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::integration_preflight)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git integration preflight response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitOperationState => {
            match serde_json::from_value::<GitOperationStateRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git operation state request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::operation_state)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git operation state response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitBlame => match serde_json::from_value::<GitBlameRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git blame request")
                    .with_details(error.to_string())
            })
            .and_then(git::blame)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git blame response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::GitLineHistory => {
            match serde_json::from_value::<GitLineHistoryRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git line history request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::line_history)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git line history response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubParseRemote => {
            match serde_json::from_value::<ParseRemoteRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid GitHub remote request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_github::parse_remote)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("GitHub repository should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubRequestPlan => {
            match serde_json::from_value::<RequestPlanRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid GitHub request plan")
                        .with_details(error.to_string())
                })
                .and_then(lithe_github::request_plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("GitHub request plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitPullRequestCheckoutPreview => {
            match serde_json::from_value::<git::PullRequestCheckoutPreviewRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid pull-request checkout preview",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::checkout_pull_request_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Pull-request checkout preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitPullRequestCheckout => {
            match serde_json::from_value::<git::PullRequestCheckoutRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid pull-request checkout")
                        .with_details(error.to_string())
                })
                .and_then(git::checkout_pull_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Pull-request checkout should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubPullRequestCheckoutPlan => {
            match serde_json::from_value::<lithe_github::PullRequestCheckoutPlanRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid pull-request checkout plan",
                )
                .with_details(error.to_string())
            })
            .and_then(lithe_github::pull_request_checkout_plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Pull-request checkout plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubDeviceAuthorizationTransition => {
            match serde_json::from_value::<lithe_github::DeviceAuthorizationTransitionRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid GitHub device transition",
                )
                .with_details(error.to_string())
            })
            .and_then(lithe_github::device_authorization_transition)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Device transition should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubNormalizeResponse => {
            match serde_json::from_value::<NormalizeResponseRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid GitHub response")
                        .with_details(error.to_string())
                })
                .and_then(lithe_github::normalize_response)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DiagnosticsPrepareLogEvent => {
            match serde_json::from_value::<lithe_diagnostics::logging::LogEventRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::logging::prepare_log_event(request))
                        .expect("Log event should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log event request"),
                ),
            }
        }
        CoreCommand::DiagnosticsQueryLogs => {
            match serde_json::from_value::<lithe_diagnostics::logging::LogQueryRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::logging::query_logs(request))
                        .expect("Log query should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log query request"),
                ),
            }
        }
        CoreCommand::DiagnosticsRecordPluginOperation => CoreResponse::success(
            id,
            log_v2_result(lithe_diagnostics::plugin_operations::record_request(
                parsed.payload,
            )),
        ),
        CoreCommand::DiagnosticsDrainCoreEvents => CoreResponse::success(
            id,
            serde_json::to_value(lithe_diagnostics::core_events::drain())
                .expect("Core logs encode"),
        ),
        CoreCommand::DiagnosticsConfigureCoreEvents => {
            match serde_json::from_value::<CoreEventConfiguration>(parsed.payload) {
                Ok(configuration) => {
                    lithe_diagnostics::core_events::configure(
                        configuration.debug,
                        configuration.roots,
                    );
                    CoreResponse::success(id, json!({}))
                }
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Core event configuration",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::DiagnosticsEncodeLogBatch => {
            match serde_json::from_value::<lithe_diagnostics::record::EncodeLogBatchRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::record::encode_log_batch(request))
                        .expect("Log batch should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log batch request"),
                ),
            }
        }
        CoreCommand::DiagnosticsParseLogFilter => {
            match serde_json::from_value::<lithe_diagnostics::filter::ParseLogFilterRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::filter::parse_log_filter(request))
                        .expect("Filter response serializes"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid log parse_log_filter request",
                    ),
                ),
            }
        }
        CoreCommand::DiagnosticsComposeLogFilter => {
            match serde_json::from_value::<lithe_diagnostics::filter::ComposeLogFilterRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::filter::compose_log_filter(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid log compose_log_filter request",
                    ),
                ),
            }
        }
        CoreCommand::DiagnosticsFormatLogRecords => {
            match serde_json::from_value::<lithe_diagnostics::format::FormatLogRecordsRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::format::format_log_records(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid log format_log_records request",
                    ),
                ),
            }
        }
        CoreCommand::DiagnosticsPlanLogCollection => {
            match serde_json::from_value::<lithe_diagnostics::collection::PlanLogCollectionRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::collection::plan_log_collection(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log collection request"),
                ),
            }
        }
        CoreCommand::DiagnosticsQueryLogsV2 => {
            match serde_json::from_value::<lithe_diagnostics::query_v2::LogQueryV2Request>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::query_v2::query_logs_v2(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid structured log query"),
                ),
            }
        }
        CoreCommand::DiagnosticsPlanLogRetention => {
            match serde_json::from_value::<lithe_diagnostics::retention::PlanLogRetentionRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::retention::plan_log_retention(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log retention request"),
                ),
            }
        }
        CoreCommand::DiagnosticsResolveEngineLogSubject => {
            match serde_json::from_value::<crate::lsp::ResolveEngineLogSubjectRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::lsp::resolve_engine_log_subject(request))
                        .expect("Engine log subject should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid engine log subject request",
                    ),
                ),
            }
        }
        CoreCommand::DiagnosticsResolveLogChannel => {
            match serde_json::from_value::<lithe_diagnostics::channel::ResolveLogChannelRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::channel::resolve_log_channel(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log channel request"),
                ),
            }
        }
        CoreCommand::DiagnosticsRedactText => {
            match serde_json::from_value::<RedactTextRequest>(parsed.payload).map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid diagnostics redact request",
                )
                .with_details(error.to_string())
            }) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::redaction::redact_text(request))
                        .expect("Diagnostics redact response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DiagnosticsBuildManifest => {
            match serde_json::from_value::<BuildManifestRequest>(parsed.payload).map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid diagnostics manifest request",
                )
                .with_details(error.to_string())
            }) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::manifest::build_manifest(request))
                        .expect("Diagnostics manifest response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    };
    // Workspace steps already reconcile cancelled writes under a bounded cleanup
    // deadline. Preserve that continuation so a retry cannot duplicate a commit.
    if response.is_success() && !preserve_workspace_outcome {
        match crate::protocol::cancellation::check() {
            Ok(()) => response,
            Err(error) => CoreResponse::failure(response_id, error),
        }
    } else {
        response
    }
}

/// Commands whose engines read and run through the request's workspace host;
/// every other command refuses a remote workspace instead of touching the
/// local placeholder folder.
fn has_remote_workspace_adapter(command: &str) -> bool {
    command.starts_with("git.")
        || command.starts_with("build.")
        || matches!(
            command,
            "workspace.repositories"
                | "workspace.snapshot"
                | "workspace.listDirectories"
                | "workspace.search"
                | "workspace.searchEverywhere"
                | "file.read"
                | "file.write"
                | "workspace.searchIndex.warm"
                | "workspace.searchIndex.update"
                | "workspace.searchIndex.invalidate"
                | "workspace.replacePreview"
                | "lsp.startServer"
        )
}

#[cfg(test)]
mod tests {
    use super::execute_json;
    use serde_json::{json, Value};

    #[test]
    fn configure_core_events_sets_the_debug_gate_and_rejects_unknown_fields() {
        use lithe_diagnostics::core_events::{accepts, configure};
        use lithe_diagnostics::record::LogLevel;
        let send = |payload: Value| -> Value {
            serde_json::from_str(&execute_json(
                &json!({"command": "diagnostics.configureCoreEvents", "payload": payload})
                    .to_string(),
            ))
            .unwrap()
        };
        assert_eq!(send(json!({"debug": true, "roots": ["vcs"]}))["ok"], true);
        let scoped = (
            accepts(LogLevel::Debug, "vcs"),
            accepts(LogLevel::Debug, "app"),
        );
        let rejected = send(json!({"debug": true, "level": "debug"}));
        // Restore the process default before asserting so a failure cannot leak DEBUG.
        configure(false, None);
        assert_eq!(scoped, (true, false));
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"]["code"], "invalid_request");
        assert!(!accepts(LogLevel::Debug, "vcs"));
        assert!(accepts(LogLevel::Warn, "vcs"));
    }

    #[test]
    fn routes_semantic_lsp_runtime_commands() {
        let unknown_session = "missing-runtime-session";
        let requests = [
            json!({
                "command": "lsp.stopServer",
                "payload": { "sessionId": unknown_session }
            }),
            json!({
                "command": "lsp.syncDocument",
                "payload": {
                    "sessionId": unknown_session,
                    "uri": "file:///tmp/main.go",
                    "languageId": "go",
                    "text": "package main\n"
                }
            }),
            json!({
                "command": "lsp.closeDocument",
                "payload": {
                    "sessionId": unknown_session,
                    "uri": "file:///tmp/main.go"
                }
            }),
            json!({
                "command": "lsp.request",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "completion-1",
                    "operation": "completion",
                    "uri": "file:///tmp/main.go",
                    "position": { "line": 0, "utf16Column": 0 }
                }
            }),
            json!({
                "command": "lsp.buildRequest",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "build-1",
                    "method": "lithe/build/model",
                    "params": {}
                }
            }),
            json!({
                "command": "lsp.functionRequest",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "function-1",
                    "function": { "pluginID": "dev.lithe.fixture.provider", "functionID": "snapshot.refresh" },
                    "params": {}
                }
            }),
            json!({
                "command": "lsp.cancelOperation",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "completion-1"
                }
            }),
            json!({
                "command": "lsp.pollEvents",
                "payload": { "sessionId": unknown_session }
            }),
            json!({
                "command": "lsp.waitEvents",
                "payload": { "sessionId": unknown_session }
            }),
            json!({
                "command": "lsp.destroyServer",
                "payload": { "sessionId": unknown_session }
            }),
        ];

        for request in requests {
            let response: Value =
                serde_json::from_str(&execute_json(&request.to_string())).unwrap();
            assert_eq!(response["ok"], false, "request should reach the runtime");
            assert_eq!(response["error"]["code"], "invalid_request");
            assert_eq!(response["error"]["details"], unknown_session);
        }

        let invalid_start: Value = serde_json::from_str(&execute_json(
            &json!({
                "command": "lsp.startServer",
                "payload": {
                    "providerId": "go",
                    "executablePath": "",
                    "rootUri": "file:///tmp/project",
                    "workingDirectory": "/tmp/project"
                }
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(invalid_start["ok"], false);
        assert_eq!(invalid_start["error"]["code"], "invalid_request");
        assert_eq!(
            invalid_start["error"]["details"],
            "executablePath/workingDirectory"
        );
    }
}
