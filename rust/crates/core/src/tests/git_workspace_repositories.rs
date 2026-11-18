//! `git.workspaceRepositorySelection` and `git.workspaceChangeGroups` against
//! their shared fixtures, and against real discovery and status output passed
//! through unchanged.

use super::support::temporary_root;
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

struct Folder(PathBuf);
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn call(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({"id":"workspace-repositories-test", "command":command, "payload":payload})
            .to_string(),
    ))
    .unwrap()
}

fn data(command: &str, payload: Value) -> Value {
    let response = call(command, payload);
    assert_eq!(response["ok"], true, "{command}: {response}");
    response["data"].clone()
}

fn git(root: &Path, arguments: &[&str]) {
    // The shared host owns the subprocess deadline and process-tree cleanup.
    let result = data("git.command", json!({"root": root, "arguments": arguments}));
    assert_eq!(result["exitCode"], 0, "{arguments:?}: {result}");
}

fn init(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "Workspace Test"]);
    git(root, &["config", "user.email", "workspace@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "core.autocrlf", "false"]);
}

fn check_fixture(command: &str, fixture: &str) {
    let fixture: Value = serde_json::from_str(fixture).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = call(command, case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["response"], "{}", case["name"]);
    }
    for case in fixture["invalid"].as_array().unwrap() {
        let response = call(command, case["request"].clone());
        assert_eq!(response["ok"], false, "{}: {response}", case["name"]);
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "{}",
            case["name"]
        );
    }
}

#[test]
fn selection_change_groups_and_history_merge_match_their_shared_fixtures() {
    check_fixture(
        "git.workspaceRepositorySelection",
        include_str!("../../../../fixtures/git/workspace-repository-selection-v1.json"),
    );
    check_fixture(
        "git.workspaceChangeGroups",
        include_str!("../../../../fixtures/git/workspace-change-groups-v1.json"),
    );
    check_fixture(
        "git.workspaceHistoryMerge",
        include_str!("../../../../fixtures/git/workspace-history-merge-v1.json"),
    );
    check_fixture(
        "git.workspaceCommonReferences",
        include_str!("../../../../fixtures/git/workspace-common-references-v1.json"),
    );
    check_fixture(
        "git.workspaceBranchPlan",
        include_str!("../../../../fixtures/git/workspace-branch-plan-v1.json"),
    );
}

/// Discovery entries with the status each repository reports, as hosts send them.
fn repositories_with_changes(root: &Path) -> Value {
    let mut repositories =
        data("workspace.repositories", json!({"root": root}))["repositories"].clone();
    for repository in repositories.as_array_mut().unwrap() {
        let status = data("git.status", json!({"root": repository["path"]}));
        repository["changes"] = status["changes"].clone();
    }
    repositories
}

fn group<'a>(groups: &'a Value, id: &str) -> &'a Value {
    groups["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["id"] == id)
        .unwrap_or_else(|| panic!("no group {id}: {groups}"))
}

#[test]
fn discovery_and_status_pass_through_to_selection_and_change_groups() {
    let parent = Folder(temporary_root("workspace-change-groups"));
    init(&parent.0);
    let child = parent.0.join("libs/B");
    init(&child);
    fs::write(child.join("hello.ts"), "one").unwrap();
    git(&child, &["add", "hello.ts"]);
    git(&child, &["commit", "-qm", "child initial"]);
    fs::write(parent.0.join("parent.txt"), "parent").unwrap();
    fs::write(parent.0.join(".gitignore"), "extras/\n").unwrap();
    fs::write(
        parent.0.join(".gitmodules"),
        "[submodule \"B\"]\npath = libs/B\nurl = ./libs/B\n",
    )
    .unwrap();
    git(&parent.0, &["add", "."]);
    git(&parent.0, &["commit", "-qm", "parent initial"]);
    let extras = parent.0.join("extras");
    init(&extras);
    fs::write(extras.join("notes.txt"), "notes").unwrap();
    fs::write(child.join("hello.ts"), "two").unwrap();

    let discovered =
        data("workspace.repositories", json!({"root": parent.0}))["repositories"].clone();
    let selection = data(
        "git.workspaceRepositorySelection",
        json!({"repositories": discovered, "selected": "libs/B"}),
    );
    assert_eq!(
        selection,
        json!({"active": "libs/B", "fellBack": false, "others": [".", "extras"]})
    );

    // Child content alone leaves the parent's recorded commit where it was.
    let groups = data(
        "git.workspaceChangeGroups",
        json!({"repositories": repositories_with_changes(&parent.0)}),
    );
    let ids = groups["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, [".", "extras", "libs/B"], "{groups}");
    assert_eq!(group(&groups, ".")["toggleable"], false, "{groups}");
    assert!(group(&groups, ".").get("staging").is_none(), "{groups}");
    assert!(group(&groups, "extras").get("parentReference").is_none());
    assert_eq!(
        group(&groups, "extras")["staging"],
        json!({"operation": "stage", "paths": ["notes.txt"]})
    );
    assert_eq!(group(&groups, "libs/B")["parentReference"], "unchanged");
    assert_eq!(
        groups["stageAll"],
        json!({"operation": "stage", "repositories": [
            {"id": "extras", "paths": ["notes.txt"]},
            {"id": "libs/B", "paths": ["hello.ts"]}
        ]})
    );

    // A child commit moves the gitlink; the child keeps changes so its group stays.
    git(&child, &["commit", "-qam", "child update"]);
    fs::write(child.join("hello.ts"), "three").unwrap();
    let groups = data(
        "git.workspaceChangeGroups",
        json!({"repositories": repositories_with_changes(&parent.0)}),
    );
    assert_eq!(
        group(&groups, "libs/B")["parentReference"],
        "moved",
        "{groups}"
    );
}

/// Commits `message` in `root` with `date` as both the author and the committer date.
fn commit_at(root: &Path, message: &str, date: &str) {
    fs::write(root.join("file.txt"), message).unwrap();
    git(root, &["add", "file.txt"]);
    git(root, &["commit", "-qm", message, "--date", date]);
}

/// One `git.historyPage` page of `root` as the merge takes it.
fn history_page(root: &Path, id: &str, limit: usize) -> Value {
    let page = data(
        "git.historyPage",
        json!({"root": root, "order": "date", "limit": limit}),
    );
    let commits = page["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|commit| {
            json!({
                "hash": commit["hash"],
                "parentHashes": commit["parentHashes"],
                "timestamp": commit["dateUnixSeconds"],
            })
        })
        .collect::<Vec<_>>();
    json!({"id": id, "complete": page["hasMore"] == false, "commits": commits})
}

#[test]
fn history_pages_pass_through_to_the_history_merge() {
    let folder = Folder(temporary_root("workspace-history-merge"));
    let backend = folder.0.join("backend");
    let frontend = folder.0.join("frontend");
    init(&backend);
    init(&frontend);
    commit_at(&backend, "backend one", "2026-01-01T10:00:00+00:00");
    commit_at(&frontend, "frontend one", "2026-01-02T10:00:00+00:00");
    commit_at(&backend, "backend two", "2026-01-03T10:00:00+00:00");

    // A first page that leaves backend's older commit unread stops after it.
    let merged = data(
        "git.workspaceHistoryMerge",
        json!({"repositories": [history_page(&backend, "backend", 1), history_page(&frontend, "frontend", 100)]}),
    );
    assert_eq!(merged["pending"], json!([0]), "{merged}");
    assert_eq!(merged["entries"].as_array().unwrap().len(), 1, "{merged}");

    let merged = data(
        "git.workspaceHistoryMerge",
        json!({"repositories": [history_page(&backend, "backend", 100), history_page(&frontend, "frontend", 100)]}),
    );
    let order = merged["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["repository"].as_u64().unwrap(),
                entry["index"].as_u64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(order, [(0, 0), (1, 0), (0, 1)], "{merged}");
    assert_eq!(merged["pending"], json!([]), "{merged}");
}

/// The `references` of `root`'s `git.references` answer, unchanged, as one repository entry.
fn repository_references(root: &Path, id: &str) -> Value {
    let references = data("git.references", json!({"root": root}))["references"].clone();
    json!({"id": id, "references": references})
}

/// The branch `root` has checked out.
fn current_branch(root: &Path) -> String {
    let result = data(
        "git.command",
        json!({"root": root, "arguments": ["branch", "--show-current"]}),
    );
    result["stdout"].as_str().unwrap().trim().to_owned()
}

/// Runs one plan `git.write` with the reference the plan names.
fn write(root: &Path, operation: &Value, reference: &Value) {
    let result = data(
        "git.write",
        json!({"root": root, "operation": operation, "gitReference": reference}),
    );
    assert_eq!(result["exitCode"], 0, "{operation} {reference}: {result}");
}

#[test]
fn references_pass_through_to_common_branches_and_the_branch_plan() {
    let folder = Folder(temporary_root("workspace-common-references"));
    let backend = folder.0.join("backend");
    let frontend = folder.0.join("frontend");
    for root in [&backend, &frontend] {
        init(root);
        commit_at(root, "initial", "2026-01-01T10:00:00+00:00");
        git(root, &["branch", "-M", "main"]);
        git(root, &["branch", "dev"]);
    }
    git(&backend, &["branch", "backend-only"]);
    git(&backend, &["tag", "v1"]);
    git(&frontend, &["switch", "-q", "dev"]);

    let repositories = json!([
        repository_references(&backend, "backend"),
        repository_references(&frontend, "frontend")
    ]);
    let common = data(
        "git.workspaceCommonReferences",
        json!({"repositories": repositories}),
    );
    let names = common["local"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reference| reference["shortName"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["dev", "main"], "{common}");
    assert!(
        common["local"]
            .as_array()
            .unwrap()
            .iter()
            .all(|reference| reference["isCurrent"] == false),
        "{common}"
    );

    // The plan's references are the ones git.write takes; a rollback returns each repository.
    let plan = data(
        "git.workspaceBranchPlan",
        json!({"operation": "checkout", "reference": "refs/heads/dev", "repositories": repositories}),
    );
    assert_eq!(plan["skipped"], json!(["frontend"]), "{plan}");
    let step = &plan["steps"][0];
    assert_eq!(step["repository"], "backend", "{plan}");
    write(&backend, &json!("checkout"), &step["reference"]);
    assert_eq!(current_branch(&backend), "dev");
    for action in step["rollback"].as_array().unwrap() {
        write(&backend, &action["operation"], &action["reference"]);
    }
    assert_eq!(current_branch(&backend), "main");

    let plan = data(
        "git.workspaceBranchPlan",
        json!({"operation": "createBranch", "reference": "refs/heads/main", "name": "feature",
               "checkout": true, "repositories": repositories}),
    );
    for (root, step) in [&backend, &frontend]
        .into_iter()
        .zip(plan["steps"].as_array().unwrap())
    {
        let result = data(
            "git.write",
            json!({"root": root, "operation": "createBranch", "gitReference": step["reference"],
                   "name": "feature", "checkout": true}),
        );
        assert_eq!(result["exitCode"], 0, "{result}");
        assert_eq!(current_branch(root), "feature");
        for action in step["rollback"].as_array().unwrap() {
            write(root, &action["operation"], &action["reference"]);
        }
    }
    assert_eq!(current_branch(&backend), "main");
    assert_eq!(current_branch(&frontend), "dev");
    let branches = data(
        "git.command",
        json!({"root": frontend, "arguments": ["branch", "--list", "feature"]}),
    );
    assert_eq!(branches["stdout"], "", "{branches}");
}
