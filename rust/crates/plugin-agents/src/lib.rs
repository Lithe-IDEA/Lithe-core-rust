//! `agents[]` of a plugin manifest (plan API-45): the agent command-line
//! tools a plugin drives, how Core starts them, and where they keep their
//! local session files.
//!
//! Core knows no agent. Everything agent-specific comes from the plugin:
//!
//! - `tool` names one of the plugin's required tools. The host discovers it
//!   and probes its version like any other tool, so the command is always the
//!   official CLI the user installed, never a path the plugin chose.
//! - `native.function` is a backend function that answers the command line
//!   of a native session. The answer can only run `tool` and carries no
//!   environment, so a plugin cannot redirect the CLI's model traffic.
//! - `acp.arguments` runs `tool` as an Agent Client Protocol server, and
//!   `terminal.arguments` runs its interactive interface in a terminal.
//! - `update.arguments` runs `tool` to update itself, after the user
//!   confirms.
//! - `sessionSources` names the directories of the CLI's session files and
//!   the backend functions that parse and display them. Core reads the files
//!   only after the user consents, per agent.
//! - `configuration` names where the CLI keeps its instructions, skills and
//!   the files that hold its MCP servers and hooks, and the backend functions
//!   that read and edit those files. Core writes them only when the user
//!   applies a change, and only through a staged copy it checks first.
//!
//! Note: the decision is recorded in .agents/notes/proposed/architecture/2026-11-25-agent-native-cli-and-local-session-records.md

use globset::Glob;
use lithe_plugin_model::{
    invalid, is_environment_name, is_function_id, is_identifier, PluginRejection, UIIcon,
    Validation,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Most agents one plugin declares.
pub const MAX_AGENTS: usize = 8;

/// Most session sources one agent declares.
pub const MAX_SESSION_SOURCES: usize = 4;

/// Most root candidates of one session source.
const MAX_ROOTS: usize = 4;

/// Most include patterns of one session source.
const MAX_INCLUDES: usize = 16;

/// Longest agent or session-source ID, in bytes.
const MAX_ID_BYTES: usize = 64;

/// Longest display name, in characters.
const MAX_DISPLAY_NAME_CHARS: usize = 64;

/// Most fixed arguments of `acp`, `terminal` or `update`.
const MAX_DECLARED_ARGUMENTS: usize = 64;

/// Longest fixed argument, in bytes.
const MAX_DECLARED_ARGUMENT_BYTES: usize = 4096;

/// Most arguments a launch function may answer.
pub const MAX_LAUNCH_ARGUMENTS: usize = 1024;

/// Longest argument a launch function may answer, in bytes.
pub const MAX_LAUNCH_ARGUMENT_BYTES: usize = 32 * 1024;

/// Default and bounds of `native.timeoutMilliseconds`.
pub const DEFAULT_LAUNCH_TIMEOUT_MILLISECONDS: u64 = 15_000;
const MIN_LAUNCH_TIMEOUT_MILLISECONDS: u64 = 1_000;
const MAX_LAUNCH_TIMEOUT_MILLISECONDS: u64 = 120_000;

/// Prefix of the activation event of one agent, `onAgent:<id>`.
pub const ON_AGENT_PREFIX: &str = "onAgent:";

/// Manifest permission of `agents[]`: Core may run each agent's tool in the
/// forms it declares, after the user starts a session.
pub const PERMISSION_AGENTS_RUN: &str = "agents.run";

/// Manifest permission of `sessionSources`: after the user consents per
/// agent, Core reads the listed session files and the backend parses them.
pub const PERMISSION_AGENTS_SESSIONS: &str = "agents.sessions.read";

/// Manifest permission of `configuration`: when the user applies a change
/// from Lithe's agent library, Core edits the declared configuration files,
/// writes the instruction file and links skills into the skills directory.
pub const PERMISSION_AGENTS_CONFIGURATION: &str = "agents.configuration";

/// Most configuration files one agent declares.
pub const MAX_CONFIGURATION_FILES: usize = 8;

/// Template variable of the user's home directory in a session root.
const USER_HOME_VARIABLE: &str = "${userHome}";

/// One agent a plugin drives.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDeclaration {
    /// The agent's ID, unique within the plugin.
    pub id: String,
    /// The name Lithe shows for the agent.
    pub display_name: String,
    /// The icon Lithe shows for the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<UIIcon>,
    /// One of the plugin's `requiresTools`: the only executable any form of
    /// this agent runs.
    pub tool: String,
    /// The native form: the backend speaks the CLI's own protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeDeclaration>,
    /// The ACP form: the arguments that start the CLI's ACP server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp: Option<ArgumentsDeclaration>,
    /// The terminal form: the arguments that start the CLI's own interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<ArgumentsDeclaration>,
    /// How the user updates the CLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateDeclaration>,
    /// Where the CLI keeps its session files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_sources: Vec<SessionSourceDeclaration>,
    /// Where the CLI keeps the configuration Lithe's agent library manages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationDeclaration>,
}

/// The configuration of an agent that Lithe's agent library reads and
/// changes: an instruction file and a skills directory Core handles itself,
/// and files in the CLI's own formats the backend reads and edits.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationDeclaration {
    /// The Markdown file the CLI reads as the user's own instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<RootsDeclaration>,
    /// The directory of the CLI's user skills, one directory with a
    /// `SKILL.md` per skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<RootsDeclaration>,
    /// The files that hold the CLI's MCP servers and hooks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ConfigurationFileDeclaration>,
    /// Backend function that reports the MCP servers and hooks of `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<String>,
    /// Backend function that applies changes to a staged copy of one file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<String>,
}

/// A path given as root candidates; the first that resolves is used.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootsDeclaration {
    /// Path candidates, written like session roots.
    pub roots: Vec<String>,
}

/// One configuration file in the CLI's own format.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationFileDeclaration {
    /// The file's ID, unique within the agent.
    pub id: String,
    /// Path candidates, written like session roots.
    pub roots: Vec<String>,
}

/// The backend function that answers a native session's command line.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeDeclaration {
    /// The backend function that answers the launch arguments.
    pub function: String,
    /// How long Core waits for that function; see
    /// [`NativeDeclaration::timeout_milliseconds`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_milliseconds: Option<u64>,
    /// The image types the CLI accepts with a prompt, from
    /// [`IMAGE_TYPES`]. Empty means the agent takes text only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_types: Vec<String>,
}

/// The image types a prompt may carry. Core reads each image's first bytes
/// to tell its type, so only types with a fixed signature are listed.
pub const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

impl NativeDeclaration {
    /// How long Core waits for the launch function.
    pub fn timeout_milliseconds(&self) -> u64 {
        self.timeout_milliseconds
            .unwrap_or(DEFAULT_LAUNCH_TIMEOUT_MILLISECONDS)
    }
}

/// Fixed arguments Core passes to the agent's tool.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArgumentsDeclaration {
    /// The arguments, after the tool's path.
    pub arguments: Vec<String>,
}

/// How the user updates the agent's CLI: arguments for its own tool, such
/// as `update`. A CLI without a self-update command declares none.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDeclaration {
    /// The arguments of the update command, after the tool's path.
    pub arguments: Vec<String>,
}

/// The format of a session source's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionFormat {
    /// One JSON value per line.
    JsonLines,
}

/// One directory of an agent's session files.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSourceDeclaration {
    /// The source's ID, unique within the agent.
    pub id: String,
    /// Root candidates. The first one whose variables all resolve is used.
    pub roots: Vec<String>,
    /// Root-relative glob patterns of the files to read.
    pub include: Vec<String>,
    /// The format of the files.
    pub format: SessionFormat,
    /// Backend function that parses new lines of one file.
    pub scan: String,
    /// Backend function that turns a byte range of one file into messages.
    pub read: String,
}

/// What the rest of the manifest offers the agents.
pub struct DeclarationContext<'a> {
    /// The plugin's `requiresTools`.
    pub required_tools: &'a BTreeSet<String>,
    /// Whether the plugin declares a backend, the only process that serves
    /// launch, scan and read functions.
    pub has_backend: bool,
}

impl AgentDeclaration {
    /// Whether Core starts the backend for this agent: a native session, a
    /// session source or configuration files are served by backend functions.
    /// A terminal or ACP agent alone runs only its tool.
    pub fn needs_backend(&self) -> bool {
        self.native.is_some()
            || !self.session_sources.is_empty()
            || self
                .configuration
                .as_ref()
                .is_some_and(|configuration| configuration.read.is_some())
    }
}

/// The activation event that starts the backend for `agent`, declared by
/// every agent that [needs the backend](AgentDeclaration::needs_backend).
pub fn activation_event(agent: &AgentDeclaration) -> String {
    format!("{ON_AGENT_PREFIX}{}", agent.id)
}

/// Whether any agent needs the backend.
pub fn uses_backend(agents: &[AgentDeclaration]) -> bool {
    agents.iter().any(AgentDeclaration::needs_backend)
}

/// Whether any agent declares configuration, which needs
/// `agents.configuration`.
pub fn has_configuration(agents: &[AgentDeclaration]) -> bool {
    agents.iter().any(|agent| agent.configuration.is_some())
}

/// Whether any agent has a native session, which needs `agents.native`.
pub fn has_native(agents: &[AgentDeclaration]) -> bool {
    agents.iter().any(|agent| agent.native.is_some())
}

/// Checks the section and sorts it by agent ID.
pub fn validate_declarations(
    agents: &mut [AgentDeclaration],
    context: &DeclarationContext<'_>,
) -> Validation<()> {
    if agents.len() > MAX_AGENTS {
        return Err(invalid(
            "agents",
            format!("A plugin declares at most {MAX_AGENTS} agents."),
        ));
    }
    let mut ids = BTreeSet::new();
    for (index, agent) in agents.iter().enumerate() {
        let field = format!("agents[{index}]");
        if !is_short_id(&agent.id) {
            return Err(invalid(
                format!("{field}.id"),
                "An agent ID uses lowercase letters, digits and hyphens.",
            ));
        }
        if !ids.insert(agent.id.as_str()) {
            return Err(invalid(
                format!("{field}.id"),
                "An agent is declared more than once.",
            ));
        }
        check_display_name(&agent.display_name, &format!("{field}.displayName"))?;
        if let Some(icon) = &agent.icon {
            icon.validate_at(&format!("{field}.icon"))?;
        }
        if !context.required_tools.contains(&agent.tool) {
            return Err(invalid(
                format!("{field}.tool"),
                "An agent's tool must be one of the plugin's requiresTools.",
            ));
        }
        if agent.native.is_none() && agent.acp.is_none() && agent.terminal.is_none() {
            return Err(invalid(
                field.clone(),
                "An agent declares at least one of native, acp and terminal.",
            ));
        }
        if let Some(native) = &agent.native {
            if !is_function_id(&native.function) {
                return Err(invalid(
                    format!("{field}.native.function"),
                    "A launch function ID is dot-separated lowerCamelCase segments.",
                ));
            }
            if let Some(timeout) = native.timeout_milliseconds {
                if !(MIN_LAUNCH_TIMEOUT_MILLISECONDS..=MAX_LAUNCH_TIMEOUT_MILLISECONDS)
                    .contains(&timeout)
                {
                    return Err(invalid(
                        format!("{field}.native.timeoutMilliseconds"),
                        "A launch timeout is between 1000 and 120000 milliseconds.",
                    ));
                }
            }
            let mut types = BTreeSet::new();
            for kind in &native.image_types {
                if !IMAGE_TYPES.contains(&kind.as_str()) || !types.insert(kind.as_str()) {
                    return Err(invalid(
                        format!("{field}.native.imageTypes"),
                        "Image types are distinct values of image/png, image/jpeg, image/gif and image/webp.",
                    ));
                }
            }
        }
        if let Some(acp) = &agent.acp {
            check_arguments(&acp.arguments, &format!("{field}.acp.arguments"))?;
        }
        if let Some(terminal) = &agent.terminal {
            check_arguments(&terminal.arguments, &format!("{field}.terminal.arguments"))?;
        }
        if let Some(update) = &agent.update {
            check_arguments(&update.arguments, &format!("{field}.update.arguments"))?;
            if update.arguments.is_empty() {
                return Err(invalid(
                    format!("{field}.update.arguments"),
                    "An update runs the tool with at least one argument.",
                ));
            }
        }
        check_session_sources(&agent.session_sources, &field)?;
        if let Some(configuration) = &agent.configuration {
            check_configuration(configuration, &format!("{field}.configuration"))?;
        }
        if agent.needs_backend() && !context.has_backend {
            return Err(invalid(
                field,
                "Native sessions, session sources and configuration files are served by the plugin backend.",
            ));
        }
    }
    agents.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn is_short_id(value: &str) -> bool {
    value.len() <= MAX_ID_BYTES
        && is_identifier(value)
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn check_display_name(name: &str, field: &str) -> Validation<()> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() != name.len()
        || name.chars().count() > MAX_DISPLAY_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(invalid(
            field,
            "A display name is 1 to 64 characters without surrounding spaces or control characters.",
        ));
    }
    Ok(())
}

fn check_arguments(arguments: &[String], field: &str) -> Validation<()> {
    if arguments.len() > MAX_DECLARED_ARGUMENTS
        || arguments
            .iter()
            .any(|argument| argument.len() > MAX_DECLARED_ARGUMENT_BYTES || argument.contains('\0'))
    {
        return Err(invalid(
            field,
            "At most 64 arguments of at most 4 KiB each, without NUL, are allowed.",
        ));
    }
    Ok(())
}

fn check_session_sources(
    sources: &[SessionSourceDeclaration],
    agent_field: &str,
) -> Validation<()> {
    if sources.len() > MAX_SESSION_SOURCES {
        return Err(invalid(
            format!("{agent_field}.sessionSources"),
            format!("An agent declares at most {MAX_SESSION_SOURCES} session sources."),
        ));
    }
    let mut ids = BTreeSet::new();
    for (index, source) in sources.iter().enumerate() {
        let field = format!("{agent_field}.sessionSources[{index}]");
        if !is_short_id(&source.id) {
            return Err(invalid(
                format!("{field}.id"),
                "A session source ID uses lowercase letters, digits and hyphens.",
            ));
        }
        if !ids.insert(source.id.as_str()) {
            return Err(invalid(
                format!("{field}.id"),
                "A session source is declared more than once.",
            ));
        }
        if source.roots.is_empty() || source.roots.len() > MAX_ROOTS {
            return Err(invalid(
                format!("{field}.roots"),
                format!("A session source has 1 to {MAX_ROOTS} root candidates."),
            ));
        }
        for (root_index, root) in source.roots.iter().enumerate() {
            parse_root(root).ok_or_else(|| {
                invalid(
                    format!("{field}.roots[{root_index}]"),
                    "A root starts with ${userHome} or ${env:NAME}, followed by relative path segments without `..`.",
                )
            })?;
        }
        if source.include.is_empty() || source.include.len() > MAX_INCLUDES {
            return Err(invalid(
                format!("{field}.include"),
                format!("A session source has 1 to {MAX_INCLUDES} include patterns."),
            ));
        }
        for (pattern_index, pattern) in source.include.iter().enumerate() {
            let relative = !pattern.is_empty()
                && !pattern.starts_with('/')
                && !pattern.contains('\\')
                && !pattern.contains('\0')
                && !pattern.split('/').any(|segment| segment == "..");
            if !relative || Glob::new(pattern).is_err() {
                return Err(invalid(
                    format!("{field}.include[{pattern_index}]"),
                    "An include pattern is a root-relative glob without `..`.",
                ));
            }
        }
        for (name, function) in [("scan", &source.scan), ("read", &source.read)] {
            if !is_function_id(function) {
                return Err(invalid(
                    format!("{field}.{name}"),
                    "A session function ID is dot-separated lowerCamelCase segments.",
                ));
            }
        }
    }
    Ok(())
}

fn check_roots(roots: &[String], field: &str) -> Validation<()> {
    if roots.is_empty() || roots.len() > MAX_ROOTS {
        return Err(invalid(
            field,
            format!("A path has 1 to {MAX_ROOTS} candidates."),
        ));
    }
    for (index, root) in roots.iter().enumerate() {
        let named = parse_root(root).is_some_and(|(_, segments)| !segments.is_empty());
        if !named {
            return Err(invalid(
                format!("{field}[{index}]"),
                "A configuration path starts with ${userHome} or ${env:NAME}, followed by at least one relative path segment without `..`.",
            ));
        }
    }
    Ok(())
}

fn check_configuration(configuration: &ConfigurationDeclaration, field: &str) -> Validation<()> {
    if configuration.instructions.is_none()
        && configuration.skills.is_none()
        && configuration.files.is_empty()
    {
        return Err(invalid(
            field,
            "Configuration declares at least one of instructions, skills and files.",
        ));
    }
    if let Some(instructions) = &configuration.instructions {
        check_roots(&instructions.roots, &format!("{field}.instructions.roots"))?;
    }
    if let Some(skills) = &configuration.skills {
        check_roots(&skills.roots, &format!("{field}.skills.roots"))?;
    }
    if configuration.files.len() > MAX_CONFIGURATION_FILES {
        return Err(invalid(
            format!("{field}.files"),
            format!("An agent declares at most {MAX_CONFIGURATION_FILES} configuration files."),
        ));
    }
    let mut ids = BTreeSet::new();
    for (index, file) in configuration.files.iter().enumerate() {
        let file_field = format!("{field}.files[{index}]");
        if !is_short_id(&file.id) {
            return Err(invalid(
                format!("{file_field}.id"),
                "A configuration file ID uses lowercase letters, digits and hyphens.",
            ));
        }
        if !ids.insert(file.id.as_str()) {
            return Err(invalid(
                format!("{file_field}.id"),
                "A configuration file is declared more than once.",
            ));
        }
        check_roots(&file.roots, &format!("{file_field}.roots"))?;
    }
    match (configuration.files.is_empty(), &configuration.read) {
        (false, None) => {
            return Err(invalid(
                format!("{field}.read"),
                "Configuration files need a read function.",
            ))
        }
        (true, Some(_)) => {
            return Err(invalid(
                format!("{field}.read"),
                "A read function reads the declared configuration files.",
            ))
        }
        _ => {}
    }
    if configuration.edit.is_some() && configuration.read.is_none() {
        return Err(invalid(
            format!("{field}.edit"),
            "An edit function needs configuration files and a read function.",
        ));
    }
    for (name, function) in [("read", &configuration.read), ("edit", &configuration.edit)] {
        if function
            .as_deref()
            .is_some_and(|function| !is_function_id(function))
        {
            return Err(invalid(
                format!("{field}.{name}"),
                "A configuration function ID is dot-separated lowerCamelCase segments.",
            ));
        }
    }
    Ok(())
}

/// The first part of a root: where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RootBase {
    UserHome,
    Environment(String),
}

/// Splits a root template into its base variable and relative segments.
fn parse_root(root: &str) -> Option<(RootBase, Vec<&str>)> {
    let (base, rest) = if let Some(rest) = root.strip_prefix(USER_HOME_VARIABLE) {
        (RootBase::UserHome, rest)
    } else {
        let inner = root.strip_prefix("${env:")?;
        let (name, rest) = inner.split_once('}')?;
        if !is_environment_name(name) {
            return None;
        }
        (RootBase::Environment(name.to_string()), rest)
    };
    if rest.contains('$') || rest.contains('\\') || rest.contains('\0') {
        return None;
    }
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    let segments: Vec<&str> = rest
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments
        .iter()
        .any(|segment| *segment == ".." || *segment == ".")
    {
        return None;
    }
    Some((base, segments))
}

/// The environment variables the session and configuration roots of `agent`
/// read, so the host passes only those to [`resolve_root`].
pub fn root_variables(agent: &AgentDeclaration) -> BTreeSet<String> {
    let configuration = agent.configuration.iter().flat_map(|configuration| {
        configuration
            .instructions
            .iter()
            .chain(configuration.skills.iter())
            .flat_map(|roots| roots.roots.iter())
            .chain(
                configuration
                    .files
                    .iter()
                    .flat_map(|file| file.roots.iter()),
            )
    });
    agent
        .session_sources
        .iter()
        .flat_map(|source| source.roots.iter())
        .chain(configuration)
        .filter_map(|root| match parse_root(root)?.0 {
            RootBase::Environment(name) => Some(name),
            RootBase::UserHome => None,
        })
        .collect()
}

/// Resolves the first root candidate whose variable has a value. An
/// environment variable that is unset or empty skips its candidate. `None`
/// when no candidate resolves.
pub fn resolve_root(
    roots: &[String],
    user_home: &str,
    environment: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let (base, segments) = parse_root(root)?;
        let mut path = match base {
            RootBase::UserHome => PathBuf::from(user_home),
            RootBase::Environment(name) => {
                let value = environment.get(&name)?.trim();
                if value.is_empty() {
                    return None;
                }
                PathBuf::from(value)
            }
        };
        if !path.is_absolute() {
            return None;
        }
        for segment in segments {
            path.push(segment);
        }
        Some(path)
    })
}

/// A checked launch answer of a native session: the arguments for the
/// agent's tool. The tool itself is not in the answer's control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLaunch {
    /// The arguments, after the tool's path.
    pub arguments: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LaunchAnswer {
    executable: LaunchExecutable,
    #[serde(default)]
    arguments: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum LaunchExecutable {
    Tool { tool: String },
}

/// Checks what a native launch function answered for `agent`. The answer
/// may name only the agent's own tool and carries no environment.
pub fn decode_native_launch(agent: &AgentDeclaration, answer: Value) -> Validation<NativeLaunch> {
    if let Some(object) = answer.as_object() {
        if object.contains_key("environment") || object.contains_key("env") {
            return Err(PluginRejection::new(
                "plugin.agents.launchEnvironment",
                "An agent launch cannot set environment variables.",
            ));
        }
    }
    let answer: LaunchAnswer = serde_json::from_value(answer).map_err(|error| {
        PluginRejection::new(
            "plugin.agents.invalidLaunch",
            format!("The agent launch answer is malformed: {error}"),
        )
    })?;
    let LaunchExecutable::Tool { tool } = answer.executable;
    if tool != agent.tool {
        return Err(PluginRejection::new(
            "plugin.agents.undeclaredTool",
            "An agent launch runs only the agent's own tool.",
        ));
    }
    if answer.arguments.len() > MAX_LAUNCH_ARGUMENTS
        || answer
            .arguments
            .iter()
            .any(|argument| argument.len() > MAX_LAUNCH_ARGUMENT_BYTES || argument.contains('\0'))
    {
        return Err(PluginRejection::new(
            "plugin.agents.invalidLaunch",
            "At most 1024 arguments of at most 32 KiB each, without NUL, are allowed.",
        ));
    }
    Ok(NativeLaunch {
        arguments: answer.arguments,
    })
}

/// The params Core passes to a native launch function.
pub fn launch_params(agent_id: &str, cwd: &str, resume: Option<&str>) -> Value {
    let mut params = json!({ "agentID": agent_id, "cwd": cwd });
    if let Some(resume) = resume {
        params["resume"] = json!(resume);
    }
    params
}

/// What the host shows of one agent: its forms and how it updates. The
/// session functions stay inside Core.
pub fn catalog_entry(plugin_id: &str, agent: &AgentDeclaration) -> Value {
    let mut entry = json!({
        "pluginID": plugin_id,
        "agentID": agent.id,
        "displayName": agent.display_name,
        "tool": agent.tool,
        "modes": modes(agent),
        "forms": forms(agent),
        "readsSessions": !agent.session_sources.is_empty(),
    });
    if let Some(configuration) = &agent.configuration {
        entry["configuration"] = json!({
            "instructions": configuration.instructions.is_some(),
            "skills": configuration.skills.is_some(),
            "files": !configuration.files.is_empty(),
            "editable": configuration.edit.is_some(),
        });
    }
    if let Some(icon) = &agent.icon {
        entry["icon"] = json!(icon);
    }
    if let Some(native) = agent.native.as_ref().filter(|n| !n.image_types.is_empty()) {
        entry["imageTypes"] = json!(native.image_types);
    }
    if let Some(terminal) = &agent.terminal {
        entry["terminalArguments"] = json!(terminal.arguments);
    }
    if let Some(update) = &agent.update {
        entry["update"] = json!(update);
    }
    let variables = root_variables(agent);
    if !variables.is_empty() {
        entry["sessionRootVariables"] = json!(variables);
    }
    entry
}

/// The programs an agent runs, for the install review: each form with its
/// fixed arguments, the update command, and where its session records are
/// read. A native launch's arguments come from the backend at start, so the
/// entry names the function that decides them.
pub fn programs(agent: &AgentDeclaration) -> Vec<Value> {
    let entry = |form: &str| {
        json!({
            "kind": "agent",
            "agentID": agent.id,
            "tool": agent.tool,
            "form": form,
        })
    };
    let mut programs = Vec::new();
    if let Some(native) = &agent.native {
        let mut program = entry("native");
        program["launchFunction"] = json!(native.function);
        programs.push(program);
    }
    for (form, arguments) in [
        ("acp", agent.acp.as_ref().map(|acp| &acp.arguments)),
        ("terminal", agent.terminal.as_ref().map(|t| &t.arguments)),
        ("update", agent.update.as_ref().map(|u| &u.arguments)),
    ] {
        if let Some(arguments) = arguments {
            let mut program = entry(form);
            program["arguments"] = json!(arguments);
            programs.push(program);
        }
    }
    for source in &agent.session_sources {
        programs.push(json!({
            "kind": "agentSessions",
            "agentID": agent.id,
            "tool": agent.tool,
            "source": source.id,
            "roots": source.roots,
            "include": source.include,
        }));
    }
    if let Some(configuration) = &agent.configuration {
        let mut program = json!({
            "kind": "agentConfiguration",
            "agentID": agent.id,
            "tool": agent.tool,
            "files": configuration
                .files
                .iter()
                .map(|file| json!({ "id": file.id, "roots": file.roots }))
                .collect::<Vec<_>>(),
            "editable": configuration.edit.is_some(),
        });
        if let Some(instructions) = &configuration.instructions {
            program["instructions"] = json!(instructions.roots);
        }
        if let Some(skills) = &configuration.skills {
            program["skills"] = json!(skills.roots);
        }
        programs.push(program);
    }
    programs
}

/// The connection modes in the order Core prefers them.
fn modes(agent: &AgentDeclaration) -> Vec<&'static str> {
    let mut modes = Vec::new();
    if agent.native.is_some() {
        modes.push("native");
    }
    if agent.acp.is_some() {
        modes.push("acp");
    }
    modes
}

fn forms(agent: &AgentDeclaration) -> Vec<&'static str> {
    let mut forms = Vec::new();
    if agent.native.is_some() || agent.acp.is_some() {
        forms.push("conversation");
    }
    if agent.terminal.is_some() {
        forms.push("terminal");
    }
    forms
}

#[cfg(test)]
mod tests;
