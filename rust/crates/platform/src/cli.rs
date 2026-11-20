//! Launcher text and PATH comparison without host I/O.

use crate::Platform;

/// Produces a shell launcher, rejecting characters the native command format cannot carry.
pub fn launcher(executable: &str, platform: Platform) -> Option<String> {
    if executable.is_empty() || executable.contains(['\0', '\r', '\n']) {
        return None;
    }
    match platform {
        Platform::Windows => {
            if executable.contains('"') {
                return None;
            }
            Some(format!("@echo off\r\nrem Lithe CLI managed launcher\r\nsetlocal DisableDelayedExpansion\r\n\"{}\" --cli %*\r\n", executable.replace('%', "%%")))
        }
        Platform::Unix => Some(format!(
            "#!/bin/sh\n# Lithe CLI managed launcher\nexec '{}' --cli \"$@\"\n",
            executable.replace('\'', "'\"'\"'")
        )),
    }
}

/// Recognizes the complete owned format so app moves can repair an earlier launcher.
pub fn is_managed_launcher(source: &str, platform: Platform) -> bool {
    let executable = match platform {
        Platform::Windows => source.strip_prefix("@echo off\r\nrem Lithe CLI managed launcher\r\nsetlocal DisableDelayedExpansion\r\n\"")
            .and_then(|s| s.strip_suffix("\" --cli %*\r\n"))
            .map(|s| s.replace("%%", "%")),
        Platform::Unix => source.strip_prefix("#!/bin/sh\n# Lithe CLI managed launcher\nexec '")
            .and_then(|s| s.strip_suffix("' --cli \"$@\"\n"))
            .map(|s| s.replace("'\"'\"'", "'")),
    };
    executable
        .is_some_and(|exe| launcher(&exe, platform).is_some_and(|expected| expected == source))
}

/// Compares native PATH entries with Windows separator and case conventions.
pub fn same_path_entry(left: &str, right: &str, platform: Platform) -> bool {
    match platform {
        Platform::Unix => left == right,
        Platform::Windows => {
            left.trim()
                .trim_matches('"')
                .trim_end_matches(['\\', '/'])
                .replace('\\', "/")
                .to_lowercase()
                == right
                    .trim()
                    .trim_matches('"')
                    .trim_end_matches(['\\', '/'])
                    .replace('\\', "/")
                    .to_lowercase()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_quotes_native_paths_and_marks_ownership() {
        let windows = launcher(r"C:\User Space\100%\lithe.exe", Platform::Windows).unwrap();
        assert!(windows.contains(r#""C:\User Space\100%%\lithe.exe" --cli %*"#));
        assert!(windows.contains("DisableDelayedExpansion"));
        assert!(is_managed_launcher(&windows, Platform::Windows));
        assert!(launcher("bad\"path", Platform::Windows).is_none());
        let unix = launcher("/Apps/Lithe's/lithe", Platform::Unix).unwrap();
        assert!(unix.contains("'\"'\"'"));
        assert!(is_managed_launcher(&unix, Platform::Unix));
        assert!(!is_managed_launcher(
            "#!/bin/sh\necho unrelated\n",
            Platform::Unix
        ));
        for platform in Platform::ALL {
            assert!(launcher("bad\npath", platform).is_none());
        }
    }

    #[test]
    fn windows_path_registration_matches_case_slashes_and_quotes() {
        assert!(same_path_entry(
            r#" "C:\Users\Äpfel\bin\" "#,
            "c:/users/äpfel/bin",
            Platform::Windows
        ));
        assert!(!same_path_entry("/Bin", "/bin", Platform::Unix));
    }
}
