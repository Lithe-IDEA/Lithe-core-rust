//! `workspace.createEntry`, `renameEntry`, `copyEntry` and `resolveEntry`
//! on the host's file system.

use super::support::{temporary_root, Cleanup};
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn execute(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({"id": "file-changes", "command": command, "payload": payload}).to_string(),
    ))
    .expect("core response should be JSON")
}

/// A new workspace root, removed when the returned guard drops.
fn workspace(label: &str) -> (PathBuf, Cleanup) {
    let root = temporary_root(label);
    fs::create_dir_all(&root).expect("workspace root");
    (root.clone(), Cleanup(root))
}

fn root_text(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

fn error_code(response: &Value) -> &str {
    assert_eq!(response["ok"], false, "{response}");
    response["error"]["code"].as_str().expect("error code")
}

/// Whether this host lets the test create symbolic links: Windows needs
/// Developer Mode or elevation.
fn links_allowed(root: &Path) -> bool {
    let probe = root.join("link-probe");
    let allowed = lithe_platform::entries::create_symlink(Path::new("missing"), &probe).is_ok();
    let _ = fs::remove_file(&probe);
    allowed
}

#[test]
fn a_file_is_created_with_its_text_and_never_replaced() {
    let (root, _cleanup) = workspace("create-file");
    fs::create_dir(root.join("src")).expect("src");
    let response = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "src/main.rs", "kind": "file", "text": "fn main() {}\n"}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["path"], "src/main.rs");
    assert_eq!(response["data"]["kind"], "file");
    assert_eq!(
        Path::new(response["data"]["fullPath"].as_str().unwrap()),
        root.join("src").join("main.rs")
    );
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "fn main() {}\n"
    );

    let again = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "src/main.rs", "kind": "file", "text": "replaced"}),
    );
    assert_eq!(error_code(&again), "entry_exists");
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "fn main() {}\n"
    );

    let empty = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "empty.txt", "kind": "file"}),
    );
    assert_eq!(empty["ok"], true, "{empty}");
    assert_eq!(fs::read(root.join("empty.txt")).unwrap(), b"");
}

#[test]
fn a_directory_needs_its_parent_and_takes_no_text() {
    let (root, _cleanup) = workspace("create-directory");
    let created = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "docs", "kind": "directory"}),
    );
    assert_eq!(created["data"]["kind"], "directory", "{created}");
    assert!(root.join("docs").is_dir());

    let orphan = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "missing/inner", "kind": "directory"}),
    );
    assert_eq!(error_code(&orphan), "entry_not_found");
    assert!(!root.join("missing").exists());

    fs::write(root.join("plain.txt"), "").unwrap();
    let under_file = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "plain.txt/inner", "kind": "directory"}),
    );
    assert_eq!(error_code(&under_file), "entry_not_found");

    for fields in [
        json!({"kind": "directory", "text": ""}),
        json!({"kind": "file", "linkTarget": "x"}),
        json!({"kind": "symlink"}),
        json!({"kind": "symlink", "linkTarget": ""}),
        json!({"kind": "symlink", "linkTarget": "x", "text": ""}),
        json!({"kind": "socket"}),
        json!({"kind": "file", "mode": 420}),
    ] {
        let mut payload = json!({"root": root_text(&root), "path": "bad"});
        for (key, value) in fields.as_object().unwrap() {
            payload[key] = value.clone();
        }
        let response = execute("workspace.createEntry", payload);
        assert_eq!(error_code(&response), "invalid_request", "{fields}");
    }
    assert!(!root.join("bad").exists());
}

#[test]
fn paths_name_one_entry_below_the_root() {
    let (root, _cleanup) = workspace("entry-paths");
    for path in ["", "/abs", "a//b", "./a", "a/", "../outside", "a/../b"] {
        let response = execute(
            "workspace.createEntry",
            json!({"root": root_text(&root), "path": path, "kind": "directory"}),
        );
        assert_eq!(error_code(&response), "invalid_request", "{path:?}");
    }
    let relative_root = execute(
        "workspace.createEntry",
        json!({"root": "relative/root", "path": "a", "kind": "directory"}),
    );
    assert_eq!(error_code(&relative_root), "invalid_request");
    let missing_root = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root.join("missing")), "path": "a", "kind": "directory"}),
    );
    assert_eq!(error_code(&missing_root), "workspace_not_found");
}

#[test]
fn a_directory_link_that_leaves_the_root_cannot_hold_new_entries() {
    let (root, _cleanup) = workspace("escape-parent");
    let (outside, _outside_cleanup) = workspace("escape-outside");
    if !links_allowed(&root) {
        return;
    }
    lithe_platform::entries::create_symlink(&outside, &root.join("out")).unwrap();
    let response = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "out/planted.txt", "kind": "file", "text": "x"}),
    );
    assert_eq!(error_code(&response), "permission_denied");
    assert!(!outside.join("planted.txt").exists());
}

#[test]
fn a_symlink_stores_its_target_verbatim() {
    let (root, _cleanup) = workspace("create-symlink");
    if !links_allowed(&root) {
        return;
    }
    fs::create_dir(root.join("shared")).unwrap();
    fs::write(root.join("shared/notes.md"), "notes").unwrap();
    let response = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "notes", "kind": "symlink", "linkTarget": "shared"}),
    );
    assert_eq!(response["data"]["kind"], "symlink", "{response}");
    assert_eq!(
        fs::read_to_string(root.join("notes/notes.md")).unwrap(),
        "notes"
    );

    // A link may point outside the root or at nothing.
    let dangling = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "dangling", "kind": "symlink", "linkTarget": "../nowhere/at/all"}),
    );
    assert_eq!(dangling["ok"], true, "{dangling}");
    let stored = fs::read_link(root.join("dangling")).unwrap();
    assert_eq!(
        stored.to_string_lossy().replace('\\', "/"),
        "../nowhere/at/all"
    );

    let taken = execute(
        "workspace.createEntry",
        json!({"root": root_text(&root), "path": "dangling", "kind": "symlink", "linkTarget": "shared"}),
    );
    assert_eq!(error_code(&taken), "entry_exists");
}

#[test]
fn a_rename_never_replaces_and_reports_the_target() {
    let (root, _cleanup) = workspace("rename");
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("b.txt"), "b").unwrap();
    let blocked = execute(
        "workspace.renameEntry",
        json!({"root": root_text(&root), "path": "a.txt", "target": "b.txt"}),
    );
    assert_eq!(error_code(&blocked), "entry_exists");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b");

    fs::create_dir(root.join("dir")).unwrap();
    let moved = execute(
        "workspace.renameEntry",
        json!({"root": root_text(&root), "path": "a.txt", "target": "dir/c.txt"}),
    );
    assert_eq!(moved["data"]["path"], "dir/c.txt", "{moved}");
    assert_eq!(moved["data"]["kind"], "file");
    assert_eq!(
        Path::new(moved["data"]["fullPath"].as_str().unwrap()),
        root.join("dir").join("c.txt")
    );
    assert!(!root.join("a.txt").exists());

    let missing = execute(
        "workspace.renameEntry",
        json!({"root": root_text(&root), "path": "a.txt", "target": "d.txt"}),
    );
    assert_eq!(error_code(&missing), "entry_not_found");

    let into_itself = execute(
        "workspace.renameEntry",
        json!({"root": root_text(&root), "path": "dir", "target": "dir/inner"}),
    );
    assert_eq!(error_code(&into_itself), "invalid_request");

    let same = execute(
        "workspace.renameEntry",
        json!({"root": root_text(&root), "path": "b.txt", "target": "b.txt"}),
    );
    assert_eq!(error_code(&same), "entry_exists");
}

#[test]
fn a_rename_that_only_changes_case_is_not_a_collision() {
    let (root, _cleanup) = workspace("rename-case");
    fs::write(root.join("readme.md"), "text").unwrap();
    let response = execute(
        "workspace.renameEntry",
        json!({"root": root_text(&root), "path": "readme.md", "target": "README.md"}),
    );
    assert_eq!(response["ok"], true, "{response}");
    let names: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["README.md"]);
}

#[test]
fn a_link_is_renamed_and_copied_as_a_link() {
    let (root, _cleanup) = workspace("move-link");
    if !links_allowed(&root) {
        return;
    }
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/file.txt"), "real").unwrap();
    lithe_platform::entries::create_symlink(Path::new("real"), &root.join("link")).unwrap();

    let copied = execute(
        "workspace.copyEntry",
        json!({"root": root_text(&root), "path": "link", "target": "link copy"}),
    );
    assert_eq!(copied["data"]["kind"], "symlink", "{copied}");
    assert!(fs::symlink_metadata(root.join("link copy"))
        .unwrap()
        .file_type()
        .is_symlink());

    // `link/x` is not inside `link`: the link is moved, not what it names.
    let renamed = execute(
        "workspace.renameEntry",
        json!({"root": root_text(&root), "path": "link", "target": "real/link"}),
    );
    assert_eq!(renamed["data"]["kind"], "symlink", "{renamed}");
    assert!(fs::symlink_metadata(root.join("real/link"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_to_string(root.join("real/file.txt")).unwrap(),
        "real"
    );
}

#[test]
fn a_copy_keeps_the_tree_and_never_replaces() {
    let (root, _cleanup) = workspace("copy");
    fs::create_dir_all(root.join("src/nested")).unwrap();
    fs::write(root.join("src/a.txt"), "a").unwrap();
    fs::write(root.join("src/nested/b.txt"), "b").unwrap();
    let links = links_allowed(&root);
    if links {
        lithe_platform::entries::create_symlink(Path::new("a.txt"), &root.join("src/alias"))
            .unwrap();
    }

    let copied = execute(
        "workspace.copyEntry",
        json!({"root": root_text(&root), "path": "src", "target": "src copy"}),
    );
    assert_eq!(copied["data"]["kind"], "directory", "{copied}");
    assert_eq!(
        fs::read_to_string(root.join("src copy/nested/b.txt")).unwrap(),
        "b"
    );
    assert_eq!(
        fs::read_to_string(root.join("src copy/a.txt")).unwrap(),
        "a"
    );
    if links {
        assert_eq!(
            fs::read_link(root.join("src copy/alias")).unwrap(),
            Path::new("a.txt")
        );
    }

    let again = execute(
        "workspace.copyEntry",
        json!({"root": root_text(&root), "path": "src/a.txt", "target": "src copy/a.txt"}),
    );
    assert_eq!(error_code(&again), "entry_exists");

    let into_itself = execute(
        "workspace.copyEntry",
        json!({"root": root_text(&root), "path": "src", "target": "src/nested/src"}),
    );
    assert_eq!(error_code(&into_itself), "invalid_request");
    assert!(!root.join("src/nested/src").exists());
}

#[test]
fn an_entry_resolves_without_following_it() {
    let (root, _cleanup) = workspace("resolve");
    fs::create_dir(root.join("dir")).unwrap();
    let resolved = execute(
        "workspace.resolveEntry",
        json!({"root": root_text(&root), "path": "dir"}),
    );
    assert_eq!(resolved["data"]["kind"], "directory", "{resolved}");
    assert_eq!(
        Path::new(resolved["data"]["fullPath"].as_str().unwrap()),
        root.join("dir")
    );
    let missing = execute(
        "workspace.resolveEntry",
        json!({"root": root_text(&root), "path": "nothing"}),
    );
    assert_eq!(error_code(&missing), "entry_not_found");
    if links_allowed(&root) {
        lithe_platform::entries::create_symlink(Path::new("dir"), &root.join("link")).unwrap();
        let link = execute(
            "workspace.resolveEntry",
            json!({"root": root_text(&root), "path": "link"}),
        );
        assert_eq!(link["data"]["kind"], "symlink", "{link}");
    }
}
