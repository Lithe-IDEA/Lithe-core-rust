use super::support::temporary_root;
use crate::execute_json;
use serde_json::Value;
use std::fs;

#[test]
fn list_directories_orders_versioned_names_by_number_with_directories_first() {
    let root = temporary_root("natural-directory-order");
    for name in [
        "v0.1.12.md",
        "v0.1.4.md",
        "v0.1.14.md",
        "TEMPLATE.md",
        "v0.1.13.md",
        "v0.2.0.md",
    ] {
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(name), "").unwrap();
    }
    fs::create_dir_all(root.join("zeta/part10")).unwrap();
    fs::create_dir_all(root.join("zeta/part2")).unwrap();
    fs::create_dir_all(root.join("Alpha")).unwrap();

    let response: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "natural-directory-order",
            "command": "workspace.listDirectories",
            "payload": {"root": root, "paths": ["", "zeta"]}
        })
        .to_string(),
    ))
    .expect("core response should be JSON");
    let removed = fs::remove_dir_all(&root);
    assert_eq!(response["ok"], true, "{response}");
    removed.expect("temporary fixture should be removable");

    let names = |index: usize| {
        response["data"]["directories"][index]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|child| child["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(0),
        [
            "Alpha",
            "zeta",
            "TEMPLATE.md",
            "v0.1.4.md",
            "v0.1.12.md",
            "v0.1.13.md",
            "v0.1.14.md",
            "v0.2.0.md",
        ]
    );
    assert_eq!(names(1), ["part2", "part10"]);
}

/// The snapshot `files` sequence is not a display order: Windows Quick Open keeps the
/// first 100 matches of it, so the natural tree order must not leak into it.
#[test]
fn workspace_snapshot_keeps_its_traversal_order_while_the_tree_rows_are_natural() {
    let root = temporary_root("snapshot-traversal-order");
    fs::create_dir_all(&root).unwrap();
    for name in ["v0.1.12.md", "v0.1.4.md", "v0.1.14.md", "TEMPLATE.md"] {
        fs::write(root.join(name), "").unwrap();
    }
    for part in ["part10", "part2"] {
        fs::create_dir_all(root.join("zeta").join(part)).unwrap();
        fs::write(root.join("zeta").join(part).join("a.md"), "").unwrap();
    }

    let files = |include_tree: bool| {
        let response: Value = serde_json::from_str(&execute_json(
            &serde_json::json!({
                "id": "snapshot-traversal-order",
                "command": "workspace.snapshot",
                "payload": {"root": root, "includeTree": include_tree}
            })
            .to_string(),
        ))
        .expect("core response should be JSON");
        assert_eq!(response["ok"], true, "{response}");
        response["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let with_tree = files(true);
    let without_tree = files(false);
    let removed = fs::remove_dir_all(&root);
    removed.expect("temporary fixture should be removable");

    let expected = [
        "zeta/part10/a.md",
        "zeta/part2/a.md",
        "TEMPLATE.md",
        "v0.1.12.md",
        "v0.1.14.md",
        "v0.1.4.md",
    ];
    assert_eq!(with_tree, expected);
    assert_eq!(without_tree, expected);
}

fn execute(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &serde_json::json!({"id": "entry-order", "command": command, "payload": payload})
            .to_string(),
    ))
    .expect("core response should be JSON")
}

// Windows and macOS plugin listings enumerate natively and only apply this
// permutation, so the shared fixture pins the cases where the old Windows
// lowercase-plus-localeCompare order diverged.
#[test]
fn order_entries_matches_the_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/workspace/entry-order-v1.json"
    ))
    .expect("entry-order fixture should be JSON");
    for case in fixture["cases"].as_array().unwrap() {
        let response = execute("workspace.orderEntries", case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["id"]);
        let entries = case["request"]["entries"].as_array().unwrap();
        let names = response["data"]["order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|index| entries[index.as_u64().unwrap() as usize]["name"].clone())
            .collect::<Vec<_>>();
        assert_eq!(Value::Array(names), case["orderedNames"], "{}", case["id"]);
    }
}

#[test]
fn order_entries_rejects_oversized_input_instead_of_truncating() {
    let long_name = "a".repeat(4097);
    let response = execute(
        "workspace.orderEntries",
        serde_json::json!({"entries": [{"name": long_name, "isDirectory": false}]}),
    );
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_request");

    let entries = (0..100_001)
        .map(|index| serde_json::json!({"name": format!("f{index}"), "isDirectory": false}))
        .collect::<Vec<_>>();
    let response = execute(
        "workspace.orderEntries",
        serde_json::json!({ "entries": entries }),
    );
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_request");
}

// Identical names cannot occur in one directory, but the order must stay total
// so a host never observes a platform-dependent unstable sort.
#[test]
fn order_entries_keeps_request_order_for_identical_names() {
    let response = execute(
        "workspace.orderEntries",
        serde_json::json!({"entries": [
            {"name": "same", "isDirectory": false},
            {"name": "same", "isDirectory": false},
            {"name": "Same", "isDirectory": false}
        ]}),
    );
    assert_eq!(response["data"]["order"], serde_json::json!([2, 0, 1]));
}
