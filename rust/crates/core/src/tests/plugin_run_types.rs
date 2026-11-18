//! Run configuration types plugins declare in `buildSystems[].runConfigurationTypes`:
//! the dialog lists them with translated text, the editor draws and saves
//! their fields, and `build.runConfigurationPlan` expands a saved entry into
//! the argv plan the host starts.

use super::support::{apply_run_changes, changed_run_document, temporary_root, Cleanup};
use crate::execute_json;
use serde_json::{json, Value};
use std::collections::BTreeMap;
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
    contract_file!("fixtures/run-configuration-types/run-configuration-types.plugin.json");
const CHINESE: &str = contract_file!("fixtures/run-configuration-types/l10n/zh-Hans.json");
const PLUGIN: &str = "dev.lithe.fixture.run-configuration-types";

fn manifest() -> Value {
    serde_json::from_str(MANIFEST).unwrap()
}

fn call(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({ "id": command, "command": command, "payload": payload }).to_string(),
    ))
    .unwrap()
}

/// The `runTypePlugins` of a request with the fixture installed.
fn plugins() -> Value {
    json!({ "plugins": [{ "manifest": manifest() }] })
}

fn run_type(id: &str) -> Value {
    json!({ "pluginID": PLUGIN, "buildSystemID": "go", "type": id })
}

fn field<'a>(form: &'a Value, id: &str) -> &'a Value {
    form["data"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["id"] == id)
        .unwrap_or(&Value::Null)
}

fn plan(root: &std::path::Path, manifest: Value, request: Value) -> Value {
    call(
        "build.runConfigurationPlan",
        json!({
            "manifest": manifest,
            "buildSystemID": "go",
            "workspaceRoot": root,
            "request": request,
        }),
    )
}

#[test]
fn declared_types_follow_the_built_in_kinds_with_translated_text() {
    let plain = call(
        "runConfig.configurationTypes",
        json!({ "runTypePlugins": plugins() }),
    );
    assert_eq!(plain["ok"], true, "{plain}");
    let declared: Vec<&Value> = plain["data"]["types"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["provider"] == "build.run-type")
        .collect();
    assert_eq!(
        declared,
        [
            &json!({
                "provider": "build.run-type",
                "runType": run_type("build"),
                "displayName": "Go Build",
                "description": "Builds and runs a Go package, file or directory.",
                "execution": "application",
                "icon": { "host": "run", "mode": "template" },
            }),
            &json!({
                "provider": "build.run-type",
                "runType": run_type("test"),
                "displayName": "Go Test",
                "execution": "task",
            }),
        ]
    );
    // No language names no translation and reports nothing.
    assert!(plain["data"].get("localizations").is_none(), "{plain}");

    // A language first asks for the files, then applies the one read.
    let asked = call(
        "runConfig.configurationTypes",
        json!({ "runTypePlugins": { "language": "zh-Hans-CN", "plugins": [{ "manifest": manifest() }] } }),
    );
    assert_eq!(
        asked["data"]["localizations"],
        json!([{
            "pluginID": PLUGIN,
            "language": "zh-Hans-CN",
            "candidates": ["l10n/zh-Hans-CN.json", "l10n/zh-Hans.json", "l10n/zh.json"],
            "maximumBytes": 1048576,
            "readRequired": true,
            "applied": null,
            "rejected": [],
        }])
    );
    let translated = call(
        "runConfig.configurationTypes",
        json!({ "runTypePlugins": { "language": "zh-Hans-CN", "plugins": [{
            "manifest": manifest(),
            "localizations": { "l10n/zh-Hans.json": CHINESE },
        }] } }),
    );
    let report = &translated["data"]["localizations"][0];
    assert_eq!(report["applied"], "l10n/zh-Hans.json", "{report}");
    assert_eq!(report["readRequired"], false, "{report}");
    let names: Vec<&str> = translated["data"]["types"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["provider"] == "build.run-type")
        .map(|item| item["displayName"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Go 构建", "Go 测试"]);

    // A manifest that does not validate adds no type, as on every surface.
    let mut broken = manifest();
    broken["buildSystems"][0]["runConfigurationTypes"][0]["execution"] = json!("service");
    let skipped = call(
        "runConfig.configurationTypes",
        json!({ "runTypePlugins": { "plugins": [{ "manifest": broken }] } }),
    );
    assert_eq!(skipped["ok"], true, "{skipped}");
    assert!(!skipped["data"]["types"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["provider"] == "build.run-type"));

    // A malformed list is a host fault.
    let malformed = call(
        "runConfig.configurationTypes",
        json!({ "runTypePlugins": { "plugins": [{ "manifest": manifest(), "extra": 1 }] } }),
    );
    assert_eq!(malformed["ok"], false, "{malformed}");
    assert_eq!(malformed["error"]["code"], "invalid_request");
    let without_language = call(
        "runConfig.configurationTypes",
        json!({ "runTypePlugins": { "plugins": [{ "manifest": manifest(), "localizations": {} }] } }),
    );
    assert_eq!(without_language["ok"], false, "{without_language}");
}

/// A translation replaces only text the manifest has; a file naming
/// anything else is refused whole and the manifest's wording stays.
#[test]
fn a_translation_that_does_not_fit_the_declarations_is_rejected_whole() {
    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut base: Value = serde_json::from_str(CHINESE).unwrap();
    base["runConfigurationTypes"]["cargo"] = json!({});
    cases.push(("unknown build system", base));
    let mut base: Value = serde_json::from_str(CHINESE).unwrap();
    base["runConfigurationTypes"]["go"]["remote"] = json!({ "displayName": "远程" });
    cases.push(("unknown type", base));
    let mut base: Value = serde_json::from_str(CHINESE).unwrap();
    base["runConfigurationTypes"]["go"]["build"]["fields"]["missing"] = json!({ "title": "无" });
    cases.push(("unknown field", base));
    let mut base: Value = serde_json::from_str(CHINESE).unwrap();
    base["runConfigurationTypes"]["go"]["build"]["fields"]["runKind"]["options"]["module"] =
        json!("模块");
    cases.push(("unknown choice", base));
    let mut base: Value = serde_json::from_str(CHINESE).unwrap();
    base["runConfigurationTypes"]["go"]["test"]["description"] = json!("新增的说明");
    cases.push(("text the manifest lacks", base));
    let mut base: Value = serde_json::from_str(CHINESE).unwrap();
    base["runConfigurationTypes"]["go"]["build"]["fields"]["race"]["title"] = json!("");
    cases.push(("empty title", base));
    let mut base: Value = serde_json::from_str(CHINESE).unwrap();
    base["runConfigurationTypes"]["go"]["build"]["displayName"] = Value::Null;
    cases.push(("null text", base));

    for (name, file) in cases {
        let reply = call(
            "runConfig.configurationTypes",
            json!({ "runTypePlugins": { "language": "zh-Hans", "plugins": [{
                "manifest": manifest(),
                "localizations": { "l10n/zh-Hans.json": file.to_string() },
            }] } }),
        );
        let report = &reply["data"]["localizations"][0];
        assert_eq!(report["applied"], Value::Null, "{name}: {report}");
        assert_eq!(
            report["rejected"][0]["error"]["code"], "plugin.build.invalidLocalization",
            "{name}: {report}"
        );
        let build = reply["data"]["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["runType"]["type"] == "build")
            .unwrap();
        assert_eq!(build["displayName"], "Go Build", "{name}");
    }
}

#[test]
fn the_editor_draws_declared_fields_and_shows_program_arguments_only_when_taken() {
    let root = temporary_root("run-types-editor");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(&root).unwrap();

    let build = call(
        "runConfig.editorForm",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("build"),
            "runTypePlugins": { "language": "zh-Hans", "plugins": [{
                "manifest": manifest(),
                "localizations": { "l10n/zh-Hans.json": CHINESE },
            }] },
        }),
    );
    assert_eq!(build["ok"], true, "{build}");
    assert_eq!(build["data"]["runType"], run_type("build"));
    assert_eq!(
        build["data"]["localizations"][0]["applied"],
        "l10n/zh-Hans.json"
    );
    assert_eq!(field(&build, "name")["automatic"], "Go 构建");
    assert_eq!(
        field(&build, "field:runKind"),
        &json!({
            "id": "field:runKind", "kind": "choice", "label": "运行种类",
            "options": ["package", "file", "directory"],
            "optionLabels": { "package": "软件包", "file": "文件", "directory": "目录" },
        })
    );
    assert_eq!(
        field(&build, "field:package"),
        &json!({
            "id": "field:package", "kind": "text", "label": "软件包路径",
            "placeholder": "example.com/app/cmd/server", "required": true,
            "visibleWhen": { "field": "field:runKind", "equals": "package" },
        })
    );
    // Untranslated text keeps the manifest's wording.
    assert_eq!(field(&build, "field:output")["label"], "Output name");
    assert_eq!(
        field(&build, "field:goToolArguments")["description"],
        "放在软件包之前传给 go run。"
    );
    assert_eq!(build["data"]["values"]["field:runKind"], "package");
    assert_eq!(build["data"]["values"]["field:race"], false);
    assert_eq!(build["data"]["values"]["field:goToolArguments"], "");
    assert_eq!(field(&build, "programArguments")["kind"], "arguments");
    assert_eq!(field(&build, "allowMultipleInstances")["kind"], "checkbox");

    // Go Test names no ${userArguments}, so it has no program arguments.
    let test = call(
        "runConfig.editorForm",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("test"),
            "runTypePlugins": plugins(),
        }),
    );
    assert_eq!(test["ok"], true, "{test}");
    assert!(field(&test, "programArguments").is_null(), "{test}");
    assert_eq!(test["data"]["values"]["field:package"], "./...");

    // A type no installed plugin declares cannot be edited.
    let missing = call(
        "runConfig.editorForm",
        json!({ "root": root, "provider": "build.run-type", "runType": run_type("remote"),
            "runTypePlugins": plugins() }),
    );
    assert_eq!(missing["error"]["code"], "not_supported", "{missing}");
    let unnamed = call(
        "runConfig.editorForm",
        json!({ "root": root, "provider": "build.run-type", "runTypePlugins": plugins() }),
    );
    assert_eq!(unnamed["ok"], false, "{unnamed}");
}

#[test]
fn saved_declared_runs_expand_into_the_types_command_line() {
    let root = temporary_root("run-types-plan");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("services/api/cmd")).unwrap();
    fs::create_dir_all(root.join("tools")).unwrap();
    fs::write(root.join("services/api/cmd/main.go"), "package main\n").unwrap();
    // A declared run lives in its plugin's run document.
    let local = ".lithe/run/plugin/dev.lithe.fixture.run-configuration-types.local.toml";

    // Wrong values come back by field.
    let invalid = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("build"),
            "runTypePlugins": plugins(),
            "values": { "name": "api", "field:runKind": "file", "field:file": "missing.go" },
        }),
    );
    assert_eq!(invalid["data"]["status"], "invalid", "{invalid}");
    assert_eq!(
        invalid["data"]["errors"],
        json!([{ "field": "field:file", "code": "notFound" }])
    );
    let required = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("build"),
            "runTypePlugins": plugins(), "values": { "name": "api" },
        }),
    );
    assert_eq!(
        required["data"]["errors"],
        json!([{ "field": "field:package", "code": "required" }]),
        "{required}"
    );

    let saved = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("build"),
            "runTypePlugins": plugins(),
            "values": {
                "name": "api", "storeAsProjectFile": false, "allowMultipleInstances": true,
                "field:runKind": "package", "field:package": "./cmd",
                // Hidden by the run kind, so neither checked nor saved.
                "field:file": "missing.go",
                "field:goToolArguments": "-v -trimpath", "field:race": true, "field:output": "",
                "programArguments": "--port 8080",
                "workingDirectory": "services/api",
                "environment": [{ "name": "MODE", "value": "dev" }],
            },
        }),
    );
    assert_eq!(saved["data"]["status"], "saved", "{saved}");
    let id = saved["data"]["id"].as_str().unwrap().to_string();
    let document = changed_run_document(&saved["data"], local).unwrap_or_else(|| panic!("{saved}"));
    let entry = &document["configurations"][0];
    assert_eq!(entry["provider"], "build.run-type");
    assert_eq!(entry["allowMultipleInstances"], true);
    assert_eq!(entry["args"], json!(["--port", "8080"]));
    assert_eq!(
        entry["extensions"]["build"],
        json!({
            "pluginID": PLUGIN, "buildSystemID": "go", "type": "build",
            "values": {
                "runKind": "package", "package": "./cmd",
                "goToolArguments": ["-v", "-trimpath"], "race": true, "output": "",
            },
        })
    );
    apply_run_changes(&root, &saved["data"]);

    // The saved entry is listed and reopens with its values.
    let resolved = call("runConfig.resolve", json!({ "root": root }));
    let listed = resolved["data"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id.as_str())
        .unwrap();
    assert_eq!(listed["plugin"]["id"], PLUGIN, "{listed}");
    let reopened = call(
        "runConfig.editorForm",
        json!({ "root": root, "configurationId": id, "runTypePlugins": plugins() }),
    );
    assert_eq!(
        reopened["data"]["values"]["field:goToolArguments"],
        "-v -trimpath"
    );
    assert_eq!(
        reopened["data"]["values"]["programArguments"],
        "--port 8080"
    );
    assert_eq!(reopened["data"]["values"]["allowMultipleInstances"], true);
    // Without the plugin the entry cannot be edited.
    let removed = call(
        "runConfig.editorForm",
        json!({ "root": root, "configurationId": id }),
    );
    assert_eq!(removed["error"]["code"], "not_supported", "{removed}");

    // The plan runs with the deepest instance holding the working directory.
    let instances = json!([
        { "root": ".", "options": { "tags": "root" } },
        { "root": "services/api", "options": { "tags": "api" } },
        { "root": "tools" },
    ]);
    let planned = plan(
        &root,
        manifest(),
        json!({ "configurationId": id, "instances": instances }),
    );
    assert_eq!(planned["ok"], true, "{planned}");
    assert_eq!(planned["data"]["status"], "ok", "{planned}");
    let plan_value = &planned["data"]["plan"];
    assert_eq!(plan_value["root"], "services/api");
    assert_eq!(
        plan_value["arguments"],
        json!([
            "run",
            "-tags",
            "api",
            "-v",
            "-trimpath",
            "-race",
            "./cmd",
            "--port",
            "8080"
        ])
    );
    assert_eq!(plan_value["workingDirectory"], "services/api");
    assert_eq!(plan_value["environment"], json!({ "MODE": "dev" }));
    assert_eq!(plan_value["execution"], "application");
    assert_eq!(plan_value["runType"], run_type("build"));

    // Another plugin's manifest cannot plan the entry.
    let mut other = manifest();
    other["id"] = json!("dev.lithe.fixture.other-run-types");
    let foreign = plan(
        &root,
        other,
        json!({ "configurationId": id, "instances": instances }),
    );
    assert_eq!(foreign["data"]["status"], "rejected", "{foreign}");
    // A value edited by hand that the type does not offer is named by field.
    let path = root.join(local);
    let saved_text = fs::read_to_string(&path).unwrap();
    let edited = saved_text.replace("runKind = \"package\"", "runKind = \"module\"");
    assert_ne!(edited, saved_text, "{saved_text}");
    fs::write(&path, edited).unwrap();
    let wrong = plan(
        &root,
        manifest(),
        json!({ "configurationId": id, "instances": instances }),
    );
    assert_eq!(wrong["data"]["status"], "rejected", "{wrong}");
    assert_eq!(
        wrong["data"]["error"]["code"],
        "plugin.build.invalidRunConfiguration"
    );
    assert_eq!(wrong["data"]["error"]["field"], "values.runKind", "{wrong}");
    fs::write(&path, saved_text).unwrap();
    // A working directory in no instance needs a single instance.
    let outside = plan(
        &root,
        manifest(),
        json!({ "configurationId": id, "instances": [{ "root": "tools" }, { "root": "docs" }] }),
    );
    assert_eq!(outside["data"]["status"], "rejected", "{outside}");
    let only = plan(
        &root,
        manifest(),
        json!({ "configurationId": id, "instances": [{ "root": "tools" }] }),
    );
    assert_eq!(only["data"]["plan"]["root"], "tools", "{only}");
    // Unknown configurations and other providers fail the call.
    let unknown = plan(
        &root,
        manifest(),
        json!({ "configurationId": "user:none", "instances": instances }),
    );
    assert_eq!(unknown["ok"], false, "{unknown}");
    let without_root = call(
        "build.runConfigurationPlan",
        json!({ "manifest": manifest(), "buildSystemID": "go",
            "request": { "configurationId": id, "instances": instances } }),
    );
    assert_eq!(without_root["ok"], false, "{without_root}");

    // A file run passes the absolute file.
    let file_run = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("build"),
            "configurationId": id, "runTypePlugins": plugins(),
            "values": {
                "name": "api", "field:runKind": "file", "field:file": "services/api/cmd/main.go",
                "field:output": "bin/api",
            },
        }),
    );
    assert_eq!(file_run["data"]["status"], "saved", "{file_run}");
    apply_run_changes(&root, &file_run["data"]);
    let planned = plan(
        &root,
        manifest(),
        json!({ "configurationId": id, "instances": instances }),
    );
    let main = root.join("services/api/cmd/main.go");
    assert_eq!(
        planned["data"]["plan"]["arguments"],
        json!([
            "run",
            "-tags",
            "root",
            "-o",
            "bin/api",
            main.to_string_lossy()
        ]),
        "{planned}"
    );
    assert_eq!(planned["data"]["plan"]["root"], ".");
}

#[test]
fn a_type_without_program_arguments_stores_none() {
    let root = temporary_root("run-types-no-arguments");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(&root).unwrap();
    let saved = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("test"),
            "runTypePlugins": plugins(),
            "values": {
                "name": "unit", "storeAsProjectFile": true,
                "field:pattern": "TestParse", "programArguments": "--ignored",
            },
        }),
    );
    assert_eq!(saved["data"]["status"], "saved", "{saved}");
    let document = changed_run_document(
        &saved["data"],
        ".lithe/run/plugin/dev.lithe.fixture.run-configuration-types.toml",
    )
    .unwrap();
    let entry = &document["configurations"][0];
    assert!(entry.get("args").is_none(), "{entry}");
    assert!(entry.get("allowMultipleInstances").is_none(), "{entry}");
    assert_eq!(entry["execution"], "task");
    apply_run_changes(&root, &saved["data"]);
    let planned = plan(
        &root,
        manifest(),
        json!({ "configurationId": saved["data"]["id"], "instances": [{ "root": "." }] }),
    );
    assert_eq!(
        planned["data"]["plan"]["arguments"],
        json!(["test", "-run=TestParse", "./..."]),
        "{planned}"
    );
    assert_eq!(planned["data"]["plan"]["execution"], "task");
}

/// The manifest refuses declarations a host could not draw or run.
#[test]
fn run_configuration_type_declarations_are_validated() {
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
        .remove("runConfigurationTypes");
    let refused = validate(missing);
    assert_eq!(refused["status"], "rejected", "{refused}");

    let path = "/buildSystems/0/runConfigurationTypes/0";
    let manifest_error = "plugin.manifest.invalid";
    let template_error = "plugin.template.invalid";
    let cases: BTreeMap<&str, (&str, Value, &str)> = BTreeMap::from([
        ("service", ("/execution", json!("service"), manifest_error)),
        (
            "unknown field",
            ("/arguments/-", json!("${field:missing}"), template_error),
        ),
        (
            "field named twice",
            ("/arguments/-", json!("${field:race}"), template_error),
        ),
        (
            "program arguments twice",
            ("/arguments/-", json!("${userArguments}"), template_error),
        ),
        (
            "choice without choices",
            ("/fields/0/choices", json!([]), manifest_error),
        ),
        (
            "choices on a text field",
            (
                "/fields/1/choices",
                json!([{ "value": "a" }]),
                manifest_error,
            ),
        ),
        (
            "required checkbox",
            ("/fields/5/required", json!(true), manifest_error),
        ),
        (
            "file default",
            ("/fields/2/default", json!("main.go"), manifest_error),
        ),
        (
            "default outside the choices",
            ("/fields/0/default", json!("module"), manifest_error),
        ),
        (
            "later controller",
            (
                "/fields/1/visibleWhen",
                json!({ "field": "race", "equals": true }),
                manifest_error,
            ),
        ),
        (
            "text controller",
            (
                "/fields/6/visibleWhen",
                json!({ "field": "package", "equals": "x" }),
                manifest_error,
            ),
        ),
        (
            "controller value outside its choices",
            (
                "/fields/1/visibleWhen",
                json!({ "field": "runKind", "equals": "module" }),
                manifest_error,
            ),
        ),
        (
            "checkbox without arguments",
            ("/fields/5/arguments", json!([]), manifest_error),
        ),
        (
            "value never passed",
            ("/fields/6/arguments", json!(["-o"]), template_error),
        ),
        (
            "arguments field with templates",
            ("/fields/4/arguments", json!(["${value}"]), manifest_error),
        ),
        (
            "field ID with a dash",
            ("/fields/6/id", json!("out-name"), manifest_error),
        ),
        (
            "unsafe icon",
            ("/icon", json!({ "path": "../run.svg" }), manifest_error),
        ),
        ("bad type ID", ("/id", json!("Build Run"), manifest_error)),
    ]);
    for (name, (pointer, value, code)) in cases {
        let mut manifest = manifest();
        set(&mut manifest, &format!("{path}{pointer}"), value);
        let outcome = validate(manifest);
        assert_eq!(outcome["status"], "rejected", "{name}: {outcome}");
        assert_eq!(outcome["error"]["code"], code, "{name}: {outcome}");
    }

    // A field the command line never names must only show or hide others.
    let mut unnamed = manifest();
    unnamed["buildSystems"][0]["runConfigurationTypes"][0]["arguments"]
        .as_array_mut()
        .unwrap()
        .retain(|argument| argument != "${field:output}");
    assert_eq!(validate(unnamed)["status"], "rejected");
    // Duplicate type IDs.
    let mut duplicate = manifest();
    let types = duplicate["buildSystems"][0]["runConfigurationTypes"]
        .as_array_mut()
        .unwrap();
    types[1]["id"] = json!("build");
    assert_eq!(validate(duplicate)["status"], "rejected");
}

/// Sets the value at a JSON Pointer whose parent exists; `-` appends.
fn set(manifest: &mut Value, pointer: &str, value: Value) {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    match manifest.pointer_mut(parent) {
        Some(Value::Object(object)) => {
            object.insert(key.to_string(), value);
        }
        Some(Value::Array(items)) if key == "-" => items.push(value),
        Some(Value::Array(items)) => items[key.parse::<usize>().unwrap()] = value,
        _ => panic!("{parent} is not a container"),
    }
}
