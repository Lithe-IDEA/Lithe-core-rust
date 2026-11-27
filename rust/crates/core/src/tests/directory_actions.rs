use crate::execute_json;
use lithe_plugin_rpc::workflow::{check_params, DIRECTORY_TYPE_ICONS};
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
fn resolve_matches_the_shared_fixture() {
    for case in fixture()["resolve"].as_array().unwrap() {
        let response = execute(
            "workspace.resolveDirectoryPresentation",
            case["request"].clone(),
        );
        assert_eq!(response["ok"], true, "{}: {response}", case["id"]);
        assert_eq!(response["data"], case["response"], "{}", case["id"]);
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

#[test]
fn plugin_directory_types_and_marks_resolve_with_layer_precedence() {
    let empty = json!({"marks": {}, "includedDirectories": [], "previousRoles": {}});

    // User can manually assign a plugin.* role to a folder.
    let user_assigned = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": "manual-plugin",
            "action": {"kind": "setRole", "role": "plugin.migrations"},
            "rules": empty
        }),
    );
    assert_eq!(user_assigned["ok"], true, "{user_assigned}");
    let rules = user_assigned["data"]["rules"].clone();

    // User also overrides a plugin-excluded folder back to sources.
    let user_override = execute(
        "workspace.reduceDirectoryAction",
        json!({
            "path": "generated/api",
            "action": {"kind": "setRole", "role": "sources"},
            "rules": rules
        }),
    );
    assert_eq!(user_override["ok"], true, "{user_override}");
    let rules = user_override["data"]["rules"].clone();

    let resolved = execute(
        "workspace.resolveDirectoryPresentation",
        json!({
            "rules": rules,
            "pluginTypes": [
                {
                    "pluginId": "com.example.sql",
                    "id": "plugin.migrations",
                    "name": "SQL Migrations",
                    "tag": "migrations",
                    "icon": "database",
                    "color": "#0d9488",
                    "tintBg": true,
                    "inheritBg": true
                },
                {
                    "pluginId": "com.example.gen",
                    "id": "plugin.cache",
                    "name": "Build Cache",
                    "tag": "cache",
                    "icon": "archive",
                    "color": "#64748b",
                    "excludeIndexing": true,
                    "matchGlob": ".cache/**"
                }
            ],
            "pluginMarks": [
                {"pluginId": "com.example.sql", "path": "db/migrations", "role": "plugin.migrations"},
                {"pluginId": "com.example.gen", "path": "generated/api", "role": "plugin.cache"},
                {"pluginId": "com.example.gen", "path": "vendor/out", "role": "excluded"}
            ],
            "paths": [
                "db/migrations",
                "manual-plugin",
                "generated/api",
                "vendor/out",
                ".cache/v1"
            ]
        }),
    );
    assert_eq!(resolved["ok"], true, "{resolved}");
    let entries = &resolved["data"]["entries"];

    // 1. Plugin mark resolves with plugin source and pluginId.
    assert_eq!(entries["db/migrations"]["role"], "plugin.migrations");
    assert_eq!(entries["db/migrations"]["source"], "plugin");
    assert_eq!(entries["db/migrations"]["pluginId"], "com.example.sql");
    assert_eq!(entries["db/migrations"]["tag"], "migrations");

    // 2. User manually assigning plugin.migrations resolves with user source and plugin styling.
    assert_eq!(entries["manual-plugin"]["role"], "plugin.migrations");
    assert_eq!(entries["manual-plugin"]["source"], "user");
    assert_eq!(entries["manual-plugin"]["icon"], "database");

    // 3. User override beats plugin-excluding mark and removes it from manualExcludedDirectories.
    assert_eq!(entries["generated/api"]["role"], "sources");
    assert_eq!(entries["generated/api"]["source"], "user");
    assert_eq!(entries["generated/api"]["excluded"], false);

    // 4. Plugin-marked excluded directory & plugin glob-excluded directory appear in manualExcludedDirectories.
    let excluded = resolved["data"]["manualExcludedDirectories"]
        .as_array()
        .unwrap();
    assert!(excluded.contains(&json!("vendor/out")));
    assert!(excluded.contains(&json!(".cache/v1")));
    assert!(!excluded.contains(&json!("generated/api")));
}

/// Whatever `workspace/setDirectoryTypes` and `workspace/setDirectoryMarks` accept from a
/// plugin must also resolve, so one plugin's registration can never make the host's
/// presentation request fail for every other layer.
#[test]
fn plugin_contributions_the_rpc_accepts_always_resolve() {
    let context = json!({"contextID": "workspace", "generation": 1});
    let base =
        json!({"id": "plugin.sample", "name": "Sample", "icon": "folder", "color": "#0d9488"});
    let long_id = format!("plugin.{}", "a".repeat(57));
    let too_long_id = format!("plugin.{}", "a".repeat(58));
    let mut types = Vec::new();
    let mut vary = |field: &str, values: Vec<Value>| {
        for value in values {
            let mut item = base.clone();
            item[field] = value;
            types.push(item);
        }
    };
    let icons = DIRECTORY_TYPE_ICONS
        .iter()
        .copied()
        .chain(["module", "package", "bogus", ""]);
    vary("icon", icons.map(|icon| json!(icon)).collect());
    vary(
        "name",
        ["Sample", "  Padded  ", " ", "", "\t"]
            .map(|name| json!(name))
            .into_iter()
            .chain([json!("n".repeat(64)), json!("n".repeat(65))])
            .collect(),
    );
    vary(
        "color",
        ["#0D9488", "#0d948", "0d9488", " #0d9488", "#0d9488 "]
            .map(|color| json!(color))
            .into(),
    );
    vary(
        "tag",
        vec![json!(""), json!("t".repeat(24)), json!("t".repeat(25))],
    );
    vary(
        "matchGlob",
        vec![
            json!("gen/**"),
            json!("g".repeat(256)),
            json!("g".repeat(257)),
        ],
    );
    vary(
        "id",
        vec![
            json!("plugin.a-b_c.d"),
            json!("plugin."),
            json!("plugin.A"),
            json!("custom.a"),
            json!(long_id),
            json!(too_long_id),
        ],
    );
    let mut accepted = 0;
    for item in types {
        let params = json!({"context": context, "types": [item]});
        if check_params("lithe/host/workspace/setDirectoryTypes", &params).is_err() {
            continue;
        }
        accepted += 1;
        let mut contributed = item.clone();
        contributed["pluginId"] = json!("com.example.sample");
        let response = execute(
            "workspace.resolveDirectoryPresentation",
            json!({"rules": {}, "pluginTypes": [contributed], "paths": ["x"]}),
        );
        assert_eq!(response["ok"], true, "{item}: {response}");
    }
    assert!(accepted > DIRECTORY_TYPE_ICONS.len(), "{accepted}");

    let mut marks = Vec::new();
    let paths = [
        "src",
        "src/main",
        "a b",
        "目录/子目录",
        "a:b",
        "a/./b",
        ".",
        "a//b",
    ];
    for path in paths
        .map(|path| json!(path))
        .into_iter()
        .chain([json!("x".repeat(1024))])
    {
        marks.push(json!({"path": path, "role": "sources"}));
    }
    let roles = [
        "plain",
        "sources",
        "tests",
        "resources",
        "testResources",
        "generated",
        "excluded",
        "module",
        "package",
        "plugin.sample",
        "custom.sample",
        "bogus",
    ];
    for role in roles {
        marks.push(json!({"path": "x", "role": role}));
    }
    for color in ["#ABCDEF", "#abc", "red"] {
        marks.push(json!({"path": "x", "role": "sources", "color": color}));
    }
    let mut accepted = 0;
    for mark in marks {
        let params = json!({"context": context, "marks": [mark]});
        if check_params("lithe/host/workspace/setDirectoryMarks", &params).is_err() {
            continue;
        }
        accepted += 1;
        let mut contributed = mark.clone();
        contributed["pluginId"] = json!("com.example.sample");
        let response = execute(
            "workspace.resolveDirectoryPresentation",
            json!({"rules": {}, "pluginMarks": [contributed], "paths": ["x"]}),
        );
        assert_eq!(response["ok"], true, "{mark}: {response}");
    }
    assert!(accepted > 10, "{accepted}");
}
