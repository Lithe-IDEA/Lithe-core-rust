//! How the runtime reaches agent plugin backends and starts agent CLIs.
//!
//! The runtime never holds one of its own locks while it uses a
//! [`BackendLink`]: the engine calls the runtime back with a backend's session
//! lock held, so a runtime lock held across an engine call could deadlock.

use lithe_lsp_engine::agent_host::{self, AgentCallFailure};
use lithe_lsp_transport::process::{LspProcessLauncher, SystemProcessLauncher};
use lithe_protocol::CoreError;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// The plugin backends and process launcher the runtime uses. The engine
/// implements it in production; tests replace it.
pub(crate) trait BackendLink: Send + Sync {
    /// The ready backend session of `plugin_id` that serves `function_id`.
    fn session_for(
        &self,
        plugin_id: &str,
        function_id: &str,
        root_uri: Option<&str>,
    ) -> Option<String>;

    /// Calls a function of a backend and waits for its answer.
    fn call(
        &self,
        session_id: &str,
        plugin_id: &str,
        function_id: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, AgentCallFailure>;

    /// Sends an agent-session notification to a backend.
    fn notify(&self, session_id: &str, method: &str, params: Value) -> Result<(), CoreError>;

    /// Starts agent CLI processes.
    fn launcher(&self) -> Arc<dyn LspProcessLauncher>;
}

/// The engine's plugin backends and real child processes.
pub(crate) struct EngineLink;

impl BackendLink for EngineLink {
    fn session_for(
        &self,
        plugin_id: &str,
        function_id: &str,
        root_uri: Option<&str>,
    ) -> Option<String> {
        agent_host::agent_backend_session(plugin_id, function_id, root_uri)
    }

    fn call(
        &self,
        session_id: &str,
        plugin_id: &str,
        function_id: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, AgentCallFailure> {
        agent_host::call_agent_function(session_id, plugin_id, function_id, params, None, timeout)
    }

    fn notify(&self, session_id: &str, method: &str, params: Value) -> Result<(), CoreError> {
        agent_host::notify_agent_backend(session_id, method, params)
    }

    fn launcher(&self) -> Arc<dyn LspProcessLauncher> {
        Arc::new(SystemProcessLauncher)
    }
}
