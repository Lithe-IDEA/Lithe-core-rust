use super::support::temporary_root;
use crate::execute_json;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

// These fixtures need Unix file names, not platform-specific Git behavior.
// Every subprocess, including setup, uses Core's five-second request deadline.
// The owner removes the repository when an assertion unwinds too.
struct Repository(PathBuf);

impl Repository {
    fn new(label: &str) -> Self {
        let repository = Self(temporary_root(label));
        fs::create_dir_all(&repository.0).unwrap();
        repository.git(&["init", "--template=", "-q", "-b", "main"]);
        repository.git(&["config", "core.autocrlf", "false"]);
        repository.git(&["config", "core.hooksPath", ".git/empty-hooks"]);
        repository.git(&["config", "core.excludesFile", ".git/empty-excludes"]);
        repository
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn data(&self, command: &str, mut payload: Value) -> Value {
        payload["root"] = json!(self.0);
        let response: Value = serde_json::from_str(&execute_json(
            &json!({
                "command": command, "timeoutMilliseconds": 5000, "payload": payload,
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        response["data"].clone()
    }

    fn git(&self, arguments: &[&str]) -> String {
        let data = self.data("git.command", json!({"arguments": arguments}));
        assert_eq!(data["exitCode"], 0, "{data}");
        assert!(data["operationError"].is_null(), "{data}");
        data["stdout"].as_str().unwrap().to_string()
    }

    fn commit(&self) {
        self.git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ]);
    }

    fn mutate(&self, operation: &str, paths: &[&str]) {
        let data = self.data("git.write", json!({"operation": operation, "paths": paths}));
        assert_eq!(data["exitCode"], 0, "{data}");
        assert!(data["operationError"].is_null(), "{data}");
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("Could not remove colon-path repository fixture: {error}");
        }
    }
}

const PATHS: [&str; 4] = ["a:b.txt", ":(glob)*.txt", r"D:\logs", ":(exclude)*.txt"];

#[test]
fn stage_and_unstage_keep_unix_colon_file_names_literal() {
    let repository = Repository::new("colon-stage");
    repository.commit();
    for path in PATHS {
        repository.write(path, "selected\n");
    }
    repository.write("unselected.txt", "keep\n");
    repository.mutate("stage", &PATHS);
    let tracked = repository.git(&["ls-files", "-z"]);
    assert_eq!(
        tracked,
        ":(exclude)*.txt\0:(glob)*.txt\0D:\\logs\0a:b.txt\0"
    );
    repository.mutate("unstage", &PATHS);
    assert_eq!(repository.git(&["ls-files", "-z"]), "");
    for path in PATHS {
        assert_eq!(
            fs::read_to_string(repository.0.join(path)).unwrap(),
            "selected\n"
        );
    }
}

#[test]
fn ignore_and_exclude_keep_unix_colon_file_names_literal() {
    let repository = Repository::new("colon-ignore");
    for path in PATHS {
        repository.write(path, "selected\n");
    }
    repository.write("unselected.txt", "keep\n");
    repository.mutate("ignore", &PATHS[..2]);
    repository.mutate("exclude", &PATHS[2..]);
    assert_eq!(
        repository.git(&[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z"
        ]),
        ":(exclude)*.txt\0:(glob)*.txt\0D:\\logs\0a:b.txt\0"
    );
    assert_eq!(
        repository.git(&["ls-files", "--others", "--exclude-standard", "-z"]),
        ".gitignore\0unselected.txt\0"
    );
}

#[test]
fn diff_and_rollback_keep_colon_path_selection_literal() {
    let repository = Repository::new("colon-diff");
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "before\n");
    }
    repository.git(&["add", "--all"]);
    repository.commit();
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "after\n");
    }
    for path in PATHS {
        for worktree_snapshot in [false, true] {
            let diff = repository.data(
                "git.diff",
                json!({
                    "pathspecs": [path], "includeFileSections": true,
                    "worktreeSnapshot": worktree_snapshot,
                }),
            );
            let files = diff["files"].as_array().unwrap();
            assert_eq!(files.len(), 1, "{path}: {diff}");
            assert_eq!(files[0]["path"], path, "{diff}");
            assert!(
                files[0]["patch"].as_str().unwrap().contains("+after"),
                "{diff}"
            );
        }
        repository.mutate("discard", &[path]);
        assert_eq!(
            fs::read_to_string(repository.0.join(path)).unwrap(),
            "before\n"
        );
        repository.write(path, "staged\n");
        repository.mutate("stage", &[path]);
        repository.write(path, "unstaged\n");
        let rollback = repository.data(
            "git.rollbackPrepare",
            json!({"changes": [{"path": path, "status": "MM"}]}),
        );
        assert_eq!(
            rollback["steps"],
            json!([{"operation": "discardAll", "paths": [path]}]),
            "{rollback}"
        );
        repository.mutate("discardAll", &[path]);
        assert_eq!(
            fs::read_to_string(repository.0.join(path)).unwrap(),
            "before\n"
        );
    }
    assert_eq!(repository.git(&["diff", "--cached", "--name-only"]), "");
    assert_eq!(
        fs::read_to_string(repository.0.join("unselected.txt")).unwrap(),
        "after\n"
    );
}

#[test]
fn patch_exchange_round_trips_colon_paths_without_touching_other_files() {
    let repository = Repository::new("colon-patch");
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "before\n");
    }
    repository.git(&["add", "--all"]);
    repository.commit();
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "after\n");
    }
    let exported = repository.data(
        "git.patchExport",
        json!({
            "source": "unstaged", "paths": PATHS,
        }),
    );
    let patch = exported["patch"].as_str().unwrap();
    let preview = repository.data(
        "git.patchPreview",
        json!({
            "patch": patch, "target": "worktree", "reverse": true,
        }),
    );
    assert_eq!(preview["applicable"], true, "{preview}");
    assert_eq!(
        preview["files"].as_array().unwrap().len(),
        PATHS.len(),
        "{preview}"
    );
    let applied = repository.data(
        "git.patchApply",
        json!({
            "patch": patch, "target": "worktree", "reverse": true,
            "expectedState": preview["expectedState"],
        }),
    );
    assert_eq!(applied["exitCode"], 0, "{applied}");
    for path in PATHS {
        assert_eq!(
            fs::read_to_string(repository.0.join(path)).unwrap(),
            "before\n"
        );
    }
    assert_eq!(
        fs::read_to_string(repository.0.join("unselected.txt")).unwrap(),
        "after\n"
    );
}

#[test]
fn blame_and_line_history_read_colon_paths_from_the_pinned_revision() {
    let repository = Repository::new("colon-history");
    for path in PATHS {
        repository.write(path, "one\ntwo\n");
    }
    repository.git(&["add", "--all"]);
    repository.commit();
    let head = repository.git(&["rev-parse", "HEAD"]).trim().to_string();
    for path in PATHS {
        let blame = repository.data("git.blame", json!({"path": path}));
        assert_eq!(
            blame["lines"].as_array().unwrap().len(),
            2,
            "{path}: {blame}"
        );
        assert_eq!(blame["lines"][0]["commitHash"], head, "{blame}");
        let history = repository.data(
            "git.lineHistory",
            json!({
                "path": path, "content": "one\ntwo\n", "startLine": 1, "endLine": 2,
            }),
        );
        assert_eq!(history["baseCommit"], head, "{path}: {history}");
        assert_eq!(history["unmappedLines"], json!([]), "{history}");
        assert_eq!(history["commits"].as_array().unwrap().len(), 1, "{history}");
        assert_eq!(history["commits"][0]["pathAtCommit"], path, "{history}");
    }
}

#[test]
fn merge_line_history_replays_a_pathspec_shaped_filename_literally() {
    let repository = Repository::new("colon-merge-history");
    let path = ":(exclude)*.txt";
    repository.write(path, "first\nbefore\nlast\n");
    repository.git(&["add", "--all"]);
    repository.commit();
    repository.git(&["checkout", "-qb", "side"]);
    repository.write(path, "first\nmerged\nlast\n");
    repository.git(&["add", "--all"]);
    repository.commit();
    repository.git(&["checkout", "-q", "main"]);
    repository.write("unselected.txt", "keep\n");
    repository.git(&["add", "--all"]);
    repository.commit();
    repository.git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "merge",
        "--no-ff",
        "--no-edit",
        "side",
    ]);
    let merge = repository.git(&["rev-parse", "HEAD"]).trim().to_string();
    // A later committed insertion moves the queried line. The merge entry
    // must still identify the original second line, not lose its range when
    // the filename is mistaken for an excluding Git pathspec during replay.
    let content = "inserted\nfirst\nmerged\nlast\n";
    repository.write(path, content);
    repository.git(&["add", "--all"]);
    repository.commit();
    let history = repository.data(
        "git.lineHistory",
        json!({"path": path, "content": content, "startLine": 3, "endLine": 3}),
    );
    assert_eq!(history["unmappedLines"], json!([]), "{history}");
    let entry = history["commits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["hash"] == merge)
        .unwrap_or_else(|| panic!("Merge entry missing: {history}"));
    assert_eq!(entry["kind"], "merge-first-parent", "{history}");
    assert_eq!(entry["pathAtCommit"], path, "{history}");
    assert_eq!(
        entry["relevantRanges"],
        json!([{"oldStart": 2, "oldEnd": 2, "newStart": 2, "newEnd": 2}]),
        "{history}"
    );
}

#[test]
fn untracked_directory_and_diff_preserve_colon_children() {
    let repository = Repository::new("colon-untracked");
    repository.commit();
    repository.write("new:dir/a:b.txt", "new\n");
    repository.write("new:dir/sub:dir/nested.txt", "nested\n");
    let listing = repository.data("git.untrackedDirectory", json!({"directory": "new:dir/"}));
    let children = listing["changes"].as_array().unwrap();
    assert_eq!(children.len(), 2, "{listing}");
    assert_eq!(children[0]["path"], "new:dir/a:b.txt", "{listing}");
    assert_eq!(children[1]["path"], "new:dir/sub:dir/", "{listing}");
    assert_eq!(children[1]["untrackedFileCount"], 1, "{listing}");
    let diff = repository.data(
        "git.diff",
        json!({
            "pathspecs": ["new:dir/"], "untracked": true, "includeFileSections": true,
        }),
    );
    let paths = diff["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        ["new:dir/a:b.txt", "new:dir/sub:dir/nested.txt"],
        "{diff}"
    );

    repository.write(":(glob)*.txt", "selected\n");
    repository.write(":(exclude)*.txt", "selected\n");
    repository.write("unselected.txt", "keep\n");
    for mode in [
        json!({"reference": "HEAD", "untracked": true}),
        json!({"worktreeSnapshot": true}),
    ] {
        for path in [":(glob)*.txt", ":(exclude)*.txt"] {
            let mut request = mode.clone();
            request["pathspecs"] = json!([path]);
            request["includeFileSections"] = json!(true);
            let diff = repository.data("git.diff", request);
            let files = diff["files"].as_array().unwrap();
            assert_eq!(files.len(), 1, "{diff}");
            assert_eq!(files[0]["path"], path, "{diff}");
        }
    }
}

#[test]
fn selected_commit_keeps_colon_paths_literal() {
    let repository = Repository::new("colon-commit");
    repository.git(&["config", "user.name", "Fixture"]);
    repository.git(&["config", "user.email", "fixture@example.invalid"]);
    repository.git(&["config", "commit.gpgsign", "false"]);
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "initial\n");
    }
    repository.git(&["add", "--all"]);
    repository.commit();

    // Selected commit with pathspec-shaped names must not commit `unselected.txt`
    // and must reconcile pre-staged colon paths in the real index.
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "committed\n");
    }
    repository.mutate("stage", &["a:b.txt", ":(exclude)*.txt"]);
    let committed = repository.data(
        "git.write",
        json!({
            "operation": "commit",
            "message": "commit colon paths",
            "paths": PATHS,
        }),
    );
    assert_eq!(committed["exitCode"], 0, "{committed}");
    assert!(committed["operationError"].is_null(), "{committed}");
    assert_eq!(
        repository.git(&[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "HEAD"
        ]),
        ":(exclude)*.txt\0:(glob)*.txt\0D:\\logs\0a:b.txt\0"
    );
    assert_eq!(
        fs::read_to_string(repository.0.join("unselected.txt")).unwrap(),
        "committed\n"
    );
    assert_eq!(
        repository.git(&["status", "--porcelain=v1", "-z"]),
        " M unselected.txt\0"
    );
}

#[test]
fn stash_push_keeps_colon_paths_literal() {
    let repository = Repository::new("colon-stash");
    repository.git(&["config", "user.name", "Fixture"]);
    repository.git(&["config", "user.email", "fixture@example.invalid"]);
    repository.git(&["config", "commit.gpgsign", "false"]);
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "initial\n");
    }
    repository.git(&["add", "--all"]);
    repository.commit();

    // Selected stash push with pathspec-shaped names must leave `unselected.txt` modified.
    for path in PATHS.into_iter().chain(["unselected.txt"]) {
        repository.write(path, "stashed\n");
    }
    let stashed = repository.data(
        "git.write",
        json!({
            "operation": "stashPush",
            "message": "stash colon paths",
            "paths": PATHS,
        }),
    );
    assert_eq!(stashed["exitCode"], 0, "{stashed}");
    assert!(stashed["operationError"].is_null(), "{stashed}");
    for path in PATHS {
        assert_eq!(
            fs::read_to_string(repository.0.join(path)).unwrap(),
            "initial\n",
            "{path}"
        );
    }
    assert_eq!(
        fs::read_to_string(repository.0.join("unselected.txt")).unwrap(),
        "stashed\n"
    );
}
