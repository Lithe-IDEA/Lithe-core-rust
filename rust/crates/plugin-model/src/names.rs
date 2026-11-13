//! The identifier alphabets of the manifest contract: plugin, function and
//! short IDs, package-relative paths, and bare command names.

use std::collections::BTreeSet;

/// ID prefix reserved to plugins published and signed by Lithe.
pub const OFFICIAL_PLUGIN_NAMESPACE: &str = "dev.lithe";

/// A package-relative path: not absolute, no `..` component, no backslash or NUL.
pub fn is_safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\0')
        && !value.contains('\\')
        && !value.split('/').any(|component| component == "..")
}

/// A bare command name the host looks up on `PATH`: no path separator, whitespace or NUL.
pub fn is_bare_command(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0')
        && !value.chars().any(char::is_whitespace)
}

/// A short identifier: lowercase letters, digits and `-`.
pub fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// True for IDs reserved to Lithe's official plugins (`dev.lithe` and
/// `dev.lithe.*`). Third-party sources and unsigned packages may not use it.
pub fn is_official_namespace(id: &str) -> bool {
    id == OFFICIAL_PLUGIN_NAMESPACE || id.starts_with(&format!("{OFFICIAL_PLUGIN_NAMESPACE}."))
}

/// The manifest's plugin ID alphabet: lowercase letters, digits, `.` and `-`.
pub fn is_plugin_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || ".-".contains(character)
        })
}

/// Exactly three dot-separated decimal components, such as `1.2.3`.
pub fn is_semantic_version(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

/// The values in ascending order without duplicates.
pub fn sorted_unique(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Longest function or command ID a plugin may declare, in bytes.
pub const MAX_FUNCTION_ID_BYTES: usize = 128;

/// Dot-separated lowerCamelCase segments, for example `snapshot.refresh`.
pub fn is_function_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FUNCTION_ID_BYTES
        && value.split('.').all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_lowercase())
                && characters.all(|character| character.is_ascii_alphanumeric())
        })
}
