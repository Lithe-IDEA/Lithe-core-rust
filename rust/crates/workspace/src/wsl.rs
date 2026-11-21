//! `remote.listWslDistributions`: the WSL distributions installed on this
//! Windows machine, for the connection window.
//!
//! The list is read from `wsl.exe --list --quiet` each time the window asks;
//! nothing is saved. Other systems have no WSL and answer without running
//! anything. Connecting to a distribution is `remote.connect` with `wsl`.

use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::wsl;
use serde::Serialize;
use std::process::Command;
use std::time::{Duration, Instant};

/// How long `wsl.exe --list` may take. The first call after boot starts the
/// WSL service, which takes a few seconds.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// `remote.listWslDistributions` result.
#[derive(Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Distributions {
    /// `wsl.exe` ran. `false` on other systems and when it could not start.
    pub available: bool,
    /// Installed distributions in `wsl.exe` order, without Docker Desktop's
    /// engine distributions.
    pub distributions: Vec<String>,
    /// What `wsl.exe` printed when it failed, such as the message that WSL
    /// is not installed, redacted; empty otherwise. Shown, never logged.
    pub diagnostics: String,
}

/// The distributions installed on this machine.
pub fn list() -> Result<Distributions, CoreError> {
    list_with(None)
}

/// `program` replaces the system `wsl.exe`, also on systems without WSL;
/// tests use a stand-in.
pub(crate) fn list_with(program: Option<&str>) -> Result<Distributions, CoreError> {
    if program.is_none() && !cfg!(windows) {
        return Ok(Distributions::default());
    }
    let argv = wsl::list_command();
    let (system, arguments) = argv
        .split_first()
        .expect("the listing always names wsl.exe");
    let mut process = Command::new(program.unwrap_or(system));
    // `wsl.exe` needs the Windows environment, as for a connection.
    process.args(arguments).env("WSL_UTF8", "1");
    let deadline = Instant::now() + LIST_TIMEOUT;
    let mut stop = None;
    let outcome = lithe_git_host::run(
        &mut process,
        None,
        || {
            if let Err(error) = lithe_protocol::cancellation::check() {
                stop = Some(error);
            } else if Instant::now() >= deadline {
                stop = Some(CoreError::new(
                    ErrorCode::TimedOut,
                    "wsl.exe did not list the distributions in time",
                ));
            }
            stop.is_some()
        },
        || {},
        |_, _| {},
    );
    if let Some(error) = stop {
        return Err(error);
    }
    match outcome.failure {
        None => {}
        // No `wsl.exe`: WSL is not part of this Windows.
        Some(lithe_git_host::Failure::Start(_)) => return Ok(Distributions::default()),
        Some(failure) => {
            return Err(CoreError::new(ErrorCode::ProcessFailed, "wsl.exe failed")
                .with_details(format!("{failure:?}")))
        }
    }
    if outcome.status.is_some_and(|status| status.success()) {
        return Ok(Distributions {
            available: true,
            distributions: wsl::parse_distributions(&outcome.stdout),
            diagnostics: String::new(),
        });
    }
    // `wsl.exe` prints its messages to standard output, for example when no
    // distribution is installed, and some to standard error.
    let mut printed = wsl::output_text(&outcome.stdout);
    printed.push('\n');
    printed.push_str(&wsl::output_text(&outcome.stderr));
    Ok(Distributions {
        available: true,
        distributions: Vec::new(),
        diagnostics: crate::probe::diagnostics(&printed),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakeWsl {
        root: PathBuf,
        program: String,
    }

    impl FakeWsl {
        fn new(label: &str, body: &str) -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "lithe-wsl-list-{label}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let program = root.join("wsl");
            std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                root,
                program: program.to_string_lossy().into_owned(),
            }
        }
    }

    impl Drop for FakeWsl {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn listings_run_wsl_quietly_and_read_its_utf16_output() {
        // "Ubuntu\r\ndocker-desktop\r\n" in UTF-16LE with a byte order mark.
        let fake = FakeWsl::new(
            "utf16",
            r#"printf '%s\n' "$@" "$WSL_UTF8" > "$0.arguments"
printf '\377\376U\000b\000u\000n\000t\000u\000\r\000\n\000d\000o\000c\000k\000e\000r\000-\000d\000e\000s\000k\000t\000o\000p\000\r\000\n\000'"#,
        );
        let listed = list_with(Some(&fake.program)).unwrap();
        assert_eq!(
            listed,
            Distributions {
                available: true,
                distributions: vec!["Ubuntu".into()],
                diagnostics: String::new(),
            }
        );
        let arguments = std::fs::read_to_string(format!("{}.arguments", fake.program)).unwrap();
        assert_eq!(arguments, "--list\n--quiet\n1\n");
    }

    #[test]
    fn failed_listings_are_available_with_the_message_and_no_names() {
        let fake = FakeWsl::new(
            "none",
            "printf 'Windows Subsystem for Linux has no installed distributions.\\r\\n'\nexit 1",
        );
        let listed = list_with(Some(&fake.program)).unwrap();
        assert!(listed.available);
        assert!(listed.distributions.is_empty());
        assert_eq!(
            listed.diagnostics,
            "Windows Subsystem for Linux has no installed distributions."
        );
    }

    #[test]
    fn a_missing_wsl_and_other_systems_are_unavailable() {
        let fake = FakeWsl::new("missing", "exit 0");
        let missing = fake.root.join("absent").to_string_lossy().into_owned();
        assert_eq!(list_with(Some(&missing)).unwrap(), Distributions::default());
        // Without an override nothing runs outside Windows.
        assert_eq!(list().unwrap(), Distributions::default());
    }
}
