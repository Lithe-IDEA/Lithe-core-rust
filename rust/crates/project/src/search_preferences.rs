//! User-selected search budgets, resolved once and shared by every desktop consumer.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};

/// Stored preferences retain absence so opening settings never creates an override.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPreferencesRequest {
    /// Maximum disk bytes searched per file; absent keeps the default.
    pub max_file_bytes: Option<u64>,
    /// Maximum disk bytes offered for replacement; absent keeps the default.
    pub replacement_max_file_bytes: Option<u64>,
}

/// Actual budgets used by matching, decoding and guarded native I/O.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPreferences {
    /// Search budget in bytes.
    pub max_file_bytes: u64,
    /// Replacement budget in bytes.
    pub replacement_max_file_bytes: u64,
    /// Largest supported setting, protecting the boundary from unbounded allocations.
    pub maximum_file_bytes: u64,
}

/// Validates stored values instead of silently replacing an invalid preference.
pub fn resolve_search_preferences(
    request: SearchPreferencesRequest,
) -> Result<SearchPreferences, CoreError> {
    let maximum = crate::files::MAX_OPEN_FILE_SIZE;
    let max_file_bytes = request.max_file_bytes.unwrap_or(16 * 1024 * 1024);
    let replacement_max_file_bytes = request.replacement_max_file_bytes.unwrap_or(max_file_bytes);
    if [max_file_bytes, replacement_max_file_bytes]
        .iter()
        .any(|bytes| *bytes == 0 || *bytes > maximum)
    {
        return Err(
            CoreError::new(ErrorCode::InvalidRequest, "Invalid search file budget")
                .with_details("invalid_file_budget"),
        );
    }
    Ok(SearchPreferences {
        max_file_bytes,
        replacement_max_file_bytes,
        maximum_file_bytes: maximum,
    })
}
