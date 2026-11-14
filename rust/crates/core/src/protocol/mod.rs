//! Request parsing, domain response models and events on top of `lithe-protocol`.
//!
//! The envelope, error categories and cancellation come from `lithe-protocol` and are re-exported
//! here, so every `crate::protocol::...` path names the same type a domain crate returns.

mod command;

pub use command::*;
pub(crate) use lithe_protocol::cancellation;
pub use lithe_protocol::{CoreError, CoreResponse, ErrorCode, ResponseData};

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
