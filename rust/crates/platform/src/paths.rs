//! Path text that follows platform rules.
//!
//! Core identifies workspace files by `/`-separated paths. These helpers
//! compare and validate such paths, convert native path text the way the
//! given platform's file systems would, and read the native absolute paths
//! and path lists a tool prints.
//!
//! [`is_separator`] is the one separator rule: every platform splits paths at
//! `/`, and only Windows also splits them at `\`. On Unix `\` and `:` are
//! ordinary file-name characters, so a folder that a Windows-style setting
//! created on macOS, such as `D:\logs`, is one name there.

use crate::Platform;
use std::borrow::Cow;
use std::path::Path;

/// Whether `path` is `ancestor` itself or lies below it.
///
/// Both paths are `/`-separated and relative to the same root, and `.` names
/// the root. Windows compares them case-insensitively, as its file systems
/// do; Unix compares them exactly.
pub fn relative_path_contains(ancestor: &str, path: &str, platform: Platform) -> bool {
    let folded;
    let (ancestor, path) = if platform.is_windows() {
        folded = (ancestor.to_lowercase(), path.to_lowercase());
        (folded.0.as_str(), folded.1.as_str())
    } else {
        (ancestor, path)
    };
    ancestor == "."
        || path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Whether `character` separates path components on `platform`.
///
/// Every platform separates at `/`. Windows also separates at `\`, which is
/// an ordinary file-name character on Unix.
pub const fn is_separator(character: char, platform: Platform) -> bool {
    character == '/' || (platform.is_windows() && character == '\\')
}

/// Writes every separator of `platform` as `/`.
///
/// On Unix only `/` separates, so the text is returned unchanged.
pub fn slash_separated(path: &str, platform: Platform) -> Cow<'_, str> {
    let native = |character: char| character != '/' && is_separator(character, platform);
    if !path.contains(native) {
        return Cow::Borrowed(path);
    }
    Cow::Owned(
        path.chars()
            .map(|character| if native(character) { '/' } else { character })
            .collect(),
    )
}

/// [`slash_separated`] text of a native path. Bytes that are not UTF-8
/// become U+FFFD.
pub fn slash_separated_path(path: &Path, platform: Platform) -> String {
    slash_separated(&path.to_string_lossy(), platform).into_owned()
}

/// Whether `/`-separated `value` fails to name a path below a root on
/// `platform`.
///
/// The path is invalid when it is empty, starts with `/`, contains NUL or has
/// a `..` component. A component is invalid when `platform` would not read it
/// as one file name: on Windows a component containing a `\` separator, or a
/// `:` that names a drive (`C:x`) or an alternate data stream
/// (`file:stream`). Native text goes through [`invalid_native_relative_path`].
pub fn invalid_relative_path(value: &str, platform: Platform) -> bool {
    value.is_empty()
        || value.starts_with('/')
        || value.contains('\0')
        || value.split('/').any(|component| {
            component == ".."
                || component.contains(|character| is_separator(character, platform))
                || (platform.is_windows() && component.contains(':'))
        })
}

/// Whether `/`-separated `value` fails to name one entry below a root on
/// `platform` in exactly one spelling.
///
/// It fails [`invalid_relative_path`], or a component is empty or `.`, so
/// `a//b`, `./a` and `a/` are refused instead of read as `a/b` and `a`. On
/// Windows each component must also be a name Windows stores as written
/// ([`crate::names::is_portable_windows_component`]): a device name or a
/// trailing `.` or space would name another file than the one asked for.
pub fn invalid_entry_path(value: &str, platform: Platform) -> bool {
    invalid_relative_path(value, platform)
        || value.split('/').any(|component| {
            component.is_empty()
                || component == "."
                || (platform.is_windows()
                    && !crate::names::is_portable_windows_component(component))
        })
}

/// [`invalid_relative_path`] for native text, such as a path a user typed,
/// that may spell `platform`'s own separators.
pub fn invalid_native_relative_path(value: &str, platform: Platform) -> bool {
    invalid_relative_path(&slash_separated(value, platform), platform)
}

/// Whether `value` fails [`invalid_relative_path`] on any platform.
///
/// A path saved in a document that every platform reads must name the same
/// files on each of them, so it may not contain `\` or `:`.
pub fn invalid_portable_relative_path(value: &str) -> bool {
    Platform::ALL
        .into_iter()
        .any(|platform| invalid_relative_path(value, platform))
}

/// Separator between the entries of a path list such as `PATH`.
pub const fn path_list_separator(platform: Platform) -> char {
    match platform {
        Platform::Windows => ';',
        Platform::Unix => ':',
    }
}

/// Whether native path text is absolute, judged from the text alone.
///
/// Unix paths start with `/`. Windows paths start with a drive letter and a
/// separator (`C:\`, `C:/`) or are UNC paths (`\\server\share`); a bare `C:`
/// or a rooted `\dir` still depends on the current drive and is not absolute.
pub fn is_absolute_path(path: &str, platform: Platform) -> bool {
    match platform {
        Platform::Unix => path.starts_with('/'),
        Platform::Windows => {
            let bytes = path.as_bytes();
            let drive = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/');
            drive || path.starts_with(r"\\") || path.starts_with("//")
        }
    }
}

/// Appends a `/`-separated relative path to a native directory with the
/// platform's separator, dropping separators that already end `directory`.
pub fn join_native(directory: &str, relative: &str, platform: Platform) -> String {
    let trimmed = directory.trim_end_matches(|character| is_separator(character, platform));
    let (separator, relative) = if platform.is_windows() {
        ('\\', relative.replace('/', "\\"))
    } else {
        ('/', relative.to_owned())
    };
    // The root directory is only separators; keep it as it is.
    if trimmed.is_empty() {
        return format!("{directory}{relative}");
    }
    format!("{trimmed}{separator}{relative}")
}

/// Reads a path stored as bytes, such as a NUL-separated Git path list.
///
/// Unix paths are arbitrary bytes. Other platforms accept only UTF-8 and
/// return `None` for anything else.
pub fn native_path_from_bytes(bytes: &[u8]) -> Option<&Path> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(Path::new(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(Path::new)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        invalid_entry_path, invalid_native_relative_path, invalid_portable_relative_path,
        invalid_relative_path, is_absolute_path, is_separator, join_native, native_path_from_bytes,
        path_list_separator, relative_path_contains, slash_separated, slash_separated_path,
    };
    use crate::Platform;
    use std::path::Path;

    #[test]
    fn an_entry_path_has_one_spelling() {
        for platform in Platform::ALL {
            for valid in ["a", "a/b.txt", ".gitignore", "a/..b", "a b/c"] {
                assert!(!invalid_entry_path(valid, platform), "{valid} {platform:?}");
            }
            for invalid in ["", "/a", "a//b", "./a", "a/.", "a/", "a/../b", "..", "a\0b"] {
                assert!(
                    invalid_entry_path(invalid, platform),
                    "{invalid} {platform:?}"
                );
            }
        }
        for windows_only in [r"a\b", "c:x", "CON", "nul.txt", "a/b.", "a /b", "a|b"] {
            assert!(
                invalid_entry_path(windows_only, Platform::Windows),
                "{windows_only}"
            );
            assert!(
                !invalid_entry_path(windows_only, Platform::Unix),
                "{windows_only}"
            );
        }
    }

    #[test]
    fn the_root_contains_every_path() {
        for platform in [Platform::Unix, Platform::Windows] {
            assert!(relative_path_contains(".", "src/main.rs", platform));
            assert!(relative_path_contains(".", ".", platform));
        }
    }

    #[test]
    fn a_directory_contains_itself_and_its_descendants_only() {
        for platform in [Platform::Unix, Platform::Windows] {
            assert!(relative_path_contains("src", "src", platform));
            assert!(relative_path_contains("src", "src/main.rs", platform));
            assert!(relative_path_contains("src/app", "src/app/a.rs", platform));
            assert!(!relative_path_contains("src", "srcs/main.rs", platform));
            assert!(!relative_path_contains("src/main.rs", "src", platform));
            assert!(!relative_path_contains("", "src", platform));
        }
    }

    #[test]
    fn only_windows_ignores_case() {
        assert!(relative_path_contains(
            "Src",
            "src/Main.rs",
            Platform::Windows
        ));
        assert!(relative_path_contains(
            "ÄPFEL",
            "äpfel/a",
            Platform::Windows
        ));
        assert!(!relative_path_contains(
            "Src",
            "src/Main.rs",
            Platform::Unix
        ));
    }

    #[test]
    fn only_windows_treats_backslash_as_a_separator() {
        assert_eq!(
            slash_separated(r"C:\work\repo", Platform::Windows),
            "C:/work/repo"
        );
        assert_eq!(
            slash_separated(r"name\with\backslash", Platform::Unix),
            r"name\with\backslash"
        );
        assert_eq!(slash_separated("a/b", Platform::Windows), "a/b");
        assert_eq!(
            slash_separated_path(Path::new(r"D:\logs\app.log"), Platform::Unix),
            r"D:\logs\app.log"
        );
        assert_eq!(
            slash_separated_path(Path::new(r"src\main.rs"), Platform::Windows),
            "src/main.rs"
        );
    }

    #[test]
    fn every_platform_separates_at_a_slash_and_only_windows_at_a_backslash() {
        for platform in Platform::ALL {
            assert!(is_separator('/', platform));
            assert!(!is_separator(':', platform));
        }
        assert!(is_separator('\\', Platform::Windows));
        assert!(!is_separator('\\', Platform::Unix));
    }

    #[test]
    fn relative_paths_that_leave_the_root_are_invalid_on_every_platform() {
        for platform in Platform::ALL {
            for path in ["", "/etc", "..", "a/../b", "a/..", "a\0b"] {
                assert!(invalid_relative_path(path, platform), "{path:?}");
            }
            for path in ["a", "a/b", "a/b/", ".a/..b", "a..b"] {
                assert!(!invalid_relative_path(path, platform), "{path:?}");
            }
        }
    }

    #[test]
    fn only_windows_rejects_backslashes_and_colons_in_a_component() {
        for path in [
            r"D:\logs",
            r"D:\logs/spring.log",
            r"a\b.log",
            "a:b",
            r"..\x",
        ] {
            assert!(invalid_relative_path(path, Platform::Windows), "{path:?}");
            assert!(!invalid_relative_path(path, Platform::Unix), "{path:?}");
        }
        assert!(invalid_relative_path("C:x", Platform::Windows));
        assert!(invalid_relative_path("file:stream", Platform::Windows));
    }

    #[test]
    fn native_paths_are_read_with_the_platform_separators() {
        assert!(!invalid_native_relative_path(
            r"src\main",
            Platform::Windows
        ));
        assert!(!invalid_native_relative_path(r"src\main", Platform::Unix));
        assert!(invalid_native_relative_path(r"..\x", Platform::Windows));
        assert!(!invalid_native_relative_path(r"..\x", Platform::Unix));
        assert!(invalid_native_relative_path(r"C:\x", Platform::Windows));
        for platform in Platform::ALL {
            assert!(invalid_native_relative_path("../x", platform));
        }
    }

    #[test]
    fn portable_paths_hold_on_every_platform() {
        assert!(!invalid_portable_relative_path("module/src"));
        for path in [r"D:\logs", "a:b", r"..\x", "../x", ""] {
            assert!(invalid_portable_relative_path(path), "{path:?}");
        }
    }

    #[test]
    fn path_lists_use_the_platform_separator() {
        assert_eq!(path_list_separator(Platform::Unix), ':');
        assert_eq!(path_list_separator(Platform::Windows), ';');
    }

    #[test]
    fn absolute_paths_follow_the_platform() {
        assert!(is_absolute_path("/home/me/go", Platform::Unix));
        assert!(!is_absolute_path("go/bin", Platform::Unix));
        assert!(!is_absolute_path(r"C:\Users\me", Platform::Unix));
        assert!(is_absolute_path(r"C:\Users\me\go", Platform::Windows));
        assert!(is_absolute_path("C:/Users/me/go", Platform::Windows));
        assert!(is_absolute_path(r"\\server\share\go", Platform::Windows));
        assert!(!is_absolute_path("C:", Platform::Windows));
        assert!(!is_absolute_path("C:go", Platform::Windows));
        assert!(!is_absolute_path(r"\go", Platform::Windows));
        assert!(!is_absolute_path("go", Platform::Windows));
    }

    #[test]
    fn joined_paths_use_the_platform_separator() {
        assert_eq!(
            join_native("/home/me/go", "bin", Platform::Unix),
            "/home/me/go/bin"
        );
        assert_eq!(
            join_native("/home/me/go/", "a/b", Platform::Unix),
            "/home/me/go/a/b"
        );
        assert_eq!(join_native("/", "bin", Platform::Unix), "/bin");
        assert_eq!(
            join_native(r"C:\Users\me\go\", "a/b", Platform::Windows),
            r"C:\Users\me\go\a\b"
        );
        assert_eq!(
            join_native("C:/Users/me/go", "bin", Platform::Windows),
            r"C:/Users/me/go\bin"
        );
    }

    #[test]
    fn utf8_path_bytes_read_on_every_platform() {
        let path = native_path_from_bytes("src/äpfel.rs".as_bytes()).expect("UTF-8 path");
        assert_eq!(path.to_str(), Some("src/äpfel.rs"));
    }

    #[test]
    fn non_utf8_path_bytes_read_only_on_unix() {
        assert_eq!(native_path_from_bytes(b"src/\xff.rs").is_some(), cfg!(unix));
    }
}
