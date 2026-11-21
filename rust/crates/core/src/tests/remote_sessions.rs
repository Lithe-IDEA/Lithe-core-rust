//! Real framed-host integration coverage for remote workspace semantic engines.

use lithe_protocol::ErrorCode;
use lithe_remote::client::session::ConnectOptions;
use lithe_remote::client::RemoteSession;
use lithe_remote::protocol::methods;
use lithe_workspace::test_support;
use serde_json::{json, Value};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const STEP: Duration = Duration::from_secs(10);

/// Owns socket deadlines, registered identity, server completion and scratch data.
struct HostFixture {
    id: String,
    root: std::path::PathBuf,
    socket: UnixStream,
    done: mpsc::Receiver<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HostFixture {
    fn new() -> Self {
        let root = super::support::temporary_root("remote-git-integration");
        std::fs::create_dir_all(root.join("host")).unwrap();
        std::fs::create_dir_all(root.join("placeholder")).unwrap();
        let root = root.canonicalize().unwrap();
        let (client, server) = UnixStream::pair().unwrap();
        server.set_read_timeout(Some(STEP)).unwrap();
        server.set_write_timeout(Some(STEP)).unwrap();
        let socket = client.try_clone().unwrap();
        let (finished, done) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = lithe_remote::server::serve_stdio(server.try_clone().unwrap(), server);
            let _ = finished.send(());
            result.unwrap();
        });
        let (session, events) = RemoteSession::from_streams(
            client.try_clone().unwrap(),
            client,
            &ConnectOptions {
                handshake_timeout: STEP,
                ..ConnectOptions::default()
            },
        )
        .unwrap();
        drop(events);
        let id = test_support::register_in_process(session);
        Self {
            id,
            root,
            socket,
            done,
            worker: Some(worker),
        }
    }

    fn call(&self, command: &str, mut payload: Value) -> Value {
        payload["root"] = json!(self.root.join("placeholder"));
        self.request(command, payload)
    }

    fn request(&self, command: &str, payload: Value) -> Value {
        self.request_at(&self.root.join("host"), command, payload)
    }

    fn request_at(&self, remote_root: &std::path::Path, command: &str, payload: Value) -> Value {
        let response: Value = serde_json::from_str(&crate::execute_json(&json!({
            "id": test_support::unique_id(), "command": command, "timeoutMilliseconds": 30_000,
            "remoteWorkspace": {"sessionId": self.id, "localRoot": self.root.join("placeholder"), "remoteRoot": remote_root},
            "payload": payload,
        }).to_string())).unwrap();
        assert_eq!(response["ok"], true, "{command}: {response}");
        assert!(
            response["data"]["operationError"].is_null(),
            "{command}: {response}"
        );
        response["data"].clone()
    }

    fn git(&self, args: &[&str]) -> Value {
        let data = self.call("git.command", json!({"arguments": args}));
        assert_eq!(data["exitCode"], 0, "{args:?}: {data}");
        data
    }
}

#[test]
fn remote_build_detects_host_models_normalizes_diagnostics_and_invalidates_sync() {
    let host = HostFixture::new();
    let root = host.root.join("host");
    let placeholder = host.root.join("placeholder");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"host-only\"\nversion = \"1.0.0\"\n[features]\nfast = []\n",
    )
    .unwrap();
    let manifest: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/build-systems/build-static-file.plugin.json"
    )))
    .unwrap();
    let detection = host.request(
        "build.detect",
        json!({"workspaceRoot": placeholder,
        "plugins": [{"manifest": manifest}], "options": {}}),
    );
    assert_eq!(detection["status"], "ok", "{detection}");
    let instance = &detection["detection"]["instances"][0];
    assert_eq!(instance["id"], "cargo:.", "{detection}");
    assert_eq!(
        instance["model"]["projects"][0]["name"], "host-only",
        "{detection}"
    );
    let call = |command, request| {
        host.request(
            command,
            json!({"manifest": manifest,
        "buildSystemID": "cargo", "workspaceRoot": placeholder, "request": request}),
        )
    };
    let diagnostics = call(
        "build.decodeDiagnostics",
        json!({"workspaceRoot": placeholder, "root": ".",
        "output": format!("{}/src/main.rs:2:3: error: remote diagnostic", root.display())}),
    );
    assert_eq!(
        diagnostics["diagnostics"][0]["path"], "src/main.rs",
        "{diagnostics}"
    );
    let sync = || {
        call(
            "build.syncFingerprint",
            json!({"workspaceRoot": placeholder, "root": ".", "projects": ["."], "options": {}}),
        )
    };
    let before = sync();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"changed\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    let after = sync();
    assert_ne!(before, after, "{before}");
    // Reported symlinks must not expose host paths outside this workspace.
    std::fs::write(host.root.join("outside.rs"), "private").unwrap();
    std::os::unix::fs::symlink(host.root.join("outside.rs"), root.join("src/link.rs")).unwrap();
    let escaped = call(
        "build.decodeDiagnostics",
        json!({"workspaceRoot": placeholder, "root": ".",
        "output": format!("{}/src/link.rs:1:1: error: escaped", root.display())}),
    );
    assert_eq!(escaped["diagnostics"], json!([]), "{escaped}");
    assert_eq!(std::fs::read_dir(placeholder).unwrap().count(), 0);
}

/// A remote root opened through a host symbolic link resolves outside the
/// local placeholder; the empty-root boundary must ask the host, not the
/// unmaterialized local projection, before scanning.
#[test]
fn remote_build_detects_a_root_opened_through_a_host_symbolic_link() {
    let host = HostFixture::new();
    let root = host.root.join("host");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"linked-host\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    let link = host.root.join("opened-link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    let manifest: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/build-systems/build-static-file.plugin.json"
    )))
    .unwrap();
    let detection = host.request_at(
        &link,
        "build.detect",
        json!({"workspaceRoot": host.root.join("placeholder"),
        "plugins": [{"manifest": manifest}], "options": {}}),
    );
    assert_eq!(detection["status"], "ok", "{detection}");
    let instance = &detection["detection"]["instances"][0];
    assert_eq!(instance["id"], "cargo:.", "{detection}");
    assert_eq!(
        instance["model"]["projects"][0]["name"], "linked-host",
        "{detection}"
    );
}

impl Drop for HostFixture {
    fn drop(&mut self) {
        let _ = crate::execute_json(&json!({
            "id": test_support::unique_id(), "command": "workspace.searchIndex.invalidate", "timeoutMilliseconds": 1_000,
            "remoteWorkspace": {"sessionId": self.id, "localRoot": self.root.join("placeholder"), "remoteRoot": self.root.join("host")},
            "payload": {"root": self.root.join("placeholder")}
        }).to_string());
        test_support::unregister(&self.id);
        let _ = self.socket.shutdown(Shutdown::Both);
        if self.done.recv_timeout(STEP + STEP).is_ok() {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        } else {
            if !std::thread::panicking() {
                panic!("remote test host did not finish within its cleanup deadline");
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn remote_directory_listing_uses_host_paths_visibility_and_natural_order() {
    let host = HostFixture::new();
    let root = host.root.join("host");
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::create_dir(root.join("hidden")).unwrap();
    std::fs::create_dir_all(root.join(".git/worktrees/nested")).unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(
        root.join("nested/.git"),
        "gitdir: ../.git/worktrees/nested\n",
    )
    .unwrap();
    std::fs::write(root.join("src/remote.txt"), "remote").unwrap();
    std::fs::write(root.join("v0.1.12.md"), "").unwrap();
    std::fs::write(root.join("v0.1.4.md"), "").unwrap();
    std::fs::write(host.root.join("placeholder/local-only.txt"), "sentinel").unwrap();

    let response = host.call(
        "workspace.listDirectories",
        json!({"paths": ["", "src", "nested"], "hiddenDirectoryNames": ["hidden"]}),
    );
    let directories = response["directories"].as_array().unwrap();
    let names = |index: usize| -> Vec<&str> {
        directories[index]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect()
    };
    assert_eq!(names(0), ["src", "v0.1.4.md", "v0.1.12.md"]);
    assert_eq!(names(1), ["remote.txt"]);
    assert_eq!(directories.len(), 2, "nested worktrees cannot be expanded");
    assert_eq!(directories[0]["nestedWorktrees"], json!(["nested"]));
    assert_eq!(
        std::fs::read_to_string(host.root.join("placeholder/local-only.txt")).unwrap(),
        "sentinel"
    );
    // File type already arrives with readDir. Growing the number of regular
    // files must not add one network round trip per file.
    let count = test_support::stat_requests;
    let before = count();
    host.call("workspace.listDirectories", json!({"paths": [""]}));
    let baseline_stats = count() - before;
    assert!(
        baseline_stats > 0,
        "the test must observe remote metadata requests"
    );
    for index in 0..64 {
        std::fs::write(root.join(format!("additional-{index}.txt")), "").unwrap();
    }
    let before = count();
    host.call("workspace.listDirectories", json!({"paths": [""]}));
    let expanded_stats = count() - before;
    eprintln!(
        "remote directory stat requests: baseline={baseline_stats}, plus 64 files={expanded_stats}"
    );
    assert_eq!(
        expanded_stats, baseline_stats,
        "regular files must not add stat RPCs"
    );
}

#[test]
fn remote_search_reads_host_content_and_applies_incremental_updates_without_local_cache_leaks() {
    let host = HostFixture::new();
    std::fs::create_dir(host.root.join("host/src")).unwrap();
    std::fs::write(
        host.root.join("host/src/RemoteName.txt"),
        "RemoteName remoteMethod\n",
    )
    .unwrap();
    // Warm the same URL locally first: host identity must separate its empty index.
    let local: Value = serde_json::from_str(&crate::execute_json(
        &json!({
            "id": test_support::unique_id(), "command": "workspace.searchEverywhere",
            "payload": {"root": host.root.join("placeholder"), "query": "RemoteName"}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(local["ok"], true, "{local}");
    assert_eq!(local["data"]["matches"], json!([]));
    let found = host.call("workspace.searchEverywhere", json!({"query": "RemoteName"}));
    for kind in ["file", "content"] {
        assert!(
            found["matches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == kind && item["path"] == "src/RemoteName.txt"),
            "{kind}: {found}"
        );
    }
    std::fs::write(host.root.join("host/src/RemoteName.txt"), "ChangedName\n").unwrap();
    host.call(
        "workspace.searchIndex.update",
        json!({"paths": ["src/RemoteName.txt"]}),
    );
    let changed = host.call("workspace.search", json!({"query": "ChangedName"}));
    assert!(
        changed["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == "src/RemoteName.txt"),
        "{changed}"
    );
    let missing = host.call("workspace.search", json!({"query": "remoteMethod"}));
    assert_eq!(missing["matches"], json!([]));
    let read = host.call("file.read", json!({"path": "src/RemoteName.txt"}));
    assert_eq!(read["text"], "ChangedName\n");
    host.call(
        "file.write",
        json!({"path": "written.txt", "text": "remote write\n"}),
    );
    assert_eq!(
        std::fs::read(host.root.join("host/written.txt")).unwrap(),
        b"remote write\n"
    );
    assert_eq!(
        std::fs::read_dir(host.root.join("placeholder"))
            .unwrap()
            .count(),
        0
    );
    let local: Value = serde_json::from_str(&crate::execute_json(
        &json!({
            "id": test_support::unique_id(), "command": "workspace.searchIndex.invalidate",
            "payload": {"root": host.root.join("placeholder")}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(local["ok"], true, "{local}");
}

#[test]
fn real_remote_git_status_stage_commit_and_history_never_touch_the_placeholder() {
    let host = HostFixture::new();
    host.git(&["init", "-q", "-b", "main"]);
    host.git(&["config", "user.name", "Lithe Remote Test"]);
    host.git(&["config", "user.email", "remote@example.invalid"]);
    std::fs::write(host.root.join("host/remote.txt"), b"host-only\n").unwrap();
    let status = host.call("git.status", json!({}));
    assert_eq!(status["repositoryRoot"], ".");
    assert_eq!(status["changes"][0]["path"], "remote.txt");
    let staged = host.call(
        "git.write",
        json!({"operation": "stage", "paths": ["remote.txt"]}),
    );
    assert_eq!(staged["exitCode"], 0, "{staged}");
    host.git(&["commit", "-qm", "first remote commit"]);
    std::fs::write(host.root.join("host/remote.txt"), b"changed remotely\n").unwrap();
    host.git(&["add", "remote.txt"]);
    host.git(&["commit", "-qm", "second remote commit"]);
    let first = host.call("git.historyPage", json!({"limit": 1}));
    assert_eq!(first["commits"][0]["subject"], "second remote commit");
    let cursor = first["nextCursor"].as_str().expect("a second page");
    let second = host.call("git.historyPage", json!({"limit": 1, "cursor": cursor}));
    assert_eq!(second["commits"][0]["subject"], "first remote commit");
    if let Some(cursor) = second["nextCursor"].as_str() {
        host.call("git.historyCursorClose", json!({"cursor": cursor}));
    }
    assert_eq!(
        std::fs::read_dir(host.root.join("placeholder"))
            .unwrap()
            .count(),
        0
    );
    assert!(host.root.join("host/.git/index").is_file());
}

#[test]
fn remote_file_context_uses_host_repository_and_preserves_placeholder_identity() {
    let host = HostFixture::new();
    host.git(&["init", "-q", "-b", "main"]);
    let nested = host.root.join("host/nested");
    std::fs::create_dir(&nested).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&nested)
        .status()
        .unwrap()
        .success());
    let context = host.call(
        "git.resolveFileContext",
        json!({"path": "nested/missing/file.txt"}),
    );
    assert_eq!(context["root"], json!(host.root.join("placeholder/nested")));
    assert_eq!(context["path"], "missing/file.txt");
    assert_eq!(context["ownership"], "git");
    assert_eq!(
        std::fs::read_dir(host.root.join("placeholder"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn remote_parent_repository_discovery_keeps_identity_for_status_and_stage() {
    discovered_repository_keeps_host_identity(false);
}

#[test]
fn remote_symlink_repository_discovery_keeps_identity_for_status_and_stage() {
    discovered_repository_keeps_host_identity(true);
}

fn discovered_repository_keeps_host_identity(symlink: bool) {
    let host = HostFixture::new();
    host.git(&["init", "-q", "-b", "main"]);
    let repository = host.root.join("host");
    std::fs::write(repository.join("remote.txt"), "remote only\n").unwrap();
    let remote_root = if symlink {
        let link = host.root.join("opened-link");
        std::os::unix::fs::symlink(&repository, &link).unwrap();
        link
    } else {
        let child = repository.join("src");
        std::fs::create_dir(&child).unwrap();
        child
    };
    let placeholder = host.root.join("placeholder");
    let host_paths = host.root.join("placeholder.lithe-remote-host");
    let expected = host_paths.join(repository.strip_prefix("/").unwrap());
    let discovered = host.request_at(
        &remote_root,
        "workspace.repositories",
        json!({"root": placeholder}),
    );
    // The identity is relative to the folder the remote host opened, never to the
    // local placeholder, so it survives the path mapping unchanged.
    assert_eq!(
        discovered["repositories"],
        json!([{
            "path": expected,
            "id": if symlink { "." } else { ".." },
            "name": "host",
            "role": "workspace"
        }])
    );
    let root = discovered["repositories"][0]["path"].as_str().unwrap();

    // Simulate the bridge's next request using only the returned repository URL.
    // A real local repository at that URL catches any placeholder fallback.
    std::fs::create_dir_all(&expected).unwrap();
    let local_git = |arguments: &[&str]| {
        let response: Value = serde_json::from_str(&crate::execute_json(
            &json!({
                "id": test_support::unique_id(), "command": "git.command", "timeoutMilliseconds": 10_000,
                "payload": {"root": expected, "arguments": arguments}
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["data"]["exitCode"], 0, "{response}");
    };
    local_git(&["init", "-q", "-b", "main"]);
    std::fs::write(expected.join("local.txt"), "local sentinel\n").unwrap();
    local_git(&["add", "local.txt"]);
    let index = std::fs::read(expected.join(".git/index")).unwrap();
    let routed = |command, payload| {
        assert!(std::path::Path::new(root).starts_with(&host_paths));
        host.request_at(&remote_root, command, payload)
    };
    let status = routed(
        "git.status",
        json!({"root": root, "repositoryRoots": [root]}),
    );
    assert_eq!(status["changes"][0]["path"], "remote.txt", "{status}");
    let prepared = routed(
        "git.prepareCommitSelection",
        json!({"repositories": [
            {"root":root, "selected":[{"path":"remote.txt", "status":"??"}]}
        ]}),
    );
    assert_eq!(prepared["prepared"], true, "{prepared}");
    assert_eq!(
        host.git(&["diff", "--cached", "--name-only"])["stdout"],
        "remote.txt\n"
    );
    assert_eq!(std::fs::read(expected.join(".git/index")).unwrap(), index);
    let context = routed("git.watchContext", json!({"root": root}));
    assert_eq!(context["repositoryRoot"], json!(expected));
    assert_eq!(context["gitCommonDirectory"], json!(expected.join(".git")));
    let staged = routed(
        "git.write",
        json!({"root": root, "operation": "stage", "paths": ["remote.txt"]}),
    );
    assert_eq!(staged["exitCode"], 0, "{staged}");
    assert_eq!(
        host.git(&["diff", "--cached", "--name-only"])["stdout"],
        "remote.txt\n"
    );
    assert_eq!(std::fs::read(expected.join(".git/index")).unwrap(), index);
    assert_eq!(
        std::fs::read_to_string(expected.join("local.txt")).unwrap(),
        "local sentinel\n"
    );
    assert_eq!(std::fs::read_dir(placeholder).unwrap().count(), 0);
}

#[test]
fn remote_git_snapshot_selected_commit_and_unsaved_line_history_preserve_the_real_index() {
    let host = HostFixture::new();
    host.git(&["init", "-q", "-b", "main"]);
    host.git(&["config", "user.name", "Lithe Remote Test"]);
    host.git(&["config", "user.email", "remote@example.invalid"]);
    std::fs::write(host.root.join("host/selected.txt"), b"one\nbase\n").unwrap();
    std::fs::write(host.root.join("host/unrelated.txt"), b"base\n").unwrap();
    host.git(&["add", "--all"]);
    host.git(&["commit", "-qm", "initial remote content"]);
    std::fs::write(host.root.join("host/selected.txt"), b"staged\nbase\n").unwrap();
    std::fs::write(host.root.join("host/unrelated.txt"), b"keep staged\n").unwrap();
    host.git(&["add", "--all"]);
    std::fs::write(host.root.join("host/selected.txt"), b"staged\nworktree\n").unwrap();
    let index = std::fs::read(host.root.join("host/.git/index")).unwrap();

    let snapshot = host.call(
        "git.diff",
        json!({"pathspecs": ["selected.txt"], "worktreeSnapshot": true}),
    );
    let patch = snapshot["patch"].as_str().unwrap();
    assert!(
        patch.contains("+staged") && patch.contains("+worktree"),
        "{snapshot}"
    );
    assert_eq!(
        std::fs::read(host.root.join("host/.git/index")).unwrap(),
        index
    );
    let committed = host.call(
        "git.write",
        json!({"operation": "commit", "paths": ["selected.txt"], "message": "selected remote content"}),
    );
    assert_eq!(committed["exitCode"], 0, "{committed}");
    assert_eq!(
        host.git(&["show", "HEAD:selected.txt"])["stdout"],
        "staged\nworktree\n"
    );
    assert_eq!(
        host.git(&["show", "HEAD:unrelated.txt"])["stdout"],
        "base\n"
    );
    assert_eq!(
        host.git(&["diff", "--cached", "--name-only"])["stdout"],
        "unrelated.txt\n"
    );

    let history = host.call(
        "git.lineHistory",
        json!({"path": "selected.txt", "content": "unsaved\nstaged\nworktree\n", "startLine": 3, "endLine": 3}),
    );
    assert_eq!(history["commits"][0]["subject"], "selected remote content");
    assert_eq!(history["unmappedLines"], json!([]));
    assert_eq!(
        std::fs::read(host.root.join("host/selected.txt")).unwrap(),
        b"staged\nworktree\n"
    );
    assert_eq!(
        std::fs::read_dir(host.root.join("placeholder"))
            .unwrap()
            .count(),
        0,
        "index and source snapshots must stay on the workspace host"
    );
    assert!(std::fs::read_dir(host.root.join("host/.git"))
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("lithe-commit-")));
}

#[test]
fn remote_git_worktree_and_clone_destinations_use_the_host_filesystem() {
    let host = HostFixture::new();
    host.git(&["init", "-q", "-b", "main"]);
    host.git(&["config", "user.name", "Lithe Remote Test"]);
    host.git(&["config", "user.email", "remote@example.invalid"]);
    std::fs::write(host.root.join("host/tracked.txt"), b"host-only\n").unwrap();
    host.git(&["add", "tracked.txt"]);
    host.git(&["commit", "-qm", "remote worktree source"]);

    let destination = host.root.join("placeholder/.worktrees/topic with spaces");
    let created = host.call(
        "git.write",
        json!({"operation": "createWorktree", "destination": destination,
        "worktreeMode": "detached", "gitReference": {
            "kind": "local", "fullName": "refs/heads/main", "shortName": "main"
        }}),
    );
    assert_eq!(created["exitCode"], 0, "{created}");
    assert!(host
        .root
        .join("host/.worktrees/topic with spaces/.git")
        .is_file());
    let entries = host.call("git.worktrees", json!({}));
    let entry = entries["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == json!(destination))
        .expect("worktree paths must retain the client's workspace identity");
    assert_eq!(entry["isCurrent"], false);
    assert_eq!(entry["isPrimary"], false);
    for operation in ["lockWorktree", "unlockWorktree", "removeWorktree"] {
        let result = host.call(
            "git.write",
            json!({"operation": operation, "destination": entry["path"]}),
        );
        assert_eq!(result["exitCode"], 0, "{operation}: {result}");
    }
    assert!(!host.root.join("host/.worktrees/topic with spaces").exists());

    let cloned = host.call(
        "git.write",
        json!({"operation": "clone", "remote": host.root.join("host"),
            "destination": host.root.join("placeholder/cloned project")}),
    );
    assert_eq!(cloned["exitCode"], 0, "{cloned}");
    assert_eq!(
        std::fs::read(host.root.join("host/cloned project/tracked.txt")).unwrap(),
        b"host-only\n"
    );
    assert_eq!(
        std::fs::read_dir(host.root.join("placeholder"))
            .unwrap()
            .count(),
        0,
        "remote Git must never create worktrees or clones in the placeholder"
    );
}

#[test]
fn remote_git_configuration_reports_the_host_executable_and_sources() {
    let host = HostFixture::new();
    host.git(&["init", "-q", "-b", "main"]);
    host.git(&["config", "fetch.prune", "true"]);
    let request = |command| -> Value {
        serde_json::from_str(&crate::execute_json(&json!({
            "id": test_support::unique_id(), "command": command, "timeoutMilliseconds": 10_000,
            "remoteWorkspace": {"sessionId": host.id, "localRoot": host.root.join("placeholder"), "remoteRoot": host.root.join("host")},
            // This client override must not be used on the workspace host.
            "gitExecution": {"executable": "/fixture/client-only/git"},
            "payload": {"root": host.root.join("placeholder")}
        }).to_string())).unwrap()
    };
    let located = request("git.executable");
    assert_eq!(located["ok"], true, "{located}");
    let executable = located["data"]["executable"].as_str().unwrap();
    assert!(executable.starts_with('/'), "{located}");
    assert_ne!(executable, "/fixture/client-only/git");
    let inspected = request("git.executionInspect");
    assert_eq!(inspected["ok"], true, "{inspected}");
    assert_eq!(inspected["data"]["executable"], executable);
    assert_eq!(inspected["data"]["version"], located["data"]["version"]);
    let entry = inspected["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["key"] == "fetch.prune" && entry["scope"] == "local")
        .unwrap();
    assert_eq!(entry["value"], "true");
    assert!(entry["origin"].as_str().unwrap().contains(".git/config"));
    // Disconnect after a cached resolution: errors must still propagate instead
    // of showing an empty setting or probing the client's selected executable.
    test_support::drop_link(&host.id);
    let closed = request("git.executable");
    assert_eq!(closed["ok"], false, "{closed}");
}

#[test]
fn remote_workspace_file_limits_preserve_host_content_and_report_disconnects() {
    use lithe_workspace::{self as workspace, fs};
    use std::io::Read;

    let host = HostFixture::new();
    let _deadline = lithe_protocol::cancellation::Scope::begin(None, Some(10_000));
    let workspace = serde_json::from_value(json!({"sessionId": host.id,
        "localRoot": host.root.join("placeholder"), "remoteRoot": host.root.join("host")}))
    .unwrap();
    let _workspace = workspace::Scope::begin(Some(workspace)).unwrap();
    let local = host.root.join("placeholder");
    let remote = host.root.join("host");
    let content = vec![b'x'; 2 * 1024 * 1024 + 7];
    fs::write(local.join("file"), &content).unwrap();
    assert_eq!(fs::read(local.join("file")).unwrap(), content);

    let oversized = vec![b'y'; methods::MAX_TRANSFER_BYTES as usize + 1];
    assert_eq!(
        fs::write(local.join("file"), &oversized)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(std::fs::read(remote.join("file")).unwrap(), content);

    // A sparse file checks the total materialization budget without a large
    // allocation, while streaming consumers can still inspect its contents.
    let sparse = std::fs::File::create(remote.join("large")).unwrap();
    sparse.set_len(32 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        fs::read(local.join("large")).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    let mut prefix = [1; 32];
    fs::File::open(local.join("large"))
        .unwrap()
        .read_exact(&mut prefix)
        .unwrap();
    assert_eq!(prefix, [0; 32]);

    assert!(!fs::try_exists(local.join("absent")).unwrap());
    assert!(fs::is_dir(&local).unwrap());
    test_support::drop_link(&host.id);
    assert!(fs::try_exists(local.join("file")).is_err());
    assert!(fs::is_dir(&local).is_err());
    assert_eq!(std::fs::read_dir(local).unwrap().count(), 0);
}

// Build detection walks a remote project through the workspace adapter; this pins
// that the shared traversal reads the host's tree, never the placeholder, and
// fails instead of shrinking when the link drops.
#[test]
fn shared_workspace_scan_reads_the_host_tree_and_stops_when_the_link_drops() {
    use crate::execution::workspace_scan::{scan_with_reader, scan_workspace};
    use crate::execution::IgnoredPaths;
    use lithe_workspace::{self as workspace, fs};

    let host = HostFixture::new();
    let _deadline = lithe_protocol::cancellation::Scope::begin(None, Some(10_000));
    let local = host.root.join("placeholder");
    let remote = host.root.join("host");
    std::fs::create_dir_all(remote.join("service/api")).unwrap();
    std::fs::write(remote.join("package.json"), "{}").unwrap();
    std::fs::write(remote.join("service/tool-a.toml"), "").unwrap();
    // The ignore rules are the host's: the placeholder has no `.gitignore`.
    std::fs::create_dir_all(remote.join("generated")).unwrap();
    std::fs::write(remote.join("generated/package.json"), "{}").unwrap();
    std::fs::write(remote.join(".gitignore"), "generated/\n").unwrap();
    let workspace = serde_json::from_value(
        json!({"sessionId": host.id, "localRoot": local, "remoteRoot": remote}),
    )
    .unwrap();
    let _workspace = workspace::Scope::begin(Some(workspace)).unwrap();

    let scan = scan_workspace(&local, 3, &IgnoredPaths::default()).unwrap();
    let files: Vec<_> = scan
        .iter()
        .map(|directory| {
            let relative = directory.path.strip_prefix(&local).unwrap();
            (
                relative.to_owned(),
                directory.files.iter().cloned().collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        files,
        [
            (
                std::path::PathBuf::new(),
                vec![".gitignore".to_owned(), "package.json".to_owned()]
            ),
            ("service".into(), vec!["tool-a.toml".to_owned()]),
            ("service/api".into(), Vec::new()),
        ]
    );
    assert_eq!(std::fs::read_dir(&local).unwrap().count(), 0);

    // A directory the host reports as unreadable is skipped, while the same
    // session keeps serving the rest of the tree.
    let unreadable = scan_with_reader(&local, 3, &|path| {
        if path.ends_with("service") {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                let kind = entry.file_type()?;
                Ok((entry.path(), kind.is_dir(), kind.is_symlink()))
            })
            .collect()
    })
    .unwrap();
    let listed: Vec<_> = unreadable
        .iter()
        .map(|directory| directory.path.strip_prefix(&local).unwrap().to_owned())
        .collect();
    assert_eq!(listed, [std::path::PathBuf::new()]);

    // The link dropping after the root was listed must fail the whole scan,
    // not publish the root alone as if the project had no modules.
    let dropped = scan_with_reader(&local, 3, &|path| {
        let entries = fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                let kind = entry.file_type()?;
                Ok((entry.path(), kind.is_dir(), kind.is_symlink()))
            })
            .collect::<std::io::Result<Vec<_>>>();
        test_support::drop_link(&host.id);
        entries
    });
    assert!(
        dropped.is_err(),
        "a dropped link must not look like a smaller project"
    );
    assert!(scan_workspace(&local, 3, &IgnoredPaths::default()).is_err());
    assert_eq!(std::fs::read_dir(local).unwrap().count(), 0);
}

#[test]
fn captured_host_process_preserves_binary_streams_and_cleans_up_limit_failures() {
    let host = HostFixture::new();
    let capture = |argv: &[&str], timeout: u64, limit: usize| {
        let _scope = lithe_protocol::cancellation::Scope::begin(None, Some(5_000));
        lithe_workspace::host::capture_process(
            serde_json::from_value(json!({
                "sessionId": host.id, "params": {"processId": 0, "argv": argv},
                "timeoutMilliseconds": timeout, "maxOutputBytes": limit
            }))
            .unwrap(),
        )
    };
    let output = capture(
        &[
            "/bin/sh",
            "-c",
            r"printf 'a\000b'; printf 'err' >&2; exit 7",
        ],
        2_000,
        6,
    )
    .unwrap();
    assert_eq!(
        lithe_remote::protocol::methods::decode_bytes(output["stdout"].as_str().unwrap()).unwrap(),
        b"a\0b"
    );
    assert_eq!(
        lithe_remote::protocol::methods::decode_bytes(output["stderr"].as_str().unwrap()).unwrap(),
        b"err"
    );
    assert_eq!(output["exitCode"], 7);
    assert!(matches!(
        capture(&["/bin/sh", "-c", "printf abcd; printf efgh >&2"], 2_000, 7)
            .unwrap_err()
            .code,
        ErrorCode::ProcessFailed
    ));
    // The long-lived child has no natural completion within the test deadline;
    // only the capture owner can terminate it when its budget expires.
    assert!(matches!(
        capture(&["/bin/sleep", "60"], 50, 128).unwrap_err().code,
        ErrorCode::TimedOut
    ));
    let connection = test_support::connection(&host.id);
    let deadline = Instant::now() + STEP;
    loop {
        let state = connection
            .session
            .request_value(methods::SERVER_STATE, json!({}), STEP)
            .unwrap();
        if state["processes"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "capture leaked a host process: {state}"
        );
        std::thread::yield_now();
    }
}

#[test]
fn native_plugin_upload_streams_beyond_one_frame_and_rejects_mismatch_without_publication() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let host = HostFixture::new();
    let source = host.root.join("native-package");
    let target = host.root.join("host/package");
    let content = vec![b'p'; methods::MAX_TRANSFER_BYTES as usize + 7];
    std::fs::write(&source, &content).unwrap();
    let digest = format!("{:x}", Sha256::digest(&content));
    let upload = |path: &std::path::Path, expected: &str| {
        let request = serde_json::from_value(json!({"sessionId": host.id,
            "sourcePath": source, "path": path, "sha256": expected, "mode": 0o500}))
        .unwrap();
        lithe_workspace::upload::upload(request)
    };
    let receipt = upload(&target, &digest).unwrap();
    assert_eq!(receipt["sha256"], digest);
    assert_eq!(receipt["size"], content.len());
    assert_eq!(std::fs::read(&target).unwrap(), content);
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert!(
        upload(&target, &digest).is_err(),
        "upload must not replace installed bytes"
    );
    let bad = host.root.join("host/bad");
    assert!(upload(&bad, &"0".repeat(64)).is_err());
    assert!(!bad.exists());
    assert_eq!(
        std::fs::read_dir(host.root.join("host")).unwrap().count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(host.root.join("placeholder"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn plugin_stdio_uses_its_owned_host_and_reclaims_the_remote_process() {
    use lithe_lsp_transport::process::{LspProcessLauncher, LspProcessSpec, SystemProcessLauncher};
    use std::io::Read;
    let host = HostFixture::new();
    let workspace = serde_json::from_value(json!({"sessionId": host.id,
        "localRoot": host.root.join("placeholder"), "remoteRoot": host.root.join("host")}))
    .unwrap();
    let scope = lithe_workspace::Scope::begin(Some(workspace)).unwrap();
    let streams = SystemProcessLauncher
        .launch(LspProcessSpec {
            executable: "/bin/sh".into(),
            arguments: vec!["-c".into(), "pwd >&2; cat".into()],
            working_directory: host.root.join("placeholder"),
            environment: Default::default(),
        })
        .unwrap();
    // Later calls execute without a current workspace. The handle must retain
    // its original connection instead of choosing a local or newly selected host.
    drop(scope);
    let input = b"Content-Length: 2\r\n\r\n{}";
    streams.handle.write_input(input).unwrap();
    streams.handle.close_input();
    let (done, completed) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut output = streams.output;
        let mut errors = streams.errors;
        let result = output
            .read_to_end(&mut stdout)
            .and_then(|_| errors.read_to_end(&mut stderr));
        let _ = done.send((result, stdout, stderr));
    });
    let result = completed.recv_timeout(STEP);
    // Termination wakes readers on assertion failures and has its own deadline.
    streams.handle.terminate();
    let outcome = match result {
        Ok(result) => result,
        Err(error) => {
            if completed.recv_timeout(STEP).is_ok() {
                reader.join().unwrap();
            }
            panic!("remote stdio did not complete: {error}")
        }
    };
    reader.join().unwrap();
    let (io_result, stdout, stderr) = outcome;
    io_result.expect("remote stdio must reach EOF without a transport error");
    assert_eq!(stdout, input);
    assert_eq!(
        String::from_utf8(stderr).unwrap().trim(),
        host.root.join("host").to_str().unwrap()
    );
    assert_eq!(
        streams.handle.process_id(),
        None,
        "remote PIDs must not enter local process registries"
    );
    assert_eq!(streams.handle.exit_status(), Some(Some(0)));
}

#[test]
fn plugin_protocol_maps_owned_uris_without_rewriting_source_text() {
    let host = HostFixture::new();
    let workspace = serde_json::from_value(json!({"sessionId": host.id,
        "localRoot": host.root.join("placeholder"), "remoteRoot": host.root.join("host")}))
    .unwrap();
    let _scope = lithe_workspace::Scope::begin(Some(workspace)).unwrap();
    let owner = lithe_workspace::current().unwrap();
    let local = url::Url::from_file_path(host.root.join("placeholder/a space.txt"))
        .unwrap()
        .to_string();
    let remote = url::Url::from_file_path(host.root.join("host/a space.txt"))
        .unwrap()
        .to_string();
    let external_path = host.root.join("outside/a space.txt");
    let external_uri = url::Url::from_file_path(&external_path)
        .unwrap()
        .to_string();
    let external_local = owner.map_uri(&external_uri, false).unwrap();
    assert_eq!(
        url::Url::parse(&external_local)
            .unwrap()
            .to_file_path()
            .unwrap(),
        host.root
            .join("placeholder.lithe-remote-host")
            .join(external_path.strip_prefix("/").unwrap())
    );
    assert_eq!(owner.map_uri(&external_local, true), Some(external_uri));
    let value = json!({"method": "textDocument/didOpen", "params": {"textDocument": {"uri": local,
        "text": format!("// {local}"), "languageId": "plaintext", "version": 1}}});
    let mapped: Value = serde_json::from_str(
        &lithe_lsp_transport::remote_process::map_message(&owner, &value.to_string(), true, None)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(mapped["params"]["textDocument"]["uri"], remote);
    assert_eq!(
        mapped["params"]["textDocument"]["text"],
        value["params"]["textDocument"]["text"]
    );
    let edit = json!({"method": "workspace/applyEdit", "params": {"edit": {"changes": {remote.clone(): [{"newText": local}]}}}});
    let mapped: Value = serde_json::from_str(
        &lithe_lsp_transport::remote_process::map_message(&owner, &edit.to_string(), false, None)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        mapped["params"]["edit"]["changes"][&local][0]["newText"],
        local
    );
    let map = |value: &Value, outbound, method| -> Value {
        serde_json::from_str(
            &lithe_lsp_transport::remote_process::map_message(
                &owner,
                &value.to_string(),
                outbound,
                method,
            )
            .unwrap(),
        )
        .unwrap()
    };
    // JSON-RPC replies have no method. The originating request keeps plugin
    // values opaque while standard LSP location results still get mapped.
    let reply = json!({"id": 17, "result": {"uri": remote, "data": {"uri": remote}}});
    assert_eq!(map(&reply, false, Some("lithe/function/invoke")), reply);
    let mapped = map(&reply, false, Some("textDocument/definition"));
    assert_eq!(mapped["result"]["uri"], local);
    assert_eq!(mapped["result"]["data"], reply["result"]["data"]);
    for method in [
        "lithe/function/invoke",
        "lithe/host/credentials/get",
        "custom/lookup",
    ] {
        let request = json!({"method": method, "params": {"uri": local, "data": {"uri": local}}});
        assert_eq!(map(&request, true, None), request);
    }
    let request = json!({"method": "initialize", "params": {"rootUri": local,
        "initializationOptions": {"uri": local}, "capabilities": {"experimental": {"uri": local}}}});
    let mapped = map(&request, true, None);
    assert_eq!(mapped["params"]["rootUri"], remote);
    assert_eq!(
        mapped["params"]["initializationOptions"],
        request["params"]["initializationOptions"]
    );
    assert_eq!(
        mapped["params"]["capabilities"],
        request["params"]["capabilities"]
    );
    let request = json!({"method": "lithe/vcs/status", "params": {
        "root": {"path": host.root.join("placeholder")},
        "tool": {"executable": host.root.join("placeholder/tools/svn")},
        "paths": ["dir/file"], "uri": local}});
    let mapped = map(&request, true, None);
    assert_eq!(
        mapped["params"]["root"]["path"],
        json!(host.root.join("host"))
    );
    assert_eq!(
        mapped["params"]["tool"]["executable"],
        json!(host.root.join("host/tools/svn"))
    );
    assert_eq!(mapped["params"]["paths"], request["params"]["paths"]);
    assert_eq!(mapped["params"]["uri"], local);
}

#[test]
fn remote_workspace_trust_keys_the_host_real_path_on_the_connected_target() {
    use std::os::unix::fs::MetadataExt;
    let host = HostFixture::new();
    let project = host.root.join("host/projects/app");
    let sibling = host.root.join("host/projects/other");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let link = host.root.join("host/link");
    std::os::unix::fs::symlink(&project, &link).unwrap();
    let call = |command: &str, payload: Value| -> Value {
        serde_json::from_str(&crate::execute_json(
            &json!({"id": test_support::unique_id(), "command": command,
                "timeoutMilliseconds": 30_000, "payload": payload})
            .to_string(),
        ))
        .unwrap()
    };
    let ok = |response: Value| -> Value {
        assert_eq!(response["ok"], true, "{response}");
        response["data"].clone()
    };
    let code = |response: Value| -> Value {
        assert_eq!(response["ok"], false, "{response}");
        response["error"]["code"].clone()
    };
    let state = |document: &Value, workspace: &std::path::Path| {
        call(
            "remote.workspaceTrust",
            json!({"document": document, "sessionId": host.id, "workspace": workspace}),
        )
    };
    let decide = |document: &Value, decision: Value| {
        call(
            "remote.editWorkspaceTrust",
            json!({"document": document, "decision": decision}),
        )
    };
    let real = json!(project);

    // A link opens the decision of the folder it points to, on this target.
    let unknown = ok(state(&Value::Null, &link));
    assert_eq!(
        unknown["storage"],
        json!({"relativePath": "toolchains/remote-trust.json", "maximumBytes": 262_144})
    );
    assert_eq!(unknown["projectCodeAllowed"], false, "{unknown}");
    let trust = &unknown["workspaceTrust"];
    assert_eq!(trust["state"], "unknown", "{unknown}");
    assert_eq!(trust["workspace"], real, "{unknown}");
    assert_eq!(
        trust["offeredLocation"],
        json!(host.root.join("host/projects"))
    );
    let target = trust["target"].clone();
    let uid = std::fs::metadata(&project).unwrap().uid();
    assert_eq!(target, json!({"hops": [], "userId": uid}), "{unknown}");
    assert_eq!(
        ok(call("remote.workspaceTrust", json!({"document": null}))),
        json!({"document": {"version": 1, "targets": []}, "storage": unknown["storage"]})
    );

    let decision =
        json!({"sessionId": host.id, "workspace": link, "trusted": true, "label": "Fixture"});
    let decided = ok(decide(&Value::Null, decision.clone()));
    assert_eq!(decided["changed"], true, "{decided}");
    assert_eq!(
        decided["decisions"]["targets"][0]["trustedWorkspaces"],
        json!([real])
    );
    let document = decided["document"].clone();
    assert!(document.is_string(), "{decided}");
    let repeated = ok(decide(&document, decision));
    assert_eq!(repeated["changed"], false, "{repeated}");
    assert!(repeated["document"].is_null(), "{repeated}");

    let trusted = ok(state(&document, &project));
    assert_eq!(trusted["projectCodeAllowed"], true, "{trusted}");
    assert_eq!(trusted["workspaceTrust"]["state"], "trusted");
    assert_eq!(
        ok(state(&document, &sibling))["workspaceTrust"]["state"],
        "unknown"
    );

    // Each failure keeps its own category.
    assert_eq!(
        code(state(&document, &host.root.join("host/missing"))),
        "workspace_not_found"
    );
    assert_eq!(
        code(state(&document, std::path::Path::new("projects/app"))),
        "invalid_request"
    );
    assert_eq!(
        code(call(
            "remote.workspaceTrust",
            json!({"document": document, "sessionId": host.id})
        )),
        "invalid_request"
    );
    assert_eq!(
        code(state(&json!("{\"version\":1,\"trustAll\":true}"), &project)),
        "parse_failed"
    );
    assert_eq!(
        code(decide(
            &document,
            json!({"sessionId": host.id, "workspace": project, "trusted": false, "location": true})
        )),
        "invalid_request"
    );
    assert_eq!(
        code(decide(
            &document,
            json!({"sessionId": test_support::unique_id(), "workspace": project, "trusted": true})
        )),
        "connection_closed"
    );
    assert_eq!(
        code(call(
            "remote.editWorkspaceTrust",
            json!({"document": document})
        )),
        "invalid_request"
    );

    // Removing an entry needs no connection to its target.
    let revoked = ok(call(
        "remote.editWorkspaceTrust",
        json!({"document": document, "revoke": {"target": target,
            "list": "trustedWorkspaces", "path": project}}),
    ));
    assert_eq!(revoked["changed"], true, "{revoked}");
    assert_eq!(revoked["decisions"]["targets"], json!([]), "{revoked}");
    assert_eq!(
        ok(state(&revoked["document"], &link))["workspaceTrust"]["state"],
        "unknown"
    );
}

// The connections window's helpers answer through the dispatcher: a quick
// address splits into fields, a missing Motty database is a result, and a
// malformed connection names its field before any `ssh -G` runs.
#[test]
fn connection_window_helpers_answer_through_the_dispatcher() {
    let call = |command: &str, payload: Value| -> Value {
        serde_json::from_str(&crate::execute_json(
            &json!({"id": test_support::unique_id(), "command": command,
                "timeoutMilliseconds": 30_000, "payload": payload})
            .to_string(),
        ))
        .unwrap()
    };

    let address = call("remote.parseAddress", json!({"text": "dev@build-box:2222"}));
    assert_eq!(
        address["data"],
        json!({"host": "build-box", "user": "dev", "port": 2222}),
        "{address}"
    );

    let directory =
        std::env::temp_dir().join(format!("lithe-core-motty-absent-{}", std::process::id()));
    let motty = call(
        "remote.mottyHosts",
        json!({"configDirectory": directory, "document": null}),
    );
    assert_eq!(motty["data"]["unavailable"], "missing", "{motty}");

    let forward = call(
        "remote.parseForwardAddress",
        json!({"text": "HTTP://Localhost:3000/app?tab=1"}),
    );
    assert_eq!(
        forward["data"],
        json!({"targetHost": "localhost", "targetPort": 3000, "scheme": "http", "path": "/app?tab=1"}),
        "{forward}"
    );
    let port = call("remote.parseForwardAddress", json!({"text": "8080"}));
    assert_eq!(
        port["data"],
        json!({"targetHost": "localhost", "targetPort": 8080}),
        "{port}"
    );
    let secret = call(
        "remote.parseForwardAddress",
        json!({"text": "https://user:secret@example.com"}),
    );
    assert_eq!(secret["error"]["code"], "invalid_request", "{secret}");
    let details: Value =
        serde_json::from_str(secret["error"]["details"].as_str().unwrap()).unwrap();
    assert_eq!(details, json!({"field": "text", "reason": "credentials"}));
    assert!(
        !secret.to_string().contains("secret@"),
        "a refused address is not echoed"
    );

    // Outside Windows there is no WSL, and nothing runs.
    if !cfg!(windows) {
        let distributions = call("remote.listWslDistributions", Value::Null);
        assert_eq!(
            distributions["data"],
            json!({"available": false, "distributions": [], "diagnostics": ""}),
            "{distributions}"
        );
    }
    // A connection names SSH or WSL, not both, before anything starts.
    let both = call(
        "remote.connect",
        json!({"connection": {"host": "devbox"}, "wsl": {"distribution": "Ubuntu"},
            "servers": [], "serverVersion": "0.1.0"}),
    );
    assert_eq!(both["error"]["code"], "invalid_request", "{both}");
    let details: Value = serde_json::from_str(both["error"]["details"].as_str().unwrap()).unwrap();
    assert_eq!(details, json!({"field": "wsl", "reason": "invalid"}));
    // A container listing names a known engine and nothing that could pick
    // the program.
    for payload in [
        json!({"engine": "nerdctl"}),
        json!({"engine": "docker", "program": "/bin/sh"}),
    ] {
        let refused = call("remote.listContainers", payload);
        assert_eq!(refused["error"]["code"], "invalid_request", "{refused}");
        assert_eq!(
            refused["error"]["message"], "Invalid container listing request",
            "{refused}"
        );
    }

    // The settings page reads automatic values and stores an edit; a damaged
    // document stops the SSH commands before OpenSSH starts.
    let preferences = call("remote.preferences", json!({"document": null}));
    assert_eq!(
        preferences["data"]["reconnect"]["delaysMilliseconds"],
        json!([1000, 2000, 4000, 8000, 16000, 29000]),
        "{preferences}"
    );
    let edited = call(
        "remote.editPreferences",
        json!({"document": null, "reconnectLimit": {"seconds": 3}}),
    );
    assert_eq!(edited["data"]["changed"], true, "{edited}");
    let stored = call(
        "remote.preferences",
        json!({"document": edited["data"]["document"]}),
    );
    assert_eq!(
        stored["data"]["reconnect"],
        json!({"source": "setting", "limitSeconds": 3, "delaysMilliseconds": [1000, 2000]}),
        "{stored}"
    );
    let damaged = call(
        "remote.effectiveSettings",
        json!({"connection": {"host": "devbox"}, "preferences": "{\"version\":9}"}),
    );
    assert_eq!(damaged["error"]["code"], "parse_failed", "{damaged}");

    for (command, payload) in [
        ("remote.editPreferences", json!({"document": null})),
        (
            "remote.connect",
            json!({"wsl": {"distribution": "Ubuntu", "program": "C:\\x.exe"},
                "servers": [], "serverVersion": "0.1.0"}),
        ),
        (
            "remote.parseForwardAddress",
            json!({"text": "3000", "extra": 1}),
        ),
        (
            "remote.effectiveSettings",
            json!({"connection": {"host": "dev@box"}}),
        ),
        (
            "remote.sshConfigHosts",
            json!({"homeDirectory": "relative"}),
        ),
        ("remote.parseAddress", json!({"text": "box", "extra": 1})),
    ] {
        let response = call(command, payload);
        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(response["error"]["code"], "invalid_request", "{response}");
    }
}

// The ports page: a forward reaches a program listening on the host's
// wildcard address, a second click returns the same forward, a target on
// another machine waits for the user's confirmation, and every failure keeps
// its own category.
#[test]
fn remote_port_forwards_start_once_per_target_and_end_with_the_connection() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    let host = HostFixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let echo = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(STEP)).unwrap();
        let mut buffer = [0_u8; 5];
        stream.read_exact(&mut buffer).unwrap();
        stream.write_all(&buffer).unwrap();
    });
    let call = |command: &str, payload: Value| -> Value {
        serde_json::from_str(&crate::execute_json(
            &json!({"id": test_support::unique_id(), "command": command,
                "timeoutMilliseconds": 30_000, "payload": payload})
            .to_string(),
        ))
        .unwrap()
    };
    let ok = |response: Value| -> Value {
        assert_eq!(response["ok"], true, "{response}");
        response["data"].clone()
    };
    let error = |response: Value| -> Value {
        assert_eq!(response["ok"], false, "{response}");
        response["error"].clone()
    };
    let start = |payload: Value| {
        let mut payload = payload;
        payload["sessionId"] = json!(host.id);
        call("remote.startForward", payload)
    };

    let started = ok(start(json!({"targetHost": "0.0.0.0", "targetPort": port})));
    assert_eq!(started["created"], true, "{started}");
    let forward = started["forward"].clone();
    assert_eq!(forward["targetHost"], "127.0.0.1", "{started}");
    assert_eq!(forward["loopbackTarget"], true, "{started}");
    assert_eq!(forward["localHost"], "127.0.0.1", "{started}");
    assert_eq!(forward["localPortChanged"], false, "{started}");
    assert!(forward.get("requestedLocalPort").is_none(), "{started}");
    // The wildcard and the loopback address name one target.
    let again = ok(start(
        json!({"targetHost": "127.0.0.1", "targetPort": port}),
    ));
    assert_eq!(again["created"], false, "{again}");
    assert_eq!(again["forward"]["id"], forward["id"], "{again}");

    let local_port = u16::try_from(forward["localPort"].as_u64().unwrap()).unwrap();
    let mut local = TcpStream::connect(("127.0.0.1", local_port)).unwrap();
    local.set_read_timeout(Some(STEP)).unwrap();
    local.write_all(b"ports").unwrap();
    let mut answer = [0_u8; 5];
    local.read_exact(&mut answer).unwrap();
    assert_eq!(&answer, b"ports");
    drop(local);
    echo.join().unwrap();

    let listed = ok(call("remote.forwards", json!({"sessionId": host.id})));
    assert_eq!(listed["forwards"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["forwards"][0]["id"], forward["id"], "{listed}");

    // A database on the host's network starts only once confirmed.
    let elsewhere = json!({"targetHost": "192.0.2.10", "targetPort": 5432});
    assert_eq!(
        error(start(elsewhere.clone()))["code"],
        "forward_target_not_confirmed"
    );
    let mut confirmed = elsewhere;
    confirmed["allowNonLoopbackTarget"] = json!(true);
    let outside = ok(start(confirmed));
    assert_eq!(outside["forward"]["loopbackTarget"], false, "{outside}");

    for (payload, field) in [
        (
            json!({"targetHost": "127.0.0.1", "targetPort": 0}),
            "targetPort",
        ),
        (
            json!({"targetHost": "-oProxyCommand", "targetPort": 22}),
            "targetHost",
        ),
    ] {
        let refused = error(start(payload));
        assert_eq!(refused["code"], "invalid_request", "{refused}");
        let details: Value = serde_json::from_str(refused["details"].as_str().unwrap()).unwrap();
        assert_eq!(details["field"], field, "{refused}");
    }
    assert_eq!(
        error(start(
            json!({"targetHost": "127.0.0.1", "targetPort": 70_000})
        ))["code"],
        "invalid_request"
    );

    // The connection keeps a bounded number of forwards; a target that
    // already has one still answers at the limit.
    let running = ok(call("remote.forwards", json!({"sessionId": host.id})))["forwards"]
        .as_array()
        .unwrap()
        .len();
    let limit = lithe_remote::client::forward_set::MAX_FORWARDS;
    for target in (1..).filter(|target| *target != port).take(limit - running) {
        ok(start(
            json!({"targetHost": "127.0.0.1", "targetPort": target}),
        ));
    }
    let full = error(start(json!({"targetHost": "::1", "targetPort": 1})));
    assert_eq!(full["code"], "forward_limit_reached", "{full}");
    let details: Value = serde_json::from_str(full["details"].as_str().unwrap()).unwrap();
    assert_eq!(details["maximum"], limit, "{full}");
    assert_eq!(
        ok(start(
            json!({"targetHost": "127.0.0.1", "targetPort": port})
        ))["created"],
        false
    );

    let stop = |forward_id: &Value| {
        call(
            "remote.stopForward",
            json!({"sessionId": host.id, "forwardId": forward_id}),
        )
    };
    assert_eq!(ok(stop(&forward["id"]))["stopped"], true);
    assert_eq!(ok(stop(&forward["id"]))["stopped"], false);
    assert_eq!(
        error(call(
            "remote.forwards",
            json!({"sessionId": test_support::unique_id()})
        ))["code"],
        "connection_closed"
    );

    // A connection whose session ended released its forwards and is
    // forgotten, so the ports page reconnects instead of listing nothing.
    test_support::drop_link(&host.id);
    for _ in 0..2 {
        assert_eq!(
            error(call("remote.forwards", json!({"sessionId": host.id})))["code"],
            "connection_closed"
        );
    }
    assert_eq!(
        error(start(
            json!({"targetHost": "127.0.0.1", "targetPort": port})
        ))["code"],
        "connection_closed"
    );
}

// Forwards that listen on the host: a host port relays to another server, a
// taken host port keeps its own category, and a reverse forward waits for the
// settings switch before it brings host connections back to a local port.
#[test]
fn remote_host_and_reverse_forwards_follow_their_own_rules() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    let echo = || {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(STEP)).unwrap();
            let mut buffer = [0_u8; 4];
            stream.read_exact(&mut buffer).unwrap();
            stream.write_all(&buffer).unwrap();
        });
        (port, worker)
    };
    let exchange = |port: u64| {
        let mut stream = TcpStream::connect(("127.0.0.1", u16::try_from(port).unwrap())).unwrap();
        stream.set_read_timeout(Some(STEP)).unwrap();
        stream.write_all(b"ping").unwrap();
        let mut answer = [0_u8; 4];
        stream.read_exact(&mut answer).unwrap();
        answer
    };
    let host = HostFixture::new();
    let call = |command: &str, payload: Value| -> Value {
        let mut payload = payload;
        payload["sessionId"] = json!(host.id);
        serde_json::from_str(&crate::execute_json(
            &json!({"id": test_support::unique_id(), "command": command,
                "timeoutMilliseconds": 30_000, "payload": payload})
            .to_string(),
        ))
        .unwrap()
    };
    let plain = |command: &str, payload: Value| -> Value {
        serde_json::from_str(&crate::execute_json(
            &json!({"id": test_support::unique_id(), "command": command,
                "payload": payload})
            .to_string(),
        ))
        .unwrap()
    };
    let ok = |response: Value| -> Value {
        assert_eq!(response["ok"], true, "{response}");
        response["data"].clone()
    };
    let code = |response: Value| -> Value {
        assert_eq!(response["ok"], false, "{response}");
        response["error"]["code"].clone()
    };

    let (target_port, target) = echo();
    let started = ok(call(
        "remote.startHostForward",
        json!({"targetHost": "localhost", "targetPort": target_port}),
    ));
    let forward = started["forward"].clone();
    assert_eq!(forward["kind"], "host", "{started}");
    assert_eq!(forward["hostListenHost"], "127.0.0.1", "{started}");
    assert_eq!(forward["allInterfaces"], false, "{started}");
    assert!(forward.get("localPort").is_none(), "{started}");
    assert_eq!(
        &exchange(forward["hostListenPort"].as_u64().unwrap()),
        b"ping"
    );
    target.join().unwrap();

    assert_eq!(
        code(call(
            "remote.startHostForward",
            json!({"targetHost": "192.0.2.10", "targetPort": 5432}),
        )),
        "forward_target_not_confirmed"
    );
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    assert_eq!(
        code(call(
            "remote.startHostForward",
            json!({"listenPort": taken.local_addr().unwrap().port(),
                "targetHost": "127.0.0.1", "targetPort": target_port}),
        )),
        "forward_host_port_in_use"
    );

    // Reverse forwards stay off until the user turns them on.
    let (local_port, local) = echo();
    let reverse = json!({"localPort": local_port, "preferences": null});
    assert_eq!(
        code(call("remote.startReverseForward", reverse.clone())),
        "reverse_forward_disabled"
    );
    let read = ok(plain("remote.preferences", json!({"document": null})));
    assert_eq!(
        read["reverseForwarding"],
        json!({"source": "automatic", "allowed": false}),
        "{read}"
    );
    let edited = ok(plain(
        "remote.editPreferences",
        json!({"document": null, "reverseForwarding": {"allowed": true}}),
    ));
    let mut allowed = reverse;
    allowed["preferences"] = edited["document"].clone();
    let started = ok(call("remote.startReverseForward", allowed.clone()));
    let back = started["forward"].clone();
    assert_eq!(back["kind"], "reverse", "{started}");
    assert_eq!(back["hostListenHost"], "127.0.0.1", "{started}");
    assert_eq!(back["localPort"], local_port, "{started}");
    assert_eq!(&exchange(back["hostListenPort"].as_u64().unwrap()), b"ping");
    local.join().unwrap();
    let again = ok(call("remote.startReverseForward", allowed.clone()));
    assert_eq!(again["created"], false, "{again}");
    assert_eq!(again["forward"]["id"], back["id"], "{again}");

    allowed["localPort"] = json!(0);
    let refused = call("remote.startReverseForward", allowed);
    assert_eq!(code(refused.clone()), "invalid_request");
    let details: Value =
        serde_json::from_str(refused["error"]["details"].as_str().unwrap()).unwrap();
    assert_eq!(details["field"], "localPort", "{refused}");

    let listed = ok(call("remote.forwards", json!({})));
    let kinds: Vec<&str> = listed["forwards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|forward| forward["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["host", "reverse"], "{listed}");
    assert_eq!(
        ok(call("remote.stopForward", json!({"forwardId": back["id"]})))["stopped"],
        true
    );
}

// The connection window lists a connected host's containers through the
// host's own engine CLI; a host without one says so instead of failing.
#[test]
fn container_listings_run_on_the_host_and_say_when_it_has_no_engine() {
    use std::os::unix::fs::PermissionsExt;
    let host = HostFixture::new();
    let engine = host.root.join("host/docker");
    std::fs::write(
        &engine,
        "#!/bin/sh\nprintf 'c0ffee\\tdev\\tubuntu:24.04\\tUp 1 minute\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    let request = |engine: &str| {
        serde_json::from_value(json!({"engine": engine, "sessionId": host.id})).unwrap()
    };
    let listed =
        test_support::list_containers_with(request("docker"), engine.to_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(listed).unwrap(),
        json!({"available": true, "diagnostics": "", "containers": [
            {"id": "c0ffee", "name": "dev", "image": "ubuntu:24.04", "status": "Up 1 minute"}
        ]})
    );
    let absent = host.root.join("host/absent");
    let missing =
        test_support::list_containers_with(request("podman"), absent.to_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(missing).unwrap(),
        json!({"available": false, "containers": [], "diagnostics": ""})
    );
    let unknown = test_support::list_containers_with(
        serde_json::from_value(json!({"engine": "docker", "sessionId": "absent"})).unwrap(),
        engine.to_str().unwrap(),
    )
    .unwrap_err();
    assert!(
        matches!(unknown.code, ErrorCode::ConnectionClosed),
        "{unknown:?}"
    );
}
