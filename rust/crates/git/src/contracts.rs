//! Serializable Git responses shared across every host boundary.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One Git repository discovered for an opened workspace.
pub struct WorkspaceRepositoryResponse {
    /// Absolute native repository root path reported by the host filesystem.
    /// Windows paths are plain drive or UNC paths without the verbatim `\\?\`
    /// prefix that canonicalization adds.
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Deterministically ordered Git repositories discovered below a workspace root.
pub struct WorkspaceRepositoriesResponse {
    pub(crate) repositories: Vec<WorkspaceRepositoryResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One normalized index or working-tree change.
pub struct GitChange {
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) original_path: Option<String>,
    /// Normalized porcelain status code for the path.
    pub status: String,
    pub(crate) staged: bool,
    pub(crate) worktree: bool,
    pub(crate) untracked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) submodule: Option<GitSubmoduleStatus>,
    /// False for parent gitlinks with only uncommitted child content.
    pub can_toggle_staging: bool,
    /// Present only on a summarized untracked directory (`path` ends with `/`):
    /// the number of non-ignored files below it. The directory contains no
    /// nested repository, so staging or cleaning `path` acts on exactly these files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untracked_file_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Git porcelain v2 distinguishes a new gitlink target from child worktree dirt.
pub struct GitSubmoduleStatus {
    pub(crate) commit_changed: bool,
    pub(crate) tracked_changes: bool,
    pub(crate) untracked_changes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Repository identity, branch divergence, and normalized file changes.
pub struct GitStatusResponse {
    pub(crate) repository_root: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
    pub(crate) changes: Vec<GitChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Immediate files and subdirectory summaries of one summarized untracked directory.
pub struct GitUntrackedDirectoryResponse {
    pub(crate) changes: Vec<GitChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Worktree-aware Git paths that a platform watcher should observe.
pub struct GitWatchContextResponse {
    pub(crate) repository_root: String,
    pub(crate) git_directory: String,
    pub(crate) git_common_directory: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One checkout registered in a repository's shared worktree metadata.
pub struct GitWorktreeResponse {
    /// Absolute checkout path reported by Git. Linked worktrees may live outside the opened workspace.
    pub path: String,
    /// Commit currently checked out by this worktree.
    pub head: String,
    /// Fully qualified local branch reference, absent for detached or bare worktrees.
    pub branch: Option<String>,
    /// Whether this is the worktree from which the request was made.
    pub is_current: bool,
    /// Whether this is the repository's primary worktree.
    pub is_primary: bool,
    pub(crate) is_bare: bool,
    pub(crate) is_detached: bool,
    pub(crate) is_locked: bool,
    /// Human-readable lock reason supplied to Git, when present.
    pub lock_reason: Option<String>,
    pub(crate) is_prunable: bool,
    /// Git's explanation for why the registration can be pruned.
    pub prune_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Deterministically ordered worktrees registered for one repository.
pub struct GitWorktreesResponse {
    pub(crate) worktrees: Vec<GitWorktreeResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Local or remote Git reference in display-ready form.
pub struct GitReferenceResponse {
    pub(crate) full_name: String,
    pub(crate) short_name: String,
    /// Reference category: local branch, remote branch, or tag.
    pub kind: String,
    /// Whether the reference resolves to a commit and therefore supports
    /// commit-only mutations such as restorable tag deletion.
    pub peels_to_commit: bool,
    pub(crate) is_current: bool,
    pub(crate) upstream_short_name: Option<String>,
    /// Commits present only on this local branch compared with its upstream.
    pub ahead: usize,
    /// Commits present only on this local branch's upstream.
    pub behind: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Commit metadata parsed from a machine-stable field format.
pub struct GitCommitResponse {
    pub(crate) hash: String,
    pub(crate) short_hash: String,
    pub(crate) parent_hashes: Vec<String>,
    pub(crate) author_name: String,
    pub(crate) author_email: String,
    /// Author date as `YYYY/MM/DD HH:MM` in the author's own time zone.
    pub date: String,
    /// Author's UTC offset in minutes (east positive) for `date`; `null` when
    /// Git did not report one. Hosts compare it with the local offset before
    /// presenting relative times.
    pub date_utc_offset_minutes: Option<i32>,
    /// Absolute author instant in Unix seconds when the date and offset are valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_unix_seconds: Option<i64>,
    pub(crate) subject: String,
    pub(crate) decorations: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// References and a bounded page of commit history.
pub struct GitHistoryResponse {
    pub(crate) references: Vec<GitReferenceResponse>,
    /// Up to five local branches ordered from most to least recently checked out.
    pub recent_references: Vec<GitReferenceResponse>,
    pub(crate) commits: Vec<GitCommitResponse>,
    pub(crate) has_more: bool,
    pub(crate) user_name: Option<String>,
    pub(crate) user_email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Repository references and identity metadata loaded independently from commit pages.
pub struct GitReferencesResponse {
    pub(crate) references: Vec<GitReferenceResponse>,
    /// Up to five local branches ordered from most to least recently checked out.
    pub recent_references: Vec<GitReferenceResponse>,
    pub(crate) user_name: Option<String>,
    pub(crate) user_email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One bounded page of commit history.
pub struct GitHistoryPageResponse {
    pub(crate) commits: Vec<GitCommitResponse>,
    /// Opaque cursor for the next page, or `None` when this page reaches the end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Deprecated offset emitted only for compatibility with offset-based requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub(crate) has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Result of explicitly releasing an incremental Git history cursor.
pub struct GitHistoryCursorCloseResponse {
    /// Whether an active cursor was found and released.
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// How the resolved push destination relates to the local branch's upstream.
///
/// Callers use this to decide whether a push is a routine repeat of the same
/// branch name or a decision the user has to confirm explicitly.
pub enum GitPushTargetKind {
    /// The branch has no upstream; pushing publishes it to the same-named
    /// remote branch.
    #[serde(rename = "publish")]
    Publish,
    /// The upstream branch has the same name as the local branch.
    #[serde(rename = "tracking")]
    Tracking,
    /// The upstream branch name differs from the local branch name. Pushing
    /// here would update a different branch, so it must not happen implicitly.
    #[serde(rename = "divergent")]
    Divergent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Resolved destination and bounded commits for one branch push.
pub struct GitPushPreviewResponse {
    /// Local branch that will be sent to the remote.
    pub local_branch: String,
    /// Commit at the local branch tip when this preview was resolved.
    pub local_head: String,
    /// Remote selected from the branch upstream or repository defaults.
    pub remote: String,
    /// Branch name created or updated on the remote.
    pub remote_branch: String,
    /// How the destination relates to the configured upstream.
    pub target_kind: GitPushTargetKind,
    /// Whether the destination branch does not exist on the remote yet.
    pub creates_remote_branch: bool,
    /// Locally observed destination OID, or `None` before first publication.
    pub remote_tracking_oid: Option<String>,
    /// Configured upstream short name, or `None` before first publication.
    pub upstream: Option<String>,
    /// Exact tag references and object IDs included in this reviewed push.
    pub tags: Vec<GitPushTagResponse>,
    /// Commits reachable from the local branch but not its resolved remote base.
    pub commits: Vec<GitCommitResponse>,
    /// Whether more commits exist beyond the bounded preview.
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One immutable tag snapshot included in a reviewed push.
pub struct GitPushTagResponse {
    /// Fully qualified tag reference under `refs/tags/`.
    pub full_name: String,
    /// Tag object ID observed while the preview was created.
    pub object_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Exact lookup result for one commit.
pub struct GitCommitLookupResponse {
    pub(crate) commit: GitCommitResponse,
    /// Message text after the subject paragraph, as Git's `%b` placeholder,
    /// with line breaks preserved and trailing whitespace removed. Empty when
    /// the message has only a subject. History pages omit it to stay compact.
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Path and status changed by a commit or comparison.
pub struct GitFileResponse {
    /// Normalized name-status code such as `A`, `M`, `D`, or `R`.
    pub status: String,
    pub(crate) path: String,
    /// True only for a worktree comparison entry absent from the index.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_untracked: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Deterministically ordered changed paths.
pub struct GitFilesResponse {
    pub(crate) files: Vec<GitFileResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Paths differing between a reference and the current checkout.
pub struct GitComparisonResponse {
    pub(crate) files: Vec<GitFileResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One stash entry with its stable reference and parsed metadata.
pub struct GitStashResponse {
    pub(crate) reference: String,
    /// Immutable object identity; stash positions may change after another push.
    pub oid: String,
    pub(crate) message: String,
    pub(crate) branch: Option<String>,
    pub(crate) date: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Stash entries in Git's native newest-first order.
pub struct GitStashesResponse {
    pub(crate) stashes: Vec<GitStashResponse>,
}

/// Result of checking whether a checkout can proceed without losing local edits.
///
/// `blocking_paths` holds files that are dirty in the working tree *and* differ
/// between HEAD and the target ref. Computing the intersection ourselves avoids
/// parsing Git's stderr, which is localized and therefore unreliable to match.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckoutPreflightResponse {
    pub(crate) blocking_paths: Vec<String>,
}

/// Staged files still holding conflict markers, which must never be committed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitConflictMarkerResponse {
    pub(crate) paths: Vec<String>,
}

/// Whether a merge or rebase can start, and what stands in the way.
///
/// The two operations differ, verified against Git rather than assumed: a merge
/// only refuses when a dirty file overlaps what it would write, while a rebase
/// refuses on any uncommitted change at all, related or not. So `blocking_paths`
/// is an overlap set for a merge and the full dirty set for a rebase.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIntegrationPreflightResponse {
    pub(crate) blocking_paths: Vec<String>,
    pub(crate) blocks_entirely: bool,
}

/// Whether a pull can fast-forward, and how far the two sides have drifted.
///
/// `diverged` is the case the UI has to ask about: both sides have commits the
/// other lacks, so `--ff-only` refuses and the user must pick merge or rebase.
/// `upstream` is absent when the branch tracks nothing, which is itself a reason
/// to stop before running anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPullPreflightResponse {
    pub(crate) upstream: Option<String>,
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
    pub(crate) diverged: bool,
    pub(crate) has_local_changes: bool,
    /// `rebase`, `merge`, or `ffOnly` when `pull.rebase` or `pull.ff` says so;
    /// absent when Git is not configured for a strategy.
    pub configured_strategy: Option<&'static str>,
}

/// A sequential operation Git left half-finished, usually because of a conflict.
///
/// `kind` is empty when nothing is in progress. `step`/`total` are only populated
/// for a rebase, which is the one operation that reports its own progress.
/// `conflicted_paths` comes from porcelain status codes rather than stderr, so it
/// stays correct under a localized Git.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitOperationStateResponse {
    /// Active operation name, or an empty string when no operation is in progress.
    pub kind: String,
    pub(crate) reference: Option<String>,
    pub(crate) step: Option<usize>,
    pub(crate) total: Option<usize>,
    pub(crate) conflicted_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Line-level blame attribution normalized for editor gutters.
pub struct GitBlameLineResponse {
    pub(crate) line: usize,
    pub(crate) commit_hash: String,
    pub(crate) author_name: String,
    pub(crate) author_time: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Blame attribution ordered by one-based source line.
pub struct GitBlameResponse {
    pub(crate) lines: Vec<GitBlameLineResponse>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One queried snapshot line that could not be mapped onto the pinned revision.
pub struct GitLineHistoryUnmappedLine {
    /// One-based line number in the submitted snapshot.
    pub line: usize,
    /// `new` when the mapping proves the line was inserted; `unreliable` when a
    /// rewrite prevents a trustworthy correspondence to any committed line.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One hunk of an entry's comparison that intersects the traced lines.
pub struct GitLineHistoryRange {
    /// First parent-side one-based line of the hunk, inclusive.
    pub old_start: usize,
    /// Last parent-side line, inclusive; equals `oldStart` for single-line hunks.
    pub old_end: usize,
    /// First entry-side one-based line, inclusive.
    pub new_start: usize,
    /// Last entry-side line, inclusive.
    pub new_end: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One history entry that touched the traced lines, with positioning metadata.
pub struct GitLineHistoryCommit {
    pub(crate) hash: String,
    /// `commit`, or `merge-first-parent` for merges reported against the first
    /// parent only; branch-internal commits of a merge are not listed separately.
    pub kind: String,
    pub(crate) author_name: String,
    /// Authoring time as Unix seconds, matching `git.blame` attribution.
    pub author_time: i64,
    pub(crate) subject: String,
    /// Workspace-relative path of the file at this entry.
    pub path_at_commit: String,
    /// Previous path when this entry renamed the file, otherwise absent.
    pub previous_path: Option<String>,
    /// Parent revision the metadata diff was taken against; empty for the root
    /// commit, whose comparison base is the empty tree on the client side.
    pub compare_base: String,
    /// Hunks of this entry's comparison that intersect the traced lines.
    pub relevant_ranges: Vec<GitLineHistoryRange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// History of a one-based snapshot line range anchored to one pinned revision.
pub struct GitLineHistoryResponse {
    /// Revision the snapshot mapping and history query were resolved against.
    pub base_commit: String,
    /// Queried lines absent from the pinned revision, in snapshot order.
    pub unmapped_lines: Vec<GitLineHistoryUnmappedLine>,
    /// Touching entries, newest first, deduplicated across traced intervals.
    pub commits: Vec<GitLineHistoryCommit>,
    /// True when more entries exist beyond the effective limit.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One aligned side-by-side diff row.
pub struct GitDiffRowResponse {
    pub(crate) old_line: Option<usize>,
    pub(crate) new_line: Option<usize>,
    pub(crate) left: Option<String>,
    /// Omitted for `context` and `information` rows, whose two sides always
    /// hold identical text; clients fall back to `left` in that case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
    /// Rendering category such as context, changed, insertion, deletion, or information.
    pub kind: String,
    pub(crate) hunk_id: Option<String>,
    /// Both sides look equal only after removing a trailing carriage return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_invisible_change: Option<bool>,
    /// For a `changed` row, the stretches of `left` that differ from `right`, as
    /// half-open UTF-16 column ranges (`diff::inline_changes`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_changes: Option<Vec<[usize; 2]>>,
    /// The stretches of `right` that differ from `left`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_changes: Option<Vec<[usize; 2]>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One original-order display line from a unified diff.
pub struct GitDiffUnifiedLineResponse {
    pub(crate) line_type: String,
    pub(crate) content: String,
    pub(crate) old_line: Option<usize>,
    pub(crate) new_line: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Independently applicable diff hunk and its original patch text.
pub struct GitDiffHunkResponse {
    pub(crate) id: String,
    pub(crate) header: String,
    pub(crate) patch: String,
    /// A rename's content-only patch preserves its file identity when applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_patch: Option<String>,
    /// First unified line (index into the file's `lines`) this hunk covers.
    /// Present for complete-file diffs, which Core splits into Git-default hunks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_start: Option<usize>,
    /// End-exclusive unified line index; paired with `line_start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// A saved file's exact patch and independently rendered document.
pub struct GitDiffFileResponse {
    pub(crate) path: String,
    /// Original Git path for a rename or copy.
    pub original_path: Option<String>,
    /// Git status used by the saved-changes list.
    pub status: String,
    pub(crate) patch: String,
    pub(crate) rows: Vec<GitDiffRowResponse>,
    pub(crate) hunks: Vec<GitDiffHunkResponse>,
    /// Unified lines in Git patch order, with display-only CR normalization.
    pub lines: Vec<GitDiffUnifiedLineResponse>,
    pub(crate) is_binary: bool,
    /// True if display lines omit CR, EOF markers or invalid UTF-8 bytes.
    pub has_lossy_line_endings: bool,
    pub(crate) additions: Option<u64>,
    pub(crate) deletions: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Complete patch plus rendering rows and independently applicable hunks.
pub struct GitDiffResponse {
    pub(crate) patch: String,
    pub(crate) rows: Vec<GitDiffRowResponse>,
    pub(crate) hunks: Vec<GitDiffHunkResponse>,
    /// True when stdout contained invalid UTF-8 replaced by U+FFFD; the
    /// patch bytes are not a faithful representation of the file content.
    #[serde(default)]
    pub has_decoding_loss: bool,
    /// Present only when per-file output was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<GitDiffFileResponse>>,
}
