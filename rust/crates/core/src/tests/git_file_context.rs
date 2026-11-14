//! `git.resolveFileContext` routes a path to the innermost repository that owns it.

use super::support::temporary_root;
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let workspace = Self(temporary_root("file-context"));
        for child in ["first", "first/nested"] {
            let directory = workspace.0.join(child);
            fs::create_dir_all(&directory).unwrap();
            assert!(Command::new("git")
                .args(["init", "-q"])
                .current_dir(directory)
                .status()
                .unwrap()
                .success());
        }
        workspace
    }

    fn root(&self, name: &str) -> String {
        lithe_git::simplified_canonical_path(self.0.join(name).canonicalize().unwrap())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn routes_a_nested_path_to_the_nested_repository() {
    let workspace = Workspace::new();
    let response: Value = serde_json::from_str(&execute_json(
        &json!({
            "id": "resolve", "command": "git.resolveFileContext",
            "payload": {"root": workspace.root("first"), "path": "nested/file.txt"}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["root"], workspace.root("first/nested"));
    assert_eq!(response["data"]["path"], "file.txt");
    assert_eq!(response["data"]["ownership"], "git");
}
