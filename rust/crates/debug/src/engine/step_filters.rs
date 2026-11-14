//! Stepping filters: normalization of the declared filters and the frame matching that marks
//! library code.

use super::*;

pub(crate) fn normalize_stepping_filters(
    mut filters: DebugSteppingFilters,
) -> Result<DebugSteppingFilters, CoreError> {
    const MAXIMUM_FILTER_COUNT: usize = 256;
    const MAXIMUM_FILTER_LENGTH: usize = 256;

    let mut normalized = Vec::with_capacity(filters.class_name_filters.len());
    for filter in filters.class_name_filters {
        let filter = filter.trim();
        if filter.is_empty() {
            continue;
        }
        if filter.chars().count() > MAXIMUM_FILTER_LENGTH || filter.chars().any(char::is_control) {
            return Err(invalid_request(
                "Debug stepping filters must be short single-line class patterns.",
            ));
        }
        normalized.push(filter.to_string());
    }
    normalized.sort();
    normalized.dedup();
    if normalized.len() > MAXIMUM_FILTER_COUNT {
        return Err(invalid_request(
            "Debug stepping filters cannot contain more than 256 class patterns.",
        ));
    }
    filters.class_name_filters = normalized;
    Ok(filters)
}

/// Reads a declared hot code replacement reply through the fields the
/// adapter named for its failure message and its replaced units.
pub(crate) fn declared_replacement(
    feature: &HotCodeReplaceFeature,
    body: &Value,
) -> Result<DebugOperationResult, CoreError> {
    if let Some(message) = feature
        .error_field
        .as_deref()
        .and_then(|field| body.get(field))
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
    {
        return Err(invalid_request(message));
    }
    let mut changed = match feature.changed_field.as_deref() {
        None => Vec::new(),
        Some(field) => body
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_request("The hot code replacement reply has no change list."))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid_request("Invalid replaced unit name."))
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    changed.sort();
    changed.dedup();
    Ok(DebugOperationResult::RedefineClasses {
        changed_classes: changed,
    })
}

/// The stepping filters in the launch-argument shape adapters that take
/// them as one argument read.
pub(crate) fn step_filter_argument(filters: &DebugSteppingFilters) -> Value {
    json!({
        "skipClasses": filters.class_name_filters,
        "skipSynthetics": filters.skip_synthetics,
        "skipStaticInitializers": filters.skip_static_initializers,
        "skipConstructors": filters.skip_constructors
    })
}

pub(crate) fn stack_frame_matches_filters(
    name: &str,
    source_path: Option<&str>,
    presentation_hint: Option<&str>,
    filters: &DebugSteppingFilters,
    root_path: &str,
) -> bool {
    filters
        .class_name_filters
        .iter()
        .any(|filter| match filter.as_str() {
            "$Libraries" => is_library_frame(source_path, presentation_hint, root_path),
            pattern => class_pattern_matches_frame(pattern, name, source_path),
        })
}

fn class_pattern_matches_frame(pattern: &str, name: &str, source_path: Option<&str>) -> bool {
    if wildcard_match(pattern, name) || name == pattern || name.starts_with(&format!("{pattern}."))
    {
        return true;
    }

    let simple_type = name.split('.').next().unwrap_or(name);
    if source_path.is_none()
        && !pattern.contains('*')
        && pattern
            .rsplit('.')
            .next()
            .is_some_and(|value| value == simple_type)
    {
        return true;
    }

    source_path.is_some_and(|source_path| source_path_matches_pattern(source_path, pattern))
}

fn source_path_matches_pattern(source_path: &str, pattern: &str) -> bool {
    let source_path = source_path.replace('\\', "/");
    let slash_pattern = format!("*{}*", pattern.replace('.', "/"));
    let dotted_pattern = format!("*{pattern}*");
    wildcard_match(&slash_pattern, &source_path) || wildcard_match(&dotted_pattern, &source_path)
}

fn is_library_frame(
    source_path: Option<&str>,
    presentation_hint: Option<&str>,
    root_path: &str,
) -> bool {
    if presentation_hint == Some("subtle") || source_path.is_none() {
        return true;
    }
    let source_path = source_path.unwrap_or_default();
    !source_path_is_within_root(source_path, root_path) && path_is_absolute_or_uri(source_path)
}

fn source_path_is_within_root(source_path: &str, root_path: &str) -> bool {
    let source_path = normalized_comparison_path(source_path);
    let root_path = normalized_comparison_path(root_path);
    source_path == root_path
        || source_path
            .strip_prefix(&root_path)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn normalized_comparison_path(path: &str) -> String {
    path.strip_prefix("file://")
        .unwrap_or(path)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn path_is_absolute_or_uri(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/') || path.contains("://") || (bytes.len() >= 2 && bytes[1] == b':')
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut star_value_index) = (None, 0);

    while value_index < value.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == value[value_index] {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}
