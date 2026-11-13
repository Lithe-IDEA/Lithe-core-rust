//! Defaults and bounded persisted options for native editor presentation.

use crate::protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    line_numbers: Option<bool>,
    whitespace: Option<bool>,
    cursor_style: Option<String>,
    cursor_blink: Option<bool>,
    auto_save_delay_milliseconds: Option<u64>,
}

pub(crate) fn resolve_command(payload: Value) -> Result<Value, CoreError> {
    let stored: Stored = serde_json::from_value(payload)
        .map_err(|_| CoreError::new(ErrorCode::InvalidRequest, "Invalid editor preferences"))?;
    let style = match stored.cursor_style.as_deref() {
        Some("block") => "block",
        Some("underline") => "underline",
        _ => "line",
    };
    Ok(json!({
        "lineNumbers":stored.line_numbers.unwrap_or(true),
        "whitespace":stored.whitespace.unwrap_or(false),
        "cursorStyle":style,
        "cursorBlink":stored.cursor_blink.unwrap_or(true),
        "autoSaveDelayMilliseconds":stored.auto_save_delay_milliseconds.unwrap_or(1500).clamp(250,60_000),
        "minimumAutoSaveDelayMilliseconds":250,
        "maximumAutoSaveDelayMilliseconds":60_000
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_and_old_preferences_get_the_existing_editor_defaults() {
        let value = resolve_command(json!({})).unwrap();
        assert_eq!(value["lineNumbers"], true);
        assert_eq!(value["whitespace"], false);
        assert_eq!(value["cursorStyle"], "line");
        assert_eq!(value["cursorBlink"], true);
        assert_eq!(value["autoSaveDelayMilliseconds"], 1500);
    }
    #[test]
    fn corrupted_values_are_bounded_without_replacing_valid_choices() {
        let value = resolve_command(json!({"lineNumbers":false,"cursorStyle":"unknown","cursorBlink":false,"autoSaveDelayMilliseconds":1})).unwrap();
        assert_eq!(value["lineNumbers"], false);
        assert_eq!(value["cursorStyle"], "line");
        assert_eq!(value["cursorBlink"], false);
        assert_eq!(value["autoSaveDelayMilliseconds"], 250);
    }
}
