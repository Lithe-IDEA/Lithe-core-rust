//! `invocation`: the executable, its runtime dependencies and version probes.

use super::*;

/// Which executable runs the tool and what it needs.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Invocation {
    /// Candidates in preference order; the host takes the first that exists.
    pub(crate) executables: Vec<ExecutableCandidate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) global_arguments: Vec<BuildArgument>,
    /// Environment variables the plugin may read. The host passes only these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) environment_inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) runtime_dependencies: Vec<RuntimeDependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<VersionProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) minimum_version: Option<String>,
}

/// Runtime tools the host discovers and manages itself for build-system
/// runtime dependencies: it exports each as `LITHE_<TOOL>_PATH` and puts its
/// directory on the process's `PATH`. Server processes never use this list;
/// they name tool definitions from `requiresTools`.
pub(crate) const KNOWN_RUNTIME_TOOLS: &[&str] = &["go", "node"];

pub(crate) use lithe_plugin_model::executables::{ExecutableCandidate, ExecutableSource, Platform};

/// A runtime the tool needs. Runtimes the host already manages (`node`,
/// `go`) take only an ID and an optional minimum version and follow the
/// user's runtime settings. Any other runtime either describes how to find
/// and probe it, or lists no executables and names the tool definition with
/// its ID, which finds the executable and checks `minimumVersion`. A
/// plugin is never limited to a host list.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuntimeDependency {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) minimum_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) executables: Vec<ExecutableCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<VersionProbe>,
}

impl RuntimeDependency {
    /// True when the host discovers this runtime with its own settings.
    pub(crate) fn is_host_managed(&self) -> bool {
        KNOWN_RUNTIME_TOOLS.contains(&self.id.as_str())
    }

    /// True when the runtime is the tool definition with the same ID: it
    /// lists no executables and the host does not manage it. Plans give it
    /// a single toolchain candidate for that ID.
    pub(crate) fn names_tool_definition(&self) -> bool {
        self.executables.is_empty() && !self.is_host_managed()
    }

    /// True when the tool definition the runtime resolves through checks
    /// `minimumVersion`, so the manifest needs no version probe: a runtime
    /// the host does not manage that is an implicit reference or has exactly
    /// one toolchain candidate.
    pub(crate) fn checked_by_tool_definition(&self) -> bool {
        !self.is_host_managed()
            && matches!(
                self.executables.as_slice(),
                [] | [ExecutableCandidate {
                    source: ExecutableSource::Toolchain { .. },
                    ..
                }]
            )
    }
}

/// How to ask an executable for its version: literal arguments, and a
/// pattern whose first capture group is the dotted version.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VersionProbe {
    pub(crate) arguments: Vec<String>,
    pub(crate) pattern: String,
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_invocation(
        &mut self,
        field: &str,
        options: &BTreeSet<String>,
        context: &DeclarationContext<'_>,
    ) -> Validation<()> {
        let invocation = &mut self.invocation;
        let field = format!("{field}.invocation");
        if invocation.environment_inputs.len() > MAX_LIST {
            return Err(invalid(
                format!("{field}.environmentInputs"),
                "Too many environment inputs.",
            ));
        }
        if let Some(index) = invocation
            .environment_inputs
            .iter()
            .position(|name| !is_environment_name(name))
        {
            return Err(invalid(
                format!("{field}.environmentInputs[{index}]"),
                "Environment variable names use uppercase letters, digits, and _.",
            ));
        }
        invocation.environment_inputs =
            sorted_unique(std::mem::take(&mut invocation.environment_inputs));

        let mut runtimes = BTreeSet::new();
        for (index, runtime) in invocation.runtime_dependencies.iter_mut().enumerate() {
            let field = format!("{field}.runtimeDependencies[{index}]");
            // A runtime without executables is a tool-definition ID, which
            // may also be plugin-qualified.
            let valid_id = if runtime.names_tool_definition() {
                lithe_plugin_model::is_tool_id(&runtime.id)
            } else {
                is_entry_identifier(&runtime.id)
            };
            if !valid_id || !runtimes.insert(runtime.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "Runtime IDs are unique identifiers; a runtime without executables is a tool-definition ID.",
                ));
            }
            if let Some(name) = &runtime.display_name {
                check_text(name, &format!("{field}.displayName"), "display name")?;
            }
            if let Some(minimum) = &runtime.minimum_version {
                validate_minimum_version(minimum, &format!("{field}.minimumVersion"))?;
            }
            if runtime.is_host_managed() {
                // The host finds these with the user's own runtime settings;
                // a plugin must not route around that choice.
                let toolchain = runtime.executables.len() == 1
                    && matches!(
                        runtime.executables[0].source,
                        ExecutableSource::Toolchain { .. }
                    );
                if (!runtime.executables.is_empty() && !toolchain) || runtime.version.is_some() {
                    return Err(invalid(
                        field,
                        format!(
                            "{} is discovered by the host; declare only its minimumVersion.",
                            runtime.id
                        ),
                    ));
                }
                if toolchain {
                    check_candidate(
                        &mut runtime.executables[0],
                        &format!("{field}.executables[0]"),
                        &invocation.environment_inputs,
                        &BTreeSet::new(),
                    )?;
                }
                continue;
            }
            if runtime.names_tool_definition() {
                // The tool definition finds the executable and checks
                // `minimumVersion`; a probe here would have nothing to run.
                if runtime.version.is_some() {
                    return Err(invalid(
                        format!("{field}.version"),
                        format!(
                            "{} names a tool definition, which checks its version; declare only its minimumVersion.",
                            runtime.id
                        ),
                    ));
                }
                continue;
            }
            if runtime.executables.len() > MAX_LIST {
                return Err(invalid(
                    format!("{field}.executables"),
                    "A plugin-described runtime has at most 32 executable candidates.",
                ));
            }
            for (candidate_index, candidate) in runtime.executables.iter_mut().enumerate() {
                let candidate_field = format!("{field}.executables[{candidate_index}]");
                if matches!(candidate.source, ExecutableSource::Runtime { .. }) {
                    return Err(invalid(
                        candidate_field,
                        "A runtime cannot be provided by another runtime.",
                    ));
                }
                check_candidate(
                    candidate,
                    &candidate_field,
                    &invocation.environment_inputs,
                    &BTreeSet::new(),
                )?;
            }
            match &runtime.version {
                Some(probe) => probe.validate(&format!("{field}.version"))?,
                None if runtime.minimum_version.is_some()
                    && !runtime.checked_by_tool_definition() =>
                {
                    return Err(invalid(
                        format!("{field}.version"),
                        "A minimumVersion needs a version probe or a single toolchain candidate.",
                    ))
                }
                None => {}
            }
        }

        if invocation.executables.is_empty() || invocation.executables.len() > MAX_LIST {
            return Err(invalid(
                format!("{field}.executables"),
                "The invocation needs 1-32 executable candidates.",
            ));
        }
        for (index, candidate) in invocation.executables.iter_mut().enumerate() {
            check_candidate(
                candidate,
                &format!("{field}.executables[{index}]"),
                &invocation.environment_inputs,
                &runtimes,
            )?;
        }
        for (list, location) in
            std::iter::once((&invocation.executables, format!("{field}.executables"))).chain(
                invocation.runtime_dependencies.iter().map(|runtime| {
                    (
                        &runtime.executables,
                        format!("{field}.runtimeDependencies.{}.executables", runtime.id),
                    )
                }),
            )
        {
            lithe_plugin_model::executables::ordered_indices(list, &location)?;
            for candidate in list {
                if let ExecutableSource::Setting { setting_id } = &candidate.source {
                    if !context.declared_settings.is_executable(setting_id) {
                        return Err(invalid(&location, "A setting candidate needs a global file setting with effect reload or nextRun."));
                    }
                }
            }
        }
        if let Some(probe) = &invocation.version {
            probe.validate(&format!("{field}.version"))?;
        }
        if let Some(minimum) = &invocation.minimum_version {
            if invocation.version.is_none() {
                return Err(invalid(
                    format!("{field}.minimumVersion"),
                    "A minimumVersion needs a version probe.",
                ));
            }
            validate_minimum_version(minimum, &format!("{field}.minimumVersion"))?;
        }
        check_argument_count(
            &invocation.global_arguments,
            &format!("{field}.globalArguments"),
        )?;
        template::validate_arguments(
            &invocation.global_arguments,
            &Allowed {
                root: true,
                options: Some(options),
                ..Allowed::none()
            },
            &format!("{field}.globalArguments"),
        )
    }
}

fn executable_path(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.path.invalid",
        format!("{field}.path"),
        "Executable paths are relative, use /, and stay inside their base directory.",
    )
}

fn check_candidate(
    candidate: &mut ExecutableCandidate,
    field: &str,
    environment_inputs: &[String],
    runtimes: &BTreeSet<String>,
) -> Validation<()> {
    match &mut candidate.source {
        ExecutableSource::ProjectFile { path, requires, .. } => {
            *requires = sorted_unique(std::mem::take(requires));
            if !is_safe_relative_path(path)
                || !requires.iter().all(|value| is_safe_relative_path(value))
            {
                return Err(executable_path(field));
            }
        }
        ExecutableSource::Environment { variable, path, .. } => {
            if !environment_inputs.contains(variable) {
                return Err(invalid(
                    format!("{field}.variable"),
                    "The variable must be listed in environmentInputs.",
                ));
            }
            if !is_safe_relative_path(path) {
                return Err(executable_path(field));
            }
        }
        ExecutableSource::Command { command, .. } => {
            if !is_bare_command(command) {
                return Err(invalid(
                    format!("{field}.command"),
                    "A command is a bare name looked up on PATH.",
                ));
            }
        }
        ExecutableSource::Setting { setting_id } => {
            if !is_setting_identifier(setting_id) {
                return Err(invalid(format!("{field}.settingID"), "Invalid setting ID."));
            }
        }
        ExecutableSource::Toolchain { toolchain_id } => {
            if !lithe_plugin_model::is_tool_id(toolchain_id) {
                return Err(invalid(
                    format!("{field}.toolchainID"),
                    "Invalid toolchain ID.",
                ));
            }
        }
        ExecutableSource::Runtime { runtime, .. } => {
            if !runtimes.contains(runtime) {
                return Err(invalid(
                    format!("{field}.runtime"),
                    "The runtime must be declared in runtimeDependencies.",
                ));
            }
        }
    }
    let platforms = &mut candidate.platforms;
    platforms.sort();
    platforms.dedup();
    Ok(())
}

impl VersionProbe {
    fn validate(&self, field: &str) -> Validation<()> {
        if self.arguments.len() > 8
            || self
                .arguments
                .iter()
                .any(|argument| argument.contains('\0') || argument.contains("${"))
        {
            return Err(invalid(
                format!("{field}.arguments"),
                "Version arguments are at most 8 literal strings.",
            ));
        }
        check_regex(&self.pattern, &format!("{field}.pattern"), 1).map(|_| ())
    }

    /// The dotted version the probe output reports, if any.
    pub(crate) fn version(&self, output: &str) -> Option<Vec<u64>> {
        let pattern = Regex::new(&self.pattern).ok()?;
        let text = pattern.captures(output)?.get(1)?.as_str().to_string();
        let components: Vec<u64> = text
            .split('.')
            .take(3)
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        (!components.is_empty()).then_some(components)
    }
}
