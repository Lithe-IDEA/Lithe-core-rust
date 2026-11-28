//! Lithe's agent library (plan API-45): MCP servers, hooks, skills and
//! instruction sets the user keeps once and turns on for each agent.
//!
//! The library is Lithe's own data, in a directory of the platform's
//! application data that the host names with `agentLibrary.configure`:
//! `library.json` holds the servers, hooks and instruction sets,
//! `skills/<name>/` holds one directory per skill, `state.json` records what
//! Core last wrote to each agent's instruction file, and `staging/` holds the
//! copies plugins edit.
//!
//! What an agent has is read from the agent's own files every time and never
//! kept. An agent plugin declares where they are (`configuration` in the
//! manifest). Core reads the instruction file and the skills directory
//! itself; the plugin backend reads the MCP servers and hooks from the CLI's
//! own formats. A library item is on for an agent when the agent's files hold
//! an identical item.
//!
//! Core changes an agent's files only for a user request, only for a plugin
//! granted `agents.configuration`, and only in these ways:
//!
//! - MCP servers and hooks: Core copies the file into `staging/`, the plugin
//!   edits only the copy, the plugin reads the copy back to show the change
//!   took effect, and Core replaces the file with the copy if the file has
//!   not changed since it was copied. The replacement is a new file, with the
//!   old file's permissions, renamed over it.
//! - Skills: a library skill is turned on as a symbolic link in the agent's
//!   skills directory; turning it off removes only a link into the library.
//! - Instructions: Core writes the active instruction set and the agent's
//!   extra text, and refuses to replace text it did not write unless the user
//!   confirms.
//!
//! Logs carry plugin and agent IDs, categories, counts and codes, never
//! names, commands, paths or text.

use crate::backend::BackendLink;
use crate::catalog::CatalogAgent;
use crate::{log, Runtime};
use lithe_plugin_agents::{resolve_root, ConfigurationDeclaration};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_request_context::Source;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

const LIBRARY_FILE: &str = "library.json";
const STATE_FILE: &str = "state.json";
const SKILLS_DIRECTORY: &str = "skills";
const STAGING_DIRECTORY: &str = "staging";
const SKILL_FILE: &str = "SKILL.md";
/// The revision of a library that was never saved.
const EMPTY_REVISION: &str = "empty";

const READ_FORMAT: &str = "litheAgentConfiguration";
const EDIT_FORMAT: &str = "litheAgentConfigurationEdit";
const FUNCTION_TIMEOUT: Duration = Duration::from_secs(10);

const MAX_SERVERS: usize = 256;
const MAX_HOOKS: usize = 256;
const MAX_HOOK_EVENTS: usize = 64;
const MAX_EVENT_BYTES: usize = 64;
const MAX_SKILLS: usize = 512;
const MAX_NAME_CHARS: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_ARGUMENTS: usize = 64;
const MAX_VARIABLE_NAMES: usize = 64;
const MAX_HOOK_TIMEOUT_SECONDS: u32 = 86_400;
const MAX_SETS: usize = 64;
const MAX_SET_TEXT_BYTES: usize = 256 * 1024;
const MAX_EXTRA_BYTES: usize = 64 * 1024;
const MAX_AGENT_ENTRIES: usize = 256;
const MAX_LIBRARY_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CONFIGURATION_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_INSTRUCTIONS_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SKILL_FILE_BYTES: u64 = 64 * 1024;
const MAX_IMPORT_FILES: usize = 1024;
const MAX_IMPORT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_IMPORT_DEPTH: usize = 16;

#[derive(Debug, Clone)]
struct Config {
    directory: PathBuf,
    user_home: String,
    environment: BTreeMap<String, String>,
}

/// The library directory and the lock that orders changes.
#[derive(Default)]
pub(crate) struct Library {
    config: RwLock<Option<Config>>,
    /// Held while a change checks and replaces a file, so two changes never
    /// interleave. Never held across a backend call.
    writing: Mutex<()>,
}

// ---------------------------------------------------------------------------
// The library's data.

/// How a client reaches an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Transport {
    Stdio,
    Http,
    Sse,
}

/// An MCP server of the library. A server with environment variables or
/// headers is not kept in version 0: their values are secrets Lithe does not
/// store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Server {
    name: String,
    transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

/// A hook command as the agents run it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Hook {
    event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matcher: Option<String>,
    command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<u32>,
}

impl Hook {
    /// Two hooks are the same when they run the same command for the same
    /// event and matcher; an empty matcher is no matcher.
    fn same(&self, event: &str, matcher: Option<&str>, command: &str) -> bool {
        self.event == event
            && self.matcher.as_deref().filter(|m| !m.is_empty())
                == matcher.filter(|m| !m.is_empty())
            && self.command == command
    }
}

/// A hook of the library, with an ID the host chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LibraryHook {
    id: String,
    event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matcher: Option<String>,
    command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<u32>,
}

impl LibraryHook {
    fn hook(&self) -> Hook {
        Hook {
            event: self.event.clone(),
            matcher: self.matcher.clone().filter(|m| !m.is_empty()),
            command: self.command.clone(),
            timeout_seconds: self.timeout_seconds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstructionSet {
    id: String,
    name: String,
    #[serde(default)]
    text: String,
}

/// Whether an agent's instruction file follows the library, and the text
/// only that agent gets after the shared set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentInstructions {
    #[serde(default)]
    enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    extra: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Instructions {
    #[serde(default)]
    sets: Vec<InstructionSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<String>,
    /// By agent key, `<pluginID>/<agentID>`.
    #[serde(default)]
    agents: BTreeMap<String, AgentInstructions>,
}

/// What the user keeps in the library, as the host reads and saves it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LibraryData {
    #[serde(default)]
    mcp_servers: Vec<Server>,
    #[serde(default)]
    hooks: Vec<LibraryHook>,
    #[serde(default)]
    instructions: Instructions,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LibraryFile {
    version: u32,
    library: LibraryData,
}

/// What Core wrote, kept apart from the library so writing an agent's file
/// never changes the library's revision.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateFile {
    version: u32,
    /// SHA-256 of the text Core last wrote to each agent's instruction file.
    #[serde(default)]
    instructions: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// What an agent plugin reports.

/// Where an item an agent has comes from: the user's own configuration,
/// which the plugin can edit, or something the CLI installed itself, such as
/// a CLI plugin, which Lithe only shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Origin {
    User,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentServer {
    name: String,
    transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    /// Names of the environment variables the server is given; never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    environment_names: Vec<String>,
    /// Names of the HTTP headers the server is sent; never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    header_names: Vec<String>,
    origin: Origin,
    /// What installed it when `origin` is `agent`, such as a CLI plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
}

impl AgentServer {
    /// Whether it is exactly `server`, with nothing Lithe does not keep.
    fn is(&self, server: &Server) -> bool {
        self.environment_names.is_empty()
            && self.header_names.is_empty()
            && self.name == server.name
            && self.transport == server.transport
            && self.command == server.command
            && self.args == server.args
            && self.url == server.url
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentHook {
    event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matcher: Option<String>,
    command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<u32>,
    origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
}

impl AgentHook {
    fn is(&self, hook: &Hook) -> bool {
        hook.same(&self.event, self.matcher.as_deref(), &self.command)
    }
}

/// A skill the CLI installed itself; the skills directory is read by Core.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProvidedSkill {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
}

/// The declared file a plugin changes for each category.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Targets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_servers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hooks: Option<String>,
}

/// The answer of a configuration `read` function.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    format: String,
    version: u32,
    #[serde(default)]
    mcp_servers: Vec<AgentServer>,
    #[serde(default)]
    hooks: Vec<AgentHook>,
    /// The hook events the CLI runs, for choosing one.
    #[serde(default)]
    hook_events: Vec<String>,
    #[serde(default)]
    skills: Vec<ProvidedSkill>,
    #[serde(default)]
    targets: Targets,
}

/// Whether a re-read staged copy shows the change the plugin was asked for.
type ShowsChange = Box<dyn Fn(&Snapshot) -> bool>;

/// The answer of a configuration `edit` function.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditAnswer {
    format: String,
    version: u32,
    changed: bool,
}

// ---------------------------------------------------------------------------
// Checks.

/// A field of `field` is invalid.
type Check = Result<(), String>;

fn check_name(name: &str, field: &str) -> Check {
    let count = name.chars().count();
    if count == 0
        || count > MAX_NAME_CHARS
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(field.to_string());
    }
    Ok(())
}

fn check_text(text: &str, field: &str, required: bool) -> Check {
    if (required && text.trim().is_empty()) || text.len() > MAX_TEXT_BYTES || text.contains('\0') {
        return Err(field.to_string());
    }
    Ok(())
}

fn check_optional_text(text: Option<&str>, field: &str) -> Check {
    text.map_or(Ok(()), |text| check_text(text, field, false))
}

fn check_optional_name(name: Option<&str>, field: &str) -> Check {
    name.map_or(Ok(()), |name| check_name(name, field))
}

fn check_description(text: Option<&str>, field: &str) -> Check {
    match text {
        Some(text) if text.chars().count() > MAX_DESCRIPTION_CHARS || text.contains('\0') => {
            Err(field.to_string())
        }
        _ => Ok(()),
    }
}

/// An ID the host chose: lowercase letters, digits and `-`, 1 to 64 bytes.
fn is_item_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A hook event as the CLIs name them: ASCII letters, digits and `_`.
fn is_event(event: &str) -> bool {
    !event.is_empty()
        && event.len() <= MAX_EVENT_BYTES
        && event.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An environment variable or header name: ASCII letters, digits, `_` and
/// `-`, 1 to 128 bytes.
fn is_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A skill's directory name: ASCII letters, digits, `.`, `_` and `-`, not
/// starting with `.`, 1 to 64 bytes.
fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn check_transport(
    transport: Transport,
    command: Option<&str>,
    args: &[String],
    url: Option<&str>,
    field: &str,
) -> Check {
    check_optional_text(command, &format!("{field}.command"))?;
    check_optional_text(url, &format!("{field}.url"))?;
    if args.len() > MAX_ARGUMENTS {
        return Err(format!("{field}.args"));
    }
    for (index, argument) in args.iter().enumerate() {
        check_text(argument, &format!("{field}.args[{index}]"), false)?;
    }
    match transport {
        Transport::Stdio => {
            if command.is_none_or(|command| command.trim().is_empty()) || url.is_some() {
                return Err(format!("{field}.command"));
            }
        }
        Transport::Http | Transport::Sse => {
            let web =
                url.is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"));
            if !web || command.is_some() || !args.is_empty() {
                return Err(format!("{field}.url"));
            }
        }
    }
    Ok(())
}

fn check_hook_fields(
    event: &str,
    matcher: Option<&str>,
    command: &str,
    timeout: Option<u32>,
    field: &str,
) -> Check {
    if !is_event(event) {
        return Err(format!("{field}.event"));
    }
    check_optional_text(matcher, &format!("{field}.matcher"))?;
    check_text(command, &format!("{field}.command"), true)?;
    if timeout.is_some_and(|seconds| seconds == 0 || seconds > MAX_HOOK_TIMEOUT_SECONDS) {
        return Err(format!("{field}.timeoutSeconds"));
    }
    Ok(())
}

impl Server {
    fn check(&self, field: &str) -> Check {
        check_name(&self.name, &format!("{field}.name"))?;
        check_transport(
            self.transport,
            self.command.as_deref(),
            &self.args,
            self.url.as_deref(),
            field,
        )
    }
}

impl LibraryData {
    fn check(&self) -> Check {
        if self.mcp_servers.len() > MAX_SERVERS {
            return Err("mcpServers".into());
        }
        let mut names = BTreeSet::new();
        for (index, server) in self.mcp_servers.iter().enumerate() {
            let field = format!("mcpServers[{index}]");
            server.check(&field)?;
            if !names.insert(server.name.as_str()) {
                return Err(format!("{field}.name"));
            }
        }
        if self.hooks.len() > MAX_HOOKS {
            return Err("hooks".into());
        }
        let mut ids = BTreeSet::new();
        for (index, hook) in self.hooks.iter().enumerate() {
            let field = format!("hooks[{index}]");
            if !is_item_id(&hook.id) || !ids.insert(hook.id.as_str()) {
                return Err(format!("{field}.id"));
            }
            check_hook_fields(
                &hook.event,
                hook.matcher.as_deref(),
                &hook.command,
                hook.timeout_seconds,
                &field,
            )?;
        }
        let instructions = &self.instructions;
        if instructions.sets.len() > MAX_SETS {
            return Err("instructions.sets".into());
        }
        let mut set_ids = BTreeSet::new();
        for (index, set) in instructions.sets.iter().enumerate() {
            let field = format!("instructions.sets[{index}]");
            if !is_item_id(&set.id) || !set_ids.insert(set.id.as_str()) {
                return Err(format!("{field}.id"));
            }
            check_name(&set.name, &format!("{field}.name"))?;
            if set.text.len() > MAX_SET_TEXT_BYTES || set.text.contains('\0') {
                return Err(format!("{field}.text"));
            }
        }
        if instructions
            .active
            .as_deref()
            .is_some_and(|active| !set_ids.contains(active))
        {
            return Err("instructions.active".into());
        }
        if instructions.agents.len() > MAX_AGENT_ENTRIES {
            return Err("instructions.agents".into());
        }
        for (key, agent) in &instructions.agents {
            let field = format!("instructions.agents[{key}]");
            if key.len() > 512 || !key.contains('/') {
                return Err(field);
            }
            if agent.extra.len() > MAX_EXTRA_BYTES || agent.extra.contains('\0') {
                return Err(format!("{field}.extra"));
            }
        }
        Ok(())
    }

    /// The instruction text of the agent `key`: the active set, then the
    /// agent's extra text, ending with one line break. Empty when neither has
    /// text.
    fn compose(&self, key: &str) -> String {
        let instructions = &self.instructions;
        let shared = instructions
            .active
            .as_deref()
            .and_then(|active| instructions.sets.iter().find(|set| set.id == active))
            .map_or("", |set| set.text.trim_end());
        let extra = instructions
            .agents
            .get(key)
            .map_or("", |agent| agent.extra.trim());
        let mut text = shared.to_string();
        if !extra.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(extra);
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text
    }
}

impl Snapshot {
    fn check(&self, file_ids: &BTreeSet<&str>) -> Check {
        if self.format != READ_FORMAT || self.version != 0 {
            return Err("format".into());
        }
        if self.mcp_servers.len() > MAX_SERVERS {
            return Err("mcpServers".into());
        }
        for (index, server) in self.mcp_servers.iter().enumerate() {
            let field = format!("mcpServers[{index}]");
            check_name(&server.name, &format!("{field}.name"))?;
            check_transport(
                server.transport,
                server.command.as_deref(),
                &server.args,
                server.url.as_deref(),
                &field,
            )?;
            for (list, names) in [
                ("environmentNames", &server.environment_names),
                ("headerNames", &server.header_names),
            ] {
                if names.len() > MAX_VARIABLE_NAMES || !names.iter().all(|n| is_variable_name(n)) {
                    return Err(format!("{field}.{list}"));
                }
            }
            check_optional_name(server.provider.as_deref(), &format!("{field}.provider"))?;
        }
        if self.hooks.len() > MAX_HOOKS {
            return Err("hooks".into());
        }
        for (index, hook) in self.hooks.iter().enumerate() {
            let field = format!("hooks[{index}]");
            check_hook_fields(
                &hook.event,
                hook.matcher.as_deref(),
                &hook.command,
                hook.timeout_seconds,
                &field,
            )?;
            check_optional_name(hook.provider.as_deref(), &format!("{field}.provider"))?;
        }
        if self.hook_events.len() > MAX_HOOK_EVENTS
            || !self.hook_events.iter().all(|event| is_event(event))
            || self.hook_events.iter().collect::<BTreeSet<_>>().len() != self.hook_events.len()
        {
            return Err("hookEvents".into());
        }
        if self.skills.len() > MAX_SKILLS {
            return Err("skills".into());
        }
        for (index, skill) in self.skills.iter().enumerate() {
            let field = format!("skills[{index}]");
            check_name(&skill.name, &format!("{field}.name"))?;
            check_description(
                skill.description.as_deref(),
                &format!("{field}.description"),
            )?;
            check_optional_name(skill.provider.as_deref(), &format!("{field}.provider"))?;
        }
        for (name, target) in [
            ("targets.mcpServers", &self.targets.mcp_servers),
            ("targets.hooks", &self.targets.hooks),
        ] {
            if target.as_deref().is_some_and(|id| !file_ids.contains(id)) {
                return Err(name.into());
            }
        }
        Ok(())
    }

    fn supports_event(&self, event: &str) -> bool {
        self.hook_events.is_empty() || self.hook_events.iter().any(|known| known == event)
    }
}

// ---------------------------------------------------------------------------
// Files.

/// Why a bounded read failed.
#[derive(Debug)]
enum ReadFailure {
    TooLarge,
    Io,
}

/// The bytes of the file at `path`, following links; `None` when nothing is
/// there.
fn read_bounded(path: &Path, max: u64) -> Result<Option<Vec<u8>>, ReadFailure> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ReadFailure::Io),
    };
    let metadata = file.metadata().map_err(|_| ReadFailure::Io)?;
    if !metadata.is_file() {
        return Err(ReadFailure::Io);
    }
    if metadata.len() > max {
        return Err(ReadFailure::TooLarge);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadFailure::Io)?;
    if bytes.len() as u64 > max {
        return Err(ReadFailure::TooLarge);
    }
    Ok(Some(bytes))
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A new name next to `target` for a file that replaces it.
fn sibling_temporary(target: &Path, purpose: &str) -> Option<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = target.parent()?;
    let name = target.file_name()?.to_string_lossy();
    Some(parent.join(format!(
        ".{name}.lithe-{purpose}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )))
}

/// Replaces the file at `target` with `bytes`: a new file next to it, with
/// the old file's permissions (owner-only for a new file), written to disk,
/// then renamed over it. `target` is never a link; callers resolve links
/// first.
fn write_replacing(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = sibling_temporary(target, "write")
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let existing = fs::metadata(target).ok();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mode = existing
            .as_ref()
            .map_or(0o600, |metadata| metadata.permissions().mode() & 0o7777);
        options.mode(mode);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        if let Some(metadata) = &existing {
            // The mode given to `open` is narrowed by the umask.
            fs::set_permissions(&temporary, metadata.permissions())?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The file a path names, through any links. A path whose link leads
/// nowhere is refused, since replacing the link would lose where it pointed.
fn real_file(path: &Path) -> Result<PathBuf, CoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(path).map_err(|_| {
            refusal(
                ErrorCode::EntryNotFound,
                "agentLibrary.brokenLink",
                "A configuration file is a link to nothing.",
            )
        }),
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(_) => Err(unreadable_file()),
    }
}

fn ensure_directory(path: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(path).map_err(|_| write_failed())
}

/// Creates the library directory owner-only.
fn ensure_private_directory(path: &Path) -> Result<(), CoreError> {
    if path.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|_| write_failed())?;
    lithe_platform::fs::restrict_directory_to_owner(path).map_err(|_| write_failed())
}

/// Removes a link, which Windows keeps as a directory when it points at one.
fn remove_link(path: &Path) -> io::Result<()> {
    fs::remove_file(path).or_else(|_| fs::remove_dir(path))
}

/// Whether the link at `link` leads into `directory`.
fn links_into(link: &Path, directory: &Path) -> bool {
    let (Ok(target), Ok(directory)) = (fs::canonicalize(link), fs::canonicalize(directory)) else {
        return false;
    };
    target.starts_with(directory)
}

// ---------------------------------------------------------------------------
// Skills.

/// A skill's `name` and `description` from the YAML front matter of its
/// `SKILL.md`. Only plain, quoted and block scalars are read.
fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    /// A block scalar being read: its key, whether it folds its lines, and
    /// the lines so far.
    type Block = (&'static str, bool, Vec<String>);
    fn finish(values: &mut BTreeMap<&'static str, String>, block: Option<Block>) {
        if let Some((key, folded, parts)) = block {
            let parts: Vec<&str> = parts
                .iter()
                .map(String::as_str)
                .filter(|part| !folded || !part.is_empty())
                .collect();
            let text = parts.join(if folded { " " } else { "\n" });
            values.insert(key, text.trim().to_string());
        }
    }

    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None);
    }
    let mut values: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut block: Option<Block> = None;
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if let Some((_, _, parts)) = block.as_mut() {
            if indented || line.trim().is_empty() {
                parts.push(line.trim().to_string());
                continue;
            }
            finish(&mut values, block.take());
        }
        if indented {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key: &'static str = match key.trim() {
            "name" => "name",
            "description" => "description",
            _ => continue,
        };
        let value = value.trim();
        if value.starts_with('|') || value.starts_with('>') {
            block = Some((key, value.starts_with('>'), Vec::new()));
        } else {
            values.insert(key, unquote(value));
        }
    }
    finish(&mut values, block.take());
    let mut take = |key| values.remove(key).filter(|value| !value.is_empty());
    let name = take("name");
    let description =
        take("description").map(|text| text.chars().take(MAX_DESCRIPTION_CHARS).collect());
    (name, description)
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if first == last && (first == b'"' || first == b'\'') {
            let inner = &value[1..value.len() - 1];
            return if first == b'"' {
                inner.replace("\\\"", "\"")
            } else {
                inner.replace("''", "'")
            };
        }
    }
    value.to_string()
}

/// The description in the `SKILL.md` of `directory`, if it has one; `None`
/// as the outer value when there is no readable `SKILL.md`.
fn skill_description(directory: &Path) -> Option<Option<String>> {
    let bytes = match read_bounded(&directory.join(SKILL_FILE), MAX_SKILL_FILE_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) | Err(ReadFailure::Io) => return None,
        // A skill too large to read for its description is still a skill.
        Err(ReadFailure::TooLarge) => return Some(None),
    };
    Some(front_matter(&String::from_utf8_lossy(&bytes)).1)
}

/// The skills of the library, by name.
fn library_skills(directory: &Path) -> Vec<(String, Option<String>)> {
    let Ok(entries) = fs::read_dir(directory.join(SKILLS_DIRECTORY)) else {
        return Vec::new();
    };
    let mut skills: Vec<(String, Option<String>)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !is_skill_name(&name) {
                return None;
            }
            Some((name, skill_description(&entry.path())?))
        })
        .take(MAX_SKILLS)
        .collect();
    skills.sort_by(|left, right| left.0.cmp(&right.0));
    skills
}

/// What one entry of an agent's skills directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillKind {
    /// A link into the library.
    Library,
    /// A directory of the agent's own.
    Own,
    /// A link somewhere else.
    Link,
}

impl SkillKind {
    fn token(self) -> &'static str {
        match self {
            SkillKind::Library => "library",
            SkillKind::Own => "own",
            SkillKind::Link => "link",
        }
    }
}

struct AgentSkill {
    name: String,
    description: Option<String>,
    kind: SkillKind,
}

/// The skills in an agent's skills directory; empty when it does not exist.
fn agent_skills(root: &Path, library: &Path) -> Result<Vec<AgentSkill>, ReadFailure> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(ReadFailure::Io),
    };
    let library_skills = library.join(SKILLS_DIRECTORY);
    let mut skills = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || check_name(&name, "").is_err() {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let kind = if kind.is_symlink() {
            if links_into(&path, &library_skills) {
                SkillKind::Library
            } else {
                SkillKind::Link
            }
        } else if kind.is_dir() {
            SkillKind::Own
        } else {
            continue;
        };
        let Some(description) = skill_description(&path) else {
            continue;
        };
        skills.push(AgentSkill {
            name,
            description,
            kind,
        });
        if skills.len() == MAX_SKILLS {
            break;
        }
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

/// Counts of a tree copy.
#[derive(Default)]
struct Copied {
    files: usize,
    bytes: u64,
    skipped: usize,
}

/// Copies the regular files and directories of `from` into the new directory
/// `to`, skipping links and other entries, within the import bounds.
fn copy_tree(from: &Path, to: &Path, depth: usize, copied: &mut Copied) -> Result<(), CoreError> {
    if depth > MAX_IMPORT_DEPTH {
        return Err(too_large_skill());
    }
    fs::create_dir(to).map_err(|_| write_failed())?;
    let entries = fs::read_dir(from).map_err(|_| unreadable_file())?;
    for entry in entries {
        let entry = entry.map_err(|_| unreadable_file())?;
        let kind = entry.file_type().map_err(|_| unreadable_file())?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, depth + 1, copied)?;
        } else if kind.is_file() {
            copied.files += 1;
            if copied.files > MAX_IMPORT_FILES {
                return Err(too_large_skill());
            }
            let length = entry.metadata().map_err(|_| unreadable_file())?.len();
            copied.bytes += length;
            if copied.bytes > MAX_IMPORT_BYTES {
                return Err(too_large_skill());
            }
            fs::copy(entry.path(), &target).map_err(|_| write_failed())?;
        } else {
            copied.skipped += 1;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Refusals.

fn refusal(code: ErrorCode, details: &'static str, message: &str) -> CoreError {
    CoreError::new(code, message).with_details(details)
}

fn not_configured() -> CoreError {
    refusal(
        ErrorCode::InvalidRequest,
        "agentLibrary.notConfigured",
        "The agent library is not configured.",
    )
}

fn unreadable_file() -> CoreError {
    refusal(
        ErrorCode::PermissionDenied,
        "agentLibrary.unreadable",
        "A file of the agent library or an agent could not be read.",
    )
}

fn write_failed() -> CoreError {
    refusal(
        ErrorCode::PermissionDenied,
        "agentLibrary.writeFailed",
        "A file of the agent library or an agent could not be written.",
    )
}

fn too_large_file() -> CoreError {
    refusal(
        ErrorCode::InvalidRequest,
        "agentLibrary.fileTooLarge",
        "A configuration file is larger than Lithe reads.",
    )
}

fn too_large_skill() -> CoreError {
    refusal(
        ErrorCode::InvalidRequest,
        "agentLibrary.skillTooLarge",
        "The skill has more files, bytes or levels than Lithe copies.",
    )
}

fn read_failure(failure: ReadFailure) -> CoreError {
    match failure {
        ReadFailure::TooLarge => too_large_file(),
        ReadFailure::Io => unreadable_file(),
    }
}

fn invalid_change(message: &str) -> CoreError {
    refusal(
        ErrorCode::InvalidRequest,
        "agentLibrary.invalidChange",
        message,
    )
}

fn require_user() -> Result<(), CoreError> {
    if lithe_request_context::trace::current().source == Source::User {
        Ok(())
    } else {
        Err(refusal(
            ErrorCode::PermissionDenied,
            "agentLibrary.userActionRequired",
            "Only the user can change the agent library or an agent's configuration.",
        ))
    }
}

// ---------------------------------------------------------------------------
// One agent's files.

/// Where an agent's configuration is on this machine.
struct AgentPaths {
    instructions: Option<PathBuf>,
    skills: Option<PathBuf>,
    /// Declared files whose path resolves, by ID.
    files: Vec<(String, PathBuf)>,
}

fn agent_paths(config: &Config, configuration: &ConfigurationDeclaration) -> AgentPaths {
    let resolve = |roots: &[String]| resolve_root(roots, &config.user_home, &config.environment);
    AgentPaths {
        instructions: configuration
            .instructions
            .as_ref()
            .and_then(|roots| resolve(&roots.roots)),
        skills: configuration
            .skills
            .as_ref()
            .and_then(|roots| resolve(&roots.roots)),
        files: configuration
            .files
            .iter()
            .filter_map(|file| Some((file.id.clone(), resolve(&file.roots)?)))
            .collect(),
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Asks the plugin for what the agent's files hold. `files` gives each
/// declared file's path; a staged copy stands in for a file being changed.
fn read_snapshot(
    link: &dyn BackendLink,
    agent: &CatalogAgent,
    configuration: &ConfigurationDeclaration,
    files: &[(String, PathBuf)],
) -> Result<Snapshot, CoreError> {
    let Some(function) = configuration.read.as_deref() else {
        return Ok(Snapshot {
            format: READ_FORMAT.into(),
            version: 0,
            mcp_servers: Vec::new(),
            hooks: Vec::new(),
            hook_events: Vec::new(),
            skills: Vec::new(),
            targets: Targets::default(),
        });
    };
    let backend = link
        .session_for(&agent.plugin_id, function, None)
        .ok_or_else(|| {
            refusal(
                ErrorCode::InvalidRequest,
                "agentLibrary.backendUnavailable",
                "The agent plugin's backend is not running.",
            )
        })?;
    let params = json!({
        "agentID": agent.agent.id,
        "files": files
            .iter()
            .map(|(id, path)| json!({
                "id": id,
                "path": path_text(path),
                "exists": fs::symlink_metadata(path).is_ok(),
            }))
            .collect::<Vec<_>>(),
    });
    let answer = link
        .call(
            &backend,
            &agent.plugin_id,
            function,
            params,
            FUNCTION_TIMEOUT,
        )
        .map_err(|_| {
            refusal(
                ErrorCode::ProcessFailed,
                "agentLibrary.readFailed",
                "The agent plugin could not read the agent's configuration.",
            )
        })?;
    let invalid = || {
        refusal(
            ErrorCode::ParseFailed,
            "agentLibrary.invalidSnapshot",
            "The agent plugin returned an invalid configuration.",
        )
    };
    let snapshot: Snapshot = serde_json::from_value(answer).map_err(|_| invalid())?;
    let ids: BTreeSet<&str> = configuration.files.iter().map(|f| f.id.as_str()).collect();
    snapshot.check(&ids).map_err(|_| invalid())?;
    Ok(snapshot)
}

/// What an agent's instruction file holds, compared with the library.
fn instructions_state(
    current: &Result<Option<Vec<u8>>, ReadFailure>,
    composed: &str,
    written: Option<&String>,
) -> &'static str {
    match current {
        Err(ReadFailure::TooLarge) => "tooLarge",
        Err(ReadFailure::Io) => "unreadable",
        Ok(None) => "absent",
        Ok(Some(bytes)) if bytes == composed.as_bytes() => "current",
        Ok(Some(bytes)) if String::from_utf8_lossy(bytes).trim().is_empty() => "empty",
        Ok(Some(bytes)) if written.is_some_and(|written| *written == digest(bytes)) => "written",
        Ok(Some(_)) => "other",
    }
}

/// An agent as `agentLibrary.read` reports it, with what Core needs to
/// derive the library's states.
struct AgentView {
    key: String,
    entry: Value,
    snapshot: Option<Snapshot>,
    skills: Option<Vec<AgentSkill>>,
}

// ---------------------------------------------------------------------------
// Commands.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Category {
    McpServers,
    Hooks,
    Skills,
    Instructions,
}

impl Category {
    fn token(self) -> &'static str {
        match self {
            Category::McpServers => "mcpServers",
            Category::Hooks => "hooks",
            Category::Skills => "skills",
            Category::Instructions => "instructions",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Operation {
    Put,
    Remove,
    Link,
    Unlink,
    Write,
}

impl Operation {
    fn token(self) -> &'static str {
        match self {
            Operation::Put => "put",
            Operation::Remove => "remove",
            Operation::Link => "link",
            Operation::Unlink => "unlink",
            Operation::Write => "write",
        }
    }
}

/// One change to one agent's configuration.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    category: Category,
    op: Operation,
    /// The MCP server or skill.
    #[serde(default)]
    name: Option<String>,
    /// The library hook to put.
    #[serde(default)]
    id: Option<String>,
    /// The agent's hook to remove.
    #[serde(default)]
    hook: Option<Hook>,
    /// Write instructions over text Lithe did not write.
    #[serde(default)]
    replace: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyRequest {
    agent: String,
    change: Change,
}

impl Library {
    fn config(&self) -> Result<Config, CoreError> {
        self.config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(not_configured)
    }

    fn lock_writing(&self) -> MutexGuard<'_, ()> {
        self.writing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `agentLibrary.configure`.
    pub fn configure(&self, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct ConfigureRequest {
            /// A directory of the platform's application data for the
            /// library.
            directory: String,
            user_home: String,
            /// Values of the variables in the catalog's
            /// `sessionRootVariables`.
            #[serde(default)]
            environment: BTreeMap<String, String>,
        }
        let request: ConfigureRequest = crate::decode(payload)?;
        let directory = PathBuf::from(&request.directory);
        if !directory.is_absolute() || !Path::new(&request.user_home).is_absolute() {
            return Err(refusal(
                ErrorCode::InvalidRequest,
                "agentLibrary.invalidDirectory",
                "The library directory and user home must be absolute paths.",
            ));
        }
        if request.environment.len() > 256
            || request
                .environment
                .iter()
                .any(|(name, value)| name.is_empty() || name.len() > 256 || value.len() > 32 * 1024)
        {
            return Err(refusal(
                ErrorCode::InvalidRequest,
                "agentLibrary.invalidEnvironment",
                "The environment for configuration paths is invalid.",
            ));
        }
        *self.config.write().unwrap_or_else(PoisonError::into_inner) = Some(Config {
            directory,
            user_home: request.user_home,
            environment: request.environment,
        });
        Ok(json!({}))
    }

    /// The library and its revision: the SHA-256 of the file, or `empty`.
    fn load(&self, config: &Config) -> Result<(LibraryData, String), CoreError> {
        let path = config.directory.join(LIBRARY_FILE);
        let Some(bytes) = read_bounded(&path, MAX_LIBRARY_FILE_BYTES).map_err(read_failure)? else {
            return Ok((LibraryData::default(), EMPTY_REVISION.to_string()));
        };
        let invalid = || {
            refusal(
                ErrorCode::ParseFailed,
                "agentLibrary.invalidLibraryFile",
                "The agent library file is not a library Lithe reads.",
            )
        };
        let file: LibraryFile = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if file.version != 0 || file.library.check().is_err() {
            return Err(invalid());
        }
        Ok((file.library, digest(&bytes)))
    }

    fn load_state(config: &Config) -> StateFile {
        read_bounded(&config.directory.join(STATE_FILE), MAX_LIBRARY_FILE_BYTES)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<StateFile>(&bytes).ok())
            .filter(|state| state.version == 0)
            .unwrap_or_default()
    }

    fn store_state(config: &Config, state: &StateFile) -> Result<(), CoreError> {
        ensure_private_directory(&config.directory)?;
        let bytes = serde_json::to_vec_pretty(state).map_err(|_| write_failed())?;
        write_replacing(&config.directory.join(STATE_FILE), &bytes).map_err(|_| write_failed())
    }

    /// `agentLibrary.read`: the library, its skills, and each agent that
    /// declares configuration with what its files hold.
    pub fn read(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        let _: EmptyRequest = crate::decode(payload)?;
        let started = Instant::now();
        let config = self.config()?;
        let (library, revision) = self.load(&config)?;
        let state = Self::load_state(&config);
        let skills = library_skills(&config.directory);
        let mut failed = 0u64;
        let views: Vec<AgentView> = runtime
            .catalog
            .configurable()
            .into_iter()
            .map(|agent| {
                let view = self.view(runtime, &config, &library, &state, &agent);
                if view.entry.get("readError").is_some() {
                    failed += 1;
                }
                view
            })
            .collect();

        let states_of = |state: &dyn Fn(&AgentView) -> Option<&'static str>| -> Value {
            let map: serde_json::Map<String, Value> = views
                .iter()
                .filter_map(|view| Some((view.key.clone(), json!(state(view)?))))
                .collect();
            Value::Object(map)
        };
        let server_states: serde_json::Map<String, Value> = library
            .mcp_servers
            .iter()
            .map(|server| {
                let states = states_of(&|view| {
                    let snapshot = view.snapshot.as_ref()?;
                    Some(
                        match snapshot.mcp_servers.iter().find(|s| s.name == server.name) {
                            Some(found) if found.is(server) => "on",
                            Some(_) => "conflict",
                            None if snapshot.targets.mcp_servers.is_none() => "unsupported",
                            None => "off",
                        },
                    )
                });
                (server.name.clone(), states)
            })
            .collect();
        let hook_states: serde_json::Map<String, Value> = library
            .hooks
            .iter()
            .map(|hook| {
                let wanted = hook.hook();
                let states = states_of(&|view| {
                    let snapshot = view.snapshot.as_ref()?;
                    Some(if snapshot.hooks.iter().any(|h| h.is(&wanted)) {
                        "on"
                    } else if snapshot.targets.hooks.is_none()
                        || !snapshot.supports_event(&hook.event)
                    {
                        "unsupported"
                    } else {
                        "off"
                    })
                });
                (hook.id.clone(), states)
            })
            .collect();
        let skill_rows: Vec<Value> = skills
            .iter()
            .map(|(name, description)| {
                let states = states_of(&|view| {
                    let skills = view.skills.as_ref()?;
                    Some(match skills.iter().find(|skill| skill.name == *name) {
                        Some(skill) if skill.kind == SkillKind::Library => "on",
                        Some(_) => "conflict",
                        None => "off",
                    })
                });
                let mut row = json!({ "name": name, "agents": states });
                if let Some(description) = description {
                    row["description"] = json!(description);
                }
                row
            })
            .collect();

        log(
            "agent.library.read_finished",
            [
                ("agents", json!(views.len())),
                ("failed", json!(failed)),
                (
                    "duration",
                    json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
                ),
            ],
        );
        Ok(json!({
            "revision": revision,
            "library": library,
            "skills": skill_rows,
            "states": { "mcpServers": server_states, "hooks": hook_states },
            "agents": views.into_iter().map(|view| view.entry).collect::<Vec<_>>(),
        }))
    }

    fn view(
        &self,
        runtime: &Runtime,
        config: &Config,
        library: &LibraryData,
        state: &StateFile,
        agent: &CatalogAgent,
    ) -> AgentView {
        let key = agent.record_key();
        let configuration = agent
            .agent
            .configuration
            .clone()
            .expect("configurable agents declare configuration");
        let mut entry = json!({
            "agent": key,
            "pluginID": agent.plugin_id,
            "agentID": agent.agent.id,
            "displayName": agent.agent.display_name,
            "mayConfigure": agent.may_configure,
            "editable": configuration.edit.is_some(),
        });
        if let Some(icon) = &agent.agent.icon {
            entry["icon"] = json!(icon);
        }
        let mut view = AgentView {
            key: key.clone(),
            entry,
            snapshot: None,
            skills: None,
        };
        if !agent.may_configure {
            return view;
        }
        let paths = agent_paths(config, &configuration);
        if let Some(path) = &paths.instructions {
            let current = read_bounded(path, MAX_INSTRUCTIONS_FILE_BYTES);
            let composed = library.compose(&key);
            view.entry["instructions"] = json!({
                "path": path_text(path),
                "state": instructions_state(&current, &composed, state.instructions.get(&key)),
            });
        }
        if let Some(root) = &paths.skills {
            let mut skills = json!({ "path": path_text(root) });
            match agent_skills(root, &config.directory) {
                Ok(found) => {
                    skills["items"] = json!(found
                        .iter()
                        .map(|skill| {
                            let mut item =
                                json!({ "name": skill.name, "kind": skill.kind.token() });
                            if let Some(description) = &skill.description {
                                item["description"] = json!(description);
                            }
                            item
                        })
                        .collect::<Vec<_>>());
                    view.skills = Some(found);
                }
                Err(_) => skills["error"] = json!("agentLibrary.unreadable"),
            }
            view.entry["skills"] = skills;
        }
        if configuration.read.is_some() {
            match read_snapshot(&*runtime.link, agent, &configuration, &paths.files) {
                Ok(snapshot) => {
                    let servers: Vec<Value> = snapshot
                        .mcp_servers
                        .iter()
                        .map(|server| {
                            let mut row = json!(server);
                            row["inLibrary"] =
                                json!(library.mcp_servers.iter().any(|kept| server.is(kept)));
                            row
                        })
                        .collect();
                    let hooks: Vec<Value> = snapshot
                        .hooks
                        .iter()
                        .map(|hook| {
                            let mut row = json!(hook);
                            row["inLibrary"] =
                                json!(library.hooks.iter().any(|kept| hook.is(&kept.hook())));
                            row
                        })
                        .collect();
                    view.entry["mcpServers"] = json!(servers);
                    view.entry["hooks"] = json!(hooks);
                    view.entry["hookEvents"] = json!(snapshot.hook_events);
                    view.entry["providedSkills"] = json!(snapshot.skills);
                    view.entry["targets"] = json!({
                        "mcpServers": snapshot.targets.mcp_servers.is_some(),
                        "hooks": snapshot.targets.hooks.is_some(),
                    });
                    view.snapshot = Some(snapshot);
                }
                Err(error) => {
                    let reason = error
                        .details
                        .as_deref()
                        .unwrap_or("agentLibrary.readFailed");
                    log(
                        "agent.library.read_failed",
                        [
                            ("plugin", json!(agent.plugin_id)),
                            ("agent", json!(agent.agent.id)),
                            ("reason", json!(reason)),
                        ],
                    );
                    view.entry["readError"] = json!(reason);
                }
            }
        }
        view
    }

    /// `agentLibrary.save`: replaces the library if it is still at
    /// `revision`.
    pub fn save(&self, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SaveRequest {
            revision: String,
            library: LibraryData,
        }
        require_user()?;
        let request: SaveRequest = crate::decode(payload)?;
        let config = self.config()?;
        if let Err(field) = request.library.check() {
            return Err(refusal(
                ErrorCode::InvalidRequest,
                "agentLibrary.invalidLibrary",
                &format!("The library is invalid at {field}."),
            ));
        }
        let path = config.directory.join(LIBRARY_FILE);
        let _writing = self.lock_writing();
        let current = match read_bounded(&path, MAX_LIBRARY_FILE_BYTES) {
            Ok(Some(bytes)) => digest(&bytes),
            Ok(None) => EMPTY_REVISION.to_string(),
            // An unreadable library is replaced only by a save that read it.
            Err(_) => return Err(unreadable_file()),
        };
        if current != request.revision {
            return Err(refusal(
                ErrorCode::InvalidRequest,
                "agentLibrary.revisionChanged",
                "The agent library changed since it was read.",
            ));
        }
        ensure_private_directory(&config.directory)?;
        let file = LibraryFile {
            version: 0,
            library: request.library,
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|_| write_failed())?;
        write_replacing(&path, &bytes).map_err(|_| write_failed())?;
        log(
            "agent.library.updated",
            [
                ("servers", json!(file.library.mcp_servers.len())),
                ("hooks", json!(file.library.hooks.len())),
                ("sets", json!(file.library.instructions.sets.len())),
            ],
        );
        Ok(json!({ "revision": digest(&bytes) }))
    }

    /// `agentLibrary.apply`: one change to one agent's configuration.
    pub fn apply(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        require_user()?;
        let request: ApplyRequest = crate::decode(payload)?;
        let started = Instant::now();
        let agent = configurable_agent(runtime, &request.agent)?;
        let category = request.change.category;
        let op = request.change.op;
        let outcome = self.apply_checked(runtime, &agent, &request.change);
        match &outcome {
            Ok(changed) => log(
                "agent.library.change_applied",
                [
                    ("plugin", json!(agent.plugin_id)),
                    ("agent", json!(agent.agent.id)),
                    ("category", json!(category.token())),
                    ("change", json!(op.token())),
                    ("changed", json!(changed)),
                    (
                        "duration",
                        json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
                    ),
                ],
            ),
            Err(error) => log(
                "agent.library.change_failed",
                [
                    ("plugin", json!(agent.plugin_id)),
                    ("agent", json!(agent.agent.id)),
                    ("category", json!(category.token())),
                    ("change", json!(op.token())),
                    (
                        "reason",
                        json!(error
                            .details
                            .as_deref()
                            .unwrap_or("agentLibrary.applyFailed")),
                    ),
                ],
            ),
        }
        outcome.map(|changed| json!({ "changed": changed }))
    }

    fn apply_checked(
        &self,
        runtime: &Runtime,
        agent: &CatalogAgent,
        change: &Change,
    ) -> Result<bool, CoreError> {
        let config = self.config()?;
        let configuration = agent
            .agent
            .configuration
            .as_ref()
            .expect("configurable agents declare configuration");
        let paths = agent_paths(&config, configuration);
        match (change.category, change.op) {
            (Category::McpServers | Category::Hooks, Operation::Put | Operation::Remove) => {
                self.apply_file(runtime, &config, agent, configuration, &paths, change)
            }
            (Category::Skills, Operation::Link | Operation::Unlink) => {
                let name = change
                    .name
                    .as_deref()
                    .filter(|name| is_skill_name(name))
                    .ok_or_else(|| invalid_change("A skill change names a library skill."))?;
                self.apply_skill(&config, &paths, change.op, name)
            }
            (Category::Instructions, Operation::Write) => {
                self.apply_instructions(&config, &agent.record_key(), &paths, change.replace)
            }
            _ => Err(invalid_change(
                "MCP servers and hooks take put and remove, skills link and unlink, instructions write.",
            )),
        }
    }

    /// Changes an MCP server or hook through the plugin, on a staged copy.
    fn apply_file(
        &self,
        runtime: &Runtime,
        config: &Config,
        agent: &CatalogAgent,
        configuration: &ConfigurationDeclaration,
        paths: &AgentPaths,
        change: &Change,
    ) -> Result<bool, CoreError> {
        let Some(edit) = configuration.edit.as_deref() else {
            return Err(refusal(
                ErrorCode::NotSupported,
                "agentLibrary.notEditable",
                "The agent plugin does not edit this agent's configuration.",
            ));
        };
        let link = &*runtime.link;
        let snapshot = read_snapshot(link, agent, configuration, &paths.files)?;
        let target = match change.category {
            Category::McpServers => snapshot.targets.mcp_servers.as_deref(),
            _ => snapshot.targets.hooks.as_deref(),
        };
        let Some((target_id, target_path)) = target
            .and_then(|id| paths.files.iter().find(|(file, _)| file == id))
            .cloned()
        else {
            return Err(refusal(
                ErrorCode::NotSupported,
                "agentLibrary.noTarget",
                "The agent keeps no editable file for this kind of item.",
            ));
        };

        // What the plugin is asked to do, and how a staged copy shows it.
        let (library, _) = self.load(config)?;
        let (plugin_change, applied): (Value, ShowsChange) = match (change.category, change.op) {
            (Category::McpServers, Operation::Put) => {
                let name = change.name.as_deref().unwrap_or_default();
                let server = library
                    .mcp_servers
                    .iter()
                    .find(|server| server.name == name)
                    .cloned()
                    .ok_or_else(unknown_item)?;
                let wanted = server.clone();
                (
                    json!({ "category": "mcpServers", "op": "put", "server": server }),
                    Box::new(move |snapshot: &Snapshot| {
                        snapshot.mcp_servers.iter().any(|s| s.is(&wanted))
                    }),
                )
            }
            (Category::McpServers, _) => {
                let name = change.name.clone().unwrap_or_default();
                if !snapshot
                    .mcp_servers
                    .iter()
                    .any(|server| server.name == name && server.origin == Origin::User)
                {
                    return Err(not_in_agent());
                }
                let gone = name.clone();
                (
                    json!({ "category": "mcpServers", "op": "remove", "name": name }),
                    Box::new(move |snapshot: &Snapshot| {
                        !snapshot
                            .mcp_servers
                            .iter()
                            .any(|s| s.name == gone && s.origin == Origin::User)
                    }),
                )
            }
            (_, Operation::Put) => {
                let id = change.id.as_deref().unwrap_or_default();
                let hook = library
                    .hooks
                    .iter()
                    .find(|hook| hook.id == id)
                    .map(LibraryHook::hook)
                    .ok_or_else(unknown_item)?;
                if !snapshot.supports_event(&hook.event) {
                    return Err(refusal(
                        ErrorCode::NotSupported,
                        "agentLibrary.unsupportedEvent",
                        "The agent does not run hooks for this event.",
                    ));
                }
                let wanted = hook.clone();
                (
                    json!({ "category": "hooks", "op": "put", "hook": hook }),
                    Box::new(move |snapshot: &Snapshot| {
                        snapshot.hooks.iter().any(|h| h.is(&wanted))
                    }),
                )
            }
            (_, _) => {
                let hook = change
                    .hook
                    .clone()
                    .ok_or_else(|| invalid_change("Removing a hook names the hook."))?;
                if !snapshot
                    .hooks
                    .iter()
                    .any(|h| h.is(&hook) && h.origin == Origin::User)
                {
                    return Err(not_in_agent());
                }
                let gone = hook.clone();
                (
                    json!({ "category": "hooks", "op": "remove", "hook": hook }),
                    Box::new(move |snapshot: &Snapshot| {
                        !snapshot
                            .hooks
                            .iter()
                            .any(|h| h.is(&gone) && h.origin == Origin::User)
                    }),
                )
            }
        };

        let real = real_file(&target_path)?;
        let original = read_bounded(&real, MAX_CONFIGURATION_FILE_BYTES).map_err(read_failure)?;
        let staging = self.stage(config, &real, original.as_deref())?;
        let outcome = (|| {
            let backend = link
                .session_for(&agent.plugin_id, edit, None)
                .ok_or_else(|| {
                    refusal(
                        ErrorCode::InvalidRequest,
                        "agentLibrary.backendUnavailable",
                        "The agent plugin's backend is not running.",
                    )
                })?;
            let params = json!({
                "agentID": agent.agent.id,
                "file": {
                    "id": target_id,
                    "staging": path_text(&staging),
                    "exists": original.is_some(),
                },
                "change": plugin_change,
            });
            let answer = link
                .call(&backend, &agent.plugin_id, edit, params, FUNCTION_TIMEOUT)
                .map_err(|_| {
                    refusal(
                        ErrorCode::ProcessFailed,
                        "agentLibrary.editFailed",
                        "The agent plugin could not change the agent's configuration.",
                    )
                })?;
            let answer: EditAnswer = serde_json::from_value(answer)
                .ok()
                .filter(|answer: &EditAnswer| answer.format == EDIT_FORMAT && answer.version == 0)
                .ok_or_else(|| {
                    refusal(
                        ErrorCode::ParseFailed,
                        "agentLibrary.invalidEdit",
                        "The agent plugin returned an invalid edit answer.",
                    )
                })?;
            if !answer.changed {
                return Ok(false);
            }
            let edited = read_bounded(&staging, MAX_CONFIGURATION_FILE_BYTES)
                .map_err(read_failure)?
                .unwrap_or_default();
            if original.as_deref() == Some(edited.as_slice()) {
                return Ok(false);
            }

            // The plugin reads the staged copy in place of the file, which
            // shows it can read what it wrote and that the change is there.
            let staged_files: Vec<(String, PathBuf)> = paths
                .files
                .iter()
                .map(|(id, path)| {
                    if *id == target_id {
                        (id.clone(), staging.clone())
                    } else {
                        (id.clone(), path.clone())
                    }
                })
                .collect();
            let staged = read_snapshot(link, agent, configuration, &staged_files)?;
            if !applied(&staged) {
                return Err(refusal(
                    ErrorCode::ParseFailed,
                    "agentLibrary.editNotApplied",
                    "The agent plugin's edit did not make the change.",
                ));
            }

            let _writing = self.lock_writing();
            let now = read_bounded(&real, MAX_CONFIGURATION_FILE_BYTES).map_err(read_failure)?;
            if now != original {
                return Err(refusal(
                    ErrorCode::InvalidRequest,
                    "agentLibrary.fileChanged",
                    "The configuration file changed while it was being edited.",
                ));
            }
            if let Some(parent) = real.parent() {
                ensure_directory(parent)?;
            }
            write_replacing(&real, &edited).map_err(|_| write_failed())?;
            Ok(true)
        })();
        let _ = fs::remove_file(&staging);
        outcome
    }

    /// A private copy of `original` for a plugin to edit; an empty file when
    /// the file does not exist yet.
    fn stage(
        &self,
        config: &Config,
        real: &Path,
        original: Option<&[u8]>,
    ) -> Result<PathBuf, CoreError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = config.directory.join(STAGING_DIRECTORY);
        ensure_private_directory(&config.directory)?;
        ensure_private_directory(&directory)?;
        let extension = real
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| extension.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        let path = directory.join(format!(
            "edit-{}-{}{extension}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        lithe_platform::fs::owner_only_file(&mut options);
        let mut file = options.open(&path).map_err(|_| write_failed())?;
        if let Err(_error) = file.write_all(original.unwrap_or_default()) {
            let _ = fs::remove_file(&path);
            return Err(write_failed());
        }
        Ok(path)
    }

    fn apply_skill(
        &self,
        config: &Config,
        paths: &AgentPaths,
        op: Operation,
        name: &str,
    ) -> Result<bool, CoreError> {
        let Some(root) = &paths.skills else {
            return Err(refusal(
                ErrorCode::NotSupported,
                "agentLibrary.noSkills",
                "The agent keeps no skills directory.",
            ));
        };
        let library_skills = config.directory.join(SKILLS_DIRECTORY);
        let skill = library_skills.join(name);
        let link = root.join(name);
        let _writing = self.lock_writing();
        match op {
            Operation::Link => {
                if !skill.join(SKILL_FILE).is_file() {
                    return Err(unknown_item());
                }
                match fs::symlink_metadata(&link) {
                    Ok(metadata)
                        if metadata.file_type().is_symlink() && links_into(&link, &skill) =>
                    {
                        Ok(false)
                    }
                    Ok(_) => Err(refusal(
                        ErrorCode::EntryExists,
                        "agentLibrary.agentSkillExists",
                        "The agent already has a skill of this name.",
                    )),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        ensure_directory(root)?;
                        lithe_platform::entries::create_symlink(&skill, &link).map_err(|_| {
                            refusal(
                                ErrorCode::PermissionDenied,
                                "agentLibrary.linkFailed",
                                "The skill could not be linked into the agent's skills directory.",
                            )
                        })?;
                        Ok(true)
                    }
                    Err(_) => Err(unreadable_file()),
                }
            }
            _ => match fs::symlink_metadata(&link) {
                Ok(metadata)
                    if metadata.file_type().is_symlink() && links_into(&link, &library_skills) =>
                {
                    remove_link(&link).map_err(|_| write_failed())?;
                    Ok(true)
                }
                Ok(_) => Err(refusal(
                    ErrorCode::PermissionDenied,
                    "agentLibrary.notLibraryLink",
                    "The agent's skill of this name is its own, not a link into the library.",
                )),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(_) => Err(unreadable_file()),
            },
        }
    }

    fn apply_instructions(
        &self,
        config: &Config,
        key: &str,
        paths: &AgentPaths,
        replace: bool,
    ) -> Result<bool, CoreError> {
        let Some(path) = &paths.instructions else {
            return Err(refusal(
                ErrorCode::NotSupported,
                "agentLibrary.noInstructions",
                "The agent keeps no instruction file.",
            ));
        };
        let (library, _) = self.load(config)?;
        if !library
            .instructions
            .agents
            .get(key)
            .is_some_and(|agent| agent.enabled)
        {
            return Err(refusal(
                ErrorCode::InvalidRequest,
                "agentLibrary.instructionsOff",
                "The library's instructions are off for this agent.",
            ));
        }
        let text = library.compose(key);
        let _writing = self.lock_writing();
        let real = real_file(path)?;
        let current = read_bounded(&real, MAX_INSTRUCTIONS_FILE_BYTES).map_err(read_failure)?;
        let mut state = Self::load_state(config);
        if current.as_deref() == Some(text.as_bytes()) {
            return Ok(false);
        }
        if let Some(current) = &current {
            let empty = String::from_utf8_lossy(current).trim().is_empty();
            let written = state
                .instructions
                .get(key)
                .is_some_and(|written| *written == digest(current));
            if !empty && !written && !replace {
                return Err(refusal(
                    ErrorCode::InvalidRequest,
                    "agentLibrary.otherInstructions",
                    "The instruction file holds text Lithe did not write.",
                ));
            }
        }
        if let Some(parent) = real.parent() {
            ensure_directory(parent)?;
        }
        write_replacing(&real, text.as_bytes()).map_err(|_| write_failed())?;
        state.version = 0;
        state
            .instructions
            .insert(key.to_string(), digest(text.as_bytes()));
        Self::store_state(config, &state)?;
        Ok(true)
    }

    /// `agentLibrary.importSkill`: copies a skill into the library, from an
    /// agent's skills directory or from a directory the user chose.
    pub fn import_skill(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct ImportRequest {
            #[serde(default)]
            agent: Option<String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            directory: Option<String>,
        }
        require_user()?;
        let request: ImportRequest = crate::decode(payload)?;
        let config = self.config()?;
        let source = match (&request.agent, &request.name, &request.directory) {
            (Some(agent), Some(name), None) => {
                let agent = configurable_agent(runtime, agent)?;
                let configuration = agent
                    .agent
                    .configuration
                    .as_ref()
                    .expect("configurable agents declare configuration");
                let root = agent_paths(&config, configuration).skills.ok_or_else(|| {
                    refusal(
                        ErrorCode::NotSupported,
                        "agentLibrary.noSkills",
                        "The agent keeps no skills directory.",
                    )
                })?;
                if !is_skill_name(name) {
                    return Err(invalid_skill_name());
                }
                let source = root.join(name);
                if fs::symlink_metadata(&source).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(refusal(
                        ErrorCode::InvalidRequest,
                        "agentLibrary.notOwnSkill",
                        "Only a skill directory of the agent's own is imported.",
                    ));
                }
                source
            }
            (None, None, Some(directory)) => {
                let directory = PathBuf::from(directory);
                if !directory.is_absolute() {
                    return Err(refusal(
                        ErrorCode::InvalidRequest,
                        "agentLibrary.invalidSkillDirectory",
                        "The skill directory must be an absolute path.",
                    ));
                }
                directory
            }
            _ => {
                return Err(refusal(
                    ErrorCode::InvalidRequest,
                    "agentLibrary.invalidImport",
                    "A skill is imported from an agent and a name, or from a directory.",
                ))
            }
        };
        if skill_description(&source).is_none() {
            return Err(refusal(
                ErrorCode::EntryNotFound,
                "agentLibrary.notSkill",
                "The directory has no readable SKILL.md.",
            ));
        }
        let name = match request.name {
            Some(name) => name,
            None => {
                let bytes = read_bounded(&source.join(SKILL_FILE), MAX_SKILL_FILE_BYTES)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                front_matter(&String::from_utf8_lossy(&bytes))
                    .0
                    .or_else(|| {
                        source
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_default()
            }
        };
        if !is_skill_name(&name) {
            return Err(invalid_skill_name());
        }

        let skills = config.directory.join(SKILLS_DIRECTORY);
        let destination = skills.join(&name);
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(refusal(
                ErrorCode::EntryExists,
                "agentLibrary.skillExists",
                "The library already has a skill of this name.",
            ));
        }
        ensure_private_directory(&config.directory)?;
        ensure_private_directory(&config.directory.join(STAGING_DIRECTORY))?;
        ensure_directory(&skills)?;
        let partial = sibling_temporary(
            &config.directory.join(STAGING_DIRECTORY).join(&name),
            "import",
        )
        .ok_or_else(write_failed)?;
        let mut copied = Copied::default();
        let outcome = copy_tree(&source, &partial, 0, &mut copied).and_then(|()| {
            let _writing = self.lock_writing();
            lithe_platform::entries::rename_no_replace(&partial, &destination).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    refusal(
                        ErrorCode::EntryExists,
                        "agentLibrary.skillExists",
                        "The library already has a skill of this name.",
                    )
                } else {
                    write_failed()
                }
            })
        });
        if outcome.is_err() {
            let _ = fs::remove_dir_all(&partial);
        }
        outcome?;
        log(
            "agent.library.skill_import_finished",
            [
                ("files", json!(copied.files)),
                ("skipped", json!(copied.skipped)),
            ],
        );
        Ok(json!({ "name": name, "files": copied.files, "skipped": copied.skipped }))
    }

    /// `agentLibrary.removeSkill`: removes a library skill and every link to
    /// it in the agents' skills directories.
    pub fn remove_skill(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct RemoveRequest {
            name: String,
        }
        require_user()?;
        let request: RemoveRequest = crate::decode(payload)?;
        let config = self.config()?;
        if !is_skill_name(&request.name) {
            return Err(invalid_skill_name());
        }
        let skill = config.directory.join(SKILLS_DIRECTORY).join(&request.name);
        if !skill.is_dir() {
            return Err(unknown_item());
        }
        let mut unlinked = 0u64;
        let _writing = self.lock_writing();
        for agent in runtime.catalog.configurable() {
            if !agent.may_configure {
                continue;
            }
            let configuration = agent
                .agent
                .configuration
                .as_ref()
                .expect("configurable agents declare configuration");
            let Some(root) = agent_paths(&config, configuration).skills else {
                continue;
            };
            let link = root.join(&request.name);
            if fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink())
                && links_into(&link, &skill)
            {
                remove_link(&link).map_err(|_| write_failed())?;
                unlinked += 1;
            }
        }
        fs::remove_dir_all(&skill).map_err(|_| write_failed())?;
        log(
            "agent.library.skill_removed",
            [("unlinked", json!(unlinked))],
        );
        Ok(json!({ "unlinked": unlinked }))
    }
}

fn unknown_item() -> CoreError {
    refusal(
        ErrorCode::EntryNotFound,
        "agentLibrary.unknownItem",
        "The library has no such item.",
    )
}

fn not_in_agent() -> CoreError {
    refusal(
        ErrorCode::EntryNotFound,
        "agentLibrary.notInAgent",
        "The agent's own configuration has no such item.",
    )
}

fn invalid_skill_name() -> CoreError {
    refusal(
        ErrorCode::InvalidRequest,
        "agentLibrary.invalidSkillName",
        "A skill name uses ASCII letters, digits, `.`, `_` and `-`.",
    )
}

/// The agent `key` names, if it declares configuration and its plugin is
/// granted `agents.configuration`.
fn configurable_agent(runtime: &Runtime, key: &str) -> Result<CatalogAgent, CoreError> {
    let agent = runtime
        .catalog
        .find_key(key)
        .filter(|agent| agent.agent.configuration.is_some())
        .ok_or_else(|| {
            refusal(
                ErrorCode::EntryNotFound,
                "agentLibrary.unknownAgent",
                "No installed plugin declares configuration for this agent.",
            )
        })?;
    if !agent.may_configure {
        return Err(refusal(
            ErrorCode::PermissionDenied,
            "agentLibrary.notGranted",
            "The user has not allowed the plugin to change this agent's configuration.",
        ));
    }
    Ok(agent)
}

#[cfg(test)]
pub(crate) fn front_matter_for_test(text: &str) -> (Option<String>, Option<String>) {
    front_matter(text)
}

#[cfg(test)]
pub(crate) fn compose_for_test(library: Value, key: &str) -> String {
    let library: LibraryData = serde_json::from_value(library).expect("library decodes");
    library.compose(key)
}
