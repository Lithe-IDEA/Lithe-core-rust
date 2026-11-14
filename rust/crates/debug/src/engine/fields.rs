//! Field readers and validators for host and adapter payloads, and the errors they report.

use super::*;

pub(crate) fn validate_identifier(value: &str, field: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.contains('\0') || value.len() > 512 {
        return Err(invalid_request(&format!("Debug {field} was invalid.")));
    }
    Ok(())
}

pub(crate) fn validate_path(value: &str, field: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(invalid_request(&format!("Debug {field} was invalid.")));
    }
    Ok(())
}

pub(crate) fn required_positive(value: Option<i64>, field: &str) -> Result<i64, CoreError> {
    value
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_request(&format!("Debug {field} must be positive.")))
}

pub(crate) fn required_nonnegative(value: Option<i64>, field: &str) -> Result<i64, CoreError> {
    value
        .filter(|value| *value >= 0)
        .ok_or_else(|| invalid_request(&format!("Debug {field} cannot be negative.")))
}

pub(crate) fn required_str<'a>(value: &'a Value, field: &str) -> Result<&'a str, CoreError> {
    value.get(field).and_then(Value::as_str).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ParseFailed,
            format!("DAP message did not contain a valid {field}."),
        )
    })
}

pub(crate) fn required_i64(value: &Value, field: &str) -> Result<i64, CoreError> {
    value.get(field).and_then(Value::as_i64).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ParseFailed,
            format!("DAP message did not contain a valid {field}."),
        )
    })
}

pub(crate) fn required_array<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a Vec<Value>, CoreError> {
    value.get(field).and_then(Value::as_array).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ParseFailed,
            format!("DAP response did not contain a valid {field} array."),
        )
    })
}

pub(crate) fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

/// The reason an adapter gave for rejecting a request, in the words meant for the user.
///
/// DAP carries two texts: `message` is a short raw reason the client may interpret
/// (`cancelled`, `notStopped`), while `body.error.format` is the text to show, with
/// `{name}` placeholders filled from `body.error.variables`. Adapters such as
/// lldb-dap and debugpy put the useful explanation only in the latter.
pub(crate) fn adapter_failure_detail(response: &Value) -> Option<String> {
    let error = response.get("body").and_then(|body| body.get("error"));
    let formatted = error
        .and_then(|error| error.get("format"))
        .and_then(Value::as_str)
        .map(|format| {
            let variables = error
                .and_then(|error| error.get("variables"))
                .and_then(Value::as_object);
            fill_error_variables(format, variables)
        });
    formatted
        .or_else(|| string_field(response, "message"))
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Replaces `{name}` with the matching string variable; unknown placeholders stay as written.
fn fill_error_variables(format: &str, variables: Option<&Map<String, Value>>) -> String {
    let Some(variables) = variables else {
        return format.to_string();
    };
    let mut result = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(open) = rest.find('{') {
        result.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match variables.get(name).and_then(Value::as_str) {
                    Some(value) => result.push_str(value),
                    None => {
                        result.push('{');
                        result.push_str(name);
                        result.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                result.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    result.push_str(rest);
    result
}

pub(crate) fn bool_field(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_bool).unwrap_or(false)
}

pub(crate) fn insert_option<T: serde::Serialize>(
    map: &mut Map<String, Value>,
    key: &str,
    value: Option<T>,
) {
    if let Some(value) = value {
        map.insert(key.to_string(), json!(value));
    }
}

pub(crate) fn insert_nonempty(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
}

pub(crate) fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub(crate) fn invalid_request(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

pub(crate) fn session_not_found(session_id: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Debug session was not found.")
        .with_details(session_id.to_string())
}
