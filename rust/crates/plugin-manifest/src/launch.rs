//! Language-server launch plans: request layers, launch values and requirements, and
//! the expansion of each server process.

use super::*;

/// Host facts substituted into a language server's launch templates. The
/// host owns every directory; Core only joins text and never touches disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageServerLaunchPlanRequest {
    /// Absolute root of the installed plugin package.
    plugin_directory: String,
    /// Absolute writable directory the host keeps for this plugin's server.
    state_directory: String,
    /// Absolute writable directory for the server's state of this workspace.
    /// A host without a workspace, such as one starting a plugin backend,
    /// omits it; a launch that names it then has no usable plan.
    #[serde(default)]
    workspace_state_directory: Option<String>,
    /// Absolute current-session `plugins/<id>/files` directory. The host creates it
    /// outside installed resources; Core never infers a writable path.
    #[serde(default)]
    log_directory: Option<String>,
    /// Absolute writable scratch directory for one start of the server. Only
    /// a launch the backend resolves needs it.
    #[serde(default)]
    temporary_directory: Option<String>,
    /// Host CPU architecture, one of `KNOWN_ARCHITECTURES`.
    architecture: String,
    /// Values the user stored for the plugin's settings, by setting ID. A
    /// missing or no longer acceptable value falls back to the default.
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    /// The current workspace's overrides of project-scoped settings.
    #[serde(default)]
    project_settings: BTreeMap<String, Value>,
}

impl LanguageServerLaunchPlanRequest {
    fn layers(&self) -> SettingLayers<'_> {
        SettingLayers {
            global: &self.settings,
            project: &self.project_settings,
        }
    }

    /// The four writable directories a backend-resolved launch receives.
    /// Each is required, because the answer may name paths in any of them.
    fn backend_launch_directories(
        &self,
    ) -> Validation<lithe_plugin_language_servers::LaunchDirectories<'_>> {
        fn required<'a>(value: Option<&'a String>, name: &str) -> Validation<&'a str> {
            match value {
                Some(directory) => launch_directory(directory, name),
                None => Err(PluginRejection::new(
                    "plugin.launch.invalidRequest",
                    format!("The launch request has no usable {name}."),
                )),
            }
        }
        Ok(lithe_plugin_language_servers::LaunchDirectories {
            state: launch_directory(&self.state_directory, "stateDirectory")?,
            workspace_state: required(
                self.workspace_state_directory.as_ref(),
                "workspaceStateDirectory",
            )?,
            log: required(self.log_directory.as_ref(), "logDirectory")?,
            temporary: required(self.temporary_directory.as_ref(), "temporaryDirectory")?,
        })
    }
}

/// Validated substitution values of one launch plan.
struct LaunchValues<'a> {
    plugin_directory: &'a str,
    state_directory: &'a str,
    workspace_state_directory: &'a str,
    log_directory: &'a str,
    architecture: &'a str,
    /// Effective value of every declared setting.
    settings: &'a BTreeMap<String, Value>,
}

/// What a launch plan needs from the host besides the command line.
#[derive(Default)]
struct LaunchRequirements {
    /// Package-relative files or directories the templates point at.
    package_paths: BTreeSet<String>,
    uses_state_directory: bool,
    uses_workspace_state_directory: bool,
    uses_log_directory: bool,
}

pub(crate) fn language_server_launch_plan(
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
    let mut plan = match &server.process.launch {
        LanguageServerLaunch::Backend {
            function,
            timeout_milliseconds,
        } => backend_resolved_launch_plan(
            manifest,
            &server.language_id,
            function,
            *timeout_milliseconds,
            request,
        )?,
        _ => server_launch_plan(manifest, &server.process, request)?,
    };
    if let Some(configuration) = &server.workspace_configuration {
        plan["workspaceConfiguration"] = expand_configuration(
            configuration,
            &manifest.effective_settings(request.layers()),
        );
    }
    // The engine enforces the session fields.
    plan["sessionPolicy"] = server.session.policy(&server.language_id);
    Ok(plan)
}

/// The first step of a language server whose backend resolves its launch:
/// the function to call and its params. The host calls
/// the function and passes the answer to `plugins.languageServers.decodeLaunch`,
/// whose plan it then starts like any other.
fn backend_resolved_launch_plan(
    manifest: &DeclarativePluginManifest,
    server_id: &str,
    function: &str,
    timeout_milliseconds: Option<u64>,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let directories = request.backend_launch_directories()?;
    if !KNOWN_ARCHITECTURES.contains(&request.architecture.as_str()) {
        return Err(PluginRejection::new(
            "plugin.launch.invalidRequest",
            format!(
                "The architecture {} is not supported.",
                request.architecture
            ),
        ));
    }
    let settings = manifest.effective_settings(request.layers());
    let mut plan = json!({
        "owner": {"pluginID":manifest.id,"version":manifest.version},
        "launch": {
            "kind": "backend",
            "function": function,
            "timeoutMilliseconds": timeout_milliseconds
                .unwrap_or(lithe_plugin_model::process::RESOLVE_TIMEOUT_MILLISECONDS),
        },
        "resolveParams": lithe_plugin_language_servers::resolve_params(
            server_id,
            &request.architecture,
            &directories,
            &settings,
        ),
        "arguments": [],
        "validationArguments": [],
        "runtimeRequirements": [],
        "requiredDirectories": ["state", "workspaceState", "log", "temporary"],
        "requiredPackagePaths": [],
    });
    if !settings.is_empty() {
        plan["settings"] = json!(settings);
    }
    Ok(plan)
}

/// `plugins.languageServers.decodeLaunch`: checks a launch function's answer
/// against the server's declaration and the directories of this start, and
/// returns the plan the host starts. A server whose launch is not resolved
/// by the backend is refused, so a host never runs an answer it did not ask
/// for.
pub fn decode_language_server_launch_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "plugins.languageServers.decodeLaunch";
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        manifest: Value,
        #[serde(rename = "languageID")]
        language_id: String,
        request: LanguageServerLaunchPlanRequest,
        result: Value,
    }
    let payload: Payload = decode_payload(payload, COMMAND)?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            let server = manifest
                .language_servers
                .iter()
                .find(|server| server.language_id == payload.language_id)
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.launch.unavailable",
                        format!(
                            "The plugin does not provide a language server for {}.",
                            payload.language_id
                        ),
                    )
                })?;
            if !server.process.is_backend_launch() {
                return Err(PluginRejection::new(
                    "plugin.launch.notBackend",
                    format!(
                        "The language server for {} is not launched by the plugin backend.",
                        payload.language_id
                    ),
                ));
            }
            let directories = payload.request.backend_launch_directories()?;
            let tool_ids: BTreeSet<&str> = manifest
                .tool_definitions
                .iter()
                .filter_map(|definition| definition["id"].as_str())
                .collect();
            let (mut plan, gates) = lithe_plugin_language_servers::decode_answer(
                payload.result,
                &directories,
                &tool_ids,
            )?;
            let settings = manifest.effective_settings(payload.request.layers());
            if !settings.is_empty() {
                plan["settings"] = json!(settings);
            }
            if plan.get("workspaceConfiguration").is_none() {
                if let Some(configuration) = &server.workspace_configuration {
                    plan["workspaceConfiguration"] = expand_configuration(configuration, &settings);
                }
            }
            let mut policy = server.session.policy(&server.language_id);
            policy["gates"] = json!(gates);
            plan["sessionPolicy"] = policy;
            Ok(("plan", plan))
        },
    )))
}

/// Expands a validated `workspaceConfiguration` with effective setting
/// values. A string that is exactly one setting keeps the setting's JSON type.
pub(crate) fn expand_configuration(value: &Value, settings: &BTreeMap<String, Value>) -> Value {
    match value {
        Value::String(text) => {
            if let Some(id) = sole_setting_reference(text) {
                return settings.get(&id).cloned().unwrap_or(Value::Null);
            }
            let parts = parse_template_with(text, LaunchVariable::parse).unwrap_or_default();
            Value::String(
                parts
                    .into_iter()
                    .map(|part| match part {
                        TemplatePart::Text(text) => text,
                        TemplatePart::Variable(LaunchVariable::Setting(id)) => {
                            lithe_plugin_model::settings::setting_text(settings.get(&id))
                        }
                        TemplatePart::Variable(_) => String::new(),
                    })
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| expand_configuration(item, settings))
                .collect(),
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, item)| (key.clone(), expand_configuration(item, settings)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub(crate) fn server_launch_plan(
    manifest: &DeclarativePluginManifest,
    process: &ServerProcess,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let effective_settings = manifest.effective_settings(request.layers());
    let values = LaunchValues {
        plugin_directory: launch_directory(&request.plugin_directory, "pluginDirectory")?,
        state_directory: launch_directory(&request.state_directory, "stateDirectory")?,
        workspace_state_directory: match &request.workspace_state_directory {
            Some(directory) => launch_directory(directory, "workspaceStateDirectory")?,
            None if process.uses_workspace_state_directory() => {
                return Err(PluginRejection::new(
                    "plugin.launch.invalidRequest",
                    "The launch request has no usable workspaceStateDirectory.",
                ))
            }
            None => "",
        },
        log_directory: match request.log_directory.as_deref() {
            Some(directory) => {
                let directory = launch_directory(directory, "logDirectory")?;
                // Recognize absolute native paths on both platforms without using
                // the build machine's Path::is_absolute semantics for the other OS.
                let absolute = directory.starts_with('/')
                    || directory.starts_with("\\\\")
                    || (directory.as_bytes().get(1) == Some(&b':')
                        && directory
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphabetic)
                        && directory
                            .as_bytes()
                            .get(2)
                            .is_some_and(|b| b"/\\".contains(b)));
                if !absolute {
                    return Err(PluginRejection::new(
                        "plugin.launch.invalidRequest",
                        "The launch request has no usable absolute logDirectory.",
                    ));
                }
                directory
            }
            None => "",
        },
        architecture: &request.architecture,
        settings: &effective_settings,
    };
    if !KNOWN_ARCHITECTURES.contains(&values.architecture) {
        return Err(PluginRejection::new(
            "plugin.launch.invalidRequest",
            format!("The architecture {} is not supported.", values.architecture),
        ));
    }

    let mut requirements = LaunchRequirements::default();
    let expand_all = |templates: &[String], requirements: &mut LaunchRequirements| {
        templates
            .iter()
            .map(|value| expand_launch_template(value, &values, requirements))
            .collect::<Vec<String>>()
    };

    let launch = match &process.launch {
        // A tool definition owns how its executable starts an entry, so the
        // plan hands the host the parts for `toolchains.instantiateLaunch`.
        LanguageServerLaunch::Runtime {
            tool,
            minimum_version,
            packaged_runtime,
            runtime_arguments,
            entry,
        } => {
            requirements.package_paths.insert(entry.clone());
            let mut launch = json!({
                "kind": "tool",
                "tool": tool,
                "entry": format!("{}/{entry}", values.plugin_directory),
                "runtimeArguments": expand_all(runtime_arguments, &mut requirements),
            });
            if let Some(version) = minimum_version {
                launch["minimumVersion"] = json!(version);
            }
            if let Some(path) = packaged_runtime {
                let path = path.replace("${arch}", values.architecture);
                requirements.package_paths.insert(path.clone());
                launch["packagedRuntime"] = json!(path);
            }
            launch
        }
        LanguageServerLaunch::Packaged { path } => {
            requirements.package_paths.insert(path.clone());
            json!({ "kind": "packaged", "path": path })
        }
        LanguageServerLaunch::SystemCommand { commands } => {
            json!({ "kind": "systemCommand", "commands": commands })
        }
        // Validation admits a backend launch only for a language server,
        // whose plan `backend_resolved_launch_plan` builds instead.
        LanguageServerLaunch::Backend { .. } => {
            return Err(PluginRejection::new(
                "plugin.launch.unavailable",
                "This process is launched by the plugin backend.",
            ));
        }
    };
    let arguments = expand_all(&process.arguments, &mut requirements);
    let validation_arguments = expand_all(&process.validation_arguments, &mut requirements);

    let mut plan = json!({
        "owner": {"pluginID":manifest.id,"version":manifest.version},
        "launch": launch,
        "arguments": arguments,
        "validationArguments": validation_arguments,
        "runtimeRequirements": process.runtime_requirements(),
    });
    if let Some(options) = &process.initialization_options {
        plan["initializationOptions"] = expand_launch_value(options, &values, &mut requirements);
    }
    let mut directories = Vec::new();
    if requirements.uses_state_directory {
        directories.push("state");
    }
    if requirements.uses_workspace_state_directory {
        directories.push("workspaceState");
    }
    if !effective_settings.is_empty() {
        plan["settings"] = json!(effective_settings);
    }
    if requirements.uses_log_directory {
        if values.log_directory.is_empty() {
            return Err(PluginRejection::new(
                "plugin.launch.invalidRequest",
                "The launch request has no usable logDirectory.",
            ));
        }
        directories.push("log");
    }
    plan["requiredDirectories"] = json!(directories);
    plan["requiredPackagePaths"] = json!(requirements.package_paths);
    Ok(plan)
}

/// Accepts a host directory as substitution text. A trailing separator is
/// dropped so `${pluginDir}/x` never yields a doubled separator.
fn launch_directory<'a>(value: &'a str, name: &str) -> Validation<&'a str> {
    let trimmed = value.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(PluginRejection::new(
            "plugin.launch.invalidRequest",
            format!("The launch request has no usable {name}."),
        ));
    }
    Ok(trimmed)
}

/// The setting ID when the whole template is exactly one `${setting:<id>}`.
/// Such a value keeps the setting's JSON type, so an LSP option can receive a
/// number or a boolean rather than its text.
fn sole_setting_reference(template: &str) -> Option<String> {
    match parse_template_with(template, LaunchVariable::parse)?.as_slice() {
        [TemplatePart::Variable(LaunchVariable::Setting(id))] => Some(id.clone()),
        _ => None,
    }
}

fn expand_launch_value(
    value: &Value,
    values: &LaunchValues<'_>,
    requirements: &mut LaunchRequirements,
) -> Value {
    match value {
        Value::String(text) => sole_setting_reference(text)
            .and_then(|id| values.settings.get(&id).cloned())
            .unwrap_or_else(|| Value::String(expand_launch_template(text, values, requirements))),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| expand_launch_value(item, values, requirements))
                .collect(),
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, item)| (key.clone(), expand_launch_value(item, values, requirements)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn expand_launch_template(
    value: &str,
    values: &LaunchValues<'_>,
    requirements: &mut LaunchRequirements,
) -> String {
    // Validation guarantees the template parses and that the text after
    // `${pluginDir}` is a `/`-rooted path inside the package.
    let parts = parse_template_with(value, LaunchVariable::parse).unwrap_or_default();
    let mut expanded = String::new();
    let mut package_path_start = None;
    for part in parts {
        match part {
            TemplatePart::Text(text) => expanded.push_str(&text),
            TemplatePart::Variable(LaunchVariable::PluginDir) => {
                expanded.push_str(values.plugin_directory);
                package_path_start = Some(expanded.len() + 1);
            }
            TemplatePart::Variable(LaunchVariable::StateDir) => {
                requirements.uses_state_directory = true;
                expanded.push_str(values.state_directory);
            }
            TemplatePart::Variable(LaunchVariable::WorkspaceStateDir) => {
                requirements.uses_workspace_state_directory = true;
                expanded.push_str(values.workspace_state_directory);
            }
            TemplatePart::Variable(LaunchVariable::LogDir) => {
                requirements.uses_log_directory = true;
                expanded.push_str(values.log_directory);
            }
            TemplatePart::Variable(LaunchVariable::Arch) => expanded.push_str(values.architecture),
            TemplatePart::Variable(LaunchVariable::Setting(id)) => {
                expanded.push_str(&lithe_plugin_model::settings::setting_text(
                    values.settings.get(&id),
                ));
            }
        }
    }
    if let Some(start) = package_path_start {
        requirements
            .package_paths
            .insert(expanded[start..].trim_end_matches('/').to_string());
    }
    expanded
}
