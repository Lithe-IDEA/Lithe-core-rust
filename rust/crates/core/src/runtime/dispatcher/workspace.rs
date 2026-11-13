//! `workspace.*` commands: snapshots, indexing, directory actions, repositories and search.

use super::{plugin_response, rules_response};
use crate::project::{
    self, DirectoryListingRequest, ReplacementPreviewRequest, SearchIndexRequest,
    SearchIndexUpdateRequest, SearchRequest, WorkspaceSnapshotRequest,
};
use crate::protocol::{CoreError, CoreResponse, ErrorCode, WorkspaceCommand};
use lithe_git::{self as git, WorkspaceRepositoriesRequest};
use serde_json::{json, Value};

/// Runs one `workspace.*` command.
pub(super) fn dispatch(
    command: WorkspaceCommand,
    id: Option<String>,
    payload: Value,
) -> CoreResponse {
    match command {
        WorkspaceCommand::ReleaseIndexing => {
            match serde_json::from_value::<project::indexing::ReleaseIndexingRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid indexing release")
                        .with_details(error.to_string())
                })
                .and_then(project::indexing::release)
            {
                Ok(released) => {
                    CoreResponse::success(id, serde_json::json!({"released": released}))
                }
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::ConfigureIndexing => {
            match serde_json::from_value::<project::indexing::ConfigureIndexingRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid indexing policy")
                        .with_details(error.to_string())
                })
                .and_then(|request| {
                    project::indexing::configure(request, lithe_lsp_engine::indexing_changed)
                }) {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("indexing status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::RefreshIndexing => {
            match serde_json::from_value::<project::indexing::RefreshIndexingRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid indexing refresh")
                        .with_details(error.to_string())
                })
                .and_then(|request| {
                    project::indexing::refresh_indexing(request, lithe_lsp_engine::indexing_changed)
                }) {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("indexing status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::Snapshot => {
            match serde_json::from_value::<WorkspaceSnapshotRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace snapshot request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::snapshot)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("snapshot should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::FilterChanges => {
            match serde_json::from_value::<crate::project::WorkspaceChangeFilterRequest>(payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::json!(crate::project::filter_workspace_changes(request)),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid workspace event paths")
                        .with_details(error.to_string()),
                ),
            }
        }
        WorkspaceCommand::ListDirectories => {
            match serde_json::from_value::<DirectoryListingRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid directory listing request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::list_directories)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("directory listing should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::OrderEntries => plugin_response(
            id,
            payload,
            crate::project::file_name_order::order_entries_command,
        ),
        WorkspaceCommand::RankNames => plugin_response(
            id,
            payload,
            crate::project::name_ranking::rank_names_command,
        ),
        WorkspaceCommand::PlanDuplicate => plugin_response(
            id,
            payload,
            crate::project::file_operations::plan_duplicate_command,
        ),
        WorkspaceCommand::PlanPaste => plugin_response(
            id,
            payload,
            crate::project::file_operations::plan_paste_command,
        ),
        WorkspaceCommand::ReduceDirectoryAction => plugin_response(
            id,
            payload,
            crate::project::directory_actions::reduce_command,
        ),
        WorkspaceCommand::RelocateDirectoryRules => plugin_response(
            id,
            payload,
            crate::project::directory_actions::relocate_command,
        ),
        WorkspaceCommand::Repositories => {
            match serde_json::from_value::<WorkspaceRepositoriesRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace repositories request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::workspace_repositories)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("workspace repositories should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::RecentProjectsReduce => {
            plugin_response(id, payload, crate::project::recent_projects::reduce_command)
        }
        WorkspaceCommand::SearchIndexWarm => {
            match serde_json::from_value::<SearchIndexRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search index request")
                        .with_details(error.to_string())
                })
                .and_then(project::warm_search_index)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search index status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::SearchIndexUpdate => {
            match serde_json::from_value::<SearchIndexUpdateRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search index update")
                        .with_details(error.to_string())
                })
                .and_then(|request| {
                    project::update_search_index(request, lithe_lsp_engine::indexing_changed)
                }) {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search index status should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::SearchIndexInvalidate => {
            match serde_json::from_value::<SearchIndexRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search index request")
                        .with_details(error.to_string())
                })
                .and_then(project::invalidate_search_index)
            {
                Ok(()) => CoreResponse::success(id, json!({})),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::Search => {
            match serde_json::from_value::<SearchRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search request")
                        .with_details(error.to_string())
                })
                .and_then(project::search)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::SearchEverywhere => {
            match serde_json::from_value::<project::SearchPageRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Search Everywhere request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::search_page)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("search everywhere should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::ReplacePreview => {
            match serde_json::from_value::<ReplacementPreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid replacement preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::replace_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("replacement preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::ReplaceText => {
            match serde_json::from_value::<project::ReplacementTextRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid text replacement request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::replace_text)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::SearchTextBatch => {
            match serde_json::from_value::<project::SearchTextBatchRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid text search request")
                        .with_details(error.to_string())
                })
                .and_then(project::search_text_batch)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        WorkspaceCommand::ClassifyVisibility => {
            rules_response(id, "workspace.classifyVisibility", payload)
        }
        WorkspaceCommand::VisibilityDefaults => {
            rules_response(id, "workspace.visibilityDefaults", payload)
        }
    }
}
