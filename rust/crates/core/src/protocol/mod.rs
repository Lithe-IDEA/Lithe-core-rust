//! Request parsing, domain response models and events on top of `lithe-protocol`.
//!
//! The envelope, error categories and cancellation come from `lithe-protocol` and are re-exported
//! here, so every `crate::protocol::...` path names the same type a domain crate returns.

mod command;
mod contracts;
mod event;

pub use command::*;
pub use contracts::*;
pub use event::*;
pub use lithe_git::contracts::*;
pub(crate) use lithe_protocol::{cancellation, json_document};
pub use lithe_protocol::{invalid_relative_path, CoreError, CoreResponse, ErrorCode, ResponseData};

/// Maps an `ai.*` failure onto the Core error of the same category name.
pub(crate) fn ai_error(error: lithe_ai::AiError) -> CoreError {
    let code = match error.code {
        lithe_ai::AiErrorCode::InvalidRequest => ErrorCode::InvalidRequest,
        lithe_ai::AiErrorCode::NotSupported => ErrorCode::NotSupported,
    };
    CoreError::new(code, error.message)
}

/// Maps a `lithe-core-rules` failure onto the Core error of the same category name.
pub(crate) fn rule_error(error: lithe_core_rules::RuleError) -> CoreError {
    let code = match error.code {
        lithe_core_rules::RuleErrorCode::InvalidRequest => ErrorCode::InvalidRequest,
        lithe_core_rules::RuleErrorCode::NotSupported => ErrorCode::NotSupported,
    };
    let converted = CoreError::new(code, error.message);
    match error.details {
        Some(details) => converted.with_details(details),
        None => converted,
    }
}
