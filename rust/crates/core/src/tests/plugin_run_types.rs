//! Run configuration types plugins declare in `buildSystems[].runConfigurationTypes`:
//! the dialog lists them with translated text, the editor draws and saves
//! their fields, and `build.runConfigurationPlan` expands a saved entry into
//! the argv plan the host starts.

use super::support::{
    apply_run_changes, changed_run_document, changed_run_paths, edit_plugin_run_document,
    temporary_root, Cleanup,
};
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
            "request": super::support::trusted(request),
        }),
    )
}

fn save_file_run(root: &std::path::Path, name: &str, directory: &str) -> Value {
    let saved = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root, "provider": "build.run-type", "runType": run_type("build"),
            "runTypePlugins": plugins(),
            "values": {
                "name": name, "workingDirectory": directory,
                "field:runKind": "file", "field:file": "apps/worker/main.go",
            },
        }),
    );
    assert_eq!(saved["data"]["status"], "saved", "{saved}");
    apply_run_changes(root, &saved["data"]);
    saved["data"].clone()
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

/// A file run at the workspace root can resolve the one project that contains
/// that file. An explicit working directory still chooses its own instance.
#[test]
fn file_run_uses_its_project_when_the_workspace_root_has_no_instance() {
    let root = temporary_root("run-types-file-project");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("apps/worker")).unwrap();
    fs::create_dir_all(root.join("tools")).unwrap();
    fs::write(root.join("apps/worker/main.go"), "package main\n").unwrap();

    for (name, directory, expected_instance) in [
        ("default", "", "apps/worker"),
        ("explicit", "tools", "tools"),
    ] {
        let saved = save_file_run(&root, name, directory);
        let planned = plan(
            &root,
            manifest(),
            json!({
                "configurationId": saved["id"],
                "instances": [{"root": "apps"}, {"root": "apps/worker"}, {"root": "tools"}],
            }),
        );
        assert_eq!(planned["data"]["status"], "ok", "{planned}");
        assert_eq!(planned["data"]["plan"]["root"], expected_instance);
        let expected_directory = if directory.is_empty() { "." } else { directory };
        assert_eq!(
            planned["data"]["plan"]["workingDirectory"],
            expected_directory
        );
    }
}

#[test]
fn file_run_accepts_an_equivalent_saved_workspace_root() {
    let root = temporary_root("run-types-root-alias");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("apps/worker")).unwrap();
    fs::create_dir_all(root.join("tools")).unwrap();
    fs::write(root.join("apps/worker/main.go"), "package main\n").unwrap();
    let saved = save_file_run(&root, "root alias", "");
    let plugin_document = changed_run_paths(&saved)
        .into_iter()
        .find(|path| path.ends_with(".toml"))
        .expect("a declared run is saved in a plugin TOML document");
    edit_plugin_run_document(&root, &plugin_document, |document| {
        document["configurations"][0]["cwd"] = json!("./");
    });

    let planned = plan(
        &root,
        manifest(),
        json!({
            "configurationId": saved["id"],
            "instances": [{"root": "apps/worker"}, {"root": "tools"}],
        }),
    );
    assert_eq!(planned["data"]["status"], "ok", "{planned}");
    assert_eq!(planned["data"]["plan"]["root"], "apps/worker");
    assert_eq!(planned["data"]["plan"]["workingDirectory"], "./");
}

#[cfg(unix)]
fn plan_file_run_after_script_becomes_link(target: &str) -> Value {
    let fixture = temporary_root("run-types-linked-file");
    let _cleanup = Cleanup(fixture.clone());
    let root = fixture.join("workspace");
    fs::create_dir_all(root.join("apps/worker")).unwrap();
    fs::create_dir_all(root.join("tools")).unwrap();
    let script = root.join("apps/worker/main.go");
    fs::write(&script, "package main\n").unwrap();
    let target = fixture.join(target);
    fs::write(&target, "package main\n").unwrap();
    let saved = save_file_run(&root, "linked script", "");
    fs::remove_file(&script).unwrap();
    std::os::unix::fs::symlink(target, script).unwrap();

    plan(
        &root,
        manifest(),
        json!({
            "configurationId": saved["id"],
            "instances": [
                {"root": "apps/worker", "options": {"tags": "worker"}},
                {"root": "tools", "options": {"tags": "tools"}},
            ],
        }),
    )
}

#[cfg(unix)]
#[test]
fn file_run_selects_the_project_of_its_link_target() {
    let planned = plan_file_run_after_script_becomes_link("workspace/tools/main.go");
    assert_eq!(planned["data"]["status"], "ok", "{planned}");
    assert_eq!(planned["data"]["plan"]["root"], "tools");
    assert_eq!(planned["data"]["plan"]["workingDirectory"], ".");
    assert_eq!(planned["data"]["plan"]["arguments"][1], "-tags");
    assert_eq!(planned["data"]["plan"]["arguments"][2], "tools");
}

#[cfg(unix)]
#[test]
fn file_run_cannot_infer_a_project_from_a_link_to_an_external_file() {
    let planned = plan_file_run_after_script_becomes_link("outside.go");
    assert_eq!(planned["data"]["status"], "rejected", "{planned}");
    assert_eq!(
        planned["data"]["error"]["code"],
        "plugin.build.invalidSelection"
    );
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

fn generated_run_type_manifest() -> Value {
    json!({
        "schemaVersion": 0,
        "id": "dev.lithe.test.generated-run-types",
        "displayName": "Synthetic Task Tool",
        "version": "1.0.0",
        "pluginAPIVersion": 0,
        "requires": {
            "buildSystems": 0,
            "runConfigurationTypes": 1
        },
        "vendor": {
            "id": "dev.lithe.test",
            "displayName": "Synthetic",
            "signatureRequirement": "publisherPackage"
        },
        "permissions": ["process.build"],
        "activationEvents": ["onBuildSystem:tasker"],
        "buildSystems": [{
            "id": "tasker",
            "displayName": "Tasker",
            "detection": { "markers": ["Taskfile"] },
            "invocation": {
                "executables": [{ "kind": "command", "command": "tasker" }]
            },
            "model": {
                "source": "command",
                "arguments": ["--dump"],
                "output": "stdout",
                "decoder": "litheBuildModel",
                "executesProjectCode": false,
                "mayUseNetwork": false
            },
            "runConfigurationTypes": [{
                "id": "target",
                "displayName": "Tasker Target",
                "execution": "task",
                "fields": [
                    {
                        "id": "targets",
                        "title": "Targets",
                        "kind": "arguments",
                        "required": true
                    },
                    {
                        "id": "profile",
                        "title": "Profile",
                        "kind": "choice",
                        "choices": [
                            { "value": "default", "title": "Default" },
                            { "value": "release", "title": "Release" }
                        ],
                        "arguments": ["--profile", "${value}"]
                    },
                    {
                        "id": "flags",
                        "title": "Flags",
                        "kind": "text",
                        "arguments": ["--flag", "${value}"]
                    }
                ],
                "arguments": ["${field:profile}", "${field:flags}", "${field:targets}", "${userArguments}"]
            }],
            "run": [{
                "id": "target",
                "fromTargets": { "tags": ["target"] },
                "configurationType": "target",
                "values": {
                    "targets": ["${target.name}"],
                    "profile": "release",
                    "flags": "${target.property:defaultFlag}"
                }
            }]
        }]
    })
}

#[test]
fn run_configuration_types_v1_validates_run_declarations() {
    let validate = |manifest: Value| {
        call(
            "plugins.declarative.validate",
            json!({ "manifest": manifest }),
        )["data"]
            .clone()
    };

    let valid = validate(generated_run_type_manifest());
    assert_eq!(valid["status"], "ok", "{valid}");

    // Version 0 refuses configurationType on buildSystems[].run[].
    let mut requires_v0 = generated_run_type_manifest();
    requires_v0["requires"]["runConfigurationTypes"] = json!(0);
    let outcome = validate(requires_v0);
    assert_eq!(outcome["status"], "rejected", "{outcome}");
    assert_eq!(outcome["error"]["code"], "plugin.manifest.invalid");

    // Unknown configurationType, invalid field/type/choice, missing required, forbidden combos.
    let cases = [
        (
            "unknown configurationType",
            "/buildSystems/0/run/0/configurationType",
            json!("missing"),
        ),
        (
            "unknown field in values",
            "/buildSystems/0/run/0/values/unknown",
            json!("x"),
        ),
        (
            "mismatched type for arguments field",
            "/buildSystems/0/run/0/values/targets",
            json!("build"),
        ),
        (
            "unknown choice literal",
            "/buildSystems/0/run/0/values/profile",
            json!("nightly"),
        ),
        (
            "missing required field value",
            "/buildSystems/0/run/0/values",
            json!({ "profile": "default" }),
        ),
        (
            "forbidden provider source",
            "/buildSystems/0/run/0/source",
            json!("provider"),
        ),
        (
            "forbidden execution override",
            "/buildSystems/0/run/0/execution",
            json!("application"),
        ),
        (
            "forbidden raw arguments",
            "/buildSystems/0/run/0/arguments",
            json!(["run"]),
        ),
    ];
    for (name, pointer, value) in cases {
        let mut bad = generated_run_type_manifest();
        set(&mut bad, pointer, value);
        let outcome = validate(bad);
        assert_eq!(outcome["status"], "rejected", "{name}: {outcome}");
        assert_eq!(
            outcome["error"]["code"], "plugin.manifest.invalid",
            "{name}: {outcome}"
        );
    }

    // `values` without `configurationType` is rejected.
    let mut values_without_type = generated_run_type_manifest();
    let run = values_without_type["buildSystems"][0]["run"][0]
        .as_object_mut()
        .unwrap();
    run.remove("configurationType");
    run.insert("arguments".into(), json!(["${target.name}"]));
    let outcome = validate(values_without_type);
    assert_eq!(outcome["status"], "rejected", "{outcome}");
    assert_eq!(outcome["error"]["code"], "plugin.manifest.invalid");
}

#[test]
fn generated_run_type_decodes_generates_edits_launches_and_copies() {
    let root = temporary_root("generated-run-type-flow");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub/Taskfile"), "build:\n\techo hi\n").unwrap();

    let manifest = generated_run_type_manifest();
    let run_type_plugins = json!({ "plugins": [{ "manifest": manifest }] });

    // 1. Decode model: Core expands templates into `model.run[].runType`.
    let raw_model = json!({
        "format": "litheBuildModel",
        "version": 0,
        "projects": [{
            "id": "sub",
            "name": "sub",
            "path": "."
        }],
        "targets": [{
            "id": "sub:build",
            "projectID": "sub",
            "name": "build",
            "path": ".",
            "tags": ["target"],
            "properties": { "defaultFlag": "fast" }
        }]
    })
    .to_string();
    let decoded = call(
        "build.decodeModel",
        json!({
            "manifest": manifest,
            "buildSystemID": "tasker",
            "request": {
                "workspaceRoot": root,
                "root": "sub",
                "output": raw_model,
            },
        }),
    );
    assert_eq!(decoded["ok"], true, "{decoded}");
    let run_entries = decoded["data"]["model"]["run"].as_array().unwrap();
    assert_eq!(run_entries.len(), 1, "{decoded}");
    let entry = &run_entries[0];
    assert_eq!(entry["provider"], "build.run-type");
    assert_eq!(entry["name"], "build");
    assert_eq!(entry["cwd"], "sub");
    assert_eq!(entry["execution"], "task");
    assert_eq!(
        entry["runType"],
        json!({
            "pluginID": "dev.lithe.test.generated-run-types",
            "buildSystemID": "tasker",
            "type": "target",
            "values": {
                "targets": ["build"],
                "profile": "release",
                "flags": "fast"
            }
        })
    );

    // 2. Generate and resolve: materializes a `generated:run-type:*` configuration.
    let snapshot = json!([{
        "projectPath": "sub",
        "name": entry["name"],
        "cwd": entry["cwd"],
        "execution": entry["execution"],
        "selector": entry["id"],
        "runType": entry["runType"],
    }]);
    let generated = call(
        "runConfig.generate",
        json!({ "root": root, "providerRuns": snapshot }),
    );
    assert_eq!(generated["ok"], true, "{generated}");
    let gen_id = generated["data"]["generated"]["configurations"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(gen_id.starts_with("generated:run-type:"), "{gen_id}");

    let resolved = call("runConfig.resolve", json!({ "root": root }));
    assert_eq!(resolved["ok"], true, "{resolved}");
    let config = resolved["data"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == gen_id)
        .expect("generated run-type configuration resolves");
    assert_eq!(config["provider"], "build.run-type");
    assert_eq!(config["cwd"], "sub");
    assert_eq!(config["plugin"]["id"], "dev.lithe.test.generated-run-types");
    assert!(config["availability"].is_null());

    // 3. Plan launch directly from the generated configuration.
    let initial_plan = call(
        "build.runConfigurationPlan",
        json!({
            "manifest": manifest,
            "buildSystemID": "tasker",
            "workspaceRoot": root,
            "request": super::support::trusted(json!({
                "configurationId": gen_id,
                "instances": [{ "root": "sub" }],
            })),
        }),
    );
    assert_eq!(initial_plan["ok"], true, "{initial_plan}");
    assert_eq!(initial_plan["data"]["status"], "ok", "{initial_plan}");
    assert_eq!(
        initial_plan["data"]["plan"]["arguments"],
        json!(["--profile", "release", "--flag", "fast", "build"])
    );
    assert_eq!(initial_plan["data"]["plan"]["workingDirectory"], "sub");

    // 4. Open editor form on the generated entry, clear optional `flags`, and
    // reset `workingDirectory` from `sub` back to `.` — neither generated value
    // may leak back after merge.
    let form = call(
        "runConfig.editorForm",
        json!({
            "root": root,
            "configurationId": gen_id,
            "runTypePlugins": run_type_plugins,
        }),
    );
    assert_eq!(form["ok"], true, "{form}");
    assert_eq!(form["data"]["values"]["field:targets"], "build");
    assert_eq!(form["data"]["values"]["field:flags"], "fast");
    assert_eq!(form["data"]["values"]["workingDirectory"], "sub");

    let saved = call(
        "runConfig.saveConfiguration",
        json!({
            "root": root,
            "provider": "build.run-type",
            "runType": {
                "pluginID": "dev.lithe.test.generated-run-types",
                "buildSystemID": "tasker",
                "type": "target"
            },
            "configurationId": gen_id,
            "runTypePlugins": run_type_plugins,
            "values": {
                "name": "build-custom",
                "workingDirectory": ".",
                "field:targets": "build test",
                "field:profile": "default",
                "field:flags": "",
                "programArguments": "--verbose",
            },
        }),
    );
    assert_eq!(saved["data"]["status"], "saved", "{saved}");
    assert_eq!(saved["data"]["id"], gen_id);
    assert_eq!(
        changed_run_paths(&saved["data"]),
        vec![".lithe/run/plugin/dev.lithe.test.generated-run-types.local.toml"]
    );
    apply_run_changes(&root, &saved["data"]);

    let edited_plan = call(
        "build.runConfigurationPlan",
        json!({
            "manifest": manifest,
            "buildSystemID": "tasker",
            "workspaceRoot": root,
            "request": super::support::trusted(json!({
                "configurationId": gen_id,
                "instances": [{ "root": "sub" }],
            })),
        }),
    );
    assert_eq!(edited_plan["ok"], true, "{edited_plan}");
    assert_eq!(edited_plan["data"]["status"], "ok", "{edited_plan}");
    assert_eq!(
        edited_plan["data"]["plan"]["arguments"],
        json!(["--profile", "default", "build", "test", "--verbose"])
    );
    assert_eq!(edited_plan["data"]["plan"]["workingDirectory"], ".");

    // 5. Copy the generated configuration into a standalone user configuration.
    let copied = call(
        "runConfig.copyConfiguration",
        json!({
            "root": root,
            "configurationId": gen_id,
            "name": "My Build Copy",
        }),
    );
    assert_eq!(copied["ok"], true, "{copied}");
    let copy_id = copied["data"]["id"].as_str().unwrap();
    assert!(copy_id.starts_with("user:"), "{copy_id}");
    apply_run_changes(&root, &copied["data"]);

    let after_copy = call("runConfig.resolve", json!({ "root": root }));
    let ids: Vec<&str> = after_copy["data"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert!(ids.contains(&gen_id.as_str()), "{ids:?}");
    assert!(ids.contains(&copy_id), "{ids:?}");
}
