use super::support::temporary_root;
use crate::execute_json;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

// Every request has a Core deadline; the owner removes the repository and linked worktrees
// together on success and assertion failure, without touching any developer repository.
struct Repository(PathBuf);

impl Repository {
    fn new() -> Self {
        let repo = Self(temporary_root("branch-force-delete"));
        fs::create_dir_all(&repo.0).unwrap();
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Lithe Fixture"]);
        repo.git(&["config", "user.email", "fixture@example.invalid"]);
        repo.git(&["config", "commit.gpgSign", "false"]);
        repo.git(&["config", "core.hooksPath", "disabled-fixture-hooks"]);
        repo.git(&["commit", "--allow-empty", "-qm", "base"]);
        repo
    }

    fn request(&self, command: &str, mut payload: Value) -> Value {
        payload["root"] = json!(self.0);
        let response: Value = serde_json::from_str(&execute_json(
            &json!({
                "id": "branch-delete", "command": command, "timeoutMilliseconds": 5000,
                "payload": payload
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        response["data"].clone()
    }

    fn git(&self, arguments: &[&str]) -> String {
        let data = self.request("git.command", json!({"arguments": arguments}));
        assert_eq!(data["exitCode"], 0, "{data}");
        assert!(data["operationError"].is_null(), "{data}");
        data["stdout"].as_str().unwrap().trim().to_string()
    }

    fn delete(&self, branch: &str, force: bool, expected: Option<&str>) -> Value {
        self.request("git.write", json!({
            "operation": "deleteBranch",
            "gitReference": {"fullName": format!("refs/heads/{branch}"), "shortName": branch, "kind": "local"},
            "force": force, "expectedTarget": expected
        }))
    }

    fn unmerged(&self) -> String {
        self.git(&["switch", "-qc", "backup/before-strip-trailers"]);
        self.git(&["commit", "--allow-empty", "-qm", "unique work"]);
        let target = self.git(&["rev-parse", "HEAD"]);
        self.git(&["switch", "-q", "main"]);
        target
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("Could not remove branch deletion fixture: {error}");
        }
    }
}

#[test]
fn branch_force_delete_requires_reviewed_target_and_preserves_recovery() {
    let repo = Repository::new();
    let target = repo.unmerged();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/git/branch-delete-v1.json"
    ))
    .unwrap();
    let refused = repo.delete("backup/before-strip-trailers", false, None);
    assert_eq!(
        refused["operationError"]["code"], fixture["unmergedCode"],
        "{refused}"
    );
    let review: Value =
        serde_json::from_str(refused["operationError"]["details"].as_str().unwrap()).unwrap();
    assert_eq!(review["name"], "backup/before-strip-trailers");
    assert_eq!(review["expectedTarget"], target);
    assert_eq!(
        repo.git(&["rev-parse", "refs/heads/backup/before-strip-trailers"]),
        target
    );

    let unreviewed = repo.delete("backup/before-strip-trailers", true, None);
    assert_eq!(
        unreviewed["operationError"]["code"], "invalid_request",
        "{unreviewed}"
    );
    let invalid = repo.delete("backup/before-strip-trailers", true, Some("HEAD"));
    assert_eq!(
        invalid["operationError"]["code"], "invalid_request",
        "{invalid}"
    );
    let deleted = repo.delete("backup/before-strip-trailers", true, Some(&target));
    assert!(deleted["operationError"].is_null(), "{deleted}");
    assert_eq!(
        deleted["branchDeletion"],
        json!({"name":"backup/before-strip-trailers", "deletedTarget":target})
    );
    assert!(repo
        .git(&["branch", "--list", "backup/before-strip-trailers"])
        .is_empty());
    repo.git(&["branch", "backup/before-strip-trailers", &target]);
    assert_eq!(
        repo.git(&["rev-parse", "refs/heads/backup/before-strip-trailers"]),
        target
    );
}

#[test]
fn branch_force_delete_rejects_changed_targets_and_checked_out_branches() {
    let repo = Repository::new();
    let target = repo.unmerged();
    let changed = repo.delete(
        "backup/before-strip-trailers",
        true,
        Some(&repo.git(&["rev-parse", "main"])),
    );
    assert_eq!(
        changed["operationError"]["code"], "git_branch_changed",
        "{changed}"
    );
    assert_eq!(
        repo.git(&["rev-parse", "refs/heads/backup/before-strip-trailers"]),
        target
    );
    let current = repo.delete("main", true, Some(&repo.git(&["rev-parse", "main"])));
    assert_eq!(
        current["operationError"]["code"], "invalid_request",
        "{current}"
    );
    let checkout = repo.0.join("linked-checkout");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        checkout.to_str().unwrap(),
        "backup/before-strip-trailers",
    ]);
    let occupied = repo.delete("backup/before-strip-trailers", true, Some(&target));
    assert_eq!(
        occupied["operationError"]["code"], "invalid_request",
        "{occupied}"
    );
    assert_eq!(
        repo.git(&["rev-parse", "refs/heads/backup/before-strip-trailers"]),
        target
    );
}
