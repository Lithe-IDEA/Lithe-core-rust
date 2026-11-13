//! Provenance of a request: whether a user started it or a background refresh did.

use serde::{Deserialize, Serialize};

/// Execution provenance is retained for diagnostics; display does not merge commands.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// A user deliberately invoked the operation.
    User,
    /// An automatic refresh or watcher invoked the query.
    Background,
    /// Provenance was not supplied; preserve each execution independently.
    #[default]
    Unknown,
}

/// The token logs record for `source`.
pub fn source_token(source: Source) -> &'static str {
    match source {
        Source::User => "user",
        Source::Background => "background",
        Source::Unknown => "unknown",
    }
}
