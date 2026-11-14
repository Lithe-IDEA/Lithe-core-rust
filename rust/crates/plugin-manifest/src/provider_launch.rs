//! Launch plans of the backends behind build, VCS and debug providers, and the debug
//! adapters' start arguments.

use super::*;

/// The launch plan of a build system's provider. A provider that shares a
/// language server gets that server's plan; `buildProvider.languageServer`
/// then tells the host to send build requests to the session it already
/// keeps for the language instead of starting a second process.
pub(crate) fn build_provider_launch_plan(
    manifest: &DeclarativePluginManifest,
    build_system_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let unavailable = |message: String| PluginRejection::new("plugin.launch.unavailable", message);
    let build_system = manifest
        .build_systems
        .iter()
        .find(|build_system| build_system.id == build_system_id)
        .ok_or_else(|| {
            unavailable(format!(
                "The plugin declares no build system {build_system_id}."
            ))
        })?;
    let provider = build_system.provider.as_ref().ok_or_else(|| {
        unavailable(format!(
            "The build system {build_system_id} has no provider process."
        ))
    })?;
    let mut description = json!({
        "buildSystemID": build_system.id,
        "executesProjectCode": provider.executes_project_code(),
        "mayUseNetwork": provider.may_use_network(),
    });
    let mut plan = match provider.process() {
        lithe_build::ProviderProcess::LanguageServer(language_id) => {
            description["languageServer"] = json!(language_id);
            language_server_launch_plan(manifest, language_id, request)?
        }
        lithe_build::ProviderProcess::Own(process) => {
            server_launch_plan(manifest, process, request)?
        }
    };
    plan["buildProvider"] = description;
    Ok(plan)
}

/// The launch plan of a VCS provider. `vcsProvider` repeats what the host
/// needs to resolve the client tool and to check the provider's answer in
/// `initialize`, which must name the same VCS.
pub(crate) fn vcs_provider_launch_plan(
    manifest: &DeclarativePluginManifest,
    vcs_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let provider = manifest
        .vcs_providers
        .iter()
        .find(|provider| provider.id == vcs_id)
        .ok_or_else(|| {
            PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The plugin declares no VCS provider {vcs_id}."),
            )
        })?;
    let mut plan = server_launch_plan(manifest, &provider.process, request)?;
    plan["vcsProvider"] = provider.descriptor(&manifest.id);
    Ok(plan)
}

/// The launch plan of the plugin's backend. `backend.hostServices` lists the
/// host services its permissions grant, which the host passes to
/// `lsp.startServer` so Core admits only those requests.
pub(crate) fn backend_launch_plan(
    manifest: &DeclarativePluginManifest,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let backend = manifest.backend.as_ref().ok_or_else(|| {
        PluginRejection::new(
            "plugin.launch.unavailable",
            "The plugin declares no backend.",
        )
    })?;
    let mut plan = server_launch_plan(manifest, &backend.process, request)?;
    plan["backend"] = lithe_plugin_backend::plan_descriptor(
        manifest.capability_version(capabilities::BACKEND),
        &manifest.backend_host_services(),
    );
    Ok(plan)
}

/// The launch plan of a debug adapter. `debugAdapter` repeats what the host
/// needs besides the process: the transport and what to show. An adapter on
/// the runtime Run selected has no process of its own to plan yet. An adapter
/// inside the plugin's language server plans that server, as a build provider
/// on a shared server does.
pub(crate) fn debug_adapter_launch_plan(
    manifest: &DeclarativePluginManifest,
    adapter_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    use lithe_debug::adapters::DebugProcess;
    let adapter = manifest
        .debug_adapters
        .iter()
        .find(|adapter| adapter.id == adapter_id)
        .ok_or_else(|| {
            PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The plugin declares no debug adapter {adapter_id}."),
            )
        })?;
    let mut plan = match &adapter.launch.process {
        Some(DebugProcess::Server(process)) => server_launch_plan(manifest, process, request)?,
        Some(DebugProcess::SelectedRuntime(_)) => {
            return Err(PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The debug adapter {adapter_id} starts on the selected runtime."),
            ));
        }
        None => {
            let language_id = adapter
                .launch
                .endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.language_id.as_deref())
                .unwrap_or_default();
            hosting_language_server_plan(manifest, language_id, request)?
        }
    };
    plan["debugAdapter"] = adapter.plan_descriptor(&manifest.id);
    Ok(plan)
}

/// The plan of the language server a `tcpLanguageServer` adapter runs in.
/// The server's own session starts it; this plan only names the server the
/// adapter belongs to. A server the backend launches is
/// asked by that session with the scratch directories of its own start, so
/// here its plan names the function and demands no directory.
fn hosting_language_server_plan(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let server = manifest
        .language_servers
        .iter()
        .find(|server| server.language_id == language_id)
        .ok_or_else(|| {
            PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The plugin does not provide a language server for {language_id}."),
            )
        })?;
    match &server.process.launch {
        LanguageServerLaunch::Backend {
            function,
            timeout_milliseconds,
        } => Ok(json!({
            "launch": {
                "kind": "backend",
                "function": function,
                "timeoutMilliseconds": timeout_milliseconds
                    .unwrap_or(lithe_plugin_model::process::RESOLVE_TIMEOUT_MILLISECONDS),
            },
            "arguments": [],
            "validationArguments": [],
            "runtimeRequirements": [],
            "requiredDirectories": [],
            "requiredPackagePaths": [],
        })),
        _ => language_server_launch_plan(manifest, language_id, request),
    }
}

/// `debug.adapters.startArguments`: checks one start against the adapter's
/// declaration and returns the DAP arguments its configuration defaults and
/// target bindings give.
pub fn debug_start_arguments_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "debug.adapters.startArguments";
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        manifest: Value,
        #[serde(rename = "adapterID")]
        adapter_id: String,
        #[serde(flatten)]
        start: lithe_debug::adapters::StartRequest,
    }
    let payload: Payload = decode_payload(payload, COMMAND)?;
    let started = validate_manifest_value(payload.manifest).and_then(|manifest| {
        let adapter = manifest
            .debug_adapters
            .iter()
            .find(|adapter| adapter.id == payload.adapter_id)
            .ok_or_else(|| {
                PluginRejection::new(
                    "plugin.launch.unavailable",
                    format!(
                        "The plugin declares no debug adapter {}.",
                        payload.adapter_id
                    ),
                )
            })?;
        lithe_debug::adapters::start_arguments(adapter, &payload.start)
    });
    // `connection` is present only for a remote target, so a host that never
    // attaches to one can ignore it.
    let connection = started
        .as_ref()
        .ok()
        .and_then(|started| started.connection.clone());
    let mut result =
        outcome(started.map(|started| ("arguments", Value::Object(started.arguments))));
    if let Some(connection) = connection {
        result["connection"] = json!(connection);
    }
    Ok(result)
}

/// `debug.adapters.executablePlan`: where a host looks for the program a
/// launch debugs, when the adapter's configuration binds `program` to
/// `target.executable`. `plan` is `null` for an adapter with no such lookup.
/// The host owns the file system, so it checks `configured` and then each
/// candidate; Core only says what to look at and in which order.
pub fn debug_executable_plan_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "debug.adapters.executablePlan";
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        manifest: Value,
        #[serde(rename = "adapterID")]
        adapter_id: String,
        #[serde(default)]
        settings: BTreeMap<String, Value>,
        #[serde(default)]
        project_settings: BTreeMap<String, Value>,
        /// Absolute root of the open project.
        #[serde(default)]
        workspace: Option<String>,
        /// Absolute path of the source file being debugged, when there is one.
        #[serde(default)]
        target_path: Option<String>,
    }
    let payload: Payload = decode_payload(payload, COMMAND)?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            let adapter = manifest
                .debug_adapters
                .iter()
                .find(|adapter| adapter.id == payload.adapter_id)
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.launch.unavailable",
                        format!(
                            "The plugin declares no debug adapter {}.",
                            payload.adapter_id
                        ),
                    )
                })?;
            let Some(lookup) = adapter
                .configurations
                .as_ref()
                .and_then(|configurations| configurations.launch.as_ref())
                .and_then(|configuration| configuration.executable.as_ref())
            else {
                return Ok(("plan", Value::Null));
            };
            let layers = SettingLayers {
                global: &payload.settings,
                project: &payload.project_settings,
            };
            let configured = manifest
                .resolve_settings(layers)
                .into_iter()
                .find(|setting| setting.id == lookup.setting)
                .and_then(|setting| match &setting.value {
                    Value::String(path) if !path.is_empty() => {
                        Some(json!({"path": path, "source": setting.source}))
                    }
                    _ => None,
                });
            Ok((
                "plan",
                json!({
                    "setting": lookup.setting,
                    "configured": configured,
                    "candidates": lookup.expanded_candidates(
                        payload.workspace.as_deref(),
                        payload.target_path.as_deref(),
                    ),
                }),
            ))
        },
    )))
}
