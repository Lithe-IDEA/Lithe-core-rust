//! API-07/08 commands against real files and the shared host-boundary fixture.

use super::support::{temporary_root, Cleanup};
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../fixtures/workspace/find-watch-v1.json"
    ))
    .unwrap()
}

fn workspace(label: &str) -> (PathBuf, Cleanup) {
    let root = temporary_root(label);
    fs::create_dir_all(&root).unwrap();
    for file in fixture()["files"].as_array().unwrap() {
        let path = root.join(file["path"].as_str().unwrap());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, file["text"].as_str().unwrap()).unwrap();
    }
    (root.clone(), Cleanup(root))
}

fn execute(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({"command":command,"payload":payload}).to_string(),
    ))
    .unwrap()
}

#[test]
fn workspace_find_pages_match_the_fixture_and_bind_query_options() {
    let (root, _cleanup) = workspace("workspace-find");
    let fixture = fixture();
    for (method, key, expected) in [
        ("workspace.findFiles", "fileRequest", "filePaths"),
        ("workspace.findText", "textRequest", "textMatches"),
    ] {
        let mut request = fixture[key].clone();
        request["root"] = json!(root);
        let mut actual = Vec::new();
        for _ in 0..5 {
            let response = execute(method, request.clone());
            assert_eq!(response["ok"], true, "{response}");
            assert_eq!(response["data"]["source"], "disk");
            for item in response["data"]["matches"].as_array().unwrap() {
                actual.push(if expected == "filePaths" {
                    item["path"].clone()
                } else {
                    item.clone()
                });
            }
            let cursor = &response["data"]["nextCursor"];
            if cursor.is_null() {
                break;
            }
            request["cursor"] = cursor.clone();
            let mut changed = request.clone();
            changed["fileMask"] = json!("*.different");
            assert_eq!(execute(method, changed)["error"]["code"], "invalid_request");
        }
        assert_eq!(json!(actual), fixture[expected]);
    }
    let bad = execute(
        "workspace.findText",
        json!({"root":root,"pattern":"[","regularExpression":true}),
    );
    assert_eq!(bad["error"]["code"], "parse_failed");
}

#[test]
fn workspace_find_reports_caps_and_does_not_read_binary_or_oversized_files() {
    let (root, _cleanup) = workspace("workspace-find-caps");
    let lines = "alpha\n".repeat(202);
    fs::write(root.join("a.txt"), lines).unwrap();
    fs::write(root.join("binary.txt"), b"alpha\0").unwrap();
    fs::write(root.join("large.txt"), vec![b'a'; 2 * 1024 * 1024 + 1]).unwrap();
    let response = execute(
        "workspace.findText",
        json!({"root":root,"pattern":"alpha","limit":200}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["matches"].as_array().unwrap().len(), 200);
    assert_eq!(response["data"]["isTruncated"], true);
    assert!(response["data"]["nextCursor"].is_null());
    let response = execute(
        "workspace.findText",
        json!({"root":root,"pattern":"BETA","caseSensitive":true,"wholeWords":true}),
    );
    assert!(response["data"]["matches"].as_array().unwrap().is_empty());
    assert_eq!(response["data"]["isTruncated"], true);
}

#[test]
fn workspace_watch_plans_and_events_are_scoped_filtered_and_bounded() {
    let (root, _cleanup) = workspace("workspace-watch-events");
    let plan = execute(
        "workspace.watchPlan",
        json!({"root":root,"path":"src","recursive":true}),
    );
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["data"]["isDirectory"], true);
    let request = json!({"root":root,"path":"","isDirectory":true,"recursive":true,"paths":fixture()["watch"]["paths"]});
    let response = execute("workspace.watchEvents", request.clone());
    assert_eq!(response["data"], fixture()["watch"]["expected"]);
    let response = execute(
        "workspace.watchEvents",
        json!({"root":root,"path":"src","isDirectory":true,"recursive":false,"paths":["src/b.txt","src/deleted","src/deep/file","a.txt"]}),
    );
    assert_eq!(
        response["data"]["changedPaths"],
        json!(["src/b.txt", "src/deleted"])
    );
    let response = execute(
        "workspace.watchEvents",
        json!({"root":root,"path":"a.txt","isDirectory":false,"paths":["a.txt","z.md"],"requiresRescan":true}),
    );
    assert_eq!(response["data"]["changedPaths"], json!(["a.txt"]));
    assert_eq!(response["data"]["requiresRescan"], true);
    let mut overflow = request;
    overflow["paths"] = json!(vec!["a.txt"; 1001]);
    assert_eq!(
        execute("workspace.watchEvents", overflow)["data"]["requiresRescan"],
        true
    );
    assert_eq!(
        execute(
            "workspace.watchPlan",
            json!({"root":root,"path":"../escape"})
        )["error"]["code"],
        "invalid_request"
    );
}

#[cfg(unix)]
#[test]
fn workspace_find_and_watch_never_traverse_a_link() {
    let (root, _cleanup) = workspace("workspace-find-links");
    std::os::unix::fs::symlink(root.join("src"), root.join("alias")).unwrap();
    std::os::unix::fs::symlink(root.join("a.txt"), root.join("link.txt")).unwrap();
    assert_eq!(
        execute("workspace.watchPlan", json!({"root":root,"path":"alias"}))["error"]["code"],
        "permission_denied"
    );
    let response = execute(
        "workspace.findFiles",
        json!({"root":root,"query":".txt","limit":200}),
    );
    assert_eq!(
        response["data"]["matches"],
        json!([{"path":"a.txt"},{"path":"src/b.txt"}])
    );
    let response = execute(
        "workspace.watchEvents",
        json!({"root":root,"path":"","isDirectory":true,"recursive":true,"paths":["alias/b.txt","link.txt"]}),
    );
    assert_eq!(response["data"]["changedPaths"], json!([]));
}
