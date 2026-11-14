//! Serializable response models of the project services: workspace snapshots,
//! directory listings, search results, file reads and writes, and local history.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One file or directory in a deterministic workspace tree.
pub struct WorkspaceNode {
    /// Normalized workspace-relative path; empty for the root.
    pub path: String,
    /// Last path component, or the root folder's name.
    pub name: String,
    /// Whether this node is a directory.
    pub is_directory: bool,
    /// Omitted for unloaded directories/files; an empty array is a known directory leaf.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<WorkspaceNode>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Complete visible workspace tree and its flattened file paths.
pub struct WorkspaceSnapshotResponse {
    /// Optional directories that could not be read; hosts must not infer deletions from this inventory.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incomplete_directories: Vec<String>,
    /// Root of the visible tree; it has no children when the request skipped the tree.
    pub root: WorkspaceNode,
    /// Every visible workspace-relative file path.
    pub files: Vec<String>,
    /// Linked worktrees checked out inside their own repository. They are left
    /// out of `root` and `files`; hosts use them to ignore watcher events.
    pub nested_worktrees: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Visible children of the directories requested by `workspace.listDirectories`.
pub struct DirectoryListingResponse {
    /// One entry per requested directory that still exists and is visible,
    /// in request order.
    pub directories: Vec<DirectoryListing>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One directory's children, each without its own children.
pub struct DirectoryListing {
    /// Normalized workspace-relative path; empty for the root.
    pub path: String,
    /// Directories first, then case-insensitive names, as in snapshots.
    pub children: Vec<WorkspaceNode>,
    /// Nested linked worktrees directly inside this directory, which
    /// `children` leaves out.
    pub nested_worktrees: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// File, content, or symbol match with optional source location.
pub struct SearchMatch {
    /// Result category: file path, file content, or symbol.
    pub kind: String,
    /// Workspace-relative path of the matched file.
    pub path: String,
    /// One-based line of a content match; absent for file and symbol matches.
    pub line: Option<usize>,
    /// Text shown for the match: the trimmed line, the path, or the symbol name.
    pub preview: String,
    /// Symbol name of a symbol match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    /// Matching fragments in the displayed name, measured in UTF-16 code units.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub name_match_ranges: Vec<SearchNameRange>,
    /// Original content line, present only when requested by the host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line: Option<String>,
    /// Content match ranges in UTF-16 units of `sourceLine`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub content_match_ranges: Vec<SearchNameRange>,
    /// Name relevance computed before category truncation; absent for content search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matching_degree: Option<i32>,
    /// Git ignore status for a returned name; absent when Git cannot answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_ignored: Option<bool>,
}

/// One half-open matched fragment in a displayed file or symbol name.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchNameRange {
    /// First matched UTF-16 code unit.
    pub start: usize,
    /// Matched length in UTF-16 code units.
    pub length: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Bounded, deterministically ordered search matches.
pub struct SearchResponse {
    /// Matches in result order, already capped per category.
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Preview of replacements occurring on one source line.
pub struct ReplacementMatch {
    /// One-based line holding the replaced occurrences.
    pub line: usize,
    /// The line before replacement.
    pub before: String,
    /// The line after every occurrence on it is replaced.
    pub after: String,
    /// Number of occurrences replaced on this line.
    pub occurrence_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// All preview matches and resulting text for one file.
pub struct ReplacementFile {
    /// Workspace-relative path of the file.
    pub path: String,
    /// Changed lines in line order.
    pub matches: Vec<ReplacementMatch>,
    /// Whole file text after replacement.
    pub replacement_text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Non-mutating replacement preview grouped by workspace-relative path.
pub struct ReplacementPreviewResponse {
    /// Files with at least one replacement, sorted by path.
    pub files: Vec<ReplacementFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Decoded contents read from one workspace-relative path.
pub struct FileReadResponse {
    /// Normalized workspace-relative path that was read.
    pub path: String,
    /// Decoded text without the byte order mark.
    pub text: String,
    /// Stable id of the encoding used to decode `text`.
    pub encoding: String,
    /// A leading byte order mark was present and is not part of `text`.
    pub has_bom: bool,
    /// Invalid bytes were replaced with U+FFFD (explicit `encoding` only).
    pub lossy: bool,
    /// SHA-256 (lowercase hex) of the exact bytes read.
    pub identity: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Path and byte count produced by a successful file write.
pub struct FileWriteResponse {
    /// Normalized workspace-relative path that was written.
    pub path: String,
    /// Encoded length including any byte order mark.
    pub bytes_written: usize,
    /// Stable id of the encoding the text was written with.
    pub encoding: String,
    /// A byte order mark was written.
    pub has_bom: bool,
    /// SHA-256 (lowercase hex) of the exact bytes written.
    pub identity: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Metadata for one retained local-history snapshot.
pub struct HistoryEntryResponse {
    /// Snapshot identity, unique within the file's history.
    pub id: String,
    /// Capture time in seconds since the Unix epoch.
    pub timestamp: i64,
    /// Workspace-relative path of the file the snapshot belongs to.
    pub relative_path: String,
    /// Why the snapshot was taken, such as `projectBaseline`; see [`crate::is_valid_reason`].
    pub reason: String,
    /// Storage-relative path of the snapshot content, read with `history.content`.
    pub content_path: String,
    /// Size of the snapshot content in bytes.
    pub byte_count: usize,
    /// User label assigned through `history.rename`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Local-history entries in deterministic newest-first order.
pub struct HistoryEntriesResponse {
    /// Snapshot metadata, newest first.
    pub entries: Vec<HistoryEntryResponse>,
}
