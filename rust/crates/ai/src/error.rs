//! Failure type whose JSON shape matches the Core error envelope.

use serde::Serialize;

/// Stable failure categories. Names serialize exactly like the matching `lithe-core` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiErrorCode {
    /// The payload does not match the command contract, or the planner refused it. The
    /// message is a stable `AI_*` code that platforms map to their own wording.
    InvalidRequest,
    /// The command name is not owned by this crate.
    NotSupported,
}

/// An AI planning failure with a stable category and a message that is safe to display.
///
/// Messages are fixed codes: diffs, pasted configuration and provider responses can carry
/// credentials or private source, so no failure echoes its input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiError {
    /// Machine-readable category.
    pub code: AiErrorCode,
    /// Stable failure code such as `AI_COMMIT_INVALID_OPTIONS`.
    pub message: String,
}

impl AiError {
    /// Creates an error from a category and a stable failure code.
    pub fn new(code: AiErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
