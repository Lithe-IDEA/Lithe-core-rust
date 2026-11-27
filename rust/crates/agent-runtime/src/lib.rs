//! Agent sessions Core runs for agent plugins (plan API-45).
//!
//! An agent plugin declares agents: a CLI it requires as a tool, how a native
//! session talks to it, and where the CLI keeps its session files. Core owns
//! everything that touches the machine. It starts the CLI, writes its input
//! and reads its output, reads the session files, and keeps the cache; the
//! plugin backend only translates. It turns the CLI's lines into Lithe's
//! session events ([`events`]) and session-file lines into records.
//!
//! Core knows no agent: every agent-specific name, argument, protocol and file
//! layout comes from a plugin manifest and its backend.
//!
//! The host drives the runtime with two command families. `agents.*` covers
//! the catalog of declared agents and the live sessions; `agentSessions.*`
//! covers the local session records. Backends reach it with
//! `lithe/host/agents/*` requests, which the engine hands to the installed
//! [`AgentHost`] and which never reach the host application.
//!
//! Security rules this crate enforces:
//!
//! - Only a request the user started may start a session, send a prompt or
//!   answer a permission request. Sessions run only in trusted local
//!   workspaces, at most [`live::MAX_SESSIONS_PER_AGENT`] per agent.
//! - The executable is always the agent's declared tool, as the host resolved
//!   it. The plugin chooses arguments only, never the environment.
//! - Session records are read only for agents the user granted
//!   `agents.sessions.read`. Plugins granted `agents.usage.read` see totals
//!   and rate limits, never text, titles, session IDs or directories.
//! - Logs carry opaque IDs and stable codes, never what flows through a
//!   session.

mod backend;
mod catalog;
mod events;
mod live;
mod records;

#[cfg(test)]
mod tests;

use backend::{BackendLink, EngineLink};
use catalog::Catalog;
use lithe_lsp_engine::agent_host::{install_agent_host, AgentBackend, AgentHost, AgentHostFailure};
use lithe_plugin_rpc::{codes, AGENTS_REPORT, AGENTS_USAGE, AGENTS_WRITE};
use lithe_protocol::{CoreError, ErrorCode};
use live::Live;
use records::Records;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The commands of the `agents.*` family.
pub const AGENT_COMMANDS: &[&str] = &[
    "agents.configure",
    "agents.catalog",
    "agents.sessions",
    "agents.start",
    "agents.prompt",
    "agents.cancel",
    "agents.respondPermission",
    "agents.close",
    "agents.waitEvents",
];

/// The commands of the `agentSessions.*` family.
pub const AGENT_SESSION_COMMANDS: &[&str] = &[
    "agentSessions.configure",
    "agentSessions.sync",
    "agentSessions.forget",
    "agentSessions.list",
    "agentSessions.search",
    "agentSessions.read",
    "agentSessions.stats",
];

/// The agent catalog, the live sessions and the session records.
pub(crate) struct Runtime {
    pub link: Arc<dyn BackendLink>,
    pub catalog: Catalog,
    pub live: Live,
    pub records: Records,
}

impl Runtime {
    pub(crate) fn new(link: Arc<dyn BackendLink>) -> Arc<Self> {
        Arc::new(Self {
            link,
            catalog: Catalog::default(),
            live: Live::default(),
            records: Records::default(),
        })
    }

    pub(crate) fn execute(
        self: &Arc<Self>,
        command: &str,
        payload: Value,
    ) -> Result<Value, CoreError> {
        match command {
            "agents.configure" => {
                let answer = self.catalog.configure(payload)?;
                // A session whose agent is gone or no longer granted ends.
                self.live.end_where(|plugin_id, agent_id| {
                    self.catalog
                        .find(plugin_id, agent_id)
                        .is_none_or(|agent| !agent.may_run)
                });
                Ok(answer)
            }
            "agents.catalog" => Ok(self.catalog.list()),
            "agents.sessions" => Ok(self.live.list()),
            "agents.start" => live::start(self, payload),
            "agents.prompt" => live::prompt(self, payload),
            "agents.cancel" => live::cancel(self, payload),
            "agents.respondPermission" => live::respond_permission(self, payload),
            "agents.close" => live::close(self, payload),
            "agents.waitEvents" => live::wait_events(self, payload),
            "agentSessions.configure" => self.records.configure(payload),
            "agentSessions.sync" => self.records.sync(self, payload),
            "agentSessions.forget" => self.records.forget(payload),
            "agentSessions.list" => self.records.list(self, payload),
            "agentSessions.search" => self.records.search(self, payload),
            "agentSessions.read" => self.records.read(self, payload),
            "agentSessions.stats" => self.records.stats(self, payload),
            _ => Err(CoreError::new(
                ErrorCode::NotSupported,
                format!("Unknown agent command {command}."),
            )
            .with_details("agents.unknownCommand")),
        }
    }
}

/// The runtime as the engine reaches it.
struct Host(Arc<Runtime>);

impl AgentHost for Host {
    fn answer(
        &self,
        backend: AgentBackend<'_>,
        method: &str,
        params: &Value,
    ) -> Result<Value, AgentHostFailure> {
        match method {
            AGENTS_WRITE => self.0.live.write(backend, params),
            AGENTS_REPORT => self.0.live.report(&self.0, backend, params),
            AGENTS_USAGE => self.0.records.usage(&self.0, params),
            _ => Err(AgentHostFailure::new(
                codes::HOST_UNAVAILABLE,
                "This host method is not available.",
            )),
        }
    }

    fn backend_ended(&self, session_id: &str) {
        self.0.live.backend_ended(session_id);
    }
}

static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

fn runtime() -> &'static Arc<Runtime> {
    RUNTIME.get_or_init(|| {
        let runtime = Runtime::new(Arc::new(EngineLink));
        install_agent_host(Arc::new(Host(runtime.clone())));
        runtime
    })
}

/// Runs one `agents.*` or `agentSessions.*` command.
pub fn execute(command: &str, payload: Value) -> Result<Value, CoreError> {
    runtime().execute(command, payload)
}

/// Decodes a command payload.
pub(crate) fn decode<T: DeserializeOwned>(payload: Value) -> Result<T, CoreError> {
    serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            format!("Invalid agent command payload: {error}"),
        )
        .with_details("agents.invalidPayload")
    })
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Records a catalog event. Logging only queues, so it is safe under locks;
/// a refused fact is a defect of this crate's fields and is dropped.
pub(crate) fn log<const N: usize>(event: &str, fields: [(&str, Value); N]) {
    log_fields(event, fields.into_iter());
}

pub(crate) fn log_fields<'a>(event: &str, fields: impl Iterator<Item = (&'a str, Value)>) {
    let fields: BTreeMap<String, Value> = fields
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    let _ = lithe_diagnostics::core_events::record(event, fields, None);
}
