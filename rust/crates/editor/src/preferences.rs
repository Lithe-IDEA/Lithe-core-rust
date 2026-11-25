//! Defaults and bounded persisted options for native editor presentation.

use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_RAINBOW_COLORS: usize = 8;
const LIGHT_BRACKET_COLORS: &[&str] = &[
    "#A4262C", "#0657A4", "#6D3298", "#00695C", "#8A5200", "#394B68",
];
const DARK_BRACKET_COLORS: &[&str] = &[
    "#FF8A80", "#82B1FF", "#D7A9FF", "#5CE1C4", "#FFBF69", "#A6B8D4",
];

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    line_numbers: Option<bool>,
    whitespace: Option<bool>,
    inlay_hints: Option<bool>,
    cursor_style: Option<String>,
    cursor_blink: Option<bool>,
    auto_save_delay_milliseconds: Option<u64>,
    rainbow_brackets: Option<bool>,
    rainbow_variables: Option<bool>,
    // Retain each raw palette independently so a malformed color can fall
    // back without discarding the other persisted preferences.
    rainbow_bracket_light_colors: Option<Value>,
    rainbow_bracket_dark_colors: Option<Value>,
}

fn effective_palette(stored: Option<&Value>, defaults: &[&str]) -> Vec<String> {
    stored
        .and_then(Value::as_array)
        .filter(|colors| (1..=MAX_RAINBOW_COLORS).contains(&colors.len()))
        .and_then(|colors| colors.iter().map(normalize_color).collect())
        .unwrap_or_else(|| defaults.iter().map(|color| (*color).to_owned()).collect())
}

fn normalize_color(value: &Value) -> Option<String> {
    let color = value.as_str()?.trim();
    let digits = color.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| color.to_ascii_uppercase())
}

/// Resolves stored editor preferences into effective values, filling in
/// defaults, normalizing rainbow palettes and clamping the auto-save delay
/// (`editor.preferences.resolve`).
pub fn resolve_command(payload: Value) -> Result<Value, CoreError> {
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
        "inlayHints":stored.inlay_hints.unwrap_or(true),
        "cursorStyle":style,
        "cursorBlink":stored.cursor_blink.unwrap_or(true),
        "autoSaveDelayMilliseconds":stored.auto_save_delay_milliseconds.unwrap_or(1500).clamp(250,60_000),
        "minimumAutoSaveDelayMilliseconds":250,
        "maximumAutoSaveDelayMilliseconds":60_000,
        "rainbowBrackets":stored.rainbow_brackets.unwrap_or(false),
        "rainbowVariables":stored.rainbow_variables.unwrap_or(false),
        "rainbowBracketLightColors":effective_palette(stored.rainbow_bracket_light_colors.as_ref(), LIGHT_BRACKET_COLORS),
        "rainbowBracketDarkColors":effective_palette(stored.rainbow_bracket_dark_colors.as_ref(), DARK_BRACKET_COLORS)
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
        assert_eq!(value["inlayHints"], true);
        assert_eq!(value["cursorStyle"], "line");
        assert_eq!(value["cursorBlink"], true);
        assert_eq!(value["autoSaveDelayMilliseconds"], 1500);
        assert_eq!(value["rainbowBrackets"], false);
        assert_eq!(value["rainbowVariables"], false);
        assert_eq!(
            value["rainbowBracketLightColors"],
            json!(LIGHT_BRACKET_COLORS)
        );
        assert_eq!(
            value["rainbowBracketDarkColors"],
            json!(DARK_BRACKET_COLORS)
        );
    }
    #[test]
    fn corrupted_values_are_bounded_without_replacing_valid_choices() {
        let value = resolve_command(json!({"lineNumbers":false,"inlayHints":false,"cursorStyle":"unknown","cursorBlink":false,"autoSaveDelayMilliseconds":1})).unwrap();
        assert_eq!(value["lineNumbers"], false);
        assert_eq!(value["inlayHints"], false);
        assert_eq!(value["cursorStyle"], "line");
        assert_eq!(value["cursorBlink"], false);
        assert_eq!(value["autoSaveDelayMilliseconds"], 250);
    }

    #[test]
    fn rainbow_palettes_normalize_and_fall_back_independently() {
        for bad_palette in [
            json!([]),
            json!(["#GG0000"]),
            json!(["#123456", 0]),
            json!("#123456"),
            json!(vec!["#123456"; MAX_RAINBOW_COLORS + 1]),
        ] {
            let value = resolve_command(json!({
                "lineNumbers": false,
                "rainbowBrackets": true,
                "rainbowVariables": true,
                "rainbowBracketLightColors": bad_palette,
                "rainbowBracketDarkColors": [" #aabbcc ", "#D3D3D3"],
            }))
            .unwrap();
            assert_eq!(value["lineNumbers"], false);
            assert_eq!(value["rainbowBrackets"], true);
            assert_eq!(value["rainbowVariables"], true);
            assert_eq!(
                value["rainbowBracketLightColors"],
                json!(LIGHT_BRACKET_COLORS)
            );
            assert_eq!(
                value["rainbowBracketDarkColors"],
                json!(["#AABBCC", "#D3D3D3"])
            );
        }
    }
}
