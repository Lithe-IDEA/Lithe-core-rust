//! Stable error categories and safe cross-boundary error serialization.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
/// Stable failure categories understood by all Core consumers.
///
/// Keep these categories independent of Rust libraries and host operating
/// systems; implementation-specific context belongs in [`CoreError::details`].
pub enum ErrorCode {
    /// The request envelope, payload, path, or operation name is invalid.
    InvalidRequest,
    /// The requested workspace or repository root does not exist.
    WorkspaceNotFound,
    /// The operation would escape an allowed root or lacks filesystem access.
    PermissionDenied,
    /// The requested behavior is valid but unavailable in this Core build.
    NotSupported,
    /// A required host-discovered executable or runtime is unavailable.
    RuntimeMissing,
    /// A required child process could not be created.
    ProcessStartFailed,
    /// A child process started but failed while serving the operation.
    ProcessFailed,
    /// A reviewed pull-request checkout would overwrite or conflict with a local ref.
    GitBranchExists,
    /// The reviewed checkout or its GitHub origin no longer matches the current repository.
    GitCheckoutChanged,
    /// The Git remote refused the credentials, or none were available. Only
    /// `git.write` reports it, as `operationError.code` of a failed response.
    GitAuthenticationFailed,
    /// A `git.write` worktree mutation (`removeWorktree`, `lockWorktree`,
    /// `unlockWorktree`) names a path that is not a registered worktree of the
    /// repository, so the caller's worktree list is out of date.
    GitWorktreeNotRegistered,
    /// A `git.write` worktree mutation names the repository's primary
    /// worktree, which Git can neither remove, lock nor unlock.
    GitWorktreePrimary,
    /// `git.write` `removeWorktree` names the worktree the request was made
    /// from; another worktree of the repository can remove it.
    GitWorktreeCurrent,
    /// `git.write` `removeWorktree` names a locked worktree; unlocking it
    /// first allows the removal.
    GitWorktreeLocked,
    /// Git refused `git.write` `removeWorktree` without `force` because the
    /// checkout has modified or untracked files, which a forced removal
    /// deletes. Reported as `operationError.code` of the failed response.
    GitWorktreeHasChanges,
    /// A commit's `author` is not `Name <email>`: a name, a space and an email address
    /// between angle brackets, without line breaks. Git would otherwise treat the value
    /// as a pattern that picks an earlier author.
    GitCommitAuthorInvalid,
    /// Input or tool output could not be decoded into the stable contract.
    ParseFailed,
    /// `file.write` text contains characters the requested encoding cannot
    /// represent; nothing is written and no character is substituted.
    EncodingUnmappable,
    /// The caller cooperatively cancelled the operation.
    Cancelled,
    /// The operation exceeded its declared deadline.
    TimedOut,
    /// A long-lived connection the request names is closed or was lost; the
    /// caller can reconnect and retry.
    ConnectionClosed,
    /// A failure does not fit a more stable cross-platform category.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Cross-boundary failure containing a stable category and actionable message.
pub struct CoreError {
    /// Machine-readable category used by application error handling.
    pub code: ErrorCode,
    /// User-facing summary that is safe to display.
    pub message: String,
    /// Optional diagnostic context that must not contain secrets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl CoreError {
    /// Creates an error without implementation-specific details.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    /// Adds safe diagnostic context while preserving the stable error category.
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::WorkspaceNotFound,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            _ => ErrorCode::Unknown,
        };
        Self::new(code, error.to_string())
    }
}

/// Workspace-relative paths use `/` in the shared contract even when the
/// caller is running on Windows. Validate both separator conventions so a
/// Windows-shaped path cannot bypass checks on another host.
pub fn invalid_relative_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized.contains('\0')
        || normalized.split('/').any(|component| component == "..")
}
