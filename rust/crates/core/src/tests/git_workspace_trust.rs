//! Issue 2208: in safe mode Core starts no Git, because a repository's own configuration
//! can name a program Git runs, such as a `core.fsmonitor` hook on every status refresh.
use super::support::temporary_root;
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

/// `trusted` is the request's `gitExecution.projectCodeAllowed`.
fn request(command: &str, payload: Value, trusted: bool) -> Value {
    serde_json::from_str(&execute_json(
        &json!({
            "timeoutMilliseconds": 10000,
            "gitExecution": { "projectCodeAllowed": trusted },
            "command": command,
            "payload": payload,
        })
        .to_string(),
    ))
    .unwrap()
}

struct Repository(PathBuf);

impl Drop for Repository {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("Could not clean the workspace trust repository: {error}");
        }
    }
}

#[test]
fn untrusted_workspace_requests_start_no_git() {
    let repository = Repository(temporary_root("git-untrusted-workspace"));
    let root = &repository.0;
    fs::create_dir_all(root.join("ignored")).unwrap();
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    let init = request(
        "git.command",
        json!({ "root": root, "arguments": ["init", "--template=", "-q", "-b", "main"] }),
        true,
    );
    assert_eq!(init["ok"], true, "{init}");
    assert_eq!(init["data"]["exitCode"], 0, "{init}");

    // A hook the repository configures: it leaves a marker whenever Git runs it.
    let marker = root.join("hook-ran");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = root.join("fsmonitor-hook");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let config = root.join(".git/config");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!("[core]\n\tfsmonitor = {}\n", hook.display()));
        fs::write(config, text).unwrap();
    }

    for (command, payload) in [
        ("git.status", json!({ "root": root })),
        (
            "git.command",
            json!({ "root": root, "arguments": ["status"] }),
        ),
        ("git.historyPage", json!({ "root": root, "limit": 10 })),
    ] {
        let refused = request(command, payload, false);
        assert_eq!(refused["ok"], false, "{command}: {refused}");
        assert_eq!(
            refused["error"]["code"], "git_untrusted_workspace",
            "{command}: {refused}"
        );
    }
    // A file request still answers, with the native ignore rules instead of Git's.
    let listing = request(
        "workspace.listDirectories",
        json!({ "root": root, "paths": [""] }),
        false,
    );
    assert_eq!(listing["ok"], true, "{listing}");
    assert!(!marker.exists(), "an untrusted request ran the hook");

    // The same status in a trusted workspace runs Git, and with it the hook.
    let status = request("git.status", json!({ "root": root }), true);
    assert_eq!(status["ok"], true, "{status}");
    #[cfg(unix)]
    assert!(marker.exists(), "the fixture's hook never ran: {status}");
}
