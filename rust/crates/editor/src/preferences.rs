//! Defaults and bounded persisted options for native editor presentation.

use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_RAINBOW_COLORS: usize = 8;
const LIGHT_RAINBOW_COLORS: &[&str] = &[
    "#A4262C", "#0657A4", "#6D3298", "#00695C", "#8A5200", "#394B68",
];
const DARK_RAINBOW_COLORS: &[&str] = &[
    "#FF8A80", "#82B1FF", "#D7A9FF", "#5CE1C4", "#FFBF69", "#A6B8D4",
];

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    line_numbers: Option<bool>,
    whitespace: Option<bool>,
    indent_guides: Option<bool>,
    rainbow_indent_guides: Option<bool>,
    inlay_hints: Option<bool>,
    render_doc_comments: Option<bool>,
    render_doc_comments_in_read_only: Option<bool>,
    cursor_style: Option<String>,
    cursor_blink: Option<bool>,
    auto_save_delay_milliseconds: Option<u64>,
    rainbow_brackets: Option<bool>,
    rainbow_variables: Option<bool>,
    // Retain each raw palette independently so a malformed color can fall
    // back without discarding the other persisted preferences.
    #[serde(default, deserialize_with = "retain_palette")]
    rainbow_light_colors: Option<Value>,
    #[serde(default, deserialize_with = "retain_palette")]
    rainbow_dark_colors: Option<Value>,
}

// Explicit null is a malformed stored palette; only an absent field is an
// ordinary default. Option<Value>'s default decoder would erase that distinction.
fn retain_palette<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

fn effective_palette(
    stored: Option<&Value>,
    defaults: &[&str],
    theme: &'static str,
    on_fallback: &mut impl FnMut(&'static str),
) -> Vec<String> {
    let normalized = stored
        .and_then(Value::as_array)
        .filter(|colors| (1..=MAX_RAINBOW_COLORS).contains(&colors.len()))
        .and_then(|colors| colors.iter().map(normalize_color).collect());
    if let Some(colors) = normalized {
        return colors;
    }
    if stored.is_some() {
        on_fallback(theme);
    }
    defaults.iter().map(|color| (*color).to_owned()).collect()
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
    resolve_command_with_fallback(payload, |_| {})
}

/// Resolves preferences and reports each malformed stored palette to the owning
/// runtime's diagnostic sink. The callback receives only the fixed theme token,
/// never a color value; absent palettes are normal defaults and emit no fact.
pub fn resolve_command_with_fallback(
    payload: Value,
    mut on_fallback: impl FnMut(&'static str),
) -> Result<Value, CoreError> {
    let stored: Stored = serde_json::from_value(payload)
        .map_err(|_| CoreError::new(ErrorCode::InvalidRequest, "Invalid editor preferences"))?;
    let style = match stored.cursor_style.as_deref() {
        Some("block") => "block",
        Some("underline") => "underline",
        _ => "line",
    };
    let indent_guides = stored.indent_guides.unwrap_or(true);
    let rainbow_indent_guides = stored.rainbow_indent_guides.unwrap_or(false);
    Ok(json!({
        "lineNumbers":stored.line_numbers.unwrap_or(true),
        "whitespace":stored.whitespace.unwrap_or(false),
        "indentGuides":indent_guides,
        "rainbowIndentGuides":rainbow_indent_guides,
        "rainbowIndentGuidesEffective":indent_guides && rainbow_indent_guides,
        "inlayHints":stored.inlay_hints.unwrap_or(true),
        "renderDocComments":stored.render_doc_comments.unwrap_or(false),
        "renderDocCommentsInReadOnly":stored.render_doc_comments_in_read_only.unwrap_or(true),
        "cursorStyle":style,
        "cursorBlink":stored.cursor_blink.unwrap_or(true),
        "autoSaveDelayMilliseconds":stored.auto_save_delay_milliseconds.unwrap_or(1500).clamp(250,60_000),
        "minimumAutoSaveDelayMilliseconds":250,
        "maximumAutoSaveDelayMilliseconds":60_000,
        "rainbowBrackets":stored.rainbow_brackets.unwrap_or(false),
        "rainbowVariables":stored.rainbow_variables.unwrap_or(false),
        "rainbowLightColors":effective_palette(stored.rainbow_light_colors.as_ref(), LIGHT_RAINBOW_COLORS, "light", &mut on_fallback),
        "rainbowDarkColors":effective_palette(stored.rainbow_dark_colors.as_ref(), DARK_RAINBOW_COLORS, "dark", &mut on_fallback)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_preferences_do_not_discard_known_stored_choices() {
        let value =
            resolve_command(json!({"lineNumbers":false,"futureOption":{"enabled":true}})).unwrap();
        assert_eq!(value["lineNumbers"], false);
    }

    #[test]
    fn palette_fallback_observer_reports_explicit_null_but_not_absence_or_valid_palettes() {
        let mut fallbacks = Vec::new();
        let result = resolve_command_with_fallback(
            json!({
                "rainbowLightColors":null,"rainbowDarkColors":["#abcdef"]
            }),
            |theme| fallbacks.push(theme),
        )
        .unwrap();
        assert_eq!(fallbacks, ["light"]);
        assert_eq!(result["rainbowDarkColors"], json!(["#ABCDEF"]));
        fallbacks.clear();
        resolve_command_with_fallback(json!({}), |theme| fallbacks.push(theme)).unwrap();
        assert!(fallbacks.is_empty());
    }
    #[test]
    fn missing_and_old_preferences_get_the_existing_editor_defaults() {
        let value = resolve_command(json!({})).unwrap();
        assert_eq!(value["lineNumbers"], true);
        assert_eq!(value["whitespace"], false);
        assert_eq!(value["indentGuides"], true);
        assert_eq!(value["rainbowIndentGuides"], false);
        assert_eq!(value["rainbowIndentGuidesEffective"], false);
        assert_eq!(value["inlayHints"], true);
        assert_eq!(value["cursorStyle"], "line");
        assert_eq!(value["cursorBlink"], true);
        assert_eq!(value["autoSaveDelayMilliseconds"], 1500);
        assert_eq!(value["rainbowBrackets"], false);
        assert_eq!(value["rainbowVariables"], false);
        assert_eq!(value["rainbowLightColors"], json!(LIGHT_RAINBOW_COLORS));
        assert_eq!(value["rainbowDarkColors"], json!(DARK_RAINBOW_COLORS));
    }
    #[test]
    fn rainbow_indent_guides_keep_the_choice_when_guides_are_hidden() {
        for indent_guides in [false, true] {
            let value = resolve_command(json!({
                "indentGuides": indent_guides,
                "rainbowIndentGuides": true,
            }))
            .unwrap();
            assert_eq!(value["indentGuides"], indent_guides);
            assert_eq!(value["rainbowIndentGuides"], true);
            assert_eq!(value["rainbowIndentGuidesEffective"], indent_guides);
        }
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
            Value::Null,
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
                "rainbowLightColors": bad_palette,
                "rainbowDarkColors": [" #aabbcc ", "#D3D3D3"],
            }))
            .unwrap();
            assert_eq!(value["lineNumbers"], false);
            assert_eq!(value["rainbowBrackets"], true);
            assert_eq!(value["rainbowVariables"], true);
            assert_eq!(value["rainbowLightColors"], json!(LIGHT_RAINBOW_COLORS));
            assert_eq!(value["rainbowDarkColors"], json!(["#AABBCC", "#D3D3D3"]));
        }
    }
}
