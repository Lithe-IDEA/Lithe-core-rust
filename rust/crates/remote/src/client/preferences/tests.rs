use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lithe-remote-preferences-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// Creates `name` in the directory, executable when `executable`.
    fn file(&self, name: &str, executable: bool) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = executable;
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ssh_file_name() -> String {
    format!("ssh{}", std::env::consts::EXE_SUFFIX)
}

fn refusal(document: &str) -> PreferenceError {
    Preferences::parse(Some(document)).expect_err("the document was accepted")
}

#[test]
fn a_missing_document_is_every_value_automatic() {
    let preferences = Preferences::parse(None).unwrap();
    assert_eq!(preferences, Preferences::default());
    assert_eq!(preferences.ssh_program(), None);
    let reconnect = preferences.reconnect();
    assert_eq!(reconnect.source, Source::Automatic);
    assert_eq!(reconnect.limit_seconds, DEFAULT_RECONNECT_LIMIT_SECONDS);
    assert_eq!(
        reconnect.delays_milliseconds,
        vec![1_000, 2_000, 4_000, 8_000, 16_000, 29_000]
    );
    assert_eq!(preferences.text().unwrap(), "{\n  \"version\": 1\n}\n");
}

#[test]
fn the_document_refuses_other_versions_unknown_fields_and_bad_values() {
    let at = |field, reason| PreferenceError { field, reason };
    assert_eq!(
        refusal("{\"version\":2}"),
        at("version", PreferenceReason::Invalid)
    );
    assert_eq!(
        refusal("{\"version\":1,\"sshPath\":\"/usr/bin/ssh\"}"),
        at("document", PreferenceReason::Invalid)
    );
    assert_eq!(
        refusal("{\"version\":1,\"sshProgram\":\"bin/ssh\"}"),
        at("sshProgram", PreferenceReason::Relative)
    );
    assert_eq!(
        refusal("{\"version\":1,\"sshProgram\":\"\"}"),
        at("sshProgram", PreferenceReason::Invalid)
    );
    assert_eq!(
        refusal("{\"version\":1,\"reconnectLimitSeconds\":3601}"),
        at("reconnectLimitSeconds", PreferenceReason::OutOfRange)
    );
    let long = format!("{{\"version\":1,\"sshProgram\":\"/{}\"}}", "a".repeat(5000));
    assert_eq!(refusal(&long), at("sshProgram", PreferenceReason::TooLong));
    let huge = format!("{{\"version\":1,\"x\":\"{}\"}}", "a".repeat(MAX_BYTES));
    assert_eq!(refusal(&huge), at("document", PreferenceReason::TooLong));
}

#[test]
fn a_stored_program_that_disappeared_is_reported_not_refused() {
    let scratch = Scratch::new("gone");
    let path = scratch.0.join(ssh_file_name());
    let document = serde_json::json!({ "version": 1, "sshProgram": path }).to_string();
    let preferences = Preferences::parse(Some(&document)).unwrap();
    let ssh = preferences.ssh(None);
    assert_eq!(ssh.source, Source::Setting);
    assert_eq!(ssh.status, ProgramStatus::Missing);
    assert_eq!(ssh.command(), path.to_string_lossy());
}

#[test]
fn automatic_finds_the_first_executable_ssh_on_path() {
    let first = Scratch::new("first");
    let second = Scratch::new("second");
    let found = second.file(&ssh_file_name(), true);
    #[cfg(unix)]
    first.file(&ssh_file_name(), false);
    let path = std::env::join_paths([PathBuf::from("relative"), first.0.clone(), second.0.clone()])
        .unwrap();
    let ssh = Preferences::default().ssh(Some(&path));
    assert_eq!(ssh.source, Source::Automatic);
    assert_eq!(ssh.status, ProgramStatus::Found);
    assert_eq!(ssh.path.as_deref(), Some(found.to_string_lossy().as_ref()));

    let empty = Scratch::new("empty");
    let none = Preferences::default().ssh(Some(empty.0.as_os_str()));
    assert_eq!(none.status, ProgramStatus::Missing);
    assert_eq!(none.path, None);
    assert_eq!(none.command(), "ssh");
}

#[test]
fn an_edit_requires_an_executable_file_and_round_trips() {
    let scratch = Scratch::new("edit");
    let program = scratch.file(&ssh_file_name(), true);
    let mut preferences = Preferences::default();
    let refused = |result: Result<(), PreferenceError>| result.unwrap_err().reason;

    assert_eq!(
        refused(preferences.set_ssh_program(Some("ssh"))),
        PreferenceReason::Relative
    );
    let missing = scratch.0.join("nothing");
    assert_eq!(
        refused(preferences.set_ssh_program(Some(&missing.to_string_lossy()))),
        PreferenceReason::Missing
    );
    assert_eq!(
        refused(preferences.set_ssh_program(Some(&scratch.0.to_string_lossy()))),
        PreferenceReason::NotExecutable
    );
    #[cfg(unix)]
    {
        let plain = scratch.file("plain", false);
        assert_eq!(
            refused(preferences.set_ssh_program(Some(&plain.to_string_lossy()))),
            PreferenceReason::NotExecutable
        );
    }
    assert_eq!(preferences, Preferences::default());

    let padded = format!("  {}  ", program.to_string_lossy());
    preferences.set_ssh_program(Some(&padded)).unwrap();
    preferences.set_reconnect_limit(Some(0)).unwrap();
    assert_eq!(
        refused(preferences.set_reconnect_limit(Some(MAX_RECONNECT_LIMIT_SECONDS + 1))),
        PreferenceReason::OutOfRange
    );
    let text = preferences.text().unwrap();
    let read = Preferences::parse(Some(&text)).unwrap();
    assert_eq!(read, preferences);
    assert_eq!(read.ssh_program(), Some(program.to_string_lossy().as_ref()));
    let ssh = read.ssh(None);
    assert_eq!(
        (ssh.source, ssh.status),
        (Source::Setting, ProgramStatus::Found)
    );
    let reconnect = read.reconnect();
    assert_eq!(
        (reconnect.source, reconnect.limit_seconds),
        (Source::Setting, 0)
    );
    assert!(reconnect.delays_milliseconds.is_empty());

    preferences.set_ssh_program(None).unwrap();
    preferences.set_reconnect_limit(None).unwrap();
    assert_eq!(preferences, Preferences::default());
}

#[test]
fn reconnect_pauses_double_up_to_thirty_seconds_and_add_up_to_the_limit() {
    assert_eq!(reconnect_delays(1), vec![1_000]);
    assert_eq!(reconnect_delays(2), vec![1_000, 1_000]);
    let hour = reconnect_delays(MAX_RECONNECT_LIMIT_SECONDS);
    assert_eq!(hour.iter().sum::<u64>(), 3_600_000);
    assert_eq!(&hour[..6], &[1_000, 2_000, 4_000, 8_000, 16_000, 30_000]);
    assert!(hour.iter().all(|pause| *pause <= 30_000));
    assert_eq!(hour.len(), 124);
}
