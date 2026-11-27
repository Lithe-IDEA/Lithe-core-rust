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

/// Template variable of the user's home directory in a session root.
const USER_HOME_VARIABLE: &str = "${userHome}";

/// One agent a plugin drives.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDeclaration {
    pub id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<UIIcon>,
    /// One of the plugin's `requiresTools`: the only executable any form of
    /// this agent runs.
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp: Option<ArgumentsDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<ArgumentsDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_sources: Vec<SessionSourceDeclaration>,
}

/// The backend function that answers a native session's command line.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeDeclaration {
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_milliseconds: Option<u64>,
}

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
    pub arguments: Vec<String>,
}

/// How the user updates the agent's CLI: arguments for its own tool, such
/// as `update`. A CLI without a self-update command declares none.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDeclaration {
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
    pub id: String,
    /// Root candidates. The first one whose variables all resolve is used.
    pub roots: Vec<String>,
    /// Root-relative glob patterns of the files to read.
    pub include: Vec<String>,
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
    /// Whether Core starts the backend for this agent: a native session or a
    /// session source is served by backend functions. A terminal or ACP agent
    /// alone runs only its tool.
    pub fn needs_backend(&self) -> bool {
        self.native.is_some() || !self.session_sources.is_empty()
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
        if (agent.native.is_some() || !agent.session_sources.is_empty()) && !context.has_backend {
            return Err(invalid(
                field,
                "Native sessions and session sources are served by the plugin backend.",
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

/// The environment variables the session roots of `agent` read, so the host
/// passes only those to [`resolve_root`].
pub fn root_variables(agent: &AgentDeclaration) -> BTreeSet<String> {
    agent
        .session_sources
        .iter()
        .flat_map(|source| source.roots.iter())
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
    if let Some(icon) = &agent.icon {
        entry["icon"] = json!(icon);
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
