//! Real Git integration: `git.executionInspect` normalizes inherited tag booleans.

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Repository(PathBuf);

impl Repository {
    fn request(&self, command: &str, mut payload: Value) -> Value {
        payload["root"] = json!(self.0);
        // Core owns subprocess deadlines; the fixture never waits on an
        // unbounded native child, including repository initialization.
        serde_json::from_str(&crate::execute_json(
            &json!({
                "timeoutMilliseconds": 5000,
                "command": command,
                "payload": payload,
            })
            .to_string(),
        ))
        .unwrap()
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("Could not clean tag boolean repository: {error}");
        }
    }
}

// Real Git integration protects the inspection-to-presentation contract.
#[test]
fn inspection_normalizes_inherited_tag_booleans_without_changing_edit_values() {
    let repo = Repository(std::env::temp_dir().join(format!(
        "lithe-tag-booleans-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir_all(&repo.0).unwrap();
    let initialized = repo.request(
        "git.command",
        json!({ "arguments": ["init", "--template=", "-q", "-b", "main"] }),
    );
    assert_eq!(initialized["ok"], true, "{initialized}");
    assert_eq!(initialized["data"]["exitCode"], 0, "{initialized}");

    // Exercise Git's parser rather than duplicating its accepted spellings.
    // Valueless and empty assignments are deliberately distinct booleans.
    let cases = [
        ("yes", "pruneTags = yes", "true"),
        ("on", "pruneTags = on", "true"),
        ("one", "pruneTags = 1", "true"),
        ("mixed", "pruneTags = TrUe", "true"),
        ("valueless", "pruneTags", "true"),
        ("no", "pruneTags = no", "false"),
        ("off", "pruneTags = off", "false"),
        ("zero", "pruneTags = 0", "false"),
        ("false", "pruneTags = FaLsE", "false"),
        ("empty", "pruneTags =", "false"),
        ("invalid", "pruneTags = invalid", "invalid"),
    ];
    let path = repo.0.join(".git/config");
    let mut config = fs::read_to_string(&path).unwrap();
    config.push_str("\n[fetch]\npruneTags = yes\n");
    for (name, configuration, _) in cases {
        config.push_str(&format!("\n[remote \"{name}\"]\n{configuration}\n"));
    }
    fs::write(path, config).unwrap();

    let response = repo.request("git.executionInspect", json!({}));
    assert_eq!(response["ok"], true, "{response}");
    let entries = response["data"]["entries"].as_array().unwrap();
    for (name, _, expected) in cases {
        let key = format!("remote.{name}.prunetags");
        let entry = entries
            .iter()
            .find(|entry| entry["key"] == key && entry["effective"] == true)
            .unwrap();
        assert_eq!(entry["value"], expected, "{key}");
        assert_eq!(entry["scope"], "local", "{key}");
        assert!(entry["origin"].as_str().unwrap().ends_with(".git/config"));
    }
    let fetch = entries
        .iter()
        .find(|entry| entry["key"] == "fetch.prunetags" && entry["effective"] == true)
        .unwrap();
    assert_eq!(fetch["value"], "true");
    let field = response["data"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == "fetch.prunetags")
        .unwrap();
    assert_eq!(field["configuredValues"], json!(["yes"]));
}

#[test]
fn inspection_normalizes_skip_fetch_all_booleans() {
    let repo = Repository(std::env::temp_dir().join(format!(
        "lithe-skip-fetch-booleans-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir_all(&repo.0).unwrap();
    let initialized = repo.request(
        "git.command",
        json!({ "arguments": ["init", "--template=", "-q", "-b", "main"] }),
    );
    assert_eq!(initialized["ok"], true, "{initialized}");
    assert_eq!(initialized["data"]["exitCode"], 0, "{initialized}");

    let path = repo.0.join(".git/config");
    let mut config = fs::read_to_string(&path).unwrap();
    config.push_str("\n[remote \"yes\"]\nurl = /tmp/yes\nskipFetchAll = yes\n");
    config.push_str("\n[remote \"valueless\"]\nurl = /tmp/valueless\nskipFetchAll\n");
    config.push_str("\n[remote \"empty\"]\nurl = /tmp/empty\nskipFetchAll =\n");
    config.push_str("\n[remote \"no\"]\nurl = /tmp/no\nskipFetchAll = no\n");
    fs::write(path, config).unwrap();

    let response = repo.request("git.executionInspect", json!({}));
    assert_eq!(response["ok"], true, "{response}");
    let entries = response["data"]["entries"].as_array().unwrap();
    for (name, expected) in [
        ("yes", "true"),
        ("valueless", "true"),
        ("empty", "false"),
        ("no", "false"),
    ] {
        let key = format!("remote.{name}.skipfetchall");
        let entry = entries
            .iter()
            .find(|entry| entry["key"] == key && entry["effective"] == true)
            .unwrap();
        assert_eq!(entry["value"], expected, "{key}");
    }
}

#[test]
fn inspection_matches_git_when_tags_are_forced_or_configuration_is_invalid() {
    let repo = Repository(std::env::temp_dir().join(format!(
        "lithe-fetch-effective-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir_all(&repo.0).unwrap();
    let git = |arguments: &[&str]| {
        let response = repo.request("git.command", json!({"arguments": arguments}));
        assert_eq!(response["ok"], true, "{response}");
        response["data"].clone()
    };
    let ok = |arguments: &[&str]| {
        let response = git(arguments);
        assert_eq!(response["exitCode"], 0, "{arguments:?}: {response}");
    };
    ok(&["init", "--template=", "-q", "-b", "main"]);
    ok(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "--no-gpg-sign",
        "-qm",
        "base",
    ]);
    ok(&["-c", "tag.gpgSign=false", "tag", "reachable"]);
    ok(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "--no-gpg-sign",
        "-qm",
        "other",
    ]);
    ok(&["-c", "tag.gpgSign=false", "tag", "extra"]);
    ok(&["reset", "--hard", "HEAD~1"]);
    let remote = repo.0.join("remote.git");
    ok(&[
        "init",
        "--bare",
        "--template=",
        "-q",
        remote.to_str().unwrap(),
    ]);
    ok(&[
        "--git-dir",
        remote.to_str().unwrap(),
        "symbolic-ref",
        "HEAD",
        "refs/heads/main",
    ]);
    ok(&["remote", "add", "origin", remote.to_str().unwrap()]);
    ok(&[
        "push",
        "-q",
        "origin",
        "HEAD:refs/heads/main",
        "refs/tags/*:refs/tags/*",
    ]);
    ok(&["tag", "-d", "reachable", "extra"]);
    ok(&["config", "lithe.fetch.tags", "none"]);
    ok(&["config", "lithe.fetch.prune", "false"]);
    ok(&["config", "fetch.pruneTags", "true"]);
    let inspect = || {
        let result = repo.request("git.executionInspect", json!({}));
        assert_eq!(result["ok"], true, "{result}");
        result["data"]["resolvedFetchBehavior"]["tags"].clone()
    };
    let fetch = || git(&["fetch", "--dry-run", "--no-tags", "--no-prune", "origin"]);
    assert_eq!(inspect()["value"], "all");
    let actual = fetch();
    assert_eq!(actual["exitCode"], 0, "{actual}");
    assert!(
        actual["stderr"].as_str().unwrap().contains("extra"),
        "{actual}"
    );

    ok(&["config", "--unset", "fetch.pruneTags"]);
    ok(&[
        "config",
        "--add",
        "remote.origin.fetch",
        "+refs/tags/*:refs/tags/*",
    ]);
    assert_eq!(inspect()["value"], "all");
    assert!(fetch()["stderr"].as_str().unwrap().contains("extra"));
    ok(&["config", "--add", "remote.origin.fetch", "^refs/tags/extra"]);
    assert_eq!(inspect()["value"], "custom");
    assert!(!fetch()["stderr"].as_str().unwrap().contains("extra"));

    ok(&["config", "--add", "remote.origin.fetch", ""]);
    assert_eq!(inspect()["value"], "none");
    assert!(!fetch()["stderr"].as_str().unwrap().contains("extra"));

    ok(&["config", "remote.origin.pruneTags", "invalid"]);
    let unknown = inspect();
    assert_eq!(unknown["value"], "unavailable");
    assert_eq!(unknown["reasonCode"], "invalidConfiguration");
    assert_ne!(fetch()["exitCode"], 0);
    ok(&["config", "--unset", "remote.origin.pruneTags"]);
    for key in ["fetch.prune", "remote.origin.prune", "remote.other.prune"] {
        ok(&["config", key, "invalid"]);
        assert_eq!(inspect()["reasonCode"], "invalidConfiguration", "{key}");
        assert_ne!(fetch()["exitCode"], 0, "{key}");
        ok(&["config", "--unset", key]);
    }
}
