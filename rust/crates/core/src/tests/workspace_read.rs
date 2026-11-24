//! API-06 disk reads through the JSON C ABI, using real isolated files.

use super::support::{temporary_root, Cleanup};
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn workspace(label: &str) -> (PathBuf, Cleanup) {
    let root = temporary_root(label);
    fs::create_dir_all(&root).unwrap();
    (root.clone(), Cleanup(root))
}

fn execute(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({"command":command,"payload":payload}).to_string(),
    ))
    .unwrap()
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../../fixtures/workspace/read-v1.json")).unwrap()
}

#[test]
fn workspace_read_directory_pages_are_sorted_filtered_and_scoped() {
    let (root, _cleanup) = workspace("workspace-read-pages");
    for name in ["c.txt", "a.txt", "b.txt", "ignored.tmp"] {
        fs::write(root.join(name), name).unwrap();
    }
    fs::create_dir(root.join(".git")).unwrap();
    let mut request = json!({"root":root,"path":"","limit":2,"hiddenFilePatterns":["*.tmp"]});
    let first = execute("workspace.readDirectory", request.clone());
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["data"]["entries"], fixture()["directoryFirst"]);
    let cursor = first["data"]["nextCursor"].as_str().expect("another page");
    request["cursor"] = json!(cursor);
    let second = execute("workspace.readDirectory", request.clone());
    assert_eq!(second["data"]["entries"], fixture()["directoryLast"]);
    assert_eq!(second["data"]["nextCursor"], Value::Null);
    request["hiddenFilePatterns"] = json!([]);
    assert_eq!(
        execute("workspace.readDirectory", request)["error"]["code"],
        "invalid_request"
    );
    let explicit = execute(
        "workspace.readFile",
        json!({"root":root,"path":"ignored.tmp"}),
    );
    assert_eq!(explicit["data"]["text"], "ignored.tmp");
    let stat = execute("workspace.stat", json!({"root":root,"path":"a.txt"}));
    assert_eq!(stat["data"]["kind"], "file");
    assert_eq!(stat["data"]["size"], 5);
    assert!(stat["data"]["modifiedMilliseconds"].is_u64());
    assert_eq!(
        execute("workspace.stat", json!({"root":root,"path":""}))["data"]["kind"],
        "directory"
    );
}

#[test]
fn workspace_read_file_preserves_bytes_and_marks_prefix_identity() {
    let (root, _cleanup) = workspace("workspace-read-bytes");
    for case in fixture()["files"].as_array().unwrap() {
        let bytes = case["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as u8)
            .collect::<Vec<_>>();
        fs::write(root.join("input"), bytes).unwrap();
        let mut request = json!({"root":root,"path":"input"});
        if let Some(maximum) = case.get("maximumBytes") {
            request["maximumBytes"] = maximum.clone();
        }
        let response = execute("workspace.readFile", request);
        assert_eq!(response["ok"], true, "{}: {response}", case["id"]);
        for (key, expected) in case["expected"].as_object().unwrap() {
            assert_eq!(response["data"][key], *expected, "{}: {key}", case["id"]);
        }
        assert_eq!(
            response["data"]["identity"]["byteCount"],
            response["data"]["byteCount"]
        );
        assert_eq!(
            response["data"]["identity"]["isTruncated"],
            response["data"]["isTruncated"]
        );
        assert_eq!(
            response["data"]["identity"]["sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert_ne!(
            response["data"]["text"].is_string(),
            response["data"]["base64"].is_string()
        );
    }
    fs::write(root.join("large"), vec![b'x'; 128 * 1024 + 1]).unwrap();
    let response = execute("workspace.readFile", json!({"root":root,"path":"large"}));
    assert_eq!(response["data"]["byteCount"], 128 * 1024);
    assert_eq!(response["data"]["isTruncated"], true);
}

#[test]
fn workspace_read_rejects_invalid_requests_before_reading() {
    let (root, _cleanup) = workspace("workspace-read-invalid");
    for case in fixture()["invalid"].as_array().unwrap() {
        let mut request = case["request"].clone();
        request["root"] = json!(root);
        let response = execute(case["command"].as_str().unwrap(), request);
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "{case}: {response}"
        );
    }
    assert_eq!(
        execute("workspace.stat", json!({"root":root,"path":"missing"}))["error"]["code"],
        "entry_not_found"
    );
    for command in [
        "workspace.readDirectory",
        "workspace.stat",
        "workspace.readFile",
    ] {
        let response: Value = serde_json::from_str(&execute_json(&json!({
            "command":command,"payload":{"root":root,"path":"input"},
            "remoteWorkspace":{"sessionId":"not-connected","localRoot":root,"remoteRoot":"/remote"}
        }).to_string())).unwrap();
        assert_eq!(response["error"]["code"], "not_supported", "{response}");
    }
}

#[cfg(unix)]
#[test]
fn workspace_read_never_follows_leaf_or_ancestor_links() {
    use std::os::unix::fs::symlink;
    let (root, _cleanup) = workspace("workspace-read-links");
    let (outside, _outside_cleanup) = workspace("workspace-read-outside");
    fs::write(outside.join("secret"), "outside").unwrap();
    symlink(&outside, root.join("alias")).unwrap();
    symlink(outside.join("secret"), root.join("leaf")).unwrap();
    for path in ["leaf", "alias/secret"] {
        assert_eq!(
            execute("workspace.readFile", json!({"root":root,"path":path}))["error"]["code"],
            "permission_denied"
        );
    }
    assert_eq!(
        execute(
            "workspace.readDirectory",
            json!({"root":root,"path":"alias"})
        )["error"]["code"],
        "permission_denied"
    );
    assert_eq!(
        execute("workspace.stat", json!({"root":root,"path":"alias/secret"}))["error"]["code"],
        "permission_denied"
    );
    assert_eq!(
        execute("workspace.stat", json!({"root":root,"path":"leaf"}))["data"]["kind"],
        "symlink"
    );
    assert_eq!(
        execute("workspace.readDirectory", json!({"root":root,"path":""}))["data"]["entries"],
        json!([])
    );
}
