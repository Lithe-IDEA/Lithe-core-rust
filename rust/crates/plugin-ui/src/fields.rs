//! Field readers shared by the view, canvas and patch checks. Each reader
//! names the JSON path of the value it refuses, so a plugin author sees
//! `view[2].nodes[14].fill` rather than a bare type error.

use lithe_plugin_model::PluginRejection;
use serde_json::{Map, Value};

/// Error code of every view result Core refuses.
pub const INVALID_VIEW: &str = "plugin.ui.invalidView";

/// Result of one check.
pub(crate) type Checked<T> = Result<T, PluginRejection>;

/// A JSON object of view data.
pub(crate) type Object = Map<String, Value>;

/// The rejection of the value at `path`.
pub(crate) fn fail(path: impl Into<String>, message: impl Into<String>) -> PluginRejection {
    PluginRejection::at(INVALID_VIEW, path, message)
}

/// The path of `key` inside the object at `path`.
pub(crate) fn at(path: &str, key: &str) -> String {
    format!("{path}.{key}")
}

/// The object at `path`, whose keys are all in `allowed`.
pub(crate) fn object<'v>(value: &'v Value, path: &str, allowed: &[&str]) -> Checked<&'v Object> {
    let map = value
        .as_object()
        .ok_or_else(|| fail(path, "Expected an object."))?;
    if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(fail(at(path, key), "Unknown field."));
    }
    Ok(map)
}

/// The value of `key`; `null` is refused rather than read as absent, so a
/// broken serializer cannot pass silently.
fn get<'v>(map: &'v Object, path: &str, key: &str, required: bool) -> Checked<Option<&'v Value>> {
    match map.get(key) {
        Some(Value::Null) => Err(fail(
            at(path, key),
            "Omit the field instead of sending null.",
        )),
        Some(value) => Ok(Some(value)),
        None if required => Err(fail(at(path, key), "Required.")),
        None => Ok(None),
    }
}

/// Single-line, non-blank text of at most `max` characters.
pub(crate) fn line<'v>(
    map: &'v Object,
    path: &str,
    key: &str,
    max: usize,
    required: bool,
) -> Checked<Option<&'v str>> {
    let Some(text) = string(map, path, key, required)? else {
        return Ok(None);
    };
    if text.trim().is_empty() || text.chars().count() > max || text.chars().any(char::is_control) {
        return Err(fail(
            at(path, key),
            format!("Expected single-line text of 1 to {max} characters."),
        ));
    }
    Ok(Some(text))
}

/// Text of at most `max` characters that may break lines.
pub(crate) fn text<'v>(
    map: &'v Object,
    path: &str,
    key: &str,
    max: usize,
    required: bool,
) -> Checked<Option<&'v str>> {
    let Some(text) = string(map, path, key, required)? else {
        return Ok(None);
    };
    if text.chars().count() > max
        || text
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(fail(
            at(path, key),
            format!("Expected text of at most {max} characters without control characters."),
        ));
    }
    Ok(Some(text))
}

/// A string of any content; callers apply their own rule.
pub(crate) fn string<'v>(
    map: &'v Object,
    path: &str,
    key: &str,
    required: bool,
) -> Checked<Option<&'v str>> {
    get(map, path, key, required)?
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| fail(at(path, key), "Expected a string."))
        })
        .transpose()
}

/// A finite number within `min..=max`.
pub(crate) fn number(
    map: &Object,
    path: &str,
    key: &str,
    (min, max): (f64, f64),
    required: bool,
) -> Checked<Option<f64>> {
    get(map, path, key, required)?
        .map(|value| in_range(value, &at(path, key), min, max))
        .transpose()
}

fn in_range(value: &Value, path: &str, min: f64, max: f64) -> Checked<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && (min..=max).contains(number))
        .ok_or_else(|| fail(path, format!("Expected a number from {min} to {max}.")))
}

/// A whole number within `min..=max`.
pub(crate) fn integer(
    map: &Object,
    path: &str,
    key: &str,
    (min, max): (u64, u64),
    required: bool,
) -> Checked<Option<u64>> {
    get(map, path, key, required)?
        .map(|value| {
            value
                .as_u64()
                .filter(|number| (min..=max).contains(number))
                .ok_or_else(|| {
                    fail(
                        at(path, key),
                        format!("Expected a whole number from {min} to {max}."),
                    )
                })
        })
        .transpose()
}

/// `true` or `false`.
pub(crate) fn boolean(map: &Object, path: &str, key: &str) -> Checked<Option<bool>> {
    get(map, path, key, false)?
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| fail(at(path, key), "Expected true or false."))
        })
        .transpose()
}

/// One of `choices`.
pub(crate) fn choice<'v>(
    map: &'v Object,
    path: &str,
    key: &str,
    choices: &[&str],
    required: bool,
) -> Checked<Option<&'v str>> {
    let Some(text) = string(map, path, key, required)? else {
        return Ok(None);
    };
    if !choices.contains(&text) {
        return Err(fail(
            at(path, key),
            format!("Expected one of {}.", choices.join(", ")),
        ));
    }
    Ok(Some(text))
}

/// An array of `min..=max` items.
pub(crate) fn array<'v>(
    map: &'v Object,
    path: &str,
    key: &str,
    (min, max): (usize, usize),
    required: bool,
) -> Checked<Option<&'v Vec<Value>>> {
    let Some(value) = get(map, path, key, required)? else {
        return Ok(None);
    };
    let items = value
        .as_array()
        .ok_or_else(|| fail(at(path, key), "Expected an array."))?;
    if !(min..=max).contains(&items.len()) {
        return Err(fail(
            at(path, key),
            format!("Expected {min} to {max} items."),
        ));
    }
    Ok(Some(items))
}

/// `min..=max` numbers, each within `range`.
pub(crate) fn numbers(
    map: &Object,
    path: &str,
    key: &str,
    count: (usize, usize),
    (min, max): (f64, f64),
    required: bool,
) -> Checked<Option<Vec<f64>>> {
    let Some(items) = array(map, path, key, count, required)? else {
        return Ok(None);
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| in_range(item, &format!("{}[{index}]", at(path, key)), min, max))
        .collect::<Checked<Vec<_>>>()
        .map(Some)
}

/// An event list drawn from `events`, without repeats.
pub(crate) fn events(map: &Object, path: &str, events: &[&str]) -> Checked<Vec<String>> {
    let Some(items) = array(map, path, "on", (0, events.len()), false)? else {
        return Ok(Vec::new());
    };
    let mut seen = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let event = item
            .as_str()
            .filter(|event| events.contains(event) && !seen.iter().any(|seen| seen == event))
            .ok_or_else(|| {
                fail(
                    format!("{path}.on[{index}]"),
                    format!("Expected distinct events from {}.", events.join(", ")),
                )
            })?;
        seen.push(event.to_owned());
    }
    Ok(seen)
}

/// A node identifier: 1 to 64 letters, digits, `.`, `_` or `-`, starting
/// with a letter or digit. Events and patches address nodes by it.
pub(crate) fn identifier<'v>(
    map: &'v Object,
    path: &str,
    key: &str,
    required: bool,
) -> Checked<Option<&'v str>> {
    let Some(id) = string(map, path, key, required)? else {
        return Ok(None);
    };
    if !is_node_id(id) {
        return Err(fail(
            at(path, key),
            "An identifier is 1 to 64 letters, digits, '.', '_' or '-', starting with a letter or digit.",
        ));
    }
    Ok(Some(id))
}

/// Whether `id` may identify a view node, tree item or canvas node.
pub(crate) fn is_node_id(id: &str) -> bool {
    id.len() <= 64
        && id
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}
