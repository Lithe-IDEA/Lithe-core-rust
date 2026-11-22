//! Language-server launch plans: request layers, launch values and requirements, and
//! the expansion of each server process.

use super::*;
use lithe_plugin_model::process::ProcessPlacement;
use std::borrow::Cow;

/// Host facts substituted into a language server's launch templates. The
/// host owns every directory; Core only joins text and never touches disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageServerLaunchPlanRequest {
    /// Absolute root of the installed plugin package.
    plugin_directory: String,
    /// Absolute writable directory the host keeps for this plugin's server.
    state_directory: String,
    /// Verified read-only deployment on the workspace host. Local package paths
    /// are never used to expand a workspace process's templates.
    #[serde(default)]
    remote_plugin_directory: Option<String>,
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
    /// Use the workspace host's architecture and accept only launches whose
    /// dependencies are available there.
    #[serde(default)]
    remote_workspace: bool,
    /// Return placement for a UI process and collect required package paths for
    /// a workspace process before the host prepares its resources.
    #[serde(default)]
    probe_local_placement: bool,
    /// Values the user stored for the plugin's settings, by setting ID. A
    /// missing or no longer acceptable value falls back to the default.
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    /// The current workspace's overrides of project-scoped settings.
    #[serde(default)]
    project_settings: BTreeMap<String, Value>,
    /// Whether the user trusts the workspace, from `toolchains.catalogState`.
    /// Absent means untrusted: a process that would run project code then
    /// has no plan and is refused with [`UNTRUSTED_WORKSPACE`].
    #[serde(default)]
    project_code_allowed: bool,
}

/// Refusal of a launch whose process would run code or configuration the
/// workspace controls while the user has not trusted the workspace. The
/// host leaves the process stopped and asks again once trust is granted.
pub(crate) const UNTRUSTED_WORKSPACE: &str = "plugin.launch.untrustedWorkspace";

impl LanguageServerLaunchPlanRequest {
    /// Converts workspace facts into execution facts for an explicit UI process.
    fn for_process(&self, process: &ServerProcess) -> Cow<'_, Self> {
        if self.remote_workspace && process.placement == Some(ProcessPlacement::Ui) {
            let mut local = self.clone();
            local.remote_workspace = false;
            local.architecture = match std::env::consts::ARCH {
                "aarch64" => "arm64",
                architecture => architecture,
            }
            .to_string();
            return Cow::Owned(local);
        }
        Cow::Borrowed(self)
    }

    fn os(&self) -> &'static str {
        if self.remote_workspace {
            "linux"
        } else {
            std::env::consts::OS
        }
    }

    fn placement_probe(&self, process: &ServerProcess) -> Option<Value> {
        (self.probe_local_placement
            && self.remote_workspace
            && process.placement == Some(ProcessPlacement::Ui))
        .then(|| json!({ "placement": ProcessPlacement::Ui }))
    }

    /// Refuses the launch of a process that runs project code, `subject`
    /// naming it, unless the workspace is trusted.
    pub(crate) fn require_trust(&self, runs_project_code: bool, subject: &str) -> Validation<()> {
        if runs_project_code && !self.project_code_allowed {
            return Err(PluginRejection::new(
                UNTRUSTED_WORKSPACE,
                format!("{subject} would run project code; the workspace is not trusted."),
            ));
        }
        Ok(())
    }

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
        if self.remote_workspace {
            // Private remote plugin state is provisioned separately. Local
            // directories cannot be included in remote launch parameters.
            return Ok(lithe_plugin_language_servers::LaunchDirectories {
                state: "",
                workspace_state: "",
                log: "",
                temporary: "",
            });
        }
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

impl DeclarativePluginManifest {
    /// Package files and trees reserved for the UI host. Backend templates are
    /// expanded for the variant so architecture-specific files stay local too.
    pub fn local_only_package_paths(&self, os: &str, arch: &str) -> BTreeSet<String> {
        let mut paths: BTreeSet<_> = self.local_only_paths.iter().cloned().collect();
        paths.extend(["plugin.json", "lithe-plugin-signature.json", "l10n"].map(str::to_owned));
        if let Some(ui) = &self.ui {
            for window in &ui.tool_windows {
                paths.extend(window.web.iter().map(|web| web.entry.clone()));
            }
            paths.extend(
                ui.tool_windows
                    .iter()
                    .filter_map(|window| window.icon.as_ref())
                    .chain(
                        ui.commands
                            .iter()
                            .filter_map(|command| command.icon.as_ref()),
                    )
                    .flat_map(UIIcon::package_paths)
                    .map(str::to_owned),
            );
        }
        paths.extend(
            self.build_systems
                .iter()
                .flat_map(lithe_build::BuildSystemDeclaration::icon_paths)
                .map(str::to_owned),
        );
        if let Some(backend) = &self.backend {
            let process = &backend.process;
            paths.extend(
                process
                    .packaged_files()
                    .into_iter()
                    .map(|(_, path)| path.to_owned()),
            );
            let mut requirements = LaunchRequirements::default();
            let settings = BTreeMap::new();
            let values = LaunchValues {
                plugin_directory: "",
                state_directory: "",
                workspace_state_directory: "",
                log_directory: "",
                architecture: arch,
                os,
                settings: &settings,
            };
            let mut templates = process
                .arguments
                .iter()
                .chain(&process.validation_arguments)
                .collect::<Vec<_>>();
            if let LanguageServerLaunch::Runtime {
                packaged_runtime,
                runtime_arguments,
                ..
            } = &process.launch
            {
                paths.extend(
                    packaged_runtime
                        .iter()
                        .map(|path| path.replace("${arch}", arch)),
                );
                templates.extend(runtime_arguments);
            }
            for template in templates {
                expand_launch_template(template, &values, &mut requirements);
            }
            if let Some(options) = &process.initialization_options {
                expand_launch_value(options, &values, &mut requirements);
            }
            paths.extend(requirements.package_paths);
        }
        paths
    }
}

/// Validated substitution values of one launch plan.
struct LaunchValues<'a> {
    plugin_directory: &'a str,
    state_directory: &'a str,
    workspace_state_directory: &'a str,
    log_directory: &'a str,
    architecture: &'a str,
    os: &'a str,
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
    require_supported_workspace(request, server)?;
    request.require_trust(
        server.executes_project_code(),
        &format!("The language server for {language_id}"),
    )?;
    if let Some(plan) = request.placement_probe(&server.process) {
        return Ok(plan);
    }
    let mut plan = match &server.process.launch {
        LanguageServerLaunch::Backend {
            function,
            timeout_milliseconds,
        } => {
            let execution = request.for_process(&server.process);
            let mut plan = backend_resolved_launch_plan(
                manifest,
                &server.language_id,
                function,
                *timeout_milliseconds,
                &execution,
            )?;
            describe_placement(&mut plan, &server.process);
            plan
        }
        _ => server_launch_plan(
            manifest,
            &server.process,
            request,
            ServerRole::LanguageServer,
        )?,
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

fn require_supported_workspace(
    request: &LanguageServerLaunchPlanRequest,
    server: &LanguageServerDeclaration,
) -> Validation<()> {
    if request.remote_workspace && !server.session.supports_remote() {
        return Err(PluginRejection::new(
            "plugin.launch.remoteUnsupported",
            "This language server is available only in local workspaces.",
        ));
    }
    Ok(())
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
            request.os(),
            &request.architecture,
            &directories,
            &settings,
        ),
        "arguments": [],
        "validationArguments": [],
        "runtimeRequirements": [],
        "requiredDirectories": if request.remote_workspace {
            Vec::<&str>::new()
        } else {
            vec!["state", "workspaceState", "log", "temporary"]
        },
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
            require_supported_workspace(&payload.request, server)?;
            // Checked again here: the answer is a second chance to start the
            // server, and trust may have been revoked since the first step.
            payload.request.require_trust(
                server.executes_project_code(),
                &format!("The language server for {}", payload.language_id),
            )?;
            let execution = payload.request.for_process(&server.process);
            let directories = execution.backend_launch_directories()?;
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
            if execution.remote_workspace && plan["launch"]["kind"] != "tool" {
                return Err(remote_resources_unavailable());
            }
            describe_placement(&mut plan, &server.process);
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
    role: ServerRole,
) -> Validation<Value> {
    if let Some(plan) = request.placement_probe(process) {
        return Ok(plan);
    }
    let request = request.for_process(process);
    let effective_settings = manifest.effective_settings(request.layers());
    let values = LaunchValues {
        plugin_directory: if request.remote_workspace {
            match request.remote_plugin_directory.as_deref() {
                Some(directory) if directory.starts_with('/') => {
                    launch_directory(directory, "remotePluginDirectory")?
                }
                Some(_) => return Err(remote_resources_unavailable()),
                None => "",
            }
        } else {
            launch_directory(&request.plugin_directory, "pluginDirectory")?
        },
        state_directory: if request.remote_workspace {
            ""
        } else {
            launch_directory(&request.state_directory, "stateDirectory")?
        },
        workspace_state_directory: match &request.workspace_state_directory {
            _ if request.remote_workspace => "",
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
            _ if request.remote_workspace => "",
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
        os: request.os(),
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

    let runtime_requirements = process.runtime_requirements();
    // Workspace processes can resolve a tool on their own attachment and
    // launch a verified package entry. The plugin backend belongs to the UI host.
    let remote_runtime = role != ServerRole::Backend
        && matches!(process.launch, LanguageServerLaunch::Runtime { .. })
        && !process.has_runtime_dependencies();
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
                "runtimeArguments": expand_all(runtime_arguments, &mut requirements),
            });
            if !values.plugin_directory.is_empty() {
                launch["entry"] = json!(format!("{}/{entry}", values.plugin_directory));
            }
            if let Some(version) = minimum_version {
                launch["minimumVersion"] = json!(version);
            }
            if let Some(path) = packaged_runtime {
                let path = path.replace("${arch}", values.architecture);
                requirements.package_paths.insert(path.clone());
                if request.remote_workspace && !values.plugin_directory.is_empty() {
                    launch["remoteExecutable"] =
                        json!(format!("{}/{path}", values.plugin_directory));
                }
                launch["packagedRuntime"] = json!(path);
            }
            launch
        }
        LanguageServerLaunch::Packaged { path } => {
            requirements.package_paths.insert(path.clone());
            let mut launch = json!({ "kind": "packaged", "path": path });
            if request.remote_workspace && !values.plugin_directory.is_empty() {
                launch["remoteExecutable"] = json!(format!("{}/{path}", values.plugin_directory));
            }
            launch
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
        "runtimeRequirements": runtime_requirements,
    });
    describe_placement(&mut plan, process);
    if let Some(options) = &process.initialization_options {
        plan["initializationOptions"] = expand_launch_value(options, &values, &mut requirements);
    }
    if request.remote_workspace
        && ((!requirements.package_paths.is_empty()
            && values.plugin_directory.is_empty()
            && !request.probe_local_placement)
            || (!remote_runtime
                && (matches!(process.launch, LanguageServerLaunch::Runtime { .. })
                    || !runtime_requirements.is_empty()))
            || requirements.uses_state_directory
            || requirements.uses_workspace_state_directory
            || requirements.uses_log_directory)
    {
        return Err(remote_resources_unavailable());
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

fn describe_placement(plan: &mut Value, process: &ServerProcess) {
    if let Some(placement) = process.placement {
        plan["placement"] = json!(placement);
    }
}

fn remote_resources_unavailable() -> PluginRejection {
    PluginRejection::new(
        "plugin.launch.platformUnavailable",
        "The plugin process requires resources that are unavailable on the workspace host.",
    )
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
            TemplatePart::Variable(LaunchVariable::Os) => expanded.push_str(values.os),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_providers_and_adapters_plan_a_verified_runtime_entry() {
        let build: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/fixtures/build-providers/build-provider.plugin.json"
        )))
        .unwrap();
        let vcs: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/fixtures/vcs/vcs-provider.plugin.json"
        )))
        .unwrap();
        let debug: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/fixtures/debug-adapters/debug-adapter.plugin.json"
        )))
        .unwrap();

        let mut build = build;
        build["buildSystems"][0]["provider"]["arguments"] = json!(["--stdio"]);
        let mut debug = debug;
        debug["requiresTools"] = json!(["interpreter"]);
        debug["debugAdapters"][0]["launch"]["process"]["launch"] = json!({
            "kind": "runtime", "tool": "interpreter", "minimumVersion": "3.9", "entry": "bin/adapter.script"
        });

        for (manifest, selector, id, entry, tool, version) in [
            (
                build,
                "buildSystemID",
                "gradle",
                "provider/gradle-tooling-provider.jar",
                "java",
                "17",
            ),
            (
                vcs,
                "vcsProviderID",
                "svn",
                "provider/main.mjs",
                "node",
                "20.3",
            ),
            (
                debug,
                "debugAdapterID",
                "example",
                "bin/adapter.script",
                "interpreter",
                "3.9",
            ),
        ] {
            let mut request = json!({
                "manifest": manifest,
                "request": {
                    "pluginDirectory": "/client/plugin",
                    "stateDirectory": "/client/state",
                    "architecture": "x86_64",
                    "projectCodeAllowed": true,
                    "remoteWorkspace": true,
                    "probeLocalPlacement": true,
                }
            });
            request[selector] = json!(id);
            let probe = language_server_launch_plan_command(request.clone()).unwrap();
            assert_eq!(probe["status"], "ok", "{id}: {probe}");
            assert_eq!(probe["plan"]["launch"]["kind"], "tool");
            assert!(probe["plan"]["launch"]["entry"].is_null());
            assert!(probe["plan"]["requiredPackagePaths"]
                .as_array()
                .unwrap()
                .contains(&json!(entry)));
            request["request"]["probeLocalPlacement"] = json!(false);
            let missing = language_server_launch_plan_command(request.clone()).unwrap();
            assert_eq!(
                missing["error"]["code"],
                "plugin.launch.platformUnavailable"
            );
            request["request"]["remotePluginDirectory"] = json!("/host/release");
            let deployed = language_server_launch_plan_command(request).unwrap();
            assert_eq!(deployed["status"], "ok", "{id}: {deployed}");
            assert_eq!(
                deployed["plan"]["launch"]["entry"],
                format!("/host/release/{entry}")
            );
            assert_eq!(deployed["plan"]["launch"]["tool"], tool);
            assert_eq!(deployed["plan"]["launch"]["minimumVersion"], version);
        }
    }
}
