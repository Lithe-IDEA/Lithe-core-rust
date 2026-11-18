//! Issue 1lck/Lithe-IDEA#2100: the run configuration list has folders, a
//! manual order and an alphabetical sort, and any configuration can be
//! copied. A folder is saved in the configuration, the order only locally.

use super::support::{
    apply_run_changes, changed_run_document, changed_run_paths, temporary_root, write_run_document,
    Cleanup,
};
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;

const PROJECT: &str = ".lithe/run/configurations.yaml";
const LOCAL: &str = ".lithe/run/local.yaml";

fn reply(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({ "id": command, "command": command, "payload": payload }).to_string(),
    ))
    .unwrap()
}

fn call(command: &str, payload: Value) -> Value {
    let reply = reply(command, payload);
    assert_eq!(reply["ok"], true, "{command}: {reply}");
    reply["data"].clone()
}

fn executable(id: &str, name: &str) -> Value {
    json!({
        "id": id, "name": name, "provider": "process.executable",
        "confidence": "declared", "extensions": {"process": {"program": "tool"}}
    })
}

fn names(resolved: &Value) -> Vec<String> {
    resolved["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_string())
        .collect()
}

fn entry<'a>(document: &'a Value, id: &str) -> &'a Value {
    document["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in {document}"))
}

#[test]
fn folders_are_saved_in_the_defining_layer_and_the_order_only_locally() {
    let root = temporary_root("run-arrangement");
    let _cleanup = Cleanup(root.clone());
    write_run_document(
        &root,
        PROJECT,
        &json!({"version": 2, "configurations": [executable("user:serve", "serve")]}),
    );
    write_run_document(
        &root,
        LOCAL,
        &json!({"version": 2, "configurations": [
            executable("user:Build", "Build"), executable("user:check", "check")
        ]}),
    );
    // Sorted by name, ignoring case, before anything is arranged.
    let resolved = call("runConfig.resolve", json!({"root": root}));
    assert_eq!(names(&resolved), ["Build", "check", "serve"]);
    assert_eq!(resolved["sortAlphabetically"], true);

    // A project configuration takes its folder to the team; a local one
    // keeps it in the local layer.
    let arranged = call(
        "runConfig.arrange",
        json!({"root": root, "folders": {"user:serve": "Servers", "user:check": "Checks"}}),
    );
    assert_eq!(changed_run_paths(&arranged), [PROJECT, LOCAL]);
    let project = changed_run_document(&arranged, PROJECT).unwrap();
    assert_eq!(entry(&project, "user:serve")["folder"], "Servers");
    let local = changed_run_document(&arranged, LOCAL).unwrap();
    assert_eq!(entry(&local, "user:check")["folder"], "Checks");
    assert!(local.get("arrangement").is_none());
    apply_run_changes(&root, &arranged);

    // A manual order is local only and takes effect once sorting is off.
    let ordered = call(
        "runConfig.arrange",
        json!({"root": root, "order": ["user:serve", "gone", "user:check"], "sortAlphabetically": false}),
    );
    assert_eq!(changed_run_paths(&ordered), [LOCAL]);
    let local = changed_run_document(&ordered, LOCAL).unwrap();
    assert_eq!(
        local["arrangement"],
        json!({"sortAlphabetically": false, "order": ["user:serve", "gone", "user:check"]})
    );
    apply_run_changes(&root, &ordered);
    let resolved = call("runConfig.resolve", json!({"root": root}));
    // Configurations the order does not name follow it by name.
    assert_eq!(names(&resolved), ["serve", "check", "Build"]);
    assert_eq!(resolved["sortAlphabetically"], false);
    let serve = entry(&resolved, "user:serve");
    assert_eq!(serve["folder"], "Servers");

    // Sorting again keeps the saved order for later.
    let sorted = call(
        "runConfig.arrange",
        json!({"root": root, "sortAlphabetically": true}),
    );
    let local = changed_run_document(&sorted, LOCAL).unwrap();
    assert_eq!(local["arrangement"]["order"][0], "user:serve");
    apply_run_changes(&root, &sorted);
    assert_eq!(
        names(&call("runConfig.resolve", json!({"root": root}))),
        ["Build", "check", "serve"]
    );

    // `null` takes a configuration out of its folder.
    let out = call(
        "runConfig.arrange",
        json!({"root": root, "folders": {"user:serve": null}}),
    );
    assert_eq!(changed_run_paths(&out), [PROJECT]);
    let project = changed_run_document(&out, PROJECT).unwrap();
    assert!(entry(&project, "user:serve").get("folder").is_none());

    // Nothing to change writes nothing.
    let unchanged = call(
        "runConfig.arrange",
        json!({"root": root, "folders": {"user:check": "Checks"}}),
    );
    assert_eq!(unchanged["changes"], json!([]));
}

#[test]
fn a_local_folder_override_wins_and_a_bad_request_changes_nothing() {
    let root = temporary_root("run-arrangement-override");
    let _cleanup = Cleanup(root.clone());
    let mut serve = executable("user:serve", "serve");
    serve["folder"] = json!("Team");
    write_run_document(
        &root,
        PROJECT,
        &json!({"version": 2, "configurations": [serve]}),
    );
    write_run_document(
        &root,
        LOCAL,
        &json!({"version": 2, "configurations": [{"id": "user:serve", "folder": "Mine"}]}),
    );
    let resolved = call("runConfig.resolve", json!({"root": root}));
    assert_eq!(entry(&resolved, "user:serve")["folder"], "Mine");

    // Moving it writes the defining project entry and drops the local
    // override that would hide the choice.
    let moved = call(
        "runConfig.arrange",
        json!({"root": root, "folders": {"user:serve": "Servers"}}),
    );
    let project = changed_run_document(&moved, PROJECT).unwrap();
    assert_eq!(entry(&project, "user:serve")["folder"], "Servers");
    let local = changed_run_document(&moved, LOCAL).unwrap();
    assert_eq!(local["configurations"], json!([]));

    for (payload, message) in [
        (
            json!({"root": root, "folders": {"user:missing": "A"}}),
            "Run configuration was not found",
        ),
        (
            json!({"root": root, "folders": {"user:serve": " padded"}}),
            "Invalid run configuration folder",
        ),
        (
            json!({"root": root, "folders": {"user:serve": "a\nb"}}),
            "Invalid run configuration folder",
        ),
        (
            json!({"root": root, "order": [""]}),
            "Invalid run configuration order",
        ),
    ] {
        let failed = reply("runConfig.arrange", payload);
        assert_eq!(failed["ok"], false, "{failed}");
        assert_eq!(failed["error"]["message"], message, "{failed}");
    }

    // A hand-edited arrangement that cannot be read is reported and ignored.
    write_run_document(
        &root,
        LOCAL,
        &json!({"version": 2, "configurations": [], "arrangement": {"order": "user:serve"}}),
    );
    let resolved = call("runConfig.resolve", json!({"root": root}));
    assert_eq!(resolved["sortAlphabetically"], true);
    assert!(resolved["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "invalidArrangement"));
    // So is a folder name no folder can have.
    let mut bad = executable("user:bad", "bad");
    bad["folder"] = json!("");
    write_run_document(
        &root,
        LOCAL,
        &json!({"version": 2, "configurations": [bad]}),
    );
    let resolved = call("runConfig.resolve", json!({"root": root}));
    assert_eq!(
        entry(&resolved, "user:bad")["availability"]["code"],
        "invalidConfiguration"
    );
}

#[test]
fn copies_keep_every_choice_and_never_hide_the_original() {
    let root = temporary_root("run-copy");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("app")).unwrap();
    let mut serve = executable("user:serve", "serve");
    serve["folder"] = json!("Servers");
    serve["args"] = json!(["--port", "8080"]);
    write_run_document(
        &root,
        PROJECT,
        &json!({"version": 2, "configurations": [serve]}),
    );

    // A project configuration is copied into the project layer, folder
    // and arguments included.
    let copied = call(
        "runConfig.copyConfiguration",
        json!({"root": root, "configurationId": "user:serve", "name": "serve (1)"}),
    );
    assert_eq!(copied["id"], "user:serve-1");
    assert_eq!(changed_run_paths(&copied), [PROJECT]);
    let project = changed_run_document(&copied, PROJECT).unwrap();
    let copy = entry(&project, "user:serve-1");
    assert_eq!(copy["name"], "serve (1)");
    assert_eq!(copy["folder"], "Servers");
    assert_eq!(copy["args"], json!(["--port", "8080"]));
    assert!(copy.get("source").is_none());
    apply_run_changes(&root, &copied);

    // A generated build provider run stays listed next to its copy, which
    // is saved in the plugin's local document.
    let reference = json!({
        "pluginID": "dev.example.engine", "buildSystemID": "engine", "root": "app",
        "run": "serve", "projectID": "opaque/project", "targetID": "opaque/target",
        "execution": "service"
    });
    call(
        "runConfig.generate",
        json!({"root": root, "providerRuns": [
            {"projectPath": "app", "name": "Engine", "reference": reference}
        ]}),
    );
    let resolved = call("runConfig.resolve", json!({"root": root}));
    let generated = resolved["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["provider"] == "build.provider")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let copied = call(
        "runConfig.copyConfiguration",
        json!({"root": root, "configurationId": generated, "name": "Engine (1)"}),
    );
    let plugin = ".lithe/run/plugin/dev.example.engine.local.toml";
    assert_eq!(changed_run_paths(&copied), [plugin]);
    apply_run_changes(&root, &copied);
    let resolved = call("runConfig.resolve", json!({"root": root}));
    assert_eq!(
        names(&resolved),
        ["Engine", "Engine (1)", "serve", "serve (1)"]
    );
    let copy = entry(&resolved, copied["id"].as_str().unwrap());
    assert_eq!(copy["extensions"]["build"]["targetID"], "opaque/target");
    assert_eq!(entry(&resolved, &generated)["source"], "local");

    let failed = reply(
        "runConfig.copyConfiguration",
        json!({"root": root, "configurationId": "user:missing", "name": "x"}),
    );
    assert_eq!(
        failed["error"]["message"],
        "Run configuration was not found"
    );
    let failed = reply(
        "runConfig.copyConfiguration",
        json!({"root": root, "configurationId": "user:serve", "name": "  "}),
    );
    assert_eq!(failed["error"]["message"], "Invalid run configuration name");
}
