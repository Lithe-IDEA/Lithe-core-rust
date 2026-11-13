//! Shared field checks of adapter declarations.

use super::*;

/// A name in an argument path: a DAP argument or a key of a nested object.
pub(crate) fn is_argument_name(name: &str) -> bool {
    let mut characters = name.chars();
    name.len() <= MAX_ARGUMENT_NAME
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
}

/// A dotted path to an argument, such as `hostName` or `connect.host`.
pub(crate) fn is_argument_path(path: &str) -> bool {
    let mut depth = 0;
    path.split('.').all(|name| {
        depth += 1;
        depth <= MAX_ARGUMENT_PATH_DEPTH && is_argument_name(name)
    })
}

/// Whether two argument paths are the same or one is inside the other.
pub(crate) fn paths_overlap(left: &str, right: &str) -> bool {
    left.split('.')
        .zip(right.split('.'))
        .all(|(left, right)| left == right)
}

/// Whether the arguments already hold something at the path, or a value that
/// is not an object on the way to it.
pub(crate) fn default_occupies(defaults: &Map<String, Value>, path: &str) -> bool {
    let mut current = defaults;
    let mut names = path.split('.').peekable();
    while let Some(name) = names.next() {
        let Some(value) = current.get(name) else {
            return false;
        };
        match value {
            Value::Object(next) if names.peek().is_some() => current = next,
            _ => return true,
        }
    }
    false
}

pub(crate) fn validate_name(value: &str, field: &str) -> Validation<()> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_NAME
        || trimmed.chars().any(char::is_control)
    {
        return Err(invalid(field, "A name is one line of 1-128 characters."));
    }
    Ok(())
}

pub(crate) fn is_local_id(value: &str) -> bool {
    let mut characters = value.chars();
    value.len() <= MAX_LOCAL_ID
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || "._-".contains(character)
        })
}

/// A `workspace/executeCommand` command ID, as language servers register them.
pub(crate) fn is_server_command(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ENDPOINT_COMMAND
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:-".contains(character))
}

pub(crate) fn is_language_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LOCAL_ID
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || "+_-".contains(character)
        })
}

/// Sorts a list and rejects a repeated entry, so equal declarations compare
/// equal whatever order the author wrote.
pub(crate) fn sort_unique(values: &mut Vec<String>, field: &str) -> Validation<()> {
    values.sort();
    let before = values.len();
    values.dedup();
    if values.len() == before {
        Ok(())
    } else {
        Err(invalid(field, "An entry is listed more than once."))
    }
}
