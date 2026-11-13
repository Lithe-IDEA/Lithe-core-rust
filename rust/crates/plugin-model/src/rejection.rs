//! How a manifest or plan request is refused: a stable `plugin.*` code with a
//! message and the offending field, and the `ok`/`rejected` outcome every
//! declarative command answers with.
//!
//! Content problems are rejections, not Core failures, so a host can show them
//! next to the plugin. Only a payload that does not decode is a Core
//! `invalid_request` failure ([`decode_payload`]).

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Stable reason a declarative manifest or plan request was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginRejection {
    /// Stable `plugin.*` code listed in the contract README.
    pub code: &'static str,
    /// User-facing English summary without host paths.
    pub message: String,
    /// JSON path of the offending manifest field, when one applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl PluginRejection {
    /// The stable rejection code, such as `plugin.manifest.invalid`.
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// The manifest or request field the rejection points at, if any.
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }

    /// A rejection that points at no field.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field: None,
        }
    }

    /// A rejection that points at `field`.
    pub fn at(code: &'static str, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field: Some(field.into()),
        }
    }
}

/// The result of validating plugin content: the value, or why it was refused.
pub type Validation<T> = Result<T, PluginRejection>;

/// A `plugin.manifest.invalid` rejection at `field`.
pub fn invalid(field: impl Into<String>, message: impl Into<String>) -> PluginRejection {
    PluginRejection::at("plugin.manifest.invalid", field, message)
}

/// A `plugin.template.invalid` rejection at `field`.
pub fn invalid_template(field: impl Into<String>, message: impl Into<String>) -> PluginRejection {
    PluginRejection::at("plugin.template.invalid", field, message)
}

/// Decodes a command payload; a payload that does not match is an invalid request.
pub fn decode_payload<T: for<'de> Deserialize<'de>>(
    payload: Value,
    command: &str,
) -> Result<T, CoreError> {
    serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            format!("Invalid {command} payload"),
        )
        .with_details(error.to_string())
    })
}

/// The command result for a validation: `status: ok` with the value under
/// `key`, or `status: rejected` with the rejection.
pub fn outcome(result: Validation<(&'static str, Value)>) -> Value {
    match result {
        Ok((key, value)) => {
            let mut object = serde_json::Map::new();
            object.insert("status".to_string(), json!("ok"));
            object.insert(key.to_string(), value);
            Value::Object(object)
        }
        Err(rejection) => json!({ "status": "rejected", "error": rejection }),
    }
}
