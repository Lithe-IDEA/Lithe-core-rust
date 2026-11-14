//! Command entry points that decode a payload and run validation, run and test plans
//! or a language-server launch plan.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidatePayload {
    manifest: Value,
    /// Capability versions the host runs. When sent, a manifest that
    /// requires anything else is refused.
    #[serde(default)]
    host_capabilities: Option<HostCapabilities>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanPayload<Request> {
    manifest: Value,
    #[serde(rename = "languageID")]
    language_id: String,
    request: Request,
}

/// A launch plan names the process by the language whose server it is, by
/// the build system whose provider it is, or by the VCS it provides.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchPlanPayload {
    manifest: Value,
    #[serde(default, rename = "languageID")]
    language_id: Option<String>,
    #[serde(default, rename = "buildSystemID")]
    build_system_id: Option<String>,
    #[serde(default, rename = "vcsProviderID")]
    vcs_provider_id: Option<String>,
    /// Selects the plugin's backend.
    #[serde(default)]
    backend: bool,
    #[serde(default, rename = "debugAdapterID")]
    debug_adapter_id: Option<String>,
    request: LanguageServerLaunchPlanRequest,
}

enum LaunchSelection<'a> {
    Language(&'a str),
    BuildSystem(&'a str),
    VcsProvider(&'a str),
    Backend,
    DebugAdapter(&'a str),
}

/// `plugins.declarative.validate`: returns `{status: "ok", plugin}` with the
/// normalized manifest, or `{status: "rejected", error}`.
pub fn validate_command(payload: Value) -> Result<Value, CoreError> {
    let payload: ValidatePayload = decode_payload(payload, "plugins.declarative.validate")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            if let Some(host) = &payload.host_capabilities {
                capabilities::check_hosted(manifest.requires(), host)?;
            }
            serde_json::to_value(manifest)
                .map(|value| ("plugin", value))
                .map_err(|error| PluginRejection::new("plugin.manifest.invalid", error.to_string()))
        },
    )))
}

/// `plugins.declarative.runPlan`: expands the run template for one file.
pub fn run_plan_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PlanPayload<RunPlanRequest> =
        decode_payload(payload, "plugins.declarative.runPlan")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            run_plan(&manifest, &payload.language_id, &payload.request).map(|plan| ("plan", plan))
        },
    )))
}

/// `plugins.declarative.discoverTests`: lists workspace and file test items.
pub fn discover_tests_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PlanPayload<TestDiscoveryRequest> =
        decode_payload(payload, "plugins.declarative.discoverTests")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            discover_tests(&manifest, &payload.language_id, &payload.request)
                .map(|items| ("items", items))
        },
    )))
}

/// `plugins.declarative.testPlan`: expands the test template for one scope.
pub fn test_plan_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PlanPayload<TestPlanRequest> =
        decode_payload(payload, "plugins.declarative.testPlan")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            test_plan(&manifest, &payload.language_id, &payload.request).map(|plan| ("plan", plan))
        },
    )))
}

/// `plugins.declarative.languageServerLaunchPlan`: expands a language
/// server's launch declaration with the host's directories and architecture.
pub fn language_server_launch_plan_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "plugins.declarative.languageServerLaunchPlan";
    let payload: LaunchPlanPayload = decode_payload(payload, COMMAND)?;
    let selected = match (
        &payload.language_id,
        &payload.build_system_id,
        &payload.vcs_provider_id,
        payload.backend,
        &payload.debug_adapter_id,
    ) {
        (Some(language_id), None, None, false, None) => LaunchSelection::Language(language_id),
        (None, Some(build_system_id), None, false, None) => {
            LaunchSelection::BuildSystem(build_system_id)
        }
        (None, None, Some(vcs_id), false, None) => LaunchSelection::VcsProvider(vcs_id),
        (None, None, None, true, None) => LaunchSelection::Backend,
        (None, None, None, false, Some(adapter_id)) => LaunchSelection::DebugAdapter(adapter_id),
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!("Invalid {COMMAND} payload"),
            )
            .with_details(
                "Name exactly one of languageID, buildSystemID, vcsProviderID, backend, and debugAdapterID.",
            ))
        }
    };
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            match selected {
                LaunchSelection::Language(language_id) => {
                    language_server_launch_plan(&manifest, language_id, &payload.request)
                }
                LaunchSelection::BuildSystem(build_system_id) => {
                    build_provider_launch_plan(&manifest, build_system_id, &payload.request)
                }
                LaunchSelection::VcsProvider(vcs_id) => {
                    vcs_provider_launch_plan(&manifest, vcs_id, &payload.request)
                }
                LaunchSelection::Backend => backend_launch_plan(&manifest, &payload.request),
                LaunchSelection::DebugAdapter(adapter_id) => {
                    debug_adapter_launch_plan(&manifest, adapter_id, &payload.request)
                }
            }
            .map(|plan| ("plan", plan))
        },
    )))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrammarPayload {
    manifest: Value,
    #[serde(rename = "languageID")]
    language_id: String,
    /// Text of the grammar file the host read from the package.
    grammar: String,
}

/// `plugins.declarative.decodeGrammar`: validates the grammar file of one
/// language and returns what the editor registers for it.
pub fn decode_grammar_command(payload: Value) -> Result<Value, CoreError> {
    let payload: GrammarPayload = decode_payload(payload, "plugins.declarative.decodeGrammar")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            let language = manifest
                .languages
                .iter()
                .find(|language| language.id == payload.language_id)
                .filter(|language| language.grammar.is_some() || language.configuration.is_some())
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.grammar.unavailable",
                        format!(
                            "The plugin declares no grammar for {}.",
                            payload.language_id
                        ),
                    )
                })?;
            let grammar = language
                .grammar
                .as_ref()
                .map(|grammar| match grammar.kind {
                    // The editor's engine parses the original text; Core only
                    // proves that it can and names the scope it registers.
                    lithe_plugin_grammar::GrammarKind::Textmate => {
                        lithe_plugin_grammar::decode_textmate(&payload.grammar).map(|scope_name| {
                            json!({
                                "kind": grammar.kind,
                                "scopeName": scope_name,
                                "source": payload.grammar,
                            })
                        })
                    }
                })
                .transpose()?;
            Ok((
                "language",
                json!({
                    "id": language.id,
                    "fileExtensions": language.file_extensions,
                    "fileNames": language.file_names,
                    "grammar": grammar,
                    "configuration": language.configuration,
                }),
            ))
        },
    )))
}
