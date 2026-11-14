//! Request payloads of the `git.*` commands and the command response they share.

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to discover Git repositories belonging to one opened workspace.
pub struct WorkspaceRepositoriesRequest {
    pub(crate) root: String,
    /// Optional traversal budget; defaults to all eligible directories below the workspace.
    #[serde(default = "default_repository_scan_max_directories")]
    pub max_directories: usize,
    /// Optional traversal depth; defaults to the complete eligible workspace tree.
    #[serde(default = "default_repository_scan_max_depth")]
    pub max_depth: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for a deterministic porcelain status snapshot.
pub struct GitStatusRequest {
    pub(crate) root: String,
    /// Staging-based UIs must retain additions deleted only from the worktree.
    /// Omission preserves the Windows final-worktree projection.
    #[serde(default)]
    pub include_index_only_changes: bool,
    /// Discovered native root bindings used to exclude independently owned untracked paths.
    #[serde(default)]
    pub repository_roots: Vec<String>,
    /// Report large wholly untracked directories as one entry with
    /// `untrackedFileCount` instead of one entry per file. Omission preserves
    /// the file-level `--untracked-files=all` projection.
    #[serde(default)]
    pub summarize_untracked_directories: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for the directories and files a host watcher should observe.
pub struct GitWatchContextRequest {
    pub(crate) root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for all worktrees registered in the current repository.
pub struct GitWorktreesRequest {
    pub(crate) root: String,
    /// Also reads the local changes of every checkout that is there and not bare, which sets
    /// each entry's `changes` and lets its `status` be `modified`.
    #[serde(default)]
    pub(crate) count_changes: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for branch and publication state used by pull request creation.
pub struct GitPullRequestContextRequest {
    pub(crate) root: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// Worktree-aware branch defaults and publication requirements for a pull request.
pub struct GitPullRequestContextResponse {
    /// Checked-out local branch, or `None` when HEAD is detached.
    pub current_branch: Option<String>,
    /// Best-effort branch that should receive the pull request.
    pub suggested_base_branch: Option<String>,
    /// Branch name shown when the current commit must be published first.
    pub suggested_publish_branch: Option<String>,
    /// Whether GitHub cannot yet see the current local HEAD.
    pub requires_publish: bool,
    /// Whether the worktree has no checked-out local branch.
    pub detached: bool,
    /// Whether tracked or untracked working-tree changes are not part of HEAD.
    pub has_uncommitted_changes: bool,
}

/// Executes one Git operation without invoking a shell.
///
/// The command boundary is intentionally argument-based. This keeps command
/// construction in the application layer while making process execution
/// available to every UI binding through the same Rust core.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommandRequest {
    pub(crate) root: String,
    #[serde(default)]
    pub(crate) arguments: Vec<String>,
    #[serde(default)]
    pub(crate) input: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to resolve the repository root containing a workspace path.
pub struct GitRepositoryRootRequest {
    pub(crate) root: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One Git subprocess executed while fulfilling a shared Git command.
pub struct GitCommandInvocation {
    /// Exact arguments passed to the Git executable, excluding the executable name.
    pub arguments: Vec<String>,
    /// Text captured from the Git process standard output stream.
    pub stdout: String,
    /// Text captured from the Git process standard error stream.
    pub stderr: String,
    /// Exit status returned by the Git process.
    pub exit_code: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// Stable output returned by argument-based Git execution.
pub struct GitCommandResponse {
    /// Exact arguments for the final Git subprocess, retained for compatibility.
    pub arguments: Vec<String>,
    /// Backward-compatible concatenation of standard output followed by standard error.
    pub output: String,
    /// Text captured from the final Git process standard output stream.
    pub stdout: String,
    /// Text captured from the final Git process standard error stream.
    pub stderr: String,
    /// Exit status returned by the final Git process.
    pub exit_code: i32,
    /// Every Git subprocess executed for the operation, in execution order.
    pub invocations: Vec<GitCommandInvocation>,
    /// Failure discovered after one or more subprocesses were recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_error: Option<CoreError>,
    /// Present when a stash restore kept its entry because the working tree
    /// contains an unresolved merge. Keeping this out of the prose response
    /// lets bindings offer recovery actions without matching localized Git
    /// output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stash_restore: Option<GitStashRestoreResponse>,
    /// Present when a tag deletion succeeded, carrying everything a host needs
    /// to offer a restore without re-querying the repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_deletion: Option<GitTagDeletionResponse>,
    /// Present when a local branch deletion succeeded, carrying the commit the
    /// branch pointed at so the host can offer to recreate it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_deletion: Option<GitBranchDeletionResponse>,
    /// Durable recovery point and authoritative outcome for a reviewed history action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_rewrite: Option<GitHistoryRewriteResult>,
    /// Internal composite-operation guard; subsequent probes must not change its outcome.
    #[serde(skip)]
    pub(crate) outcome_authoritative: bool,
    /// Non-fatal follow-up failures after the requested repository mutation succeeded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<GitOperationWarning>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Stable non-fatal outcome emitted after a Git mutation has already succeeded.
pub struct GitOperationWarning {
    /// Machine-readable warning category interpreted by platform presentation.
    pub code: String,
    /// Safe English fallback for clients without a localized presentation.
    pub message: String,
    /// Optional Git diagnostic retained for troubleshooting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl GitOperationWarning {
    pub(crate) fn new(code: &str, message: &str, details: Option<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            details,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Recovery context when restoring a stash produces conflicts.
pub struct GitStashRestoreResponse {
    pub(crate) stash_reference: String,
    pub(crate) conflicted_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Deletion record that lets a host rebuild the deleted tag later.
///
/// `deleted_target` is the commit the deleted ref resolved to (peeled for
/// annotated tags), so a restore can re-point a new tag at the same commit.
pub struct GitTagDeletionResponse {
    /// Short name of the deleted tag, without the `refs/tags/` prefix.
    pub name: String,
    pub(crate) deleted_target: String,
    /// `lightweight` or `annotated`, taken from the tag object type.
    pub kind: String,
    /// Annotation message; `None` only for lightweight tags. Empty annotated
    /// messages remain `Some` so a restore does not change the tag form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Deletion record that lets a host recreate the deleted local branch later.
pub struct GitBranchDeletionResponse {
    /// Short branch name, without the `refs/heads/` prefix.
    pub name: String,
    /// Commit the deleted branch pointed at when it was removed.
    pub deleted_target: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Complete identity of a Git reference supplied by a platform client.
pub struct GitReferenceRequest {
    /// Fully qualified reference, such as `refs/remotes/origin/main`.
    pub full_name: String,
    /// User-facing short name, such as `origin/main`.
    pub short_name: String,
    /// Reference namespace: `local`, `remote`, or `tag`.
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Repository snapshot that a previously reviewed push preview was based on.
pub struct GitPushExpectationRequest {
    /// Local branch resolved when the preview was created.
    pub local_branch: String,
    /// Commit at the tip of the local branch when the preview was created.
    pub local_head: String,
    /// Push remote resolved from the repository configuration.
    pub remote: String,
    /// Destination branch name on the resolved remote.
    pub remote_branch: String,
    /// Destination relationship recorded by the preview. Optional so an older
    /// caller that predates target kinds keeps its previous behaviour.
    #[serde(default)]
    pub target_kind: Option<GitPushTargetKind>,
    /// Locally observed destination OID, or `None` when the remote branch was absent.
    #[serde(default)]
    pub remote_tracking_oid: Option<String>,
    /// Exact reviewed tags that may be sent with the branch.
    #[serde(default)]
    pub tags: Vec<GitPushTagExpectationRequest>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// One tag identity copied from a push preview into the mutation request.
pub struct GitPushTagExpectationRequest {
    /// Fully qualified tag reference under `refs/tags/`.
    pub full_name: String,
    /// Tag object ID observed while the preview was created.
    pub object_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Typed mutation request translated into a controlled Git invocation.
pub struct GitWriteRequest {
    pub(crate) root: String,
    /// Stable mutation discriminator interpreted by [`write()`].
    pub operation: String,
    /// Optional Fetch choices; omitted values preserve the legacy scope and pruning policy.
    #[serde(default)]
    pub fetch_options: Option<GitFetchOptions>,
    #[serde(default)]
    pub(crate) paths: Vec<String>,
    #[serde(default)]
    pub(crate) reference: Option<String>,
    /// Preferred typed reference. Legacy callers may still use `reference` and
    /// `referenceKind`, but new cross-platform workflows send all identity fields.
    #[serde(default)]
    pub git_reference: Option<GitReferenceRequest>,
    /// Reference category used by checkout: `local`, `remote`, or `tag`.
    #[serde(default)]
    pub reference_kind: Option<String>,
    #[serde(default)]
    pub(crate) revision: Option<String>,
    /// Commit revisions selected by an operation that rewrites a contiguous range.
    #[serde(default)]
    pub revisions: Vec<String>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) message: Option<String>,
    /// `createTag`: create an explicitly lightweight tag, overriding `tag.gpgSign`.
    /// Omission keeps the legacy message-based annotated/lightweight choice.
    #[serde(default)]
    pub lightweight: Option<bool>,
    /// `createTag`: sign the annotated tag with the configured GPG key.
    /// Omission keeps the legacy unsigned form.
    #[serde(default)]
    pub signed: Option<bool>,
    #[serde(default)]
    pub(crate) remote: Option<String>,
    #[serde(default)]
    pub(crate) destination: Option<String>,
    /// Operation-specific strategy, such as reset mode or pull reconciliation.
    #[serde(default)]
    pub mode: Option<String>,
    /// Complete local branch ref for a background update after Fetch. The guarded
    /// path integrates its fetched upstream without a second network wait.
    /// Omission preserves interactive Pull compatibility.
    #[serde(default)]
    pub expected_branch: Option<String>,
    #[serde(default)]
    pub(crate) include_untracked: bool,
    #[serde(default)]
    pub(crate) checkout: bool,
    /// Worktree creation mode; omitted values retain `newBranch` compatibility.
    #[serde(default)]
    pub worktree_mode: Option<String>,
    /// Whether worktree creation should leave tracked files unpopulated.
    /// Independent of `checkout`, which belongs to branch creation workflows.
    #[serde(default)]
    pub no_checkout: bool,
    #[serde(default)]
    pub(crate) amend: bool,
    /// `commit`: the author as `Name <email>`, in place of the configured identity.
    /// The committer stays the configured identity.
    #[serde(default)]
    pub author: Option<String>,
    /// `commit`: append a `Signed-off-by` trailer for the committer.
    #[serde(default)]
    pub sign_off: bool,
    /// Reviewed HEAD/index required by a planned workspace commit.
    #[serde(default)]
    pub expected_commit_state: Option<GitCommitState>,
    /// Exact child commits to record; requires expected_commit_state.
    #[serde(default)]
    pub gitlink_updates: Vec<GitCommitGitlink>,
    #[serde(default)]
    pub(crate) force: bool,
    /// Tag scope for push: `none`, `all`, or `reachable`.
    #[serde(default)]
    pub push_tags: Option<String>,
    /// Optional reviewed preview snapshot that must still match before pushing.
    #[serde(default)]
    pub expected_push: Option<GitPushExpectationRequest>,
    /// Verify submodule commits are published before a workspace branch push.
    #[serde(default)]
    pub check_submodules: bool,
    /// Destination the user explicitly confirmed. Required only when the branch
    /// tracks a divergent upstream; every other destination stays implicit.
    #[serde(default)]
    pub push_target: Option<PushTargetChoiceRequest>,
    #[serde(default)]
    pub(crate) auto_stash: bool,
    /// Mandatory immutable preview for undo, reword, squash, and drop.
    #[serde(default)]
    pub expected_state: Option<GitHistoryRewriteExpectation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to resolve the exact destination and commits for a branch push.
pub struct GitPushPreviewRequest {
    pub(crate) root: String,
    #[serde(default)]
    pub(crate) reference: Option<String>,
    /// Preferred complete local branch identity.
    #[serde(default)]
    pub git_reference: Option<GitReferenceRequest>,
    #[serde(default = "default_push_preview_limit")]
    pub(crate) limit: usize,
    /// Tag scope to resolve into an immutable preview: `none`, `all`, or `reachable`.
    #[serde(default)]
    pub push_tags: Option<String>,
    /// Destination the user picked for a divergent upstream. The preview then
    /// describes that exact remote branch, so a reviewed push can be bound to it.
    /// Ignored when the branch is not divergent.
    #[serde(default)]
    pub push_target: Option<PushTargetChoiceRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for a structured diff suitable for side-by-side rendering.
pub struct GitDiffRequest {
    pub(crate) root: String,
    /// Inspect a saved stash, including its untracked parent, independently of the worktree.
    #[serde(default)]
    pub stash_reference: Option<String>,
    /// Return validated per-file sections instead of a flattened diff.
    #[serde(default)]
    pub include_file_sections: bool,
    /// Omit the redundant whole-file patch when individual hunks suffice.
    #[serde(default)]
    pub omit_file_patch: bool,
    pub(crate) pathspecs: Vec<String>,
    #[serde(default)]
    pub(crate) reference: Option<String>,
    /// Compare the supplied legacy `reference` target against Git's empty tree.
    /// This is used for cumulative review ranges that begin with a root commit.
    #[serde(default)]
    pub empty_tree_base: bool,
    #[serde(default)]
    pub(crate) git_reference: Option<GitReferenceRequest>,
    /// Optional typed comparison target. When present, `gitReference` is the
    /// base and Core constructs the validated two-reference range.
    #[serde(default)]
    pub target_git_reference: Option<GitReferenceRequest>,
    #[serde(default)]
    pub(crate) commit: Option<String>,
    #[serde(default)]
    pub(crate) staged: bool,
    #[serde(default)]
    pub(crate) untracked: bool,
    /// Compare HEAD with the complete worktree state that a selected-path
    /// commit would stage, independently of the real index.
    #[serde(default)]
    pub worktree_snapshot: bool,
    #[serde(default = "default_review_context_lines")]
    pub(crate) context_lines: usize,
    #[serde(default)]
    pub(crate) ignore_all_whitespace: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to apply a patch to the index or working tree.
pub struct GitApplyRequest {
    pub(crate) root: String,
    pub(crate) patch: String,
    /// Patch target or validation mode, including `stage`, `unstage`, and `worktree`.
    pub mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for metadata and parent information about one commit.
pub struct GitCommitRequest {
    pub(crate) root: String,
    pub(crate) commit: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for the paths changed by one commit.
pub struct GitCommitFilesRequest {
    pub(crate) root: String,
    pub(crate) commit: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to compare a reference with the current checkout.
pub struct GitComparisonRequest {
    pub(crate) root: String,
    #[serde(default)]
    pub(crate) reference: Option<String>,
    #[serde(default)]
    pub(crate) git_reference: Option<GitReferenceRequest>,
    /// Optional typed target for a comparison between two references.
    #[serde(default)]
    pub target_git_reference: Option<GitReferenceRequest>,
    /// Include untracked worktree files; incompatible with reference-to-reference comparisons.
    #[serde(default)]
    pub include_untracked: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for the repository's ordered stash list.
pub struct GitStashesRequest {
    pub(crate) root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to identify local edits that would block switching references.
pub struct GitCheckoutPreflightRequest {
    pub(crate) root: String,
    #[serde(default)]
    pub(crate) reference: Option<String>,
    #[serde(default)]
    pub(crate) git_reference: Option<GitReferenceRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to find staged files that still contain conflict markers.
pub struct GitConflictMarkerRequest {
    pub(crate) root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to determine whether a merge or rebase can start safely.
pub struct GitIntegrationPreflightRequest {
    pub(crate) root: String,
    #[serde(default)]
    pub(crate) reference: Option<String>,
    #[serde(default)]
    pub(crate) git_reference: Option<GitReferenceRequest>,
    /// Either "merge" or "rebase"; the two have different tolerances for a dirty tree.
    pub operation: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to determine whether the tracked branch can fast-forward.
pub struct GitPullPreflightRequest {
    pub(crate) root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to inspect an interrupted merge, rebase, cherry-pick, or revert.
pub struct GitOperationStateRequest {
    pub(crate) root: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for line attribution on one workspace-relative file.
pub struct GitBlameRequest {
    pub(crate) root: String,
    pub(crate) path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for the committed history of one one-based snapshot line range.
pub struct GitLineHistoryRequest {
    pub(crate) root: String,
    /// Workspace-relative file path validated as a safe pathspec.
    pub path: String,
    /// Editor buffer content at query time; the line range addresses this
    /// snapshot, not the on-disk file, so unsaved edits stay answerable.
    pub content: String,
    /// First one-based snapshot line to trace, inclusive.
    pub start_line: usize,
    /// Last one-based snapshot line to trace, inclusive.
    pub end_line: usize,
    /// Maximum deduplicated entries to return; defaults to 50, capped at 200.
    #[serde(default)]
    pub limit: Option<usize>,
}

fn default_review_context_lines() -> usize {
    80
}

fn default_push_preview_limit() -> usize {
    DEFAULT_PUSH_PREVIEW_LIMIT
}

fn default_repository_scan_max_directories() -> usize {
    DEFAULT_REPOSITORY_SCAN_MAX_DIRECTORIES
}

fn default_repository_scan_max_depth() -> usize {
    DEFAULT_REPOSITORY_SCAN_MAX_DEPTH
}
