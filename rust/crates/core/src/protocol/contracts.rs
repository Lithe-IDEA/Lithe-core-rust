//! Serializable response models shared across every host boundary.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One file or directory in a deterministic workspace tree.
pub struct WorkspaceNode {
    pub path: String,
    pub name: String,
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
    pub root: WorkspaceNode,
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
    pub path: String,
    pub line: Option<usize>,
    pub preview: String,
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
    pub start: usize,
    pub length: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Bounded, deterministically ordered search matches.
pub struct SearchResponse {
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Preview of replacements occurring on one source line.
pub struct ReplacementMatch {
    pub line: usize,
    pub before: String,
    pub after: String,
    pub occurrence_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// All preview matches and resulting text for one file.
pub struct ReplacementFile {
    pub path: String,
    pub matches: Vec<ReplacementMatch>,
    pub replacement_text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Non-mutating replacement preview grouped by workspace-relative path.
pub struct ReplacementPreviewResponse {
    pub files: Vec<ReplacementFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Decoded contents read from one workspace-relative path.
pub struct FileReadResponse {
    pub path: String,
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
    pub id: String,
    pub timestamp: i64,
    pub relative_path: String,
    pub reason: String,
    pub content_path: String,
    pub byte_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Local-history entries in deterministic newest-first order.
pub struct HistoryEntriesResponse {
    pub entries: Vec<HistoryEntryResponse>,
}
