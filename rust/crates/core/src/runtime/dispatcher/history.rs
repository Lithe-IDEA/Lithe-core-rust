//! `history.*` commands: local file history.

use super::plugin_response;
use crate::project::{
    HistoryContentRequest, HistoryDeleteRequest, HistoryEntriesRequest,
    HistoryIgnorePreviewRequest, HistoryRecordRequest, HistoryRelocateRequest,
    HistoryRenameRequest, HistoryStorageRequest,
};
use crate::protocol::{CoreError, CoreResponse, ErrorCode, HistoryCommand};
use serde_json::Value;

/// Runs one `history.*` command.
pub(super) fn dispatch(
    command: HistoryCommand,
    id: Option<String>,
    payload: Value,
) -> CoreResponse {
    match command {
        HistoryCommand::Record => {
            match serde_json::from_value::<HistoryRecordRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history record request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::record)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::Entries => {
            match serde_json::from_value::<HistoryEntriesRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history entries request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::entries)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history entries should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::Content => {
            match serde_json::from_value::<HistoryContentRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history content request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::content)
            {
                Ok(data) => CoreResponse::success(id, serde_json::json!({"text": data})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::PlanEvent => {
            plugin_response(id, payload, crate::project::plan_event_command)
        }
        HistoryCommand::Relocate => {
            match serde_json::from_value::<HistoryRelocateRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid history relocate request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::project::relocate)
            {
                Ok(()) => CoreResponse::success(id, serde_json::json!({"relocated": true})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::Rename => {
            match serde_json::from_value::<HistoryRenameRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history rename request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::rename)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history entry should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::Delete => {
            match serde_json::from_value::<HistoryDeleteRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history delete request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::delete)
            {
                Ok(()) => CoreResponse::success(id, serde_json::json!({"deleted": true})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::IgnorePreview => {
            match serde_json::from_value::<HistoryIgnorePreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid history ignore preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(crate::project::ignore_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history ignore preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::Usage => {
            match serde_json::from_value::<HistoryStorageRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history usage request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::usage)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("history usage should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        HistoryCommand::Clear => {
            match serde_json::from_value::<HistoryStorageRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid history clear request")
                        .with_details(error.to_string())
                })
                .and_then(crate::project::clear)
            {
                Ok(()) => CoreResponse::success(id, serde_json::json!({"cleared": true})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    }
}
