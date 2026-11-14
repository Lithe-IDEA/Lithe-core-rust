//! Deterministic Git inspection and mutation behind the shared command contract.
//!
//! `lithe-core` routes every `git.*` command here. Repository access goes through
//! the request's workspace host (`lithe-workspace`), so the same code serves local
//! and remote workspaces; process tracing, cancellation and the console journal come
//! from `lithe-request-context` and `lithe-protocol`. [`contracts`] holds the
//! serialized responses, which `lithe-core` re-exports unchanged.

// The initial projection/routing IR is deliberately not command- or host-facing
// until both native products can consume the same versioned contract.
mod authentication_failure;
mod branch_names;
pub mod catalog;
mod change_targets;
mod changed_file_groups;
mod changelists;
mod command_trace;
mod commit_message_context;
mod commit_selection;
mod commit_state;
pub mod contracts;
mod deleted_selection;
mod diff_blocks;
pub mod workspace_commit;
pub mod workspace_status;
pub use commit_state::{inspect as commit_state, GitCommitGitlink, GitCommitState};

mod branches;
mod checkout;
mod commits;
pub mod configuration;
pub mod console;
mod diff;
pub(crate) mod execution_events;
pub mod execution_policy;
mod fetch;
mod fetch_execution;
pub mod file_context;
mod github_checkout;
mod graph;
mod history;
mod history_edits;
mod history_query;
mod history_window;
mod ignore;
mod line_history;
mod metadata_watch;
mod mutations;
mod parse;
mod patch_exchange;
mod preflight;
mod process;
mod progress;
mod push;
mod rebase_session;
mod reference_tree;
mod references;
mod remote_history;
mod remote_process;
mod repository;
mod repository_colors;
mod requests;
mod rewrite;
mod rollback;
mod setup;
mod shelf;
mod stash;
mod status;
mod untracked_directories;
mod update_plan;
mod worktree_rules;
mod worktrees;
mod write;

// The response and reference types stay owned by the module; callers consume
// them through the command contract instead of naming more shared types.
pub use branch_names::{preview as branch_name_preview, GitBranchNamePreviewRequest};
pub use fetch::{plan as fetch_plan, GitFetchOptions, GitFetchPlanRequest};
pub use untracked_directories::{untracked_directory, GitUntrackedDirectoryRequest};

pub use setup::{
    configure_identity, initialize as initialize_repository, inspect as repository_setup,
    GitConfigureIdentityRequest, GitSetupRequest,
};

pub use change_targets::command as normalize_change_targets_command;
pub use changed_file_groups::command as changed_file_groups_command;
pub use changelists::changelists_command;
pub use commit_message_context::commit_message_context_command;
pub use commit_selection::prepare_commit_selection_command;
pub use deleted_selection::reconcile_deleted_selection_command;
pub use diff_blocks::{
    apply_diff_block_command, plan_diff_blocks_command, stage_gutter_change_command,
};
pub use github_checkout::{
    checkout as checkout_pull_request, preview as checkout_pull_request_preview,
    PullRequestCheckoutPreviewRequest, PullRequestCheckoutRequest,
};
pub use graph::command as graph_projection_command;
pub use history::{
    close_history_cursor, history, history_locate, history_page, references,
    GitHistoryCursorCloseRequest, GitHistoryLocateRequest, GitHistoryPageRequest,
    GitHistoryRequest, GitReferencesRequest,
};
pub use history_query::query_command as history_query_command;
pub use history_window::command as history_window_command;
pub use metadata_watch::classify_command as metadata_classification_command;
pub use metadata_watch::{
    classify_git_metadata_change, GitMetadataChangeKind, GitMetadataEventClassification,
    GitMetadataWatchContext,
};
pub use patch_exchange::{
    apply_reviewed as apply_patch_reviewed, export as export_patch, preview as preview_patch,
    PatchApplyRequest, PatchExportRequest, PatchPreviewRequest,
};
pub use rebase_session::plan_preview_command as rebase_plan_preview_command;
pub use rebase_session::{
    control as rebase_control, preview as rebase_preview, session as rebase_session,
    start as rebase_start, GitRebaseControlRequest, GitRebasePreviewRequest,
    GitRebaseSessionRequest, GitRebaseStartRequest,
};
pub use reference_tree::command as reference_tree_command;
pub use repository_colors::assign_command as repository_color_assignments_command;
pub use rollback::{rollback_plan_command, rollback_prepare_command};
pub use shelf::shelf_command;
pub use update_plan::command as update_plan_command;
pub use worktree_rules::creation_preview_command as worktree_creation_preview_command;

/// Internals the `git.*` integration tests in `lithe-core` assert against.
#[cfg(feature = "test-support")]
pub mod test_support {
    pub use crate::diff_blocks::fixture_support as diff_blocks;
    pub use crate::untracked_directories::UNTRACKED_DIRECTORY_SUMMARY_THRESHOLD;
}

pub use rewrite::history_action_capabilities_command;
pub use rewrite::{
    history_rewrite_preview, GitHistoryRewriteExpectation, GitHistoryRewritePreviewRequest,
    GitHistoryRewriteResult,
};

use branches::*;
use checkout::*;
pub use commits::*;
pub use diff::*;
use history_edits::*;
use ignore::*;
pub use line_history::*;
use parse::*;
pub use preflight::*;
pub use process::*;
pub use push::*;
use references::*;
pub use repository::*;
pub use requests::*;
pub use stash::*;
pub use status::*;
pub use worktrees::*;
pub use write::*;

use crate::contracts::{
    GitBlameLineResponse, GitBlameResponse, GitChange, GitCheckoutPreflightResponse,
    GitCommitLookupResponse, GitCommitResponse, GitComparisonResponse, GitConflictMarkerResponse,
    GitDiffFileResponse, GitDiffHunkResponse, GitDiffResponse, GitDiffRowResponse,
    GitDiffUnifiedLineResponse, GitFileResponse, GitFilesResponse, GitIntegrationPreflightResponse,
    GitLineHistoryCommit, GitLineHistoryRange, GitLineHistoryResponse, GitLineHistoryUnmappedLine,
    GitOperationStateResponse, GitPullPreflightResponse, GitPushPreviewResponse,
    GitPushTagResponse, GitPushTargetKind, GitReferenceResponse, GitStashResponse,
    GitStashesResponse, GitStatusResponse, GitWatchContextResponse, GitWorktreeResponse,
    GitWorktreesResponse, WorkspaceRepositoriesResponse, WorkspaceRepositoryResponse,
};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_workspace::fs;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::io::Read;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const DEFAULT_PUSH_PREVIEW_LIMIT: usize = 500;

const INTERNAL_REF_PREFIX: &str = "refs/lithe/";

/// Commit date argument for every `parse_commit` producer. The trailing `%z`
/// carries the author's UTC offset, which `parse_commit` splits off so the
/// displayed `date` keeps its `%Y/%m/%d %H:%M` shape.
const GIT_COMMIT_DATE_ARGUMENT: &str = "--date=format:%Y/%m/%d %H:%M %z";

const DEFAULT_REPOSITORY_SCAN_MAX_DIRECTORIES: usize = usize::MAX;

const DEFAULT_REPOSITORY_SCAN_MAX_DEPTH: usize = usize::MAX;

const FULL_FILE_CONTEXT_LINES: usize = 1_073_741_823;

static TEMPORARY_INDEX_SEQUENCE: AtomicU64 = AtomicU64::new(0);

static AUTO_STASH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Decodes a test payload the way `lithe-core` decodes a typed `git.*` request.
#[cfg(test)]
pub(crate) fn test_request<T: serde::de::DeserializeOwned>(
    payload: serde_json::Value,
) -> Result<T, CoreError> {
    serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid Git request")
            .with_details(error.to_string())
    })
}

/// The `ok`/`data`/`error` envelope `lithe-core` wraps around a `git.*` result.
#[cfg(test)]
pub(crate) fn test_envelope<T: Serialize>(result: Result<T, CoreError>) -> serde_json::Value {
    match result {
        Ok(data) => serde_json::json!({"ok": true, "data": data}),
        Err(error) => serde_json::json!({"ok": false, "error": error}),
    }
}

#[cfg(test)]
mod tests;
