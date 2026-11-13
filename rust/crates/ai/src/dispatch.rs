//! Command routing and the JSON envelope shared with `lithe-core`.

use crate::{agent_configuration, commands, inline_edit, AiError, AiErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

/// Every command name this crate owns. `lithe-core` routes exactly these names here.
pub const COMMANDS: &[&str] = &[
    "ai.commitInputBudget",
    "ai.planCommit",
    "ai.decodeCommitMessage",
    "ai.configuration.sources",
    "ai.configuration.parse",
    "ai.pullRequest.plan",
    "ai.pullRequest.decode",
    "ai.inlineEdit.plan",
    "ai.inlineEdit.decode",
];

/// Reports whether `command` is owned by this crate.
pub fn is_ai_command(command: &str) -> bool {
    COMMANDS.contains(&command)
}

/// Runs one AI command on an already decoded payload.
pub fn execute(command: &str, payload: Value) -> Result<Value, AiError> {
    match command {
        "ai.commitInputBudget" => commands::commit_input_budget_command(payload),
        "ai.planCommit" => commands::plan_commit_command(payload),
        "ai.decodeCommitMessage" => commands::decode_commit_command(payload),
        "ai.configuration.sources" => agent_configuration::configuration_sources_command(payload),
        "ai.configuration.parse" => agent_configuration::parse_configuration_command(payload),
        "ai.pullRequest.plan" => commands::plan_pull_request_command(payload),
        "ai.pullRequest.decode" => commands::decode_pull_request_command(payload),
        "ai.inlineEdit.plan" => inline_edit::plan_command(payload),
        "ai.inlineEdit.decode" => inline_edit::decode_command(payload),
        _ => Err(AiError::new(
            AiErrorCode::NotSupported,
            "Unknown AI command",
        )),
    }
}

#[derive(Deserialize)]
struct Request {
    id: Option<String>,
    command: String,
    #[serde(default)]
    payload: Value,
}

/// Executes one Core-style JSON request and returns the JSON response envelope.
///
/// Unknown envelope fields (`operationId`, `origin`, ...) are ignored so callers can send the
/// same request object they send to `lithe-core`.
pub fn execute_json(request: &str) -> String {
    let parsed: Request = match serde_json::from_str(request) {
        Ok(parsed) => parsed,
        Err(_) => {
            let error = AiError::new(AiErrorCode::InvalidRequest, "Invalid JSON request");
            return failure(None, &error);
        }
    };
    match execute(&parsed.command, parsed.payload) {
        Ok(data) => json!({"id": parsed.id, "ok": true, "data": data}).to_string(),
        Err(error) => failure(parsed.id, &error),
    }
}

fn failure(id: Option<String>, error: &AiError) -> String {
    json!({"id": id, "ok": false, "error": error}).to_string()
}

#[cfg(test)]
mod tests {
    use super::{execute, COMMANDS};
    use crate::AiErrorCode;
    use serde_json::json;

    #[test]
    fn every_listed_command_is_routed() {
        for command in COMMANDS {
            let error = execute(command, json!(null)).expect_err(command);
            assert_eq!(error.code, AiErrorCode::InvalidRequest, "{command}");
        }
        let unknown = execute("ai.unknown", json!({})).unwrap_err();
        assert_eq!(unknown.code, AiErrorCode::NotSupported);
    }
}
