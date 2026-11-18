//! Before-launch tasks plugins declare in `buildSystems[].beforeLaunchTasks`
//! (API-29): the `beforeLaunch` editor field offers them as kinds of step,
//! `runConfig.checkBeforeLaunchTask` checks the step's form,
//! `runConfig.beforeLaunch` expands them with the runs, and
//! `build.beforeLaunchTaskPlan` turns one into the argv plan the host starts.

use super::support::{apply_run_changes, temporary_root, write_run_document, Cleanup};
use crate::execute_json;
use serde_json::{json, Value};
use std::fs;

macro_rules! contract_file {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/",
            $path
        ))
    };
}

const MANIFEST: &str =
    contract_file!("fixtures/before-launch-tasks/before-launch-tasks.plugin.json");
const CHINESE: &str = contract_file!("fixtures/before-launch-tasks/l10n/zh-Hans.json");
const PLUGIN: &str = "dev.lithe.fixture.before-launch-tasks";

fn manifest() -> Value {
    serde_json::from_str(MANIFEST).unwrap()
}

/// The whole reply, `ok` or not.
fn reply(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({ "id": command, "command": command, "payload": payload }).to_string(),
    ))
    .unwrap()
}

/// The data of a reply that must succeed.
fn call(command: &str, payload: Value) -> Value {
    let reply = reply(command, payload);
    assert_eq!(reply["ok"], true, "{command}: {reply}");
    reply["data"].clone()
}

/// The `runTypePlugins` of a request with the fixture installed.
fn plugins() -> Value {
    json!({ "plugins": [{ "manifest": manifest() }] })
}

fn task(id: &str) -> Value {
    json!({ "pluginID": PLUGIN, "buildSystemID": "go", "type": id })
}

/// A saved task step of kind `id` with `values`.
fn task_step(id: &str, values: Value) -> Value {
    let mut reference = task(id);
    reference["values"] = values;
    json!({ "task": reference })
}

fn executable(id: &str, cwd: &str, steps: Vec<Value>) -> Value {
    let mut entry = json!({
        "id": id, "name": id.trim_start_matches("user:"), "provider": "process.executable",
        "confidence": "declared", "extensions": {"process": {"program": "echo"}}
    });
    if cwd != "." {
        entry["cwd"] = json!(cwd);
    }
    if !steps.is_empty() {
        entry["beforeLaunch"] = json!(steps);
    }
    entry
}

fn before_launch_field(form: &Value) -> Value {
    form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["id"] == "beforeLaunch")
        .cloned()
        .unwrap_or_else(|| panic!("no beforeLaunch field in {form}"))
}

fn plan(root: &std::path::Path, request: Value) -> Value {
    reply(
        "build.beforeLaunchTaskPlan",
        json!({
            "manifest": manifest(),
            "buildSystemID": "go",
            "workspaceRoot": root,
            "request": request,
        }),
    )
}

#[test]
fn task_kinds_are_offered_on_the_step_field_and_not_as_configurations() {
    let root = temporary_root("before-launch-task-kinds");
    let _cleanup = Cleanup(root.clone());
    write_run_document(
        &root,
        ".lithe/run/local.yaml",
        &json!({"version": 2, "configurations": [executable("user:app", ".", vec![])]}),
    );

    // A task is a step, not a configuration the user creates.
    let types = call(
        "runConfig.configurationTypes",
        json!({ "runTypePlugins": plugins() }),
    );
    assert!(
        types["types"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["provider"] != "build.run-type"),
        "{types}"
    );

    let form = call(
        "runConfig.editorForm",
        json!({
            "root": root, "configurationId": "user:app",
            "runTypePlugins": { "language": "zh-Hans", "plugins": [{
                "manifest": manifest(),
                "localizations": { "l10n/zh-Hans.json": CHINESE },
            }] },
        }),
    );
    assert_eq!(form["localizations"][0]["applied"], "l10n/zh-Hans.json");
    let field = before_launch_field(&form);
    let tasks = field["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "{field}");
    assert_eq!(tasks[0]["runType"], task("command"));
    assert_eq!(tasks[0]["displayName"], "运行 Go 命令");
    assert_eq!(
        tasks[0]["description"],
        "在配置的工作目录中运行一条 go 命令，例如 generate ./...。"
    );
    assert_eq!(tasks[0]["icon"]["host"], "run");
    let command = &tasks[0]["fields"][0];
    assert_eq!(command["id"], "field:command", "{command}");
    assert_eq!(command["kind"], "arguments");
    assert_eq!(command["label"], "命令");
    assert_eq!(command["required"], true);
    assert_eq!(tasks[0]["values"], json!({ "field:command": "" }));
    assert_eq!(tasks[1]["displayName"], "Go 生成");
    assert_eq!(
        tasks[1]["values"],
        json!({ "field:verbose": false, "field:package": "./..." })
    );

    // Without a plugin that declares tasks the field offers runs only.
    let plain = call(
        "runConfig.editorForm",
        json!({ "root": root, "configurationId": "user:app" }),
    );
    assert!(
        before_launch_field(&plain).get("tasks").is_none(),
        "{plain}"
    );
}

#[test]
fn task_steps_are_checked_saved_and_reopened() {
    let root = temporary_root("before-launch-task-save");
    let _cleanup = Cleanup(root.clone());
    let local = ".lithe/run/local.yaml";
    write_run_document(
        &root,
        local,
        &json!({"version": 2, "configurations": [
            executable("user:app", ".", vec![]),
            executable("user:gen", ".", vec![]),
        ]}),
    );

    // The step's form is checked by field.
    let check = |run_type: Value, values: Value| {
        call(
            "runConfig.checkBeforeLaunchTask",
            json!({ "root": root, "runType": run_type, "values": values, "runTypePlugins": plugins() }),
        )
    };
    let empty = check(task("command"), json!({ "field:command": "  " }));
    assert_eq!(
        empty,
        json!({ "status": "invalid", "errors": [{ "field": "field:command", "code": "required" }] })
    );
    let checked = check(
        task("command"),
        json!({ "field:command": "generate ./..." }),
    );
    assert_eq!(checked["status"], "ok", "{checked}");
    let step = checked["step"].clone();
    assert_eq!(
        step,
        json!({
            "task": {
                "pluginID": PLUGIN, "buildSystemID": "go", "type": "command",
                "values": { "command": ["generate", "./..."] },
            },
            "name": "Run Go Command",
            "summary": "generate ./...",
            "values": { "field:command": "generate ./..." },
        })
    );
    let generate = check(task("generate"), json!({ "field:verbose": true }));
    assert_eq!(
        generate["step"]["summary"], "Print files ./...",
        "{generate}"
    );
    // A task no installed plugin declares fails the call.
    let unknown = reply(
        "runConfig.checkBeforeLaunchTask",
        json!({ "root": root, "runType": task("vet"), "runTypePlugins": plugins() }),
    );
    assert_eq!(unknown["error"]["code"], "not_supported", "{unknown}");

    // The editor saves the step as the reference it names.
    let form = call(
        "runConfig.editorForm",
        json!({ "root": root, "configurationId": "user:app", "runTypePlugins": plugins() }),
    );
    let mut values = form["values"].as_object().unwrap().clone();
    values.insert("beforeLaunch".into(), json!([step, "user:gen"]));
    let saved = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root, "provider": "process.executable", "configurationId": "user:app",
            "values": values, "runTypePlugins": plugins(),
        }),
    );
    assert_eq!(saved["status"], "saved", "{saved}");
    apply_run_changes(&root, &saved);
    let document = serde_yaml_value(&fs::read_to_string(root.join(local)).unwrap());
    let entry = document["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "user:app")
        .cloned()
        .unwrap();
    assert_eq!(
        entry["beforeLaunch"],
        json!([
            task_step("command", json!({ "command": ["generate", "./..."] })),
            { "configuration": "user:gen" },
        ]),
        "{document}"
    );

    // It reopens with its form values, and without the plugin as a step the
    // editor keeps but cannot describe.
    let reopened = call(
        "runConfig.editorForm",
        json!({ "root": root, "configurationId": "user:app", "runTypePlugins": plugins() }),
    );
    assert_eq!(
        reopened["values"]["beforeLaunch"],
        json!([step, "user:gen"])
    );
    let removed = call(
        "runConfig.editorForm",
        json!({ "root": root, "configurationId": "user:app" }),
    );
    assert_eq!(
        removed["values"]["beforeLaunch"][0],
        json!({
            "task": step["task"], "name": "command", "summary": "", "missing": true,
        })
    );
    // Saving it without the plugin names why the list cannot run.
    let mut values = removed["values"].as_object().unwrap().clone();
    values.insert("name".into(), json!("app"));
    let refused = call(
        "runConfig.saveConfiguration",
        json!({ "root": root, "provider": "process.executable", "configurationId": "user:app", "values": values }),
    );
    assert_eq!(
        refused,
        json!({ "status": "invalid", "errors": [{ "field": "beforeLaunch", "code": "taskNotDeclared" }] })
    );

    // updateOptions checks a task step against the installed plugins too.
    let update = |plugins: Option<Value>| {
        let mut payload = json!({
            "root": root, "scope": "local", "configurationId": "user:gen",
            "beforeLaunch": [task_step("generate", json!({}))],
        });
        if let Some(plugins) = plugins {
            payload["runTypePlugins"] = plugins;
        }
        reply("runConfig.updateOptions", payload)
    };
    let missing = update(None);
    assert_eq!(missing["ok"], false, "{missing}");
    assert!(
        missing["error"]["details"]
            .as_str()
            .unwrap()
            .contains("taskNotDeclared"),
        "{missing}"
    );
    let updated = update(Some(plugins()));
    assert_eq!(updated["ok"], true, "{updated}");
    apply_run_changes(&root, &updated["data"]);
    let gen = call(
        "runConfig.beforeLaunch",
        json!({ "root": root, "configurationId": "user:gen", "runTypePlugins": plugins() }),
    );
    assert_eq!(gen["steps"][0]["task"]["type"], "generate", "{gen}");
}

#[test]
fn task_steps_run_in_the_owning_configuration_directory() {
    let root = temporary_root("before-launch-task-expand");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("services/api")).unwrap();
    fs::create_dir_all(root.join("tools")).unwrap();
    let project = ".lithe/run/configurations.yaml";
    let generate = task_step(
        "generate",
        json!({ "verbose": true, "package": "./internal/..." }),
    );
    let document = json!({"version": 2, "configurations": [
        executable("user:app", "services/api", vec![generate.clone(), json!({ "configuration": "user:gen" })]),
        executable("user:gen", "tools", vec![task_step("command", json!({ "command": ["mod", "tidy"] }))]),
    ]});
    write_run_document(&root, project, &document);

    // gen's own steps run first, each task in the directory of the
    // configuration whose list names it.
    let expand = |plugins: Value| {
        call(
            "runConfig.beforeLaunch",
            json!({ "root": root, "configurationId": "user:app", "runTypePlugins": plugins }),
        )
    };
    let app = expand(plugins());
    assert_eq!(app["status"], "ok", "{app}");
    assert_eq!(
        app["steps"],
        json!([
            {
                "task": generate["task"], "name": "Go Generate",
                "summary": "Print files ./internal/...", "workingDirectory": "services/api",
            },
            {
                "task": {
                    "pluginID": PLUGIN, "buildSystemID": "go", "type": "command",
                    "values": { "command": ["mod", "tidy"] },
                },
                "name": "Run Go Command", "summary": "mod tidy", "workingDirectory": "tools",
            },
            { "configurationId": "user:gen", "name": "gen", "summary": "echo" },
        ])
    );
    // A project file names the lists, so the user confirms them; changing a
    // task's values asks again.
    let fingerprint = app["confirmation"]["fingerprint"].clone();
    assert_eq!(fingerprint.as_str().unwrap().len(), 64);
    let mut changed = document.clone();
    changed["configurations"][0]["beforeLaunch"][0]["task"]["values"]["verbose"] = json!(false);
    write_run_document(&root, project, &changed);
    assert_ne!(
        expand(plugins())["confirmation"]["fingerprint"],
        fingerprint
    );

    // A task no installed plugin declares, or whose values no longer fit it,
    // is refused by the configuration whose list names it.
    let refused = expand(json!({ "plugins": [] }));
    assert_eq!(
        refused,
        json!({ "status": "rejected", "refusal": {
            "code": "taskNotDeclared", "configurationId": "user:app", "task": task("generate"),
        } })
    );
    let mut emptied = document.clone();
    emptied["configurations"][1]["beforeLaunch"][0]["task"]["values"] = json!({});
    write_run_document(&root, project, &emptied);
    assert_eq!(
        expand(plugins())["refusal"],
        json!({ "code": "invalidTaskValues", "configurationId": "user:gen", "task": task("command") })
    );
    write_run_document(&root, project, &document);

    // The host plans a step as runConfig.beforeLaunch returned it: the task's
    // command line with the options of the instance holding its directory.
    let instances = json!([
        { "root": ".", "options": { "tags": "root" } },
        { "root": "services/api", "options": { "tags": "api" } },
    ]);
    let planned = plan(
        &root,
        json!({ "task": app["steps"][0]["task"], "workingDirectory": "services/api", "instances": instances }),
    );
    assert_eq!(planned["data"]["status"], "ok", "{planned}");
    let value = &planned["data"]["plan"];
    assert_eq!(
        value["arguments"],
        json!(["generate", "-tags", "api", "-v", "./internal/..."]),
        "{value}"
    );
    assert_eq!(value["root"], "services/api");
    assert_eq!(value["workingDirectory"], "services/api");
    assert_eq!(value["execution"], "task");
    assert_eq!(value["beforeLaunchTask"], task("generate"));
    assert!(value.get("runType").is_none(), "{value}");
    let tools = plan(
        &root,
        json!({ "task": app["steps"][1]["task"], "workingDirectory": "tools", "instances": instances }),
    );
    assert_eq!(
        tools["data"]["plan"]["arguments"],
        json!(["mod", "tidy"]),
        "{tools}"
    );
    assert_eq!(tools["data"]["plan"]["root"], ".");
    // Values that do not fit are named by field; a directory outside the
    // project, or a task of another plugin, fails.
    let empty = plan(
        &root,
        json!({ "task": task_step("command", json!({}))["task"], "workingDirectory": ".", "instances": instances }),
    );
    assert_eq!(empty["data"]["status"], "rejected", "{empty}");
    assert_eq!(
        empty["data"]["error"]["code"],
        "plugin.build.invalidBeforeLaunchTask"
    );
    assert_eq!(empty["data"]["error"]["field"], "values.command");
    let outside = plan(
        &root,
        json!({ "task": app["steps"][1]["task"], "workingDirectory": "../elsewhere", "instances": instances }),
    );
    assert_eq!(outside["ok"], false, "{outside}");
    let mut foreign = app["steps"][1]["task"].clone();
    foreign["pluginID"] = json!("dev.lithe.fixture.other");
    let other = plan(
        &root,
        json!({ "task": foreign, "workingDirectory": "tools", "instances": instances }),
    );
    assert_eq!(other["data"]["status"], "rejected", "{other}");
}

#[test]
fn before_launch_task_declarations_are_validated() {
    let validate = |manifest: Value| {
        lithe_plugin_manifest::validate_command(json!({ "manifest": manifest })).unwrap()
    };
    assert_eq!(
        validate(manifest())["status"],
        "ok",
        "{}",
        validate(manifest())
    );

    let mut missing = manifest();
    missing["requires"]
        .as_object_mut()
        .unwrap()
        .remove("beforeLaunchTasks");
    assert_eq!(validate(missing)["status"], "rejected");

    let tasks = "/buildSystems/0/beforeLaunchTasks";
    let cases = [
        // A task always runs to completion.
        ("/0/execution", json!("task"), "plugin.manifest.invalid"),
        // It has no program arguments to pass.
        (
            "/0/arguments/-",
            json!("${userArguments}"),
            "plugin.template.invalid",
        ),
        ("/1/id", json!("command"), "plugin.manifest.invalid"),
        (
            "/0/fields/0/id",
            json!("go-command"),
            "plugin.manifest.invalid",
        ),
    ];
    for (pointer, value, code) in cases {
        let mut manifest = manifest();
        let (parent, key) = format!("{tasks}{pointer}")
            .rsplit_once('/')
            .map(|(parent, key)| (parent.to_string(), key.to_string()))
            .unwrap();
        match manifest.pointer_mut(&parent).unwrap() {
            Value::Object(object) => {
                object.insert(key, value);
            }
            Value::Array(items) => items.push(value),
            _ => unreachable!(),
        }
        let outcome = validate(manifest);
        assert_eq!(outcome["status"], "rejected", "{pointer}: {outcome}");
        assert_eq!(outcome["error"]["code"], code, "{pointer}: {outcome}");
    }
}

fn serde_yaml_value(text: &str) -> Value {
    serde_yaml_ng::from_str(text).expect("run documents are YAML")
}
