//! `commandInstallers[]`: how the host installs a bare command that one of the
//! plugin's `systemCommand` launches, or the CLI of one of its agents, needs
//! and the machine lacks.
//!
//! The host never installs on its own. It shows the user what the
//! installation runs or downloads, whether it uses the network and where the
//! command will land, and installs only after the user confirms. A tool
//! installation is one process of a tool the plugin requires, run either into
//! a directory Lithe owns for the plugin (`managed`) or wherever the tool
//! installs by default (`toolDefault`). A download installation (version 1)
//! fetches a pinned archive for the host's platform, checks its SHA-256
//! digest and extracts it into the plugin's managed directory; the archive
//! rules are those of `managedRuntimes`. This module validates the
//! declaration, lists which launch needs which command, expands the plan for
//! the chosen target, and reads the directory a `toolDefault` installation
//! uses from the output of the tool's own probes.
//!
//! Note: the decision is recorded in .agents/notes/implemented/architecture/2026-11-15-plugin-command-installers.md

use super::archives::{
    bounded_text, is_archive_layout_path, validate_archive_source, ArchivePlatform, ArchiveSource,
    MAX_ARTIFACTS,
};
use super::*;
use lithe_platform::{paths, Platform};
use sha2::{Digest, Sha256};

/// Most installers one plugin may declare.
const MAX_COMMAND_INSTALLERS: usize = 16;

/// Most commands one installation may provide.
const MAX_INSTALLER_COMMANDS: usize = 8;

/// Most arguments of an installation or a directory probe.
const MAX_INSTALLER_ARGUMENTS: usize = 64;

/// Longest single argument or environment value, in bytes.
const MAX_INSTALLER_VALUE_BYTES: usize = 4096;

/// Most environment variables one target may set.
const MAX_INSTALLER_ENVIRONMENT: usize = 16;

/// Most directory probes of a `toolDefault` target.
const MAX_DIRECTORY_PROBES: usize = 4;

/// Timeout of an installation that declares none.
const DEFAULT_TIMEOUT_SECONDS: u64 = 600;

/// Longest timeout an installation may declare.
const MAX_TIMEOUT_SECONDS: u64 = 3600;

/// Longest upstream version of a download, in bytes.
const MAX_DOWNLOAD_VERSION_BYTES: usize = 128;

/// First `commandInstallers` version with download installers.
const DOWNLOAD_VERSION: u32 = 1;

/// Program kinds of the `systemCommand` launches an installer may serve, the
/// consumer kind the host shows, and the program key that names the consumer.
/// A plugin backend has a single process and so no key.
const SYSTEM_COMMAND_PROGRAMS: &[(&str, &str, Option<&str>)] = &[
    ("systemBackend", "backend", None),
    (
        "systemBuildProvider",
        "buildProvider",
        Some("buildSystemID"),
    ),
    ("systemDebugAdapter", "debugAdapter", Some("debugAdapter")),
    ("systemLanguageServer", "languageServer", Some("languageID")),
    ("systemVcsProvider", "vcsProvider", Some("vcs")),
];

/// The consumer kind of an agent's CLI, keyed by the agent's ID.
const AGENT_CONSUMER: &str = "agent";

/// One way to install bare commands the plugin's launches name: a run of a
/// tool (`tool` and `arguments`) or a pinned download (`download`), never
/// both.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommandInstallerDeclaration {
    /// Lowercase letters, digits, and hyphens; unique within the plugin.
    pub(crate) id: String,
    /// Bare commands the installation provides. Each is named by a
    /// `systemCommand` launch of this manifest, or by the `commands` of the
    /// tool definition of one of its agents, and by no other installer.
    pub(crate) commands: Vec<String>,
    /// Tool-definition ID, listed in `requiresTools`, whose executable runs
    /// the installation. Required unless the installer downloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool: Option<String>,
    /// Lowest acceptable version of `tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) minimum_version: Option<String>,
    /// Arguments after the tool's executable, used by a target that does not
    /// replace them. Literal: no template variables. Required with `tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) arguments: Option<Vec<String>>,
    /// Pinned archives to download instead of running a tool; needs
    /// `requires.commandInstallers` 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) download: Option<CommandDownload>,
    /// Whether the installation may download; the host shows it before the
    /// user confirms. Always true for a download installer.
    pub(crate) may_use_network: bool,
    /// How long the host lets the installation run, 1 to 3600 seconds. A
    /// download installer's timeout covers the download and the extraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_seconds: Option<u64>,
    /// Where the installation may put the commands; at least one target. A
    /// download installer has only `managed`.
    pub(crate) targets: InstallTargets,
}

/// The archives of one upstream release, one per platform it supports.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommandDownload {
    /// Upstream release identity the host shows, independent of the
    /// plugin's own version.
    version: String,
    /// One archive per supported operating system and architecture, ordered
    /// by platform after validation.
    artifacts: Vec<CommandArchive>,
}

/// One platform's archive. The rules are those of a `managedRuntimes`
/// artifact.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommandArchive {
    /// macos, windows or linux.
    os: String,
    /// arm64 or x86_64.
    arch: String,
    /// HTTPS URL of the archive.
    url: String,
    /// Fallback HTTPS URLs serving the same bytes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mirrors: Vec<String>,
    /// SHA-256 digest of the archive, lowercased after validation.
    sha256: String,
    /// zip or tar.gz; the host never infers a format from a URL suffix.
    archive: String,
    /// Archive root removed while extracting; empty removes nothing. Every
    /// entry lies below it.
    #[serde(default)]
    strip_prefix: String,
}

/// The locations the user may choose between.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InstallTargets {
    /// A directory Lithe owns for the plugin, removed with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed: Option<ManagedTarget>,
    /// Wherever the tool installs when it is told nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_default: Option<ToolDefaultTarget>,
}

/// Installation into the directory the host chose for the plugin. A tool
/// installer's arguments or environment must name `${installDirectory}`,
/// which is the only template variable of this contract. A download
/// installer's managed target sets only `commandDirectory`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedTarget {
    /// Replaces the installer's `arguments`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<Vec<String>>,
    /// Variables set for the installation, over the tool's environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
    /// Package-style relative path, below the install directory, of the
    /// directory the commands land in; the install directory itself when
    /// absent. For a download, the path is below the extracted root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_directory: Option<String>,
}

/// Installation where the tool installs by default. Its directory probes
/// tell the host, and through it the user, where that is.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolDefaultTarget {
    /// Replaces the installer's `arguments`. Literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arguments: Option<Vec<String>>,
    /// Variables set for the installation, over the tool's environment.
    /// Literal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
    /// Probes, tried in order, that print the directory the commands land
    /// in. Without probes the host can say only that the tool decides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    directory: Vec<DirectoryProbe>,
}

/// One run of the installer's tool that prints a directory.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectoryProbe {
    /// Arguments after the tool's executable. Literal and not empty.
    arguments: Vec<String>,
    /// The output is a path list in the host's separator; its first entry
    /// counts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    path_list: bool,
    /// Package-style relative path appended to the printed directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    append: Option<String>,
}

/// The target a plan expands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum InstallTarget {
    Managed,
    ToolDefault,
}

/// The single variable a managed target may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InstallDirectory;

fn parse_install_template(value: &str) -> Option<Vec<TemplatePart<InstallDirectory>>> {
    parse_template_with(value, |name| {
        (name == "installDirectory").then_some(InstallDirectory)
    })
}

/// Checks one value that may not use any template.
fn check_literal(value: &str, field: &str) -> Validation<()> {
    if value.contains("${") {
        return Err(invalid_template(
            field.to_string(),
            "Only targets.managed may use ${installDirectory}; other installer values are literal.",
        ));
    }
    check_value_size(value, field)
}

fn check_value_size(value: &str, field: &str) -> Validation<()> {
    if value.len() > MAX_INSTALLER_VALUE_BYTES || value.contains('\0') {
        return Err(invalid(
            field.to_string(),
            format!("An installer value is at most {MAX_INSTALLER_VALUE_BYTES} bytes without NUL."),
        ));
    }
    Ok(())
}

/// Checks one managed value and reports whether it names the directory.
fn check_managed_value(value: &str, field: &str) -> Validation<bool> {
    check_value_size(value, field)?;
    let parts = parse_install_template(value).ok_or_else(|| {
        invalid_template(
            field.to_string(),
            "A managed value may use only ${installDirectory}.",
        )
    })?;
    Ok(parts
        .iter()
        .any(|part| matches!(part, TemplatePart::Variable(_))))
}

fn check_arguments(arguments: &[String], field: &str) -> Validation<()> {
    if arguments.len() > MAX_INSTALLER_ARGUMENTS {
        return Err(invalid(
            field.to_string(),
            format!("An installer passes at most {MAX_INSTALLER_ARGUMENTS} arguments."),
        ));
    }
    Ok(())
}

fn check_environment_names(environment: &BTreeMap<String, String>, field: &str) -> Validation<()> {
    if environment.len() > MAX_INSTALLER_ENVIRONMENT {
        return Err(invalid(
            field.to_string(),
            format!("A target sets at most {MAX_INSTALLER_ENVIRONMENT} environment variables."),
        ));
    }
    match environment.keys().find(|name| !is_environment_name(name)) {
        Some(name) => Err(invalid(
            format!("{field}.{name}"),
            "An environment variable name uses uppercase letters, digits, and _.",
        )),
        None => Ok(()),
    }
}

impl CommandInstallerDeclaration {
    /// Checks one installer; `version` is the manifest's
    /// `requires.commandInstallers`.
    fn validate(
        &mut self,
        field: &str,
        launched: &BTreeSet<String>,
        runtime_tools: RuntimeTools<'_>,
        version: u32,
    ) -> Validation<()> {
        if !is_identifier(&self.id) || self.id.len() > 64 {
            return Err(invalid(
                format!("{field}.id"),
                "An installer ID uses lowercase letters, digits, and hyphens.",
            ));
        }
        if self.commands.is_empty() || self.commands.len() > MAX_INSTALLER_COMMANDS {
            return Err(invalid(
                format!("{field}.commands"),
                format!("An installer provides 1 to {MAX_INSTALLER_COMMANDS} commands."),
            ));
        }
        let mut seen = BTreeSet::new();
        for (index, command) in self.commands.iter().enumerate() {
            let entry = format!("{field}.commands[{index}]");
            if !is_bare_command(command) {
                return Err(invalid(
                    entry,
                    "An installed command is a bare command name.",
                ));
            }
            if !seen.insert(command) {
                return Err(invalid(entry, "A command is listed more than once."));
            }
            if !launched.contains(command) {
                return Err(invalid(
                    entry,
                    format!(
                        "No systemCommand launch or agent tool of this plugin names {command}."
                    ),
                ));
            }
        }
        if let Some(seconds) = self.timeout_seconds {
            if !(1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
                return Err(invalid(
                    format!("{field}.timeoutSeconds"),
                    format!("An installation times out after 1 to {MAX_TIMEOUT_SECONDS} seconds."),
                ));
            }
        }
        let targets = format!("{field}.targets");
        if self.targets.managed.is_none() && self.targets.tool_default.is_none() {
            return Err(invalid(
                targets,
                "An installer declares at least one target.",
            ));
        }
        if self.download.is_some() {
            return self.validate_download(field, version);
        }
        let Some(tool) = &self.tool else {
            return Err(invalid(
                format!("{field}.tool"),
                "An installer runs a tool or, with requires.commandInstallers 1, downloads an archive.",
            ));
        };
        runtime_tools.check(tool, format!("{field}.tool"))?;
        if let Some(version) = &self.minimum_version {
            validate_minimum_version(version, &format!("{field}.minimumVersion"))?;
        }
        let Some(arguments) = &self.arguments else {
            return Err(invalid(
                format!("{field}.arguments"),
                "A tool installer lists the arguments it passes to the tool.",
            ));
        };
        check_arguments(arguments, &format!("{field}.arguments"))?;
        for (index, argument) in arguments.iter().enumerate() {
            check_literal(argument, &format!("{field}.arguments[{index}]"))?;
        }
        if let Some(managed) = &self.targets.managed {
            self.validate_managed(managed, &format!("{targets}.managed"))?;
        }
        if let Some(tool_default) = &self.targets.tool_default {
            validate_tool_default(tool_default, &format!("{targets}.toolDefault"))?;
        }
        Ok(())
    }

    /// Checks a download installer: no tool, network use declared, only the
    /// managed target with at most a command directory, and archives that
    /// follow the `managedRuntimes` rules.
    fn validate_download(&mut self, field: &str, version: u32) -> Validation<()> {
        if version < DOWNLOAD_VERSION {
            return Err(invalid(
                "requires.commandInstallers",
                "Download installers require requires.commandInstallers >= 1.",
            ));
        }
        for (name, present) in [
            ("tool", self.tool.is_some()),
            ("minimumVersion", self.minimum_version.is_some()),
            ("arguments", self.arguments.is_some()),
        ] {
            if present {
                return Err(invalid(
                    format!("{field}.{name}"),
                    "A download installer runs no tool; it declares download instead.",
                ));
            }
        }
        if !self.may_use_network {
            return Err(invalid(
                format!("{field}.mayUseNetwork"),
                "A download installer uses the network.",
            ));
        }
        let targets = format!("{field}.targets");
        if self.targets.tool_default.is_some() {
            return Err(invalid(
                format!("{targets}.toolDefault"),
                "A download installer extracts only into the managed directory.",
            ));
        }
        let Some(managed) = &self.targets.managed else {
            return Err(invalid(
                format!("{targets}.managed"),
                "A download installer extracts into the managed directory.",
            ));
        };
        let managed_field = format!("{targets}.managed");
        if managed.arguments.is_some() {
            return Err(invalid(
                format!("{managed_field}.arguments"),
                "A download installer runs no tool and passes no arguments.",
            ));
        }
        if !managed.environment.is_empty() {
            return Err(invalid(
                format!("{managed_field}.environment"),
                "A download installer runs no tool and sets no environment.",
            ));
        }
        if let Some(directory) = &managed.command_directory {
            if !is_safe_relative_path(directory) || !is_archive_layout_path(directory) {
                return Err(invalid(
                    format!("{managed_field}.commandDirectory"),
                    "The command directory is a relative path inside the extracted archive.",
                ));
            }
        }
        let Some(download) = &mut self.download else {
            return Ok(());
        };
        let field = format!("{field}.download");
        if !bounded_text(&download.version, MAX_DOWNLOAD_VERSION_BYTES) {
            return Err(invalid(
                format!("{field}.version"),
                format!("A download version is a nonempty upstream identity of at most {MAX_DOWNLOAD_VERSION_BYTES} bytes."),
            ));
        }
        if !(1..=MAX_ARTIFACTS).contains(&download.artifacts.len()) {
            return Err(invalid(
                format!("{field}.artifacts"),
                format!("A download declares from 1 to {MAX_ARTIFACTS} platform archives."),
            ));
        }
        let mut platforms = BTreeSet::new();
        for (index, artifact) in download.artifacts.iter_mut().enumerate() {
            let field = format!("{field}.artifacts[{index}]");
            if !archives::is_archive_platform(&artifact.os, &artifact.arch)
                || !platforms.insert((artifact.os.clone(), artifact.arch.clone()))
            {
                return Err(invalid(
                    &field,
                    "Archives have unique supported operating-system and architecture pairs.",
                ));
            }
            validate_archive_source(
                &field,
                ArchiveSource {
                    url: &artifact.url,
                    mirrors: &artifact.mirrors,
                    sha256: &mut artifact.sha256,
                    archive: &artifact.archive,
                },
                || Ok(()),
            )?;
            if !is_archive_layout_path(&artifact.strip_prefix) {
                return Err(invalid(
                    format!("{field}.stripPrefix"),
                    "An archive path is a bounded relative path without traversal.",
                ));
            }
        }
        download
            .artifacts
            .sort_by(|left, right| (&left.os, &left.arch).cmp(&(&right.os, &right.arch)));
        Ok(())
    }

    /// The archive of a download installer for `platform`, if it has one.
    fn archive_for(&self, platform: &ArchivePlatform) -> Option<&CommandArchive> {
        self.download.as_ref().and_then(|download| {
            download
                .artifacts
                .iter()
                .find(|artifact| platform.matches(&artifact.os, &artifact.arch))
        })
    }

    fn validate_managed(&self, managed: &ManagedTarget, field: &str) -> Validation<()> {
        let mut names_directory = false;
        if let Some(arguments) = &managed.arguments {
            check_arguments(arguments, &format!("{field}.arguments"))?;
            for (index, argument) in arguments.iter().enumerate() {
                names_directory |=
                    check_managed_value(argument, &format!("{field}.arguments[{index}]"))?;
            }
        }
        check_environment_names(&managed.environment, &format!("{field}.environment"))?;
        for (name, value) in &managed.environment {
            names_directory |= check_managed_value(value, &format!("{field}.environment.{name}"))?;
        }
        if !names_directory {
            return Err(invalid_template(
                field.to_string(),
                "A managed target passes ${installDirectory} in its arguments or environment.",
            ));
        }
        if let Some(directory) = &managed.command_directory {
            if !is_safe_relative_path(directory) {
                return Err(invalid(
                    format!("{field}.commandDirectory"),
                    "The command directory is a relative path inside the install directory.",
                ));
            }
        }
        Ok(())
    }

    fn effective_arguments<'a>(&'a self, replacement: &'a Option<Vec<String>>) -> &'a [String] {
        replacement
            .as_deref()
            .or(self.arguments.as_deref())
            .unwrap_or_default()
    }

    fn timeout_milliseconds(&self) -> u64 {
        self.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS) * 1000
    }

    /// The targets the host offers, in a stable order.
    fn target_names(&self) -> Vec<InstallTarget> {
        let mut targets = Vec::new();
        if self.targets.managed.is_some() {
            targets.push(InstallTarget::Managed);
        }
        if self.targets.tool_default.is_some() {
            targets.push(InstallTarget::ToolDefault);
        }
        targets
    }
}

fn validate_tool_default(target: &ToolDefaultTarget, field: &str) -> Validation<()> {
    if let Some(arguments) = &target.arguments {
        check_arguments(arguments, &format!("{field}.arguments"))?;
        for (index, argument) in arguments.iter().enumerate() {
            check_literal(argument, &format!("{field}.arguments[{index}]"))?;
        }
    }
    check_environment_names(&target.environment, &format!("{field}.environment"))?;
    for (name, value) in &target.environment {
        check_literal(value, &format!("{field}.environment.{name}"))?;
    }
    if target.directory.len() > MAX_DIRECTORY_PROBES {
        return Err(invalid(
            format!("{field}.directory"),
            format!("A target probes its directory at most {MAX_DIRECTORY_PROBES} ways."),
        ));
    }
    for (index, probe) in target.directory.iter().enumerate() {
        let entry = format!("{field}.directory[{index}]");
        if probe.arguments.is_empty() {
            return Err(invalid(
                format!("{entry}.arguments"),
                "A directory probe passes at least one argument.",
            ));
        }
        check_arguments(&probe.arguments, &format!("{entry}.arguments"))?;
        for (position, argument) in probe.arguments.iter().enumerate() {
            check_literal(argument, &format!("{entry}.arguments[{position}]"))?;
        }
        if let Some(append) = &probe.append {
            if !is_safe_relative_path(append) {
                return Err(invalid(
                    format!("{entry}.append"),
                    "The appended path is relative and stays below the printed directory.",
                ));
            }
        }
    }
    Ok(())
}

impl DeclarativePluginManifest {
    /// Checks `commandInstallers` against the plugin's own launches and
    /// `requiresTools`, then orders the installers by ID.
    pub(crate) fn validate_command_installers(
        &mut self,
        runtime_tools: RuntimeTools<'_>,
    ) -> Validation<()> {
        if self.command_installers.is_empty() {
            return Ok(());
        }
        if self.command_installers.len() > MAX_COMMAND_INSTALLERS {
            return Err(invalid(
                "commandInstallers",
                format!("A plugin declares at most {MAX_COMMAND_INSTALLERS} command installers."),
            ));
        }
        let launched: BTreeSet<String> = self
            .command_consumers()
            .into_iter()
            .flat_map(|consumer| consumer.commands)
            .collect();
        let version = self
            .requires
            .get(capabilities::COMMAND_INSTALLERS)
            .copied()
            .unwrap_or(0);
        let mut ids = BTreeSet::new();
        let mut installed = BTreeSet::new();
        for (index, installer) in self.command_installers.iter_mut().enumerate() {
            let field = format!("commandInstallers[{index}]");
            installer.validate(&field, &launched, runtime_tools, version)?;
            if !ids.insert(installer.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "An installer ID is declared more than once.",
                ));
            }
            for (position, command) in installer.commands.iter().enumerate() {
                if !installed.insert(command.clone()) {
                    return Err(invalid(
                        format!("{field}.commands[{position}]"),
                        format!("{command} already has an installer."),
                    ));
                }
            }
        }
        self.command_installers
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }

    /// Every `systemCommand` launch the host may start, in the order of the
    /// reviewed program list, then every agent whose tool this plugin defines,
    /// in declaration order, with the commands of that definition. The host
    /// finds an agent's CLI through the tool definition, whose `commands` are
    /// the bare names it looks up; an agent whose tool another plugin defines
    /// names no command here.
    fn command_consumers(&self) -> Vec<CommandConsumer> {
        let mut consumers = self.system_command_consumers();
        for agent in &self.agents {
            let Some(definition) = self
                .tool_definitions
                .iter()
                .find(|definition| definition["id"] == agent.tool.as_str())
            else {
                continue;
            };
            let commands: Vec<String> = definition["commands"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|command| command.as_str().map(str::to_string))
                .collect();
            if !commands.is_empty() {
                consumers.push(CommandConsumer {
                    kind: AGENT_CONSUMER,
                    id: Some(agent.id.clone()),
                    commands,
                });
            }
        }
        consumers
    }

    /// The `systemCommand` launches of the reviewed program list, in its order.
    fn system_command_consumers(&self) -> Vec<CommandConsumer> {
        self.programs()
            .iter()
            .filter_map(|program| {
                let kind = program["kind"].as_str()?;
                let (_, consumer, key) = SYSTEM_COMMAND_PROGRAMS
                    .iter()
                    .find(|(program_kind, _, _)| *program_kind == kind)?;
                let commands = program["commands"]
                    .as_array()?
                    .iter()
                    .filter_map(|command| command.as_str().map(str::to_string))
                    .collect();
                Some(CommandConsumer {
                    kind: consumer,
                    id: key.and_then(|key| program[key].as_str().map(str::to_string)),
                    commands,
                })
            })
            .collect()
    }

    fn command_installer(&self, id: &str) -> Validation<&CommandInstallerDeclaration> {
        self.command_installers
            .iter()
            .find(|installer| installer.id == id)
            .ok_or_else(|| {
                PluginRejection::at(
                    "plugin.commandInstaller.unknown",
                    "installerID",
                    format!("The plugin declares no command installer {id}."),
                )
            })
    }
}

/// One launch that starts a bare command.
struct CommandConsumer {
    kind: &'static str,
    id: Option<String>,
    commands: Vec<String>,
}

/// The install review entry of one installer: everything that decides what
/// the installation runs or downloads. A tool installer's entry is the same
/// as in version 0.
pub(crate) fn programs(installer: &CommandInstallerDeclaration) -> Value {
    if let Some(download) = &installer.download {
        return json!({
            "kind": "commandInstaller",
            "installer": installer.id,
            "commands": installer.commands,
            "download": download,
            "targets": installer.targets,
            "mayUseNetwork": installer.may_use_network,
            "timeoutSeconds": installer.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS),
        });
    }
    json!({
        "kind": "commandInstaller",
        "installer": installer.id,
        "commands": installer.commands,
        "tool": installer.tool,
        "minimumVersion": installer.minimum_version,
        "arguments": installer.arguments,
        "targets": installer.targets,
        "mayUseNetwork": installer.may_use_network,
        "timeoutSeconds": installer.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequirementsPayload {
    manifest: Value,
    /// The host's platform. With it, a download installer without an
    /// archive for the platform is left out.
    #[serde(default)]
    platform: Option<ArchivePlatform>,
}

/// Rejects a platform no archive may target.
fn check_platform(platform: Option<&ArchivePlatform>) -> Validation<()> {
    match platform {
        Some(platform) if !platform.is_supported() => Err(platform_invalid()),
        _ => Ok(()),
    }
}

fn platform_invalid() -> PluginRejection {
    PluginRejection::at(
        "plugin.commandInstaller.platformInvalid",
        "platform",
        "A download installation needs a supported operating system and architecture.",
    )
}

/// `plugins.commandInstallers.requirements`: lists every launch that starts
/// a bare command, the installer of each command, and the installers. Given
/// the host's platform, a download installer without an archive for it is
/// left out, and its commands have no installer.
pub fn command_installer_requirements_command(payload: Value) -> Result<Value, CoreError> {
    let payload: RequirementsPayload =
        decode_payload(payload, "plugins.commandInstallers.requirements")?;
    Ok(outcome(requirements(payload)))
}

fn requirements(payload: RequirementsPayload) -> Validation<(&'static str, Value)> {
    let manifest = validate_manifest_value(payload.manifest)?;
    check_platform(payload.platform.as_ref())?;
    let offered: Vec<&CommandInstallerDeclaration> = manifest
        .command_installers
        .iter()
        .filter(|installer| match (&installer.download, &payload.platform) {
            (Some(_), Some(platform)) => installer.archive_for(platform).is_some(),
            _ => true,
        })
        .collect();
    let installer_of = |command: &str| {
        offered
            .iter()
            .find(|installer| installer.commands.iter().any(|name| name == command))
            .map(|installer| installer.id.clone())
    };
    let consumers: Vec<Value> = manifest
        .command_consumers()
        .into_iter()
        .map(|consumer| {
            json!({
                "kind": consumer.kind,
                "id": consumer.id,
                "commands": consumer.commands.iter().map(|command| json!({
                    "command": command,
                    "installer": installer_of(command),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let installers: Vec<Value> = offered
        .iter()
        .map(|installer| {
            json!({
                "id": installer.id,
                "commands": installer.commands,
                "tool": installer.tool,
                "minimumVersion": installer.minimum_version,
                "download": installer.download.as_ref().map(|download| json!({
                    "version": download.version,
                })),
                "mayUseNetwork": installer.may_use_network,
                "targets": installer.target_names(),
            })
        })
        .collect();
    Ok((
        "requirements",
        json!({ "consumers": consumers, "installers": installers }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanPayload {
    manifest: Value,
    #[serde(rename = "installerID")]
    installer_id: String,
    target: InstallTarget,
    /// Absolute directory the host made for a managed tool installation.
    #[serde(default)]
    install_directory: Option<String>,
    /// The host's platform, which selects a download installer's archive.
    #[serde(default)]
    platform: Option<ArchivePlatform>,
}

/// `plugins.commandInstallers.plan`: expands one installer for the target
/// the user chose. For a tool installer the host resolves `tool`, runs it
/// with `arguments` and `environment` under the timeout, and then looks for
/// the commands in `commandDirectory`, or for `toolDefault` in the directory
/// the probes report. For a download installer `download` names the archive
/// for the host's platform; the host downloads it, checks the digest,
/// extracts it into a directory of its own below the plugin's managed
/// directory under the timeout, and looks for the commands in
/// `download.commandDirectory` below the extracted root.
pub fn command_installer_plan_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PlanPayload = decode_payload(payload, "plugins.commandInstallers.plan")?;
    Ok(outcome(plan(payload, Platform::HOST)))
}

fn plan(payload: PlanPayload, platform: Platform) -> Validation<(&'static str, Value)> {
    let manifest = validate_manifest_value(payload.manifest)?;
    let installer = manifest.command_installer(&payload.installer_id)?;
    check_platform(payload.platform.as_ref())?;
    let undeclared = || {
        PluginRejection::at(
            "plugin.commandInstaller.targetUndeclared",
            "target",
            format!(
                "The command installer {} does not declare this target.",
                installer.id
            ),
        )
    };
    let directory_invalid = |message: &str| {
        PluginRejection::at(
            "plugin.commandInstaller.directoryInvalid",
            "installDirectory",
            message.to_string(),
        )
    };
    let mut plan = json!({
        "installer": installer.id,
        "target": payload.target,
        "commands": installer.commands,
        "tool": installer.tool,
        "minimumVersion": installer.minimum_version,
        "mayUseNetwork": installer.may_use_network,
        "timeoutMilliseconds": installer.timeout_milliseconds(),
        "download": Value::Null,
    });
    if installer.download.is_some() {
        let managed = match payload.target {
            InstallTarget::Managed => installer.targets.managed.as_ref(),
            InstallTarget::ToolDefault => None,
        }
        .ok_or_else(undeclared)?;
        if payload.install_directory.is_some() {
            return Err(directory_invalid(
                "A download installation takes no install directory; the host makes one for the archive.",
            ));
        }
        let host = payload.platform.as_ref().ok_or_else(platform_invalid)?;
        let artifact = installer.archive_for(host).ok_or_else(|| {
            PluginRejection::at(
                "plugin.commandInstaller.unavailable",
                "platform",
                format!(
                    "The command installer {} has no archive for this operating system and architecture.",
                    installer.id
                ),
            )
        })?;
        plan["arguments"] = json!([]);
        plan["environment"] = json!({});
        plan["commandDirectory"] = Value::Null;
        plan["directoryProbes"] = json!([]);
        plan["download"] = download_plan(installer, artifact, managed);
        return Ok(("plan", plan));
    }
    match payload.target {
        InstallTarget::Managed => {
            let managed = installer.targets.managed.as_ref().ok_or_else(undeclared)?;
            let directory = payload
                .install_directory
                .as_deref()
                .filter(|directory| {
                    paths::is_absolute_path(directory, platform)
                        && !directory.contains(['\n', '\r', '\0'])
                })
                .ok_or_else(|| {
                    directory_invalid(
                        "A managed installation needs the absolute directory the host made.",
                    )
                })?;
            let expand = |value: &str| {
                parse_install_template(value)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|part| match part {
                        TemplatePart::Text(text) => text,
                        TemplatePart::Variable(InstallDirectory) => directory.to_string(),
                    })
                    .collect::<String>()
            };
            plan["arguments"] = json!(installer
                .effective_arguments(&managed.arguments)
                .iter()
                .map(|argument| expand(argument))
                .collect::<Vec<_>>());
            plan["environment"] = json!(managed
                .environment
                .iter()
                .map(|(name, value)| (name.clone(), expand(value)))
                .collect::<BTreeMap<_, _>>());
            plan["commandDirectory"] = json!(match &managed.command_directory {
                Some(relative) => paths::join_native(directory, relative, platform),
                None => directory.to_string(),
            });
            plan["directoryProbes"] = json!([]);
        }
        InstallTarget::ToolDefault => {
            let target = installer
                .targets
                .tool_default
                .as_ref()
                .ok_or_else(undeclared)?;
            if payload.install_directory.is_some() {
                return Err(directory_invalid(
                    "A toolDefault installation takes no install directory.",
                ));
            }
            plan["arguments"] = json!(installer.effective_arguments(&target.arguments));
            plan["environment"] = json!(target.environment);
            plan["commandDirectory"] = Value::Null;
            plan["directoryProbes"] = json!(target
                .directory
                .iter()
                .map(|probe| json!({ "arguments": probe.arguments }))
                .collect::<Vec<_>>());
        }
    }
    Ok(("plan", plan))
}

/// The selected archive of a download plan. `cacheKey` digests the fields
/// that decide the extracted content, so the host keeps one extraction per
/// archive; mirrors are left out of it, so adding a mirror reuses the
/// extraction.
fn download_plan(
    installer: &CommandInstallerDeclaration,
    artifact: &CommandArchive,
    managed: &ManagedTarget,
) -> Value {
    let version = installer
        .download
        .as_ref()
        .map(|download| download.version.as_str())
        .unwrap_or_default();
    // Serialize this explicitly built object, not the input's field order.
    let mut selected = json!({
        "installer": installer.id,
        "version": version,
        "os": artifact.os,
        "arch": artifact.arch,
        "url": artifact.url,
        "sha256": artifact.sha256,
        "archive": artifact.archive,
        "stripPrefix": artifact.strip_prefix,
    });
    let identity = Sha256::digest(selected.to_string().as_bytes());
    selected["cacheKey"] = json!(format!("{identity:x}"));
    selected["commandDirectory"] = json!(managed.command_directory.as_deref().unwrap_or_default());
    if !artifact.mirrors.is_empty() {
        selected["mirrors"] = json!(artifact.mirrors);
    }
    selected
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefaultDirectoryPayload {
    manifest: Value,
    #[serde(rename = "installerID")]
    installer_id: String,
    /// Standard output of each directory probe in plan order; `null` for a
    /// probe that did not exit successfully.
    outputs: Vec<Option<String>>,
}

/// `plugins.commandInstallers.defaultDirectory`: the directory a
/// `toolDefault` installation puts its commands in, read from the output of
/// the first probe that names an absolute directory; `null` when none does.
pub fn command_installer_default_directory_command(payload: Value) -> Result<Value, CoreError> {
    let payload: DefaultDirectoryPayload =
        decode_payload(payload, "plugins.commandInstallers.defaultDirectory")?;
    Ok(outcome(default_directory(payload, Platform::HOST)))
}

fn default_directory(
    payload: DefaultDirectoryPayload,
    platform: Platform,
) -> Validation<(&'static str, Value)> {
    let manifest = validate_manifest_value(payload.manifest)?;
    let installer = manifest.command_installer(&payload.installer_id)?;
    let target = installer.targets.tool_default.as_ref().ok_or_else(|| {
        PluginRejection::at(
            "plugin.commandInstaller.targetUndeclared",
            "installerID",
            format!(
                "The command installer {} does not declare toolDefault.",
                installer.id
            ),
        )
    })?;
    if payload.outputs.len() != target.directory.len() {
        return Err(PluginRejection::at(
            "plugin.commandInstaller.probeOutputsInvalid",
            "outputs",
            format!(
                "The installer has {} directory probes; send one output for each.",
                target.directory.len()
            ),
        ));
    }
    let directory = target
        .directory
        .iter()
        .zip(&payload.outputs)
        .find_map(|(probe, output)| probe_directory(probe, output.as_deref()?, platform));
    Ok(("directory", json!(directory)))
}

/// The directory one probe's output names, or `None` when it names no
/// absolute directory.
fn probe_directory(probe: &DirectoryProbe, output: &str, platform: Platform) -> Option<String> {
    let output = output.trim();
    let directory = if probe.path_list {
        output
            .split(paths::path_list_separator(platform))
            .map(str::trim)
            .find(|entry| !entry.is_empty())?
    } else {
        output
    };
    if directory.is_empty()
        || directory.contains(['\n', '\r', '\0'])
        || !paths::is_absolute_path(directory, platform)
    {
        return None;
    }
    Some(match &probe.append {
        Some(relative) => paths::join_native(directory, relative, platform),
        None => directory.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGIN: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/command-installers/command-installer.plugin.json"
    ));
    const DOWNLOAD_PLUGIN: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/command-installers/command-installer-download.plugin.json"
    ));
    const AGENT_PLUGIN: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/command-installers/command-installer-agent.plugin.json"
    ));
    const PLANS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/command-installers/command-installer-plans.json"
    ));

    /// Every golden case, on both platforms: path rules are the platform's,
    /// so a case whose result depends on them carries one input and result
    /// per platform.
    #[test]
    fn plan_fixtures_match_on_every_platform() {
        let plans: Value = serde_json::from_str(PLANS).unwrap();
        for case in plans["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let file = case
                .get("plugin")
                .unwrap_or(&plans["plugin"])
                .as_str()
                .unwrap();
            let plugin: Value = serde_json::from_str(match file {
                "command-installer.plugin.json" => PLUGIN,
                "command-installer-download.plugin.json" => DOWNLOAD_PLUGIN,
                "command-installer-agent.plugin.json" => AGENT_PLUGIN,
                other => panic!("unknown plugin fixture {other}"),
            })
            .unwrap();
            for (platform, key) in [(Platform::Unix, "unix"), (Platform::Windows, "windows")] {
                let specific = &case["platforms"][key];
                let mut payload = json!({ "manifest": plugin });
                for source in [&case["payload"], &specific["payload"]] {
                    if let Some(fields) = source.as_object() {
                        for (field, value) in fields {
                            payload[field] = value.clone();
                        }
                    }
                }
                for edit in case["manifestEdits"].as_array().into_iter().flatten() {
                    let pointer = edit["path"].as_str().unwrap();
                    *payload["manifest"].pointer_mut(pointer).unwrap() = edit["value"].clone();
                }
                let result = match case["operation"].as_str().unwrap() {
                    "requirements" => requirements(serde_json::from_value(payload).unwrap()),
                    "plan" => plan(serde_json::from_value(payload).unwrap(), platform),
                    "defaultDirectory" => {
                        default_directory(serde_json::from_value(payload).unwrap(), platform)
                    }
                    other => panic!("unknown operation {other}"),
                };
                let expected = if specific.get("expected").is_some() {
                    &specific["expected"]
                } else {
                    &case["expected"]
                };
                let outcome = outcome(result);
                // A rejection is matched by its stable code and field; the
                // message is English prose that may be reworded.
                if expected["status"] == "rejected" {
                    assert_eq!(outcome["status"], "rejected", "{name} on {key}: {outcome}");
                    assert_eq!(
                        outcome["error"]["code"], expected["code"],
                        "{name} on {key}: {outcome}"
                    );
                    assert_eq!(
                        outcome["error"]["field"], expected["field"],
                        "{name} on {key}: {outcome}"
                    );
                } else {
                    assert_eq!(&outcome, expected, "{name} on {key}");
                }
            }
        }
    }

    fn probe(path_list: bool, append: Option<&str>) -> DirectoryProbe {
        DirectoryProbe {
            arguments: vec!["env".to_string()],
            path_list,
            append: append.map(str::to_string),
        }
    }

    #[test]
    fn a_probe_reads_the_first_path_list_entry_and_appends() {
        assert_eq!(
            probe_directory(
                &probe(true, Some("bin")),
                ":/home/me/go:/opt/go\n",
                Platform::Unix
            ),
            Some("/home/me/go/bin".to_string())
        );
        assert_eq!(
            probe_directory(
                &probe(true, Some("bin")),
                "C:\\Users\\me\\go;D:\\go\r\n",
                Platform::Windows
            ),
            Some("C:\\Users\\me\\go\\bin".to_string())
        );
    }

    #[test]
    fn a_probe_without_an_absolute_directory_names_none() {
        for output in ["", "  \n", "relative/bin", "/a\n/b"] {
            assert_eq!(
                probe_directory(&probe(false, None), output, Platform::Unix),
                None,
                "{output:?}"
            );
        }
        assert_eq!(
            probe_directory(&probe(false, None), "/home/me/bin\n", Platform::Unix),
            Some("/home/me/bin".to_string())
        );
        assert_eq!(
            probe_directory(&probe(false, None), "/home/me/bin", Platform::Windows),
            None
        );
    }
}
