//! `agents.*` and `agentSessions.*` commands, run by `lithe-agent-runtime`
//! (plan API-45).

use crate::protocol::{AgentsCommand, CoreResponse};
use serde_json::Value;

/// Runs one agent command. `agents.waitEvents` blocks for at most the
/// timeout it was given, so the host calls it off its UI thread.
pub(super) fn dispatch(command: AgentsCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match lithe_agent_runtime::execute(command.name(), payload) {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, error),
    }
}
