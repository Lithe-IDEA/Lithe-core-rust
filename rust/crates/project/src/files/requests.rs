//! Request and response payloads of the workspace file commands.

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Workspace root and visibility overrides used to build a file-tree snapshot.
pub struct WorkspaceSnapshotRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Explicit roots of this workspace; nested roots own their inventories independently.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Extra directory names hidden from the tree, on top of the built-in ones.
    #[serde(default)]
    pub hidden_directory_names: Vec<String>,
    /// Hidden-path globs; `!` lines turn built-in hidden folders back on.
    #[serde(default)]
    pub hidden_file_patterns: Vec<String>,
    /// When false, the response keeps only the file list and returns `root`
    /// without children; hosts that load the tree per directory skip encoding
    /// and decoding a node for every path.
    #[serde(default = "default_include_tree")]
    pub include_tree: bool,
}

fn default_include_tree() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Workspace-relative directories whose visible children a host is loading.
pub struct DirectoryListingRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Workspace-relative directories; an empty string is the root itself.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Extra directory names hidden from the tree, on top of the built-in ones.
    #[serde(default)]
    pub hidden_directory_names: Vec<String>,
    /// Hidden-path globs; `!` lines turn built-in hidden folders back on.
    #[serde(default)]
    pub hidden_file_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Bounded file, content, and symbol search options.
pub struct SearchRequest {
    /// Workspace-relative scopes, applied before result caps without changing policy ownership.
    #[serde(default)]
    pub scope_paths: Vec<String>,
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Request-local ownership boundaries; omitted roots preserve single-root behavior.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Text to search for: a literal, or a pattern when `regular_expression` is set.
    pub query: String,
    /// Match letter case exactly.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Match only at word boundaries.
    #[serde(default)]
    pub whole_words: bool,
    /// Treat `query` as a regular expression.
    #[serde(default)]
    pub regular_expression: bool,
    /// Overall result cap, clamped to 1..=10000.
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    /// Cap on file-name matches; absent uses the overall cap.
    #[serde(default)]
    pub max_file_results: Option<usize>,
    /// Cap on content matches; absent uses the overall cap.
    #[serde(default)]
    pub max_content_results: Option<usize>,
    /// Cap on symbol matches; absent uses the overall cap.
    #[serde(default)]
    pub max_symbol_results: Option<usize>,
    /// Search Everywhere may discover generated file names without indexing their contents.
    #[serde(default)]
    pub include_generated_file_names: bool,
    /// Extra directory names hidden from the tree, on top of the built-in ones.
    #[serde(default)]
    pub hidden_directory_names: Vec<String>,
    /// Hidden-path globs; `!` lines turn built-in hidden folders back on.
    #[serde(default)]
    pub hidden_file_patterns: Vec<String>,
    /// Comma-separated file masks such as `*.md, *.txt`; empty means no filter.
    #[serde(default)]
    pub file_mask: String,
    /// Case-insensitive glob fragments that a candidate path must match.
    #[serde(default)]
    pub include_path_patterns: Vec<String>,
    /// Case-insensitive glob fragments that exclude matching candidate paths.
    #[serde(default)]
    pub exclude_path_patterns: Vec<String>,
    /// Open and recent files that break ties between equal file-name matches. Paths are
    /// relative to `root` with `/` separators; without it ordering is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<crate::name_ranking::RankContext>,
    /// Optional root prefix used solely for include/exclude matching.
    #[serde(default)]
    pub path_filter_prefix: String,
    /// Include untrimmed lines and UTF-16 ranges for content matches.
    #[serde(default)]
    pub include_content_ranges: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Workspace root and visibility rules used to warm or invalidate an index.
pub struct SearchIndexRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Explicit workspace roots used when warming this index variant.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Extra directory names hidden from the tree, on top of the built-in ones.
    #[serde(default)]
    pub hidden_directory_names: Vec<String>,
    /// Hidden-path globs; `!` lines turn built-in hidden folders back on.
    #[serde(default)]
    pub hidden_file_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Changed workspace-relative paths to apply to a cached search index.
pub struct SearchIndexUpdateRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Explicit workspace roots used when rebuilding after an update.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Workspace-relative paths that changed on disk.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Extra directory names hidden from the tree, on top of the built-in ones.
    #[serde(default)]
    pub hidden_directory_names: Vec<String>,
    /// Hidden-path globs; `!` lines turn built-in hidden folders back on.
    #[serde(default)]
    pub hidden_file_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Search-index size and whether an incremental request required a rebuild.
pub struct SearchIndexStatusResponse {
    /// Files held in the index.
    pub file_count: usize,
    /// Always zero: Core no longer indexes language symbols. The field stays
    /// because the macOS bridge still decodes it as required.
    pub symbol_count: usize,
    /// Trigram postings held in the index.
    pub posting_count: usize,
    /// Whether the update had to discard the index for a full rebuild.
    pub rebuilt: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Search and replacement options used to produce a non-mutating preview.
pub struct ReplacementPreviewRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Nested roots are previewed through their own root and policy.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    /// Text to replace: a literal, or a pattern when `regular_expression` is set.
    pub query: String,
    /// Replacement text; regex replacements may reference capture groups.
    pub replacement: String,
    /// Match letter case exactly.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Match only at word boundaries.
    #[serde(default)]
    pub whole_words: bool,
    /// Treat `query` as a regular expression.
    #[serde(default)]
    pub regular_expression: bool,
    /// Preserve all-uppercase or initial-uppercase shape from literal matches.
    #[serde(default)]
    pub preserve_case: bool,
    /// Comma-separated file masks such as `*.md, *.txt`; empty means no filter.
    #[serde(default)]
    pub file_mask: String,
    /// Workspace-relative files to preview; empty previews every indexed candidate.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Unsaved editor text by workspace-relative path, used instead of the file on disk.
    #[serde(default)]
    pub text_overrides: HashMap<String, String>,
    /// Extra directory names hidden from the tree, on top of the built-in ones.
    #[serde(default)]
    pub hidden_directory_names: Vec<String>,
    /// Hidden-path globs; `!` lines turn built-in hidden folders back on.
    #[serde(default)]
    pub hidden_file_patterns: Vec<String>,
}

/// Non-mutating replacement against an already captured text snapshot.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplacementTextRequest {
    /// Original content; the host retains its identity and controls any write.
    pub text: String,
    /// Search expression interpreted identically to workspace search.
    pub query: String,
    /// Replacement text, including capture expansion for regex searches.
    pub replacement: String,
    /// Whether letter case must match exactly.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Require the shared word-boundary rules for literal searches.
    #[serde(default)]
    pub whole_words: bool,
    /// Treat the query as a regular expression.
    #[serde(default)]
    pub regular_expression: bool,
    /// When present, replace one match touching or following this location.
    pub from: Option<ReplacementTextPosition>,
}

/// A one-based source line and UTF-16 column.
#[derive(Debug, Deserialize)]
pub struct ReplacementTextPosition {
    /// One-based line index.
    pub line: usize,
    /// One-based UTF-16 column.
    pub column: usize,
}

/// Provider-supplied snapshots for content search without filesystem access.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTextBatchRequest {
    /// User query, interpreted with the same rules as workspace search.
    pub query: String,
    /// Whether case must match exactly.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Restrict literal queries to shared word boundaries.
    #[serde(default)]
    pub whole_words: bool,
    /// Interpret the query as a regular expression.
    #[serde(default)]
    pub regular_expression: bool,
    /// Adjacent lines supplied for each displayed result.
    #[serde(default)]
    pub context_lines: usize,
    /// Bounded host-read batch; Core does not resolve these opaque paths.
    pub files: Vec<SearchTextSnapshot>,
}

/// One provider-owned path and its captured content.
#[derive(Debug, Deserialize)]
pub struct SearchTextSnapshot {
    /// Opaque identity returned without modification.
    pub path: String,
    /// Text read by the provider.
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to read one validated workspace-relative text file.
pub struct FileReadRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Workspace-relative path of the file to read.
    pub path: String,
    /// Stable encoding id or alias to decode with; absent means detect.
    #[serde(default)]
    pub encoding: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to replace one validated workspace-relative text file.
pub struct FileWriteRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Workspace-relative path of the file to replace.
    pub path: String,
    /// Complete new file text.
    pub text: String,
    /// Stable encoding id or alias to encode with; absent means UTF-8.
    #[serde(default)]
    pub encoding: Option<String>,
    /// Overrides the encoding's own byte-order-mark choice (Unicode encodings only).
    #[serde(default)]
    pub bom: Option<bool>,
}

fn default_max_results() -> usize {
    200
}
