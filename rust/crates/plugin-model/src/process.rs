//! Processes a plugin declares for the host to start: language servers,
//! build and VCS providers, plugin backends and debug adapters share one
//! launch declaration, one validation, and one install-review entry.
//!
//! Launch templates name host-owned locations (`${pluginDir}`,
//! `${stateDir}`, `${workspaceStateDir}`, `${logDir}`, `${arch}`) and plugin
//! settings (`${setting:<id>}`), so a package never embeds a machine path.

use crate::names::{is_bare_command, is_function_id, is_safe_relative_path};
use crate::rejection::{invalid, invalid_template, PluginRejection, Validation};
use crate::settings::{is_setting_identifier, DeclaredSettings};
use crate::template::{parse_template_with, TemplatePart};
use crate::version::validate_minimum_version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Prefix of a launch template variable that names a plugin setting.
pub(crate) const SETTING_VARIABLE_PREFIX: &str = "setting:";

/// How the host starts a process that speaks the LSP base protocol. A
/// language server and a build-system provider are the same kind of process,
/// so both declare it with these fields and share their validation, install
/// review entries, and launch plan. Flattened into its owner, which rejects
/// unknown fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcess {
    /// Where the executable comes from.
    pub launch: LanguageServerLaunch,
    /// Arguments after the executable; launch templates are expanded.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Arguments of a probe that must exit successfully before the host
    /// trusts a candidate executable; empty means no probe.
    #[serde(default)]
    pub validation_arguments: Vec<String>,
    #[serde(default)]
    runtime_dependencies: Vec<RuntimeDependency>,
    /// LSP `initializationOptions` sent verbatim after template expansion of
    /// its string values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<Value>,
}

/// What a server process is for, which names it in rejection messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRole {
    /// A language server.
    LanguageServer,
    /// A build-system provider.
    BuildProvider,
    /// A VCS provider.
    VcsProvider,
    /// The plugin backend.
    Backend,
    /// A debug adapter.
    DebugAdapter,
}

/// Tools a server process may name for its launch and dependencies: the
/// tool-definition IDs the manifest lists in `requiresTools`.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeTools<'a>(&'a BTreeSet<String>);

impl<'a> RuntimeTools<'a> {
    /// The tools a manifest lists in `requiresTools`.
    pub fn new(tools: &'a BTreeSet<String>) -> Self {
        Self(tools)
    }

    /// Checks that one tool a process names is listed in `requiresTools`.
    pub fn check(self, tool: &str, field: String) -> Validation<()> {
        if self.0.contains(tool) {
            Ok(())
        } else {
            Err(PluginRejection::at(
                "plugin.runtime.undeclared",
                field,
                format!("The tool {tool} is not listed in requiresTools."),
            ))
        }
    }
}

impl ServerRole {
    fn noun(self) -> &'static str {
        match self {
            Self::LanguageServer => "language server",
            Self::BuildProvider => "build provider",
            Self::VcsProvider => "VCS provider",
            Self::Backend => "plugin backend",
            Self::DebugAdapter => "debug adapter",
        }
    }
}

/// Where the language-server executable comes from.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum LanguageServerLaunch {
    /// An executable inside the plugin package, relative to the package root.
    Packaged {
        /// Executable path, relative to the package root.
        path: String,
    },
    /// Bare command names the host resolves through its tool discovery.
    SystemCommand {
        /// Command names in preference order.
        commands: Vec<String>,
    },
    /// A packaged entry file started by a host-discovered runtime, so the
    /// package needs no launcher script.
    Runtime {
        /// Tool-definition ID, listed in `requiresTools`, whose launch
        /// template starts the entry.
        tool: String,
        /// Lowest acceptable runtime version, for example `21` or `18.17`.
        #[serde(
            default,
            rename = "minimumVersion",
            skip_serializing_if = "Option::is_none"
        )]
        minimum_version: Option<String>,
        /// Runtime executable shipped inside the package, relative to its
        /// root; it may vary by `${arch}`. When set, the host starts exactly
        /// this file instead of discovering the tool on the machine.
        #[serde(
            default,
            rename = "packagedRuntime",
            skip_serializing_if = "Option::is_none"
        )]
        packaged_runtime: Option<String>,
        /// Runtime options placed before the entry.
        #[serde(default, rename = "runtimeArguments")]
        runtime_arguments: Vec<String>,
        /// Entry file inside the package, such as a script for `node`.
        entry: String,
    },
    /// The plugin backend resolves the command line each time the server
    /// starts: the host calls `function` and passes its answer to
    /// `plugins.languageServers.decodeLaunch`. Language servers only.
    Backend {
        /// ID of a function the plugin backend registers.
        function: String,
        /// How long the host waits for the function, within
        /// `RESOLVE_TIMEOUT_MILLISECONDS`.
        #[serde(
            default,
            rename = "timeoutMilliseconds",
            skip_serializing_if = "Option::is_none"
        )]
        timeout_milliseconds: Option<u64>,
    },
}

/// A tool the server process needs at run time, named by its tool-definition ID.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDependency {
    tool: String,
}

/// Variables of language-server launch templates. They name host-owned
/// locations, so a package never embeds a machine path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchVariable {
    /// Root of the installed, read-only plugin package.
    PluginDir,
    /// Writable directory the host keeps for this plugin's server and removes
    /// when the plugin is uninstalled.
    StateDir,
    /// Writable directory for this server's state of the current workspace.
    WorkspaceStateDir,
    /// Absolute session plugin files directory supplied by the writable storage adapter.
    LogDir,
    /// Host CPU architecture, one of `KNOWN_ARCHITECTURES`.
    Arch,
    /// Current value of the plugin setting with this ID.
    Setting(String),
}

impl LaunchVariable {
    /// The launch variable a `${name}` template names; `None` for an unknown name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pluginDir" => Some(Self::PluginDir),
            "stateDir" => Some(Self::StateDir),
            "workspaceStateDir" => Some(Self::WorkspaceStateDir),
            "logDir" => Some(Self::LogDir),
            "arch" => Some(Self::Arch),
            _ => name
                .strip_prefix(SETTING_VARIABLE_PREFIX)
                .filter(|id| is_setting_identifier(id))
                .map(|id| Self::Setting(id.to_string())),
        }
    }
}

/// Which launch value a template belongs to. The validation probe runs before
/// any workspace is open, so it cannot name state directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchTemplateContext {
    Start,
    ValidationProbe,
}

impl ServerProcess {
    /// Package files the launch starts, as (launch field, package-relative
    /// path): a `packaged` executable, or a `runtime` entry and its shipped
    /// runtime. A `${arch}` runtime path varies per machine, so only the
    /// installer can resolve it and it is left out.
    pub fn packaged_files(&self) -> Vec<(&'static str, &str)> {
        match &self.launch {
            LanguageServerLaunch::Packaged { path } => vec![("path", path)],
            LanguageServerLaunch::Runtime {
                entry,
                packaged_runtime,
                ..
            } => {
                let mut files = vec![("entry", entry.as_str())];
                if let Some(runtime) = packaged_runtime
                    .as_deref()
                    .filter(|path| !path.contains("${"))
                {
                    files.push(("packagedRuntime", runtime));
                }
                files
            }
            LanguageServerLaunch::SystemCommand { .. } | LanguageServerLaunch::Backend { .. } => {
                Vec::new()
            }
        }
    }

    /// Whether the plugin backend resolves the command line at start.
    pub fn is_backend_launch(&self) -> bool {
        matches!(self.launch, LanguageServerLaunch::Backend { .. })
    }

    /// Checks the launch, its arguments and probe, the initialization
    /// options, and the runtime dependencies. Arguments and options are
    /// launch templates.
    pub fn validate(
        &self,
        field: &str,
        declared_settings: &DeclaredSettings,
        role: ServerRole,
        runtime_tools: RuntimeTools<'_>,
    ) -> Validation<()> {
        let noun = role.noun();
        match &self.launch {
            LanguageServerLaunch::Packaged { path } => {
                if !is_safe_relative_path(path) {
                    return Err(invalid(
                        format!("{field}.launch.path"),
                        format!("The packaged {noun} must stay inside the plugin package."),
                    ));
                }
            }
            LanguageServerLaunch::SystemCommand { commands } => {
                if commands.is_empty() || commands.iter().any(|command| !is_bare_command(command)) {
                    return Err(invalid(
                        format!("{field}.launch.commands"),
                        "System commands must be bare executable names.",
                    ));
                }
            }
            LanguageServerLaunch::Runtime {
                tool,
                minimum_version,
                packaged_runtime,
                runtime_arguments,
                entry,
            } => {
                runtime_tools.check(tool, format!("{field}.launch.tool"))?;
                if let Some(version) = minimum_version {
                    validate_minimum_version(version, &format!("{field}.launch.minimumVersion"))?;
                }
                if let Some(path) = packaged_runtime {
                    let runtime_field = format!("{field}.launch.packagedRuntime");
                    if !is_packaged_runtime_path(path) {
                        return Err(invalid(
                            runtime_field,
                            "The packaged runtime must be a path inside the plugin package that varies only by ${arch}.",
                        ));
                    }
                    // The plugin ships the runtime it needs, so there is no
                    // discovered candidate whose version could be checked.
                    if minimum_version.is_some() {
                        return Err(invalid(
                            runtime_field,
                            "A packaged runtime cannot also state a minimum version.",
                        ));
                    }
                }
                if !is_safe_relative_path(entry) {
                    return Err(invalid(
                        format!("{field}.launch.entry"),
                        format!("The {noun} entry must stay inside the plugin package."),
                    ));
                }
                validate_launch_templates(
                    runtime_arguments,
                    LaunchTemplateContext::Start,
                    declared_settings,
                    &format!("{field}.launch.runtimeArguments"),
                )?;
                // The host's probe of a runtime is its version check; a second
                // probe would have no defined command line.
                if !self.validation_arguments.is_empty() {
                    return Err(invalid(
                        format!("{field}.validationArguments"),
                        "A runtime launch is verified by its minimum version, not by a probe.",
                    ));
                }
            }
            LanguageServerLaunch::Backend {
                function,
                timeout_milliseconds,
            } => {
                return self.validate_backend_launch(field, function, *timeout_milliseconds, role);
            }
        }
        if self
            .arguments
            .iter()
            .chain(&self.validation_arguments)
            .any(|argument| argument.contains('\0'))
        {
            return Err(invalid(
                format!("{field}.arguments"),
                format!("{} arguments must not contain NUL.", capitalized(noun)),
            ));
        }
        validate_launch_templates(
            &self.arguments,
            LaunchTemplateContext::Start,
            declared_settings,
            &format!("{field}.arguments"),
        )?;
        validate_launch_templates(
            &self.validation_arguments,
            LaunchTemplateContext::ValidationProbe,
            declared_settings,
            &format!("{field}.validationArguments"),
        )?;
        if let Some(options) = &self.initialization_options {
            let options_field = format!("{field}.initializationOptions");
            if !options.is_object() {
                return Err(invalid(
                    options_field,
                    "initializationOptions must be a JSON object.",
                ));
            }
            validate_launch_value(options, declared_settings, &options_field)?;
        }
        for dependency in &self.runtime_dependencies {
            let dependencies_field = format!("{field}.runtimeDependencies");
            runtime_tools.check(&dependency.tool, dependencies_field.clone())?;
            // The launch runtime is already discovered for the process, and
            // only the launch declaration can state its version requirement.
            if matches!(&self.launch, LanguageServerLaunch::Runtime { tool, .. } if *tool == dependency.tool)
            {
                return Err(invalid(
                    dependencies_field,
                    "The launch runtime must not be repeated as a runtime dependency.",
                ));
            }
        }
        Ok(())
    }

    /// Checks a launch the plugin backend resolves. Only a language server
    /// may use it, and its command line, probe, options, and runtimes all
    /// come from the backend's answer, so the manifest declares none of them.
    fn validate_backend_launch(
        &self,
        field: &str,
        function: &str,
        timeout_milliseconds: Option<u64>,
        role: ServerRole,
    ) -> Validation<()> {
        if role != ServerRole::LanguageServer {
            return Err(invalid(
                format!("{field}.launch.kind"),
                format!(
                    "A {} cannot be started through the plugin backend.",
                    role.noun()
                ),
            ));
        }
        if !is_function_id(function) {
            return Err(invalid(
                format!("{field}.launch.function"),
                "The launch function must be a plugin function ID.",
            ));
        }
        validate_resolve_timeout(
            timeout_milliseconds,
            &format!("{field}.launch.timeoutMilliseconds"),
        )?;
        let declared = [
            ("arguments", !self.arguments.is_empty()),
            ("validationArguments", !self.validation_arguments.is_empty()),
            ("runtimeDependencies", !self.runtime_dependencies.is_empty()),
            (
                "initializationOptions",
                self.initialization_options.is_some(),
            ),
        ];
        if let Some((name, _)) = declared.iter().find(|(_, present)| *present) {
            return Err(invalid(
                format!("{field}.{name}"),
                "A backend launch takes its command line and options from the backend's answer.",
            ));
        }
        Ok(())
    }

    /// Install-review entry of the process, without its owner's ID: the
    /// executable, its arguments and probe, and the initialization options.
    pub fn program(&self, role: ServerRole) -> Value {
        let suffix = match role {
            ServerRole::LanguageServer => "LanguageServer",
            ServerRole::BuildProvider => "BuildProvider",
            ServerRole::VcsProvider => "VcsProvider",
            ServerRole::Backend => "Backend",
            ServerRole::DebugAdapter => "DebugAdapter",
        };
        let mut program = match &self.launch {
            LanguageServerLaunch::Packaged { path } => json!({
                "kind": format!("packaged{suffix}"),
                "path": path,
                "arguments": self.arguments,
                "validationArguments": self.validation_arguments,
            }),
            LanguageServerLaunch::SystemCommand { commands } => json!({
                "kind": format!("system{suffix}"),
                "commands": commands,
                "arguments": self.arguments,
                "validationArguments": self.validation_arguments,
            }),
            LanguageServerLaunch::Runtime {
                tool,
                packaged_runtime,
                runtime_arguments,
                entry,
                ..
            } => json!({
                "kind": format!("runtime{suffix}"),
                "tool": tool,
                "packagedRuntime": packaged_runtime,
                "entry": entry,
                "runtimeArguments": runtime_arguments,
                "arguments": self.arguments,
                "validationArguments": self.validation_arguments,
            }),
            // The backend chooses the command line when the server starts,
            // so the review shows the function that will choose it.
            LanguageServerLaunch::Backend {
                function,
                timeout_milliseconds,
            } => json!({
                "kind": format!("backend{suffix}"),
                "function": function,
                "timeoutMilliseconds": timeout_milliseconds
                    .unwrap_or(RESOLVE_TIMEOUT_MILLISECONDS),
            }),
        };
        // Initialization options can name code the server loads (bundles or
        // extensions, for example), so changing them is a new program too.
        if let Some(options) = &self.initialization_options {
            program["initializationOptions"] = options.clone();
        }
        program
    }

    /// Whether any start template names `${workspaceStateDir}`, which only
    /// a process bound to one workspace can be given.
    pub fn uses_workspace_state_directory(&self) -> bool {
        let runtime_arguments = match &self.launch {
            LanguageServerLaunch::Runtime {
                runtime_arguments, ..
            } => runtime_arguments.as_slice(),
            _ => &[],
        };
        let mentions = |text: &str| {
            parse_template_with(text, LaunchVariable::parse)
                .unwrap_or_default()
                .iter()
                .any(|part| {
                    matches!(
                        part,
                        TemplatePart::Variable(LaunchVariable::WorkspaceStateDir)
                    )
                })
        };
        fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
            match value {
                Value::String(text) => out.push(text),
                Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
                Value::Object(members) => members.values().for_each(|item| strings(item, out)),
                _ => {}
            }
        }
        let mut option_strings = Vec::new();
        if let Some(options) = &self.initialization_options {
            strings(options, &mut option_strings);
        }
        self.arguments
            .iter()
            .chain(runtime_arguments)
            .map(String::as_str)
            .chain(option_strings)
            .any(mentions)
    }

    /// Install-review entries of the runtimes the host discovers for it.
    pub fn runtime_programs(&self) -> Vec<Value> {
        self.runtime_requirements()
            .into_iter()
            .map(|mut runtime| {
                runtime["kind"] = json!("runtime");
                runtime
            })
            .collect()
    }

    /// Runtimes the host must discover for this process, sorted by tool. The
    /// launch runtime comes first in declaration terms but is merged here so
    /// hosts resolve every tool the same way. Every entry names a tool
    /// definition, so every entry carries `kind: "tool"`.
    pub fn runtime_requirements(&self) -> Vec<Value> {
        let mut requirements: Vec<(&str, Option<&str>)> = self
            .runtime_dependencies
            .iter()
            .map(|dependency| (dependency.tool.as_str(), None))
            .collect();
        // A packaged runtime is part of the plugin, not a host requirement.
        if let LanguageServerLaunch::Runtime {
            tool,
            minimum_version,
            packaged_runtime: None,
            ..
        } = &self.launch
        {
            requirements.push((tool.as_str(), minimum_version.as_deref()));
        }
        requirements.sort();
        requirements.dedup();
        requirements
            .into_iter()
            .map(|(tool, minimum_version)| {
                let mut requirement = json!({ "tool": tool, "kind": "tool" });
                if let Some(version) = minimum_version {
                    requirement["minimumVersion"] = json!(version);
                }
                requirement
            })
            .collect()
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

/// A packaged runtime path is package-relative text whose only variable is
/// `${arch}`, so one package can carry a runtime per architecture.
fn is_packaged_runtime_path(value: &str) -> bool {
    let Some(parts) = parse_template_with(value, LaunchVariable::parse) else {
        return false;
    };
    parts.iter().all(|part| {
        matches!(
            part,
            TemplatePart::Text(_) | TemplatePart::Variable(LaunchVariable::Arch)
        )
    }) && is_safe_relative_path(&value.replace("${arch}", "arch"))
}

/// Checks the start and probe arguments of a process that has no launch of
/// its own to validate, such as a debug adapter on the selected runtime.
pub fn validate_process_arguments(
    arguments: &[String],
    validation_arguments: &[String],
    declared_settings: &DeclaredSettings,
    field: &str,
) -> Validation<()> {
    if arguments
        .iter()
        .chain(validation_arguments)
        .any(|argument| argument.contains('\0'))
    {
        return Err(invalid(
            format!("{field}.arguments"),
            "Process arguments must not contain NUL.",
        ));
    }
    validate_launch_templates(
        arguments,
        LaunchTemplateContext::Start,
        declared_settings,
        &format!("{field}.arguments"),
    )?;
    validate_launch_templates(
        validation_arguments,
        LaunchTemplateContext::ValidationProbe,
        declared_settings,
        &format!("{field}.validationArguments"),
    )
}

pub(crate) fn validate_launch_templates(
    values: &[String],
    context: LaunchTemplateContext,
    declared_settings: &DeclaredSettings,
    field: &str,
) -> Validation<()> {
    for (index, value) in values.iter().enumerate() {
        validate_launch_template(
            value,
            context,
            declared_settings,
            &format!("{field}[{index}]"),
        )?;
    }
    Ok(())
}

/// Collects setting IDs referenced by every template string in a JSON value.
pub fn collect_setting_references(value: &Value, references: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            if let Some(parts) = parse_template_with(text, LaunchVariable::parse) {
                for part in parts {
                    if let TemplatePart::Variable(LaunchVariable::Setting(id)) = part {
                        references.insert(id);
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_setting_references(item, references);
            }
        }
        Value::Object(members) => {
            for item in members.values() {
                collect_setting_references(item, references);
            }
        }
        _ => {}
    }
}

/// Checks every string inside a JSON value as a start-time launch template.
pub fn validate_launch_value(
    value: &Value,
    declared_settings: &DeclaredSettings,
    field: &str,
) -> Validation<()> {
    match value {
        Value::String(text) => {
            validate_launch_template(text, LaunchTemplateContext::Start, declared_settings, field)
        }
        Value::Array(items) => items.iter().enumerate().try_for_each(|(index, item)| {
            validate_launch_value(item, declared_settings, &format!("{field}[{index}]"))
        }),
        Value::Object(members) => members.iter().try_for_each(|(key, item)| {
            validate_launch_value(item, declared_settings, &format!("{field}.{key}"))
        }),
        _ => Ok(()),
    }
}

pub(crate) fn validate_launch_template(
    value: &str,
    context: LaunchTemplateContext,
    declared_settings: &DeclaredSettings,
    field: &str,
) -> Validation<()> {
    if value.contains('\0') {
        return Err(invalid_template(
            field.to_string(),
            "Launch values must not contain NUL.",
        ));
    }
    let Some(parts) = parse_template_with(value, LaunchVariable::parse) else {
        return Err(invalid_template(
            field.to_string(),
            "The value uses an unknown or unterminated template variable.",
        ));
    };
    let mut package_reference = false;
    for (index, part) in parts.iter().enumerate() {
        match part {
            // A setting must exist, may not shape a package path, and is not
            // available to the probe, which runs before settings are read.
            TemplatePart::Variable(LaunchVariable::Setting(id)) => {
                if context == LaunchTemplateContext::ValidationProbe {
                    return Err(invalid_template(
                        field.to_string(),
                        "The validation probe cannot use settings.",
                    ));
                }
                if declared_settings.is_immediate(id) {
                    return Err(invalid_template(
                        field.to_string(),
                        format!(
                            "The setting {id} applies immediately, so only workspaceConfiguration may use it."
                        ),
                    ));
                }
                if !declared_settings.contains(id) {
                    return Err(invalid_template(
                        field.to_string(),
                        format!("The value refers to the undeclared setting {id}."),
                    ));
                }
                // Text and paths are a whole value, or the value of one
                // `name=` argument, never part of a longer one.
                if declared_settings.requires_whole_value(id) {
                    let whole = match parts.as_slice() {
                        [_] => true,
                        [TemplatePart::Text(prefix), _] => prefix.ends_with('='),
                        _ => false,
                    };
                    if !whole || index + 1 != parts.len() {
                        return Err(invalid_template(
                            field.to_string(),
                            format!(
                                "The text or path setting {id} must be a whole value or follow a name= prefix."
                            ),
                        ));
                    }
                }
                if package_reference {
                    return Err(invalid_template(
                        field.to_string(),
                        "A package path may only vary by ${arch}.",
                    ));
                }
            }
            TemplatePart::Variable(
                LaunchVariable::StateDir
                | LaunchVariable::WorkspaceStateDir
                | LaunchVariable::LogDir,
            ) if context == LaunchTemplateContext::ValidationProbe => {
                return Err(invalid_template(
                    field.to_string(),
                    "The validation probe runs without a workspace and cannot use state directories.",
                ));
            }
            // Everything after `${pluginDir}` is a package path the host
            // verifies before launch, so it has to be `/`-rooted text and may
            // only vary by architecture.
            TemplatePart::Variable(LaunchVariable::PluginDir) => {
                let rooted = matches!(
                    parts.get(index + 1),
                    Some(TemplatePart::Text(text)) if text.starts_with('/') && text.len() > 1
                );
                if package_reference || !rooted {
                    return Err(invalid_template(
                        field.to_string(),
                        "${pluginDir} must be used once and be followed by a path inside the package.",
                    ));
                }
                package_reference = true;
            }
            TemplatePart::Variable(variable)
                if package_reference && *variable != LaunchVariable::Arch =>
            {
                return Err(invalid_template(
                    field.to_string(),
                    "A package path may only vary by ${arch}.",
                ));
            }
            TemplatePart::Text(text)
                if package_reference
                    && (text.contains('\\')
                        || text.split('/').any(|component| component == "..")) =>
            {
                return Err(invalid_template(
                    field.to_string(),
                    "A package path must stay inside the plugin package.",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// How long the host waits for a launch function by default, in
/// milliseconds; a manifest may choose a value within the bounds below.
pub const RESOLVE_TIMEOUT_MILLISECONDS: u64 = 30_000;

pub(crate) const MIN_RESOLVE_TIMEOUT_MILLISECONDS: u64 = 1_000;

pub(crate) const MAX_RESOLVE_TIMEOUT_MILLISECONDS: u64 = 120_000;

/// Checks `launch.timeoutMilliseconds` of a backend launch.
pub fn validate_resolve_timeout(value: Option<u64>, field: &str) -> Validation<()> {
    match value {
        Some(milliseconds)
            if !(MIN_RESOLVE_TIMEOUT_MILLISECONDS..=MAX_RESOLVE_TIMEOUT_MILLISECONDS)
                .contains(&milliseconds) =>
        {
            Err(invalid(
                field,
                "timeoutMilliseconds must be between 1000 and 120000.",
            ))
        }
        _ => Ok(()),
    }
}
