//! Issue 1lck/Lithe-IDEA#1616: a run configuration names built-in process runs
//! to start first. Core checks and expands the references and says whether a
//! project file named them, so the host confirms them before running.

use super::support::{apply_run_changes, temporary_root, write_run_document, Cleanup};
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;

fn call(command: &str, payload: Value) -> Value {
    let reply: Value = serde_json::from_str(&execute_json(
        &json!({ "id": command, "command": command, "payload": payload }).to_string(),
    ))
    .unwrap();
    assert_eq!(reply["ok"], true, "{command}: {reply}");
    reply["data"].clone()
}

fn executable(id: &str, program: &str, steps: &[&str]) -> Value {
    let mut entry = json!({
        "id": id, "name": id.trim_start_matches("user:"), "provider": "process.executable",
        "confidence": "declared", "extensions": {"process": {"program": program}}
    });
    if !steps.is_empty() {
        entry["beforeLaunch"] = json!(steps
            .iter()
            .map(|step| json!({ "configuration": step }))
            .collect::<Vec<_>>());
    }
    entry
}

fn step_ids(reply: &Value) -> Vec<String> {
    reply["steps"]
        .as_array()
        .unwrap_or_else(|| panic!("no steps in {reply}"))
        .iter()
        .map(|step| step["configurationId"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn steps_expand_nested_runs_first_and_project_lists_need_confirmation() {
    let root = temporary_root("before-launch-expand");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(
        root.join("scripts/compile.sh"),
        "#!/bin/sh\necho compiled\n",
    )
    .unwrap();
    let project = ".lithe/run/configurations.yaml";
    let local = ".lithe/run/local.yaml";
    let compile = json!({
        "id": "user:compile", "name": "compile", "provider": "process.shell",
        "confidence": "declared", "args": ["--fast"],
        "extensions": {"process": {"script": "scripts/compile.sh"}}
    });
    write_run_document(
        &root,
        project,
        &json!({"version": 2, "configurations": [
            executable("user:app", "echo", &["user:gen"]), compile
        ]}),
    );
    write_run_document(
        &root,
        local,
        &json!({"version": 2, "configurations": [executable("user:gen", "echo", &["user:compile"])]}),
    );

    // The app starts gen, which starts compile first; a project file names the
    // app's list, so the user confirms it.
    let app = call(
        "runConfig.beforeLaunch",
        json!({"root": root, "configurationId": "user:app"}),
    );
    assert_eq!(app["status"], "ok", "{app}");
    assert_eq!(step_ids(&app), ["user:compile", "user:gen"]);
    assert_eq!(app["steps"][0]["name"], "compile");
    assert_eq!(app["steps"][0]["summary"], "scripts/compile.sh --fast");
    assert_eq!(app["steps"][1]["summary"], "echo");
    let fingerprint = app["confirmation"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(fingerprint.len(), 64);

    // gen's own list is this machine's, so it runs without asking.
    let gen = call(
        "runConfig.beforeLaunch",
        json!({"root": root, "configurationId": "user:gen"}),
    );
    assert_eq!(step_ids(&gen), ["user:compile"]);
    assert!(gen["confirmation"].is_null(), "{gen}");
    // A run that names no steps has none.
    let none = call(
        "runConfig.beforeLaunch",
        json!({"root": root, "configurationId": "user:compile"}),
    );
    assert_eq!(
        none,
        json!({"status": "ok", "steps": [], "confirmation": null})
    );

    // Changing what a step starts asks again.
    let mut changed: Value =
        serde_json::from_str(&fs::read_to_string(root.join(project)).unwrap()).unwrap();
    changed["configurations"][1]["args"] = json!(["--clean"]);
    write_run_document(&root, project, &changed);
    let again = call(
        "runConfig.beforeLaunch",
        json!({"root": root, "configurationId": "user:app"}),
    );
    assert_ne!(again["confirmation"]["fingerprint"], json!(fingerprint));

    // An empty local list hides the project's steps.
    let cleared = call(
        "runConfig.updateOptions",
        json!({"root": root, "scope": "local", "configurationId": "user:app", "beforeLaunch": []}),
    );
    apply_run_changes(&root, &cleared);
    let local_document: Value = serde_yaml_value(&fs::read_to_string(root.join(local)).unwrap());
    let entry = local_document["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "user:app")
        .cloned()
        .unwrap();
    assert_eq!(entry["beforeLaunch"], json!([]), "{local_document}");
    let app = call(
        "runConfig.beforeLaunch",
        json!({"root": root, "configurationId": "user:app"}),
    );
    assert_eq!(
        app,
        json!({"status": "ok", "steps": [], "confirmation": null})
    );

    // A list set on this machine runs without asking, even though it starts
    // runs a project file defines.
    let local_list = call(
        "runConfig.updateOptions",
        json!({"root": root, "scope": "local", "configurationId": "user:app", "beforeLaunch": ["user:compile"]}),
    );
    apply_run_changes(&root, &local_list);
    let app = call(
        "runConfig.beforeLaunch",
        json!({"root": root, "configurationId": "user:app"}),
    );
    assert_eq!(step_ids(&app), ["user:compile"]);
    assert!(app["confirmation"].is_null(), "{app}");
}

#[test]
fn steps_that_cannot_run_are_refused_by_code() {
    let root = temporary_root("before-launch-refused");
    let _cleanup = Cleanup(root.clone());
    let local = ".lithe/run/local.yaml";
    write_run_document(
        &root,
        local,
        &json!({"version": 2, "configurations": [
            executable("user:a", "echo", &["user:missing"]),
            executable("user:b", "echo", &["user:c"]),
            executable("user:c", "echo", &["user:b"]),
            executable("user:self", "echo", &["user:self"]),
        ]}),
    );
    let refusal = |id: &str| {
        let reply = call(
            "runConfig.beforeLaunch",
            json!({"root": root, "configurationId": id}),
        );
        assert_eq!(reply["status"], "rejected", "{reply}");
        reply["refusal"].clone()
    };
    assert_eq!(
        refusal("user:a"),
        json!({"code": "notFound", "configurationId": "user:missing"})
    );
    assert_eq!(
        refusal("user:b"),
        json!({"code": "cycle", "configurationId": "user:b"})
    );

    // A configuration that names itself is unavailable, and the list still resolves.
    let resolved = call("runConfig.resolve", json!({"root": root}));
    let configurations = resolved["configurations"].as_array().unwrap();
    assert_eq!(configurations.len(), 4, "{resolved}");
    let own = configurations
        .iter()
        .find(|entry| entry["id"] == "user:self")
        .unwrap();
    assert_eq!(own["availability"]["code"], "invalidConfiguration", "{own}");
    assert_eq!(refusal("user:a")["code"], "notFound");
    assert_eq!(
        call(
            "runConfig.beforeLaunch",
            json!({"root": root, "configurationId": "user:b"})
        )["refusal"]["code"],
        "cycle"
    );

    // updateOptions refuses a list that cannot run and writes nothing.
    let reply: Value = serde_json::from_str(&execute_json(
        &json!({"id": "u", "command": "runConfig.updateOptions", "payload": {
            "root": root, "scope": "local", "configurationId": "user:a", "beforeLaunch": ["user:a"]
        }})
        .to_string(),
    ))
    .unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"]["details"]
            .as_str()
            .unwrap()
            .contains("notAllowed"),
        "{reply}"
    );
}

#[test]
fn the_editor_form_lists_candidates_and_checks_the_steps_it_saves() {
    let root = temporary_root("before-launch-form");
    let _cleanup = Cleanup(root.clone());
    let local = ".lithe/run/local.yaml";
    write_run_document(
        &root,
        local,
        &json!({"version": 2, "configurations": [
            executable("user:prepare", "echo", &[]),
            executable("user:serve", "echo", &["user:prepare"]),
        ]}),
    );
    let form = call(
        "runConfig.editorForm",
        json!({"root": root, "configurationId": "user:prepare"}),
    );
    assert_eq!(form["values"]["beforeLaunch"], json!([]));
    let field = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["id"] == "beforeLaunch")
        .cloned()
        .unwrap();
    assert_eq!(field["kind"], "beforeLaunch");
    assert_eq!(
        field["candidates"],
        json!([{"id": "user:serve", "name": "serve"}])
    );
    let serve = call(
        "runConfig.editorForm",
        json!({"root": root, "configurationId": "user:serve"}),
    );
    assert_eq!(serve["values"]["beforeLaunch"], json!(["user:prepare"]));

    // prepare -> serve -> prepare would never start.
    let mut values = form["values"].as_object().unwrap().clone();
    values.insert("beforeLaunch".into(), json!(["user:serve"]));
    let cycle = call(
        "runConfig.saveConfiguration",
        json!({"root": root, "provider": "process.executable", "configurationId": "user:prepare", "values": values}),
    );
    assert_eq!(cycle["status"], "invalid", "{cycle}");
    assert_eq!(
        cycle["errors"],
        json!([{"field": "beforeLaunch", "code": "conflict"}])
    );

    // A new run may start an existing one first; the list is saved in its entry.
    let created = call(
        "runConfig.saveConfiguration",
        json!({"root": root, "provider": "process.executable", "values": {
            "name": "deploy", "program": "echo", "beforeLaunch": ["user:serve"]
        }}),
    );
    assert_eq!(created["status"], "saved", "{created}");
    apply_run_changes(&root, &created);
    let deploy = call(
        "runConfig.beforeLaunch",
        json!({"root": root, "configurationId": "user:deploy"}),
    );
    assert_eq!(step_ids(&deploy), ["user:prepare", "user:serve"]);
}

fn serde_yaml_value(text: &str) -> Value {
    serde_yaml_ng::from_str(text).expect("run documents are YAML")
}
