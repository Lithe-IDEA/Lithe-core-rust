//! Lexical native-path policy independent of the machine running Core.

use lithe_platform::paths::{is_separator, slash_separated};
use lithe_platform::Platform;

/// Separator rules for a host platform name (`windows`, `macos`, `linux`).
pub fn rules(platform: &str) -> Platform {
    if platform == "windows" {
        Platform::Windows
    } else {
        Platform::Unix
    }
}

/// Normalizes a bounded absolute path using the target platform's separators.
/// Native adapters must supply canonical filesystem facts separately.
pub fn normalize(path: &str, platform: &str, case_sensitive: bool) -> Option<String> {
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return None;
    }
    let windows = rules(platform).is_windows();
    let text = slash_separated(path, rules(platform)).into_owned();
    let (prefix, remainder) = if windows {
        let bytes = text.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'/'
        {
            (text[..3].to_owned(), &text[3..])
        } else {
            // Anything else must be a UNC path: `//server/share/...`.
            let unc = text.strip_prefix("//")?;
            let mut components = unc.splitn(3, '/');
            let server = components.next()?;
            let share = components.next()?;
            if server.is_empty()
                || share.is_empty()
                || [server, share].iter().any(|part| {
                    part.contains([':', '?', '*', '<', '>', '"', '|'])
                        || matches!(*part, "." | "..")
                })
            {
                return None;
            }
            (
                format!("//{server}/{share}/"),
                components.next().unwrap_or(""),
            )
        }
    } else {
        ("/".to_owned(), text.strip_prefix('/')?)
    };
    let remainder = remainder.strip_suffix('/').unwrap_or(remainder);
    if !remainder.is_empty()
        && remainder.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || (windows
                    && (part.contains([':', '?', '*', '<', '>', '"', '|'])
                        || part.ends_with([' ', '.'])))
        })
    {
        return None;
    }
    let result = format!("{prefix}{remainder}");
    Some(if case_sensitive {
        result
    } else {
        // This is an identity key, never a rewritten executable path.
        result.to_lowercase()
    })
}

/// Compares complete native path components with target-platform case rules.
pub fn within(path: &str, root: &str, platform: &str, case_sensitive: bool) -> bool {
    let Some(path) = normalize(path, platform, case_sensitive) else {
        return false;
    };
    let Some(root) = normalize(root, platform, case_sensitive) else {
        return false;
    };
    path == root || path.starts_with(&format!("{}/", root.trim_end_matches('/')))
}

/// The last component of a native path, split with the target platform's separators.
pub fn file_name<'a>(path: &'a str, platform: &str) -> &'a str {
    path.rsplit(|character| is_separator(character, rules(platform)))
        .next()
        .unwrap_or(path)
}

/// Joins a package-independent relative executable layout beneath a native home.
pub fn join(home: &str, relative: &str, platform: &str) -> Option<String> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.chars().any(char::is_control)
        || relative
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return None;
    }
    normalize(
        &format!(
            "{}/{relative}",
            home.trim_end_matches(|character| is_separator(character, rules(platform)))
        ),
        platform,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn only_windows_reads_a_backslash_as_a_separator() {
        assert_eq!(
            normalize(r"C:\Tools\jdk", "windows", true).as_deref(),
            Some("C:/Tools/jdk")
        );
        assert_eq!(
            normalize(r"/opt/a\b/jdk", "linux", true).as_deref(),
            Some(r"/opt/a\b/jdk")
        );
        assert_eq!(
            normalize(r"/opt/a\b/jdk", "macos", true).as_deref(),
            Some(r"/opt/a\b/jdk")
        );
        assert_eq!(normalize(r"\opt\jdk", "linux", true), None);
    }
}
