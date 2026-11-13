//! `editor.*` commands: workbench state, text edits, find, navigation and preferences.

use super::{plugin_response, rules_response};
use crate::protocol::{CoreError, CoreResponse, EditorCommand, ErrorCode};
use serde_json::Value;

/// Runs one `editor.*` command.
pub(super) fn dispatch(
    command: EditorCommand,
    id: Option<String>,
    payload: Value,
    document: Option<&str>,
) -> CoreResponse {
    match command {
        EditorCommand::WorkbenchStateReduce => {
            rules_response(id, "editor.workbenchState.reduce", payload)
        }
        EditorCommand::Find => match lithe_editor::find::command(payload, document) {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        EditorCommand::PreferencesResolve => {
            plugin_response(id, payload, lithe_editor::preferences::resolve_command)
        }
        EditorCommand::RecentReduce => {
            plugin_response(id, payload, lithe_editor::recent::reduce_command)
        }
        EditorCommand::RecoveryDecide => {
            plugin_response(id, payload, lithe_editor::recovery::decide_command)
        }
        EditorCommand::PlanTextEdits => {
            plugin_response(id, payload, lithe_editor::text_edit_plan::plan_command)
        }
        EditorCommand::ParseNavigationInput => {
            plugin_response(id, payload, lithe_editor::navigation_input::parse_command)
        }
        EditorCommand::NavigationHistoryReduce => plugin_response(
            id,
            payload,
            lithe_editor::navigation_history::reduce_command,
        ),
        EditorCommand::LineEdit => {
            match serde_json::from_value::<lithe_editor::LineEditRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid editor line edit request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_editor::line_edit)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Editor line edit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        EditorCommand::LineCommentToken => {
            match serde_json::from_value::<lithe_editor::LineCommentTokenRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid editor line comment token request",
                    )
                    .with_details(error.to_string())
                })
                .map(lithe_editor::line_comment_token)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Comment token response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    }
}
