//! Failure type that `lithe-core` maps one to one onto its error envelope.

/// Stable failure categories, each matching the `lithe-core` error code of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleErrorCode {
    /// The request envelope or payload does not match the command contract.
    InvalidRequest,
    /// The command name is not a rule owned by this crate.
    NotSupported,
}

/// A rule failure with a stable category and a message that is safe to display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    /// Machine-readable category.
    pub code: RuleErrorCode,
    /// Short summary of the failure.
    pub message: String,
    /// Optional diagnostic context; never contains secrets.
    pub details: Option<String>,
}

impl RuleError {
    /// Creates an error without diagnostic details.
    pub fn new(code: RuleErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    /// Attaches diagnostic context while keeping the category.
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}
