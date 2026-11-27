use crate::execute_json;
use serde_json::{json, Value};

fn execute(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({"id": "directory-actions", "command": command, "payload": payload}).to_string(),
    ))
    .expect("core response should be JSON")
}

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../fixtures/workspace/directory-rules-v1.json"
    ))
    .expect("directory rules fixture should be JSON")
}

/// Sorts include lists so the comparison does not depend on insertion order.
fn normalized(mut value: Value) -> Value {
    fn visit(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if let Some(Value::Array(list)) = map.get_mut("includedDirectories") {
                    list.sort_by_key(|item| item.as_str().unwrap_or_default().to_owned());
                }
                map.values_mut().for_each(visit);
            }
            Value::Array(list) => list.iter_mut().for_each(visit),
            _ => {}
        }
    }
    visit(&mut value);
    value
}

#[test]
fn reduce_matches_the_shared_fixture() {
    for case in fixture()["reduce"].as_array().unwrap() {
        let response = execute("workspace.reduceDirectoryAction", case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["id"]);
        assert_eq!(
            normalized(response["data"].clone()),
            normalized(case["response"].clone()),
            "{}",
            case["id"]
        );
    }
}

#[test]
fn relocate_matches_the_shared_fixture() {
    for case in fixture()["relocate"].as_array().unwrap() {
        let response = execute("workspace.relocateDirectoryRules", case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["id"]);
        assert_eq!(
            normalized(response["data"].clone()),
            normalized(case["response"].clone()),
            "{}",
            case["id"]
        );
    }
}

#[test]
fn invalid_requests_are_rejected() {
    for case in fixture()["invalid"].as_array().unwrap() {
        let response = execute(case["command"].as_str().unwrap(), case["request"].clone());
        assert_eq!(response["ok"], false, "{case}");
        assert_eq!(response["error"]["code"], "invalid_request", "{case}");
    }
}

/// The action-sequence contract shared with the Git-ignore search tests: replaying every
/// step through Core must reach the role and include state the fixture lists.
#[test]
fn action_sequences_reach_the_documented_role_and_inclusion() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/workspace/directory-actions-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut rules = json!({"marks": {}, "includedDirectories": [], "previousRoles": {}});
        for step in case["steps"].as_array().unwrap() {
            let response = execute(
                "workspace.reduceDirectoryAction",
                json!({"path": "archive", "action": step["action"], "rules": rules}),
            );
            assert_eq!(response["ok"], true, "{}: {response}", case["id"]);
            rules = response["data"]["rules"].clone();
            let role = rules["marks"]["archive"].as_str().unwrap_or("plain");
            let included = rules["includedDirectories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "archive");
            assert_eq!(role, step["role"], "{}", case["id"]);
            assert_eq!(included, step["included"], "{}", case["id"]);
        }
    }
}

#[test]
fn custom_types_colors_and_presentation_resolve_deterministically() {
    let mut rules = json!({"marks": {}, "includedDirectories": [], "previousRoles": {}});

    // 1. Upsert a custom directory type with glob matching and indexing exclusion.
    let upsert = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": ".",
            "action": {
                "kind": "upsertCustomType",
                "directoryType": {
                    "id": "custom.legacy",
                    "name": "Legacy Archive",
                    "tag": "legacy",
                    "icon": "archive",
                    "color": "#8B949E",
                    "tintBg": true,
                    "tintText": false,
                    "inheritBg": true,
                    "excludeIndexing": true,
                    "readOnlyHint": true,
                    "matchGlob": "legacy/**"
                }
            },
            "rules": rules
        }),
    );
    assert_eq!(upsert["ok"], true, "{upsert}");
    assert_eq!(upsert["data"]["changed"], true);
    rules = upsert["data"]["rules"].clone();
    assert_eq!(rules["customTypes"][0]["color"], "#8b949e");

    // 2. Assign built-in tests role and quick color override.
    let set_tests = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": "src/test",
            "action": {"kind": "setRole", "role": "tests"},
            "rules": rules
        }),
    );
    assert_eq!(set_tests["ok"], true, "{set_tests}");
    rules = set_tests["data"]["rules"].clone();

    let set_color = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": "src/test",
            "action": {"kind": "setColor", "color": "#4E8DF5"},
            "rules": rules
        }),
    );
    assert_eq!(set_color["ok"], true, "{set_color}");
    rules = set_color["data"]["rules"].clone();

    // 3. Resolve presentation across user override, shared rules, glob match and Git ignore.
    let shared_rules = json!({
        "marks": {"src/test": "resources", "docs": "generated"},
        "includedDirectories": [],
        "previousRoles": {}
    });
    let resolved = execute(
        "workspace.resolveDirectoryPresentation",
        json!({
            "rules": rules,
            "sharedRules": shared_rules,
            "gitExcluded": ["target"],
            "paths": [
                "src/test",
                "src/test/OrderTest.java",
                "docs",
                "legacy/v1",
                "legacy/v1/Old.java",
                "target"
            ]
        }),
    );
    assert_eq!(resolved["ok"], true, "{resolved}");
    let entries = &resolved["data"]["entries"];

    // User layer beats shared layer and reports autoFallbackRole.
    assert_eq!(entries["src/test"]["role"], "tests");
    assert_eq!(entries["src/test"]["source"], "user");
    assert_eq!(entries["src/test"]["color"], "#4e8df5");
    assert_eq!(entries["src/test"]["autoFallbackRole"], "resources");

    // Child inherits background tint from parent.
    assert_eq!(entries["src/test/OrderTest.java"]["source"], "inherited");
    assert_eq!(entries["src/test/OrderTest.java"]["bgColor"], "#4e8df5");

    // Shared layer applies when no user override exists.
    assert_eq!(entries["docs"]["role"], "generated");
    assert_eq!(entries["docs"]["source"], "shared");
    assert_eq!(entries["docs"]["readOnlyHint"], true);

    // Glob match applies custom type and propagates exclusion to children & indexing.
    assert_eq!(entries["legacy/v1"]["role"], "custom.legacy");
    assert_eq!(entries["legacy/v1"]["source"], "glob");
    assert_eq!(entries["legacy/v1"]["excluded"], true);
    assert_eq!(entries["legacy/v1/Old.java"]["excluded"], true);
    assert_eq!(entries["legacy/v1/Old.java"]["bgColor"], "#8b949e");

    let manual_excluded = resolved["data"]["manualExcludedDirectories"]
        .as_array()
        .unwrap();
    assert!(manual_excluded.iter().any(|v| v == "legacy/v1"));

    // VCS exclusion applies when not included.
    assert_eq!(entries["target"]["role"], "excluded");
    assert_eq!(entries["target"]["source"], "vcs");
    assert_eq!(entries["target"]["excluded"], true);

    // 4. Relocating a directory also moves its color override.
    let relocated = execute(
        "workspace.relocateDirectoryRules",
        json!({
            "source": "src/test",
            "destination": "tests",
            "rules": rules
        }),
    );
    assert_eq!(relocated["ok"], true, "{relocated}");
    assert_eq!(relocated["data"]["rules"]["marks"]["tests"], "tests");
    assert_eq!(relocated["data"]["rules"]["colors"]["tests"], "#4e8df5");

    // 5. Deleting a custom type cleans up referencing marks.
    let mark_custom = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": "old",
            "action": {"kind": "setRole", "role": "custom.legacy"},
            "rules": rules
        }),
    );
    assert_eq!(mark_custom["ok"], true);
    let deleted = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": ".",
            "action": {"kind": "deleteCustomType", "id": "custom.legacy"},
            "rules": mark_custom["data"]["rules"]
        }),
    );
    assert_eq!(deleted["ok"], true);
    assert!(deleted["data"]["rules"].get("customTypes").is_none());
    assert!(deleted["data"]["rules"]["marks"].get("old").is_none());
}

#[test]
fn invalid_custom_type_and_color_are_rejected() {
    let empty = json!({"marks": {}, "includedDirectories": [], "previousRoles": {}});
    let bad_color = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": "src",
            "action": {"kind": "setColor", "color": "not-a-color"},
            "rules": empty
        }),
    );
    assert_eq!(bad_color["ok"], false);
    assert_eq!(bad_color["error"]["code"], "invalid_request");

    let bad_id = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": ".",
            "action": {
                "kind": "upsertCustomType",
                "directoryType": {
                    "id": "sources",
                    "name": "Bad",
                    "icon": "folder",
                    "color": "#4e8df5"
                }
            },
            "rules": empty
        }),
    );
    assert_eq!(bad_id["ok"], false);
    assert_eq!(bad_id["error"]["code"], "invalid_request");
}

#[test]
fn custom_icons_can_be_added_referenced_and_deleted_with_fallback() {
    let empty = json!({"marks": {}, "includedDirectories": [], "previousRoles": {}});
    let added = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": ".",
            "action": {
                "kind": "upsertCustomIcon",
                "directoryIcon": {
                    "id": "icon.rocket",
                    "name": "Rocket",
                    "path": "directory-icons/icon.rocket.svg"
                }
            },
            "rules": empty
        }),
    );
    assert_eq!(added["ok"], true, "{added}");
    let rules = added["data"]["rules"].clone();

    let with_type = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": ".",
            "action": {
                "kind": "upsertCustomType",
                "directoryType": {
                    "id": "custom.launch",
                    "name": "Launch",
                    "icon": "icon.rocket",
                    "color": "#4e8df5"
                }
            },
            "rules": rules
        }),
    );
    assert_eq!(with_type["ok"], true, "{with_type}");

    let deleted = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": ".",
            "action": {"kind": "deleteCustomIcon", "id": "icon.rocket"},
            "rules": with_type["data"]["rules"]
        }),
    );
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert!(deleted["data"]["rules"].get("customIcons").is_none());
    assert_eq!(deleted["data"]["rules"]["customTypes"][0]["icon"], "folder");
}
