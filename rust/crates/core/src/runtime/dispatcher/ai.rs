//! `ai.*` commands, run by `lithe-ai`.

use crate::protocol::{AiCommand, CoreResponse};
use serde_json::Value;

/// Runs one `ai.*` command.
pub(super) fn dispatch(command: AiCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match command {
        AiCommand::CommitInputBudget => ai_response(id, "ai.commitInputBudget", payload),
        AiCommand::PlanCommit => ai_response(id, "ai.planCommit", payload),
        AiCommand::DecodeCommitMessage => ai_response(id, "ai.decodeCommitMessage", payload),
        AiCommand::ConfigurationSources => ai_response(id, "ai.configuration.sources", payload),
        AiCommand::ConfigurationParse => ai_response(id, "ai.configuration.parse", payload),
        AiCommand::PullRequestPlan => ai_response(id, "ai.pullRequest.plan", payload),
        AiCommand::PullRequestDecode => ai_response(id, "ai.pullRequest.decode", payload),
        AiCommand::InlineEditPlan => ai_response(id, "ai.inlineEdit.plan", payload),
        AiCommand::InlineEditDecode => ai_response(id, "ai.inlineEdit.decode", payload),
    }
}

/// Runs one `ai.*` command in `lithe-ai`.
fn ai_response(id: Option<String>, command: &str, payload: serde_json::Value) -> CoreResponse {
    match lithe_ai::execute(command, payload) {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, crate::protocol::ai_error(error)),
    }
}
