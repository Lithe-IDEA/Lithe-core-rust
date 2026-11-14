//! `git.*` commands: status, writes, history, rebase, patches, diffs, references and
//! pull requests.

use super::plugin_response;
use crate::protocol::{CoreError, CoreResponse, ErrorCode, GitCommand};
use lithe_git::{
    self as git, GitApplyRequest, GitBlameRequest, GitBranchNamePreviewRequest,
    GitCheckoutPreflightRequest, GitCommandRequest, GitCommitFilesRequest, GitCommitRequest,
    GitComparisonRequest, GitConflictMarkerRequest, GitDiffRequest, GitHistoryCursorCloseRequest,
    GitHistoryLocateRequest, GitHistoryPageRequest, GitHistoryRequest,
    GitHistoryRewritePreviewRequest, GitIntegrationPreflightRequest, GitLineHistoryRequest,
    GitOperationStateRequest, GitPullPreflightRequest, GitPullRequestContextRequest,
    GitPushPreviewRequest, GitRebaseControlRequest, GitRebasePreviewRequest,
    GitRebaseSessionRequest, GitRebaseStartRequest, GitReferencesRequest, GitRepositoryRootRequest,
    GitStashesRequest, GitStatusRequest, GitUntrackedDirectoryRequest, GitWatchContextRequest,
    GitWorktreesRequest, GitWriteRequest, PatchApplyRequest, PatchExportRequest,
    PatchPreviewRequest,
};
use serde_json::{json, Value};

/// Runs one `git.*` command.
pub(super) fn dispatch(command: GitCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match command {
        GitCommand::WorkspaceCommitPrepare => {
            let result = serde_json::from_value::<git::workspace_commit::PrepareRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace commit request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::workspace_commit::prepare);
            match result {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Workspace commit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::WorkspaceCommitStep => {
            let result = serde_json::from_value::<git::workspace_commit::StepRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace commit request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::workspace_commit::step);
            match result {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Workspace commit response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::CommitState => {
            match serde_json::from_value::<GitStatusRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid commit state request")
                        .with_details(error.to_string())
                })
                .and_then(git::commit_state)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Commit state should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Status => match serde_json::from_value::<GitStatusRequest>(payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git status request")
                    .with_details(error.to_string())
            })
            .and_then(git::status)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        GitCommand::UntrackedDirectory => {
            match serde_json::from_value::<GitUntrackedDirectoryRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git untracked directory request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::untracked_directory)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git untracked directory should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::ClassifyMetadataChanges => {
            plugin_response(id, payload, git::metadata_classification_command)
        }
        GitCommand::WatchContext => {
            match serde_json::from_value::<GitWatchContextRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git watch context request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::watch_context)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git watch context should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }

        GitCommand::Worktrees => {
            match serde_json::from_value::<GitWorktreesRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git worktrees request")
                        .with_details(error.to_string())
                })
                .and_then(git::worktrees)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git worktrees should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }

        GitCommand::PullRequestContext => {
            match serde_json::from_value::<GitPullRequestContextRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git pull request context request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::pull_request_context)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git pull request context should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }

        GitCommand::Command => {
            match serde_json::from_value::<GitCommandRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git command request")
                        .with_details(error.to_string())
                })
                .and_then(git::command)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git command response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::RepositoryRoot => {
            match serde_json::from_value::<GitRepositoryRootRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git repository root request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::discover_repository_root)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git repository root response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::WorkspaceStatusMerge => {
            match serde_json::from_value::<git::workspace_status::Request>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid workspace Git status request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::workspace_status::plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git workspace status plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::ResolveFileContext => {
            match serde_json::from_value::<git::file_context::Request>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git file context request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::file_context::resolve)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git file context should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Remotes
        | GitCommand::Tags
        | GitCommand::ParseCheckoutMetadata
        | GitCommand::ParseGitdirPointer => {
            let operation = match command {
                GitCommand::Remotes => "remotes",
                GitCommand::Tags => "tags",
                GitCommand::ParseCheckoutMetadata => "checkout",
                GitCommand::ParseGitdirPointer => "gitdir",
                _ => unreachable!(),
            };
            match git::catalog::command(operation, payload) {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::RebasePreview => {
            match serde_json::from_value::<GitRebasePreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase preview request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::RebaseStart => {
            match serde_json::from_value::<GitRebaseStartRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase start request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_start)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase start should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::RebaseSession => {
            match serde_json::from_value::<GitRebaseSessionRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase session request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_session)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase session should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::RebaseControl => {
            match serde_json::from_value::<GitRebaseControlRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid rebase control request")
                        .with_details(error.to_string())
                })
                .and_then(git::rebase_control)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Rebase control should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::RepositorySetup => {
            match serde_json::from_value::<git::GitSetupRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git setup request")
                        .with_details(error.to_string())
                })
                .and_then(git::repository_setup)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git setup should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Initialize => {
            match serde_json::from_value::<git::GitSetupRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git setup request")
                        .with_details(error.to_string())
                })
                .and_then(git::initialize_repository)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git setup should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::ConfigureIdentity => {
            match serde_json::from_value::<git::GitConfigureIdentityRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git setup request")
                        .with_details(error.to_string())
                })
                .and_then(git::configure_identity)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git setup should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::PatchExport => {
            match serde_json::from_value::<PatchExportRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid patch export request")
                        .with_details(error.to_string())
                })
                .and_then(git::export_patch)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Patch export should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::PatchPreview => {
            match serde_json::from_value::<PatchPreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid patch preview request")
                        .with_details(error.to_string())
                })
                .and_then(git::preview_patch)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Patch preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::PatchApply => {
            match serde_json::from_value::<PatchApplyRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid patch apply request")
                        .with_details(error.to_string())
                })
                .and_then(git::apply_patch_reviewed)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Patch apply should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::HistoryRewritePreview => {
            match serde_json::from_value::<GitHistoryRewritePreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid history rewrite preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::history_rewrite_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("History rewrite preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::HistoryActionCapabilities => {
            plugin_response(id, payload, lithe_git::history_action_capabilities_command)
        }
        GitCommand::RepositoryColorAssignments => {
            plugin_response(id, payload, lithe_git::repository_color_assignments_command)
        }
        GitCommand::UpdatePlan => plugin_response(id, payload, lithe_git::update_plan_command),
        GitCommand::HistoryWindow => {
            plugin_response(id, payload, lithe_git::history_window_command)
        }
        GitCommand::GraphProjection => {
            plugin_response(id, payload, lithe_git::graph_projection_command)
        }
        GitCommand::NormalizeChangeTargets => {
            plugin_response(id, payload, lithe_git::normalize_change_targets_command)
        }
        GitCommand::RebasePlanPreview => {
            plugin_response(id, payload, lithe_git::rebase_plan_preview_command)
        }
        GitCommand::ReferenceTree => {
            plugin_response(id, payload, lithe_git::reference_tree_command)
        }
        GitCommand::ChangedFileGroups => {
            plugin_response(id, payload, lithe_git::changed_file_groups_command)
        }
        GitCommand::PrepareCommitSelection => {
            plugin_response(id, payload, lithe_git::prepare_commit_selection_command)
        }
        GitCommand::Shelf => plugin_response(id, payload, lithe_git::shelf_command),
        GitCommand::CommitMessageContext => {
            plugin_response(id, payload, lithe_git::commit_message_context_command)
        }
        GitCommand::Changelists => plugin_response(id, payload, lithe_git::changelists_command),
        GitCommand::ReconcileDeletedSelection => {
            plugin_response(id, payload, lithe_git::reconcile_deleted_selection_command)
        }
        GitCommand::RollbackPrepare => {
            plugin_response(id, payload, lithe_git::rollback_prepare_command)
        }
        GitCommand::ParsePatch => plugin_response(id, payload, lithe_git::parse_patch_command),
        GitCommand::HistoryQuery => plugin_response(id, payload, lithe_git::history_query_command),
        GitCommand::RollbackPlan => plugin_response(id, payload, lithe_git::rollback_plan_command),
        GitCommand::Write => {
            match serde_json::from_value::<GitWriteRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git write request")
                        .with_details(error.to_string())
                })
                .and_then(git::write)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git write response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::AuthRespond => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Reply {
                request_id: String,
                answer: Option<String>,
            }
            match serde_json::from_value::<Reply>(payload) {
                Ok(reply) => CoreResponse::success(
                    id,
                    json!({ "accepted": lithe_git_host::authentication::respond(&reply.request_id, reply.answer) }),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git authentication response",
                    ),
                ),
            }
        }
        GitCommand::Executable => match git::configuration::locate_executable() {
            Ok(data) => CoreResponse::success(id, data),
            Err(error) => CoreResponse::failure(id, error),
        },
        GitCommand::ExecutionInspect | GitCommand::ExecutionConfigure => {
            match git::configuration::dispatch(
                payload,
                matches!(command, GitCommand::ExecutionConfigure),
            ) {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::RemoteUrl => match git::configuration::remote_url(payload) {
            Ok(value) => CoreResponse::success(id, value),
            Err(error) => CoreResponse::failure(id, error),
        },
        GitCommand::ConsolePresentation => {
            match serde_json::from_value::<git::console::Request>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git console snapshot")
                        .with_details(error.to_string())
                })
                .and_then(git::console::present)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Console presentation should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::FetchPlan => {
            match serde_json::from_value::<git::GitFetchPlanRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Fetch plan request")
                        .with_details(error.to_string())
                })
                .and_then(git::fetch_plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Fetch plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Diff => match serde_json::from_value::<GitDiffRequest>(payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git diff request")
                    .with_details(error.to_string())
            })
            .and_then(git::diff)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git diff response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        GitCommand::PlanDiffBlocks => {
            plugin_response(id, payload, lithe_git::plan_diff_blocks_command)
        }
        GitCommand::ApplyDiffBlock => {
            plugin_response(id, payload, lithe_git::apply_diff_block_command)
        }
        GitCommand::StageGutterChange => {
            plugin_response(id, payload, lithe_git::stage_gutter_change_command)
        }
        GitCommand::Apply => match serde_json::from_value::<GitApplyRequest>(payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git apply request")
                    .with_details(error.to_string())
            })
            .and_then(git::apply)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git apply response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        GitCommand::History => {
            match serde_json::from_value::<GitHistoryRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git history request")
                        .with_details(error.to_string())
                })
                .and_then(git::history)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git history response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::References => {
            match serde_json::from_value::<GitReferencesRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git references request")
                        .with_details(error.to_string())
                })
                .and_then(git::references)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git references response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::HistoryPage => {
            match serde_json::from_value::<GitHistoryPageRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git history page request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::history_page)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git history page response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::HistoryCursorClose => {
            match serde_json::from_value::<GitHistoryCursorCloseRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git history cursor close request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::close_history_cursor)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git history cursor close response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::HistoryLocate => {
            match serde_json::from_value::<GitHistoryLocateRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git history locate request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::history_locate)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git history locate response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::BranchNamePreview => {
            match serde_json::from_value::<GitBranchNamePreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git branch name preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::branch_name_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git branch name preview response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::PushPreview => {
            match serde_json::from_value::<GitPushPreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git push preview request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::push_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git push preview response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Commit => match serde_json::from_value::<GitCommitRequest>(payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git commit request")
                    .with_details(error.to_string())
            })
            .and_then(git::commit)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git commit response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        GitCommand::CommitFiles => {
            match serde_json::from_value::<GitCommitFilesRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git commit files request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::commit_files)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git commit files response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Comparison => {
            match serde_json::from_value::<GitComparisonRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git comparison request")
                        .with_details(error.to_string())
                })
                .and_then(git::comparison)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git comparison response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Stashes => {
            match serde_json::from_value::<GitStashesRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid Git stashes request")
                        .with_details(error.to_string())
                })
                .and_then(git::stashes)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git stashes response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::CheckoutPreflight => {
            match serde_json::from_value::<GitCheckoutPreflightRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git checkout preflight request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::checkout_preflight)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git checkout preflight response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::PullPreflight => {
            match serde_json::from_value::<GitPullPreflightRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git pull preflight request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::pull_preflight)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git pull preflight response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::ConflictMarkers => {
            match serde_json::from_value::<GitConflictMarkerRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git conflict marker request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::conflict_marker_paths)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git conflict marker response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::IntegrationPreflight => {
            match serde_json::from_value::<GitIntegrationPreflightRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git integration preflight request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::integration_preflight)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Git integration preflight response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::OperationState => {
            match serde_json::from_value::<GitOperationStateRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git operation state request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::operation_state)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git operation state response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::Blame => match serde_json::from_value::<GitBlameRequest>(payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Git blame request")
                    .with_details(error.to_string())
            })
            .and_then(git::blame)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("Git blame response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        GitCommand::LineHistory => {
            match serde_json::from_value::<GitLineHistoryRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Git line history request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::line_history)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Git line history response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::PullRequestCheckoutPreview => {
            match serde_json::from_value::<git::PullRequestCheckoutPreviewRequest>(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid pull-request checkout preview",
                    )
                    .with_details(error.to_string())
                })
                .and_then(git::checkout_pull_request_preview)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data)
                        .expect("Pull-request checkout preview should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        GitCommand::PullRequestCheckout => {
            match serde_json::from_value::<git::PullRequestCheckoutRequest>(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid pull-request checkout")
                        .with_details(error.to_string())
                })
                .and_then(git::checkout_pull_request)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Pull-request checkout should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    }
}
