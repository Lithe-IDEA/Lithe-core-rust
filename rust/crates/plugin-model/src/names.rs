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

/// Language tag in the `language[-Script][-REGION]` form used by `.lproj`
/// directories and `l10n/` files, e.g. `zh-Hans` or `en`.
pub fn is_language_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let Some(language) = parts.next() else {
        return false;
    };
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
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

/// Longest tool ID, namespace and separator included, in bytes.
const MAX_TOOL_ID_BYTES: usize = 192;

/// A tool ID: a short [local ID](is_local_tool_id), or `<namespace>:<local>`
/// with a [namespace](is_tool_namespace) such as a plugin ID. It says nothing
/// about which plugin defines the tool; that is the host registry's decision.
pub fn is_tool_id(value: &str) -> bool {
    value.len() <= MAX_TOOL_ID_BYTES
        && match value.split_once(':') {
            Some((namespace, local)) => is_tool_namespace(namespace) && is_local_tool_id(local),
            None => is_local_tool_id(value),
        }
}

/// The namespace of a qualified tool or provider ID: non-empty `.`- or
/// `-`-separated parts of lowercase letters and digits.
pub fn is_tool_namespace(value: &str) -> bool {
    !value.is_empty()
        && value.split(['.', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// The local part of a tool or provider ID: at most 64 bytes, a lowercase
/// letter first, then lowercase letters, digits and `-`.
pub fn is_local_tool_id(value: &str) -> bool {
    value.len() <= 64
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
