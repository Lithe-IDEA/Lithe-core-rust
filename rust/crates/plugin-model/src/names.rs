//! The identifier alphabets of the manifest contract: plugin, function and
//! short IDs, package-relative paths, bare command names, and environment
//! variable names.

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

/// An environment variable name a manifest may set: uppercase letters,
/// digits and `_`, not starting with a digit, at most 64 bytes.
pub fn is_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with(|character: char| character.is_ascii_digit())
        && value.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
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

/// The owner and name of a `https://github.com/<owner>/<name>` repository URL,
/// spelled as the plugin index spells it: ASCII letters, digits, `.`, `_` and
/// `-` in each part, neither part `.` nor `..`, and at most a trailing `/`.
pub fn github_repository(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix("https://github.com/")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    };
    (valid(owner) && valid(name)).then_some((owner, name))
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

#[cfg(test)]
mod tests {
    use super::github_repository;

    #[test]
    fn github_repository_accepts_only_owner_and_name_urls() {
        assert_eq!(
            github_repository("https://github.com/Fixture-Owner/synthetic-repository"),
            Some(("Fixture-Owner", "synthetic-repository"))
        );
        assert_eq!(
            github_repository("https://github.com/a.b/c_d/"),
            Some(("a.b", "c_d"))
        );
        for value in [
            "http://github.com/a/b",
            "https://gitlab.com/a/b",
            "https://github.com/a",
            "https://github.com/a/b/c",
            "https://github.com/../b",
            "https://github.com/a/b c",
            "https://github.com//b",
        ] {
            assert_eq!(github_repository(value), None, "{value}");
        }
    }
}
