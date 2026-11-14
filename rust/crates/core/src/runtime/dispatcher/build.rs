//! `build.*` commands: build detection, options, plans and decoded tool output.

use super::{plugin_response, reducer_response};
use crate::protocol::{BuildCommand, CoreResponse};
/// The manifest type every domain command validates plugin manifests with.
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
use serde_json::Value;

/// Runs one `build.*` command.
pub(super) fn dispatch(command: BuildCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match command {
        BuildCommand::Detect => {
            plugin_response(id, payload, crate::build::build_detect_command::<Manifest>)
        }
        BuildCommand::ProjectOptions => {
            plugin_response(id, payload, crate::build::project_options_command)
        }
        BuildCommand::ResolveStoredOptions => {
            plugin_response(id, payload, crate::build::resolve_stored_options_command)
        }
        BuildCommand::EditStoredOption => plugin_response(
            id,
            payload,
            crate::build::build_edit_stored_option_command::<Manifest>,
        ),
        BuildCommand::InvocationPlan => plugin_response(
            id,
            payload,
            crate::build::build_invocation_plan_command::<Manifest>,
        ),
        BuildCommand::CheckToolVersion => plugin_response(
            id,
            payload,
            crate::build::build_check_tool_version_command::<Manifest>,
        ),
        BuildCommand::ResolveOptions => plugin_response(
            id,
            payload,
            crate::build::build_resolve_options_command::<Manifest>,
        ),
        BuildCommand::ModelPlan => plugin_response(
            id,
            payload,
            crate::build::build_model_plan_command::<Manifest>,
        ),
        BuildCommand::TaskPlan => plugin_response(
            id,
            payload,
            crate::build::build_task_plan_command::<Manifest>,
        ),
        BuildCommand::PrepareDebugPlan => plugin_response(
            id,
            payload,
            crate::build::build_prepare_debug_plan_command::<Manifest>,
        ),
        BuildCommand::DecodeDebugPreparation => plugin_response(
            id,
            payload,
            crate::build::build_decode_debug_preparation_command::<Manifest>,
        ),
        BuildCommand::ResolveModelInputs => plugin_response(
            id,
            payload,
            crate::build::build_resolve_model_inputs_command::<Manifest>,
        ),
        BuildCommand::ConfigurationEditPlan => plugin_response(
            id,
            payload,
            crate::build::build_configuration_edit_plan_command::<Manifest>,
        ),
        BuildCommand::DecodeConfigurationEdit => plugin_response(
            id,
            payload,
            crate::build::build_decode_configuration_edit_command::<Manifest>,
        ),
        BuildCommand::ConfigurationPlan => plugin_response(
            id,
            payload,
            crate::build::build_configuration_plan_command::<Manifest>,
        ),
        BuildCommand::DecodeConfiguration => plugin_response(
            id,
            payload,
            crate::build::build_decode_configuration_command::<Manifest>,
        ),
        BuildCommand::PrepareRunPlan => plugin_response(
            id,
            payload,
            crate::build::build_prepare_run_plan_command::<Manifest>,
        ),
        BuildCommand::DecodeRunPreparation => plugin_response(
            id,
            payload,
            crate::build::build_decode_run_preparation_command::<Manifest>,
        ),
        BuildCommand::TestPlan => plugin_response(
            id,
            payload,
            crate::build::build_test_plan_command::<Manifest>,
        ),
        BuildCommand::TestDiscoveryPlan => plugin_response(
            id,
            payload,
            crate::build::build_test_discovery_plan_command::<Manifest>,
        ),
        BuildCommand::DecodeTestDiscovery => plugin_response(
            id,
            payload,
            crate::build::build_decode_test_discovery_command::<Manifest>,
        ),
        BuildCommand::DependencyPlan => plugin_response(
            id,
            payload,
            crate::build::build_dependency_plan_command::<Manifest>,
        ),
        BuildCommand::DecodeModel => plugin_response(
            id,
            payload,
            crate::build::build_decode_model_command::<Manifest>,
        ),
        BuildCommand::DecodeTestResults => plugin_response(
            id,
            payload,
            crate::build::build_decode_test_results_command::<Manifest>,
        ),
        BuildCommand::DecodeDiagnostics => plugin_response(
            id,
            payload,
            crate::build::build_decode_diagnostics_command::<Manifest>,
        ),
        BuildCommand::DecodeDependencies => plugin_response(
            id,
            payload,
            crate::build::build_decode_dependencies_command::<Manifest>,
        ),
        BuildCommand::SyncFingerprint => plugin_response(
            id,
            payload,
            crate::build::build_sync_fingerprint_command::<Manifest>,
        ),
        BuildCommand::ResolveTestRunOutcome => reducer_response(
            id,
            payload,
            "Invalid test run outcome request",
            crate::build::resolve_test_run_outcome,
        ),
        BuildCommand::MergeTestOutcomes => reducer_response(
            id,
            payload,
            "Invalid test outcome merge request",
            crate::build::merge_test_outcomes,
        ),
    }
}
