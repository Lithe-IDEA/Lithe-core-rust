//! Zero-based LSP positions and ranges in UTF-16 code units, as hosts send
//! them and as Core returns them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Half-open document range expressed in zero-based LSP coordinates.
pub struct LspRange {
    /// First position inside the range.
    pub start: LspPosition,
    /// Position just after the range.
    pub end: LspPosition,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Zero-based LSP line and UTF-16 code-unit column.
pub struct LspPosition {
    /// Zero-based line.
    pub line: i64,
    /// Zero-based column in UTF-16 code units.
    pub utf16_column: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Normalized text replacement returned to host applications.
pub struct LspTextEditResponse {
    /// Range the replacement covers.
    pub range: LspRangeResponse,
    /// Text that replaces the range.
    pub new_text: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Serializable form of an LSP document range.
pub struct LspRangeResponse {
    /// First position inside the range.
    pub start: LspPositionResponse,
    /// Position just after the range.
    pub end: LspPositionResponse,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Serializable form of a zero-based LSP position.
pub struct LspPositionResponse {
    /// Zero-based line.
    pub line: i64,
    /// Zero-based column in UTF-16 code units.
    pub utf16_column: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// One LSP `textDocument/didChange` content change.
pub struct LspDocumentContentChange {
    /// Inclusive start / exclusive end range; omitted for a full-document replacement.
    #[serde(default)]
    pub range: Option<LspRange>,
    /// Text that replaces the range, or the whole document when the range is
    /// omitted.
    #[serde(default)]
    pub text: String,
}
