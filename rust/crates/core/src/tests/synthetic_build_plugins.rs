//! Build command integration using an inline, fictional plugin and documents.
//! Real build tools and their captured output belong to the plugin repositories;
//! these tests preserve Core's validation, planning and decoding boundaries.

use super::support::temporary_root;
use crate::build::{
    build_decode_dependencies_command, build_decode_diagnostics_command,
    build_decode_model_command, build_dependency_plan_command, build_detect_command,
    build_model_plan_command, build_task_plan_command, build_test_plan_command,
};
use lithe_plugin_manifest::{validate_command, DeclarativePluginManifest as Manifest};
use lithe_plugin_marketplace::install_review_command;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn manifest() -> Value {
    json!({
        "schemaVersion": 0,
        "id": "dev.lithe.test.build-bridge",
        "displayName": "Synthetic Build Bridge",
        "version": "0.1.0",
        "pluginAPIVersion": 0,
        "requires": { "buildSystems": 0 },
        "vendor": {
            "id": "dev.lithe.test", "displayName": "Test",
            "signatureRequirement": "publisherPackage",
        },
        "permissions": ["process.build"],
        "activationEvents": ["onBuildSystem:fixture-build"],
        "languages": [],
        "buildSystems": [{
            "id": "fixture-build", "displayName": "Fixture Build",
            "detection": { "markers": ["Fixture.build"], "nesting": "outermost" },
            "invocation": {
                "executables": [{ "kind": "command", "command": "fixture-build" }],
                "globalArguments": ["--batch", "${optionArguments}"],
            },
            "options": [
                { "id": "offline", "displayName": "Offline", "kind": "boolean",
                  "arguments": ["--offline"] },
                { "id": "modes", "displayName": "Modes", "kind": "multiSelect",
                  "choicesFromModel": true, "allowCustom": true,
                  "arguments": ["--modes", "${value}"] },
                { "id": "cache", "displayName": "Cache", "kind": "directory",
                  "arguments": ["--cache", "${value}"] },
            ],
            "model": {
                "source": "command",
                "arguments": ["model", "${pluginDir}/fixture.bridge", "${outputFile}"],
                "output": "outputFile", "decoder": "litheBuildModel",
                "executesProjectCode": true, "mayUseNetwork": false,
            },
            "tasks": [
                { "id": "compile", "displayName": "Compile", "group": "build",
                  "scopes": ["workspace", "project"],
                  "arguments": [
                      { "value": "--project", "when": "nestedProjectSelected" },
                      { "value": "${project.path}", "when": "nestedProjectSelected" },
                      "compile",
                  ] },
                { "id": "model-task", "displayName": "Model Task", "group": "other",
                  "fromModel": true, "arguments": ["${task.property:selector}"] },
                { "id": "custom", "displayName": "Custom Task", "group": "other",
                  "scopes": ["workspace", "project"], "requiresUserArguments": true,
                  "arguments": [
                      { "value": "--directory", "when": "nestedProjectSelected" },
                      { "value": "${project.path}", "when": "nestedProjectSelected" },
                      "${userArguments}",
                  ] },
            ],
            "run": [{
                "id": "application",
                "fromTasks": { "templates": ["model-task"], "executions": ["application"] },
                "workingDirectory": "root",
            }],
            "test": {
                "scopes": ["workspace", "project"], "filters": ["suite", "case"],
                "arguments": [
                    "test",
                    { "value": "--project", "when": "nestedProjectSelected" },
                    { "value": "${project.path}", "when": "nestedProjectSelected" },
                    { "value": "--select", "when": "suiteSelected" },
                    { "value": "${test.suite}", "when": ["suiteSelected", "noCaseSelected"] },
                    { "value": "${test.suite}#${test.case}", "when": ["suiteSelected", "caseSelected"] },
                ],
                "results": { "decoder": "junitXml", "reports": ["out/tests/TEST-*.xml"] },
            },
            "dependencies": {
                "source": "command", "scopes": ["workspace", "project"],
                "arguments": [
                    "dependencies", "${pluginDir}/fixture.bridge", "${outputFile}",
                    { "value": "--project=${project.path}", "when": "projectSelected" },
                ],
                "output": "outputFile", "decoder": "litheDependencies",
                "executesProjectCode": true, "mayUseNetwork": false,
            },
            "diagnostics": {
                "decoder": "lineMatcher",
                "patterns": [{
                    "regex": "^(ERR|WARN) (.+)\\|([0-9]+)\\|([0-9]+)\\|(.+)$",
                    "file": 2, "line": 3, "column": 4, "message": 5, "severity": 1,
                    "severities": { "ERR": "error", "WARN": "warning" },
                }, {
                    "regex": "^note://(.+)\\|([0-9]+)\\|(.+)$",
                    "file": 1, "line": 2, "message": 3, "defaultSeverity": "warning",
                }],
            },
        }],
    })
}

/// Owns only the temporary files this test creates, including on assertion failure.
struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let root = temporary_root("synthetic-build-plugin");
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn root(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    fn path(&self, relative: &str) -> String {
        self.0.join(relative).to_string_lossy().into_owned()
    }

    fn write(&self, relative: &str, contents: &str) -> String {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn document(&self, relative: &str, document: Value) -> String {
        // Serialize native paths as JSON data rather than replacing text inside strings.
        self.write(relative, &serde_json::to_string(&document).unwrap())
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // log-exempt: test-cleanup: best effort while an assertion may be unwinding.
        let _ = fs::remove_dir_all(&self.0);
    }
}

type Command = fn(Value) -> Result<Value, crate::protocol::CoreError>;

fn ok(command: Command, request: Value) -> Value {
    let outcome = command(json!({
        "manifest": manifest(), "buildSystemID": "fixture-build",
        "request": super::support::trusted(request),
    }))
    .expect("payload is well formed");
    assert_eq!(outcome["status"], "ok", "{outcome}");
    outcome
}

fn arguments(outcome: &Value) -> Vec<&str> {
    outcome["plan"]["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|argument| argument.as_str().unwrap())
        .collect()
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item[key].as_str().unwrap().to_owned())
        .collect()
}

fn model(workspace: &Workspace) -> Value {
    json!({
        "format": "litheBuildModel", "version": 0,
        "projects": [
            { "path": workspace.path("library"), "name": "library", "parent": workspace.root() },
            { "path": workspace.root(), "name": "workspace", "displayName": "Fixture Workspace" },
            { "path": workspace.path("app"), "name": "app", "parent": workspace.root() },
        ],
        "tasks": [
            { "project": workspace.path("library"), "name": "verify", "template": "model-task",
              "category": ["Checks", "contract"], "properties": { "selector": "library/verify" } },
            { "project": workspace.path("app"), "name": "launch", "template": "model-task",
              "execution": "application", "properties": { "selector": "app/launch" } },
        ],
        "contentRoots": [
            { "project": workspace.path("library"), "path": workspace.path("library/tests"), "kind": "testSource" },
            { "project": workspace.path("app"), "path": workspace.path("app/out/generated"), "kind": "generatedSource" },
        ],
        "optionValues": { "modes": ["ci", "release"] },
    })
}

fn decode_model(workspace: &Workspace, document: Value) -> Value {
    let output = workspace.document("state/model.json", document);
    ok(
        build_decode_model_command::<Manifest>,
        json!({
            "workspaceRoot": workspace.root(), "root": ".", "outputFile": output,
        }),
    )["model"]
        .clone()
}

#[test]
fn synthetic_build_plugin_validates_and_is_in_install_review() {
    let outcome = validate_command(json!({ "manifest": manifest() })).unwrap();
    assert_eq!(outcome["status"], "ok", "{outcome}");
    let review = install_review_command(json!({ "manifest": manifest() })).unwrap();
    assert_eq!(review["status"], "ok", "{review}");
    let programs = review["review"]["programs"].as_array().unwrap();
    let program = programs
        .iter()
        .find(|program| program["kind"] == "buildSystem")
        .unwrap();
    assert_eq!(program["buildSystemID"], "fixture-build");
}

#[test]
fn synthetic_build_detection_keeps_only_the_outermost_instance() {
    let workspace = Workspace::new();
    workspace.write("Fixture.build", "root");
    workspace.write("app/Fixture.build", "nested");
    let outcome = build_detect_command::<Manifest>(json!({
        "workspaceRoot": workspace.root(), "plugins": [{ "manifest": manifest() }], "options": {},
    }))
    .unwrap();
    assert_eq!(outcome["status"], "ok", "{outcome}");
    assert_eq!(
        strings(&outcome["detection"]["instances"], "id"),
        ["fixture-build:."],
        "{outcome}"
    );
}

#[test]
fn synthetic_model_plan_expands_package_output_and_options() {
    let workspace = Workspace::new();
    let plugin = workspace.path("plugins/bridge");
    let output = workspace.path("state/model.json");
    let cache = workspace.path("cache");
    let outcome = ok(
        build_model_plan_command::<Manifest>,
        json!({
            "root": ".", "pluginDirectory": plugin, "outputFile": output,
            "options": { "offline": true, "modes": ["release", "ci"], "cache": cache },
        }),
    );
    assert_eq!(
        arguments(&outcome),
        [
            "--batch",
            "--offline",
            "--modes",
            "ci,release",
            "--cache",
            &cache,
            "model",
            &format!("{plugin}/fixture.bridge"),
            &output,
        ]
    );
    assert_eq!(outcome["plan"]["decoder"], "litheBuildModel");
    assert_eq!(outcome["plan"]["executesProjectCode"], true);
    assert_eq!(outcome["plan"]["mayUseNetwork"], false);
}

#[test]
fn synthetic_model_decodes_sorted_projects_tasks_roots_and_choices() {
    let workspace = Workspace::new();
    let model = decode_model(&workspace, model(&workspace));
    assert_eq!(strings(&model["projects"], "path"), [".", "app", "library"]);
    assert_eq!(model["projects"][0]["displayName"], "Fixture Workspace");
    assert_eq!(model["projects"][2]["parentID"], model["projects"][0]["id"]);
    assert_eq!(model["optionValues"]["modes"], json!(["ci", "release"]));
    let tasks = model["tasks"].as_array().unwrap();
    let task = tasks.iter().find(|task| task["name"] == "verify").unwrap();
    assert_eq!(task["template"], "model-task");
    assert_eq!(task["category"], json!(["Checks", "contract"]));
    assert_eq!(task["properties"]["selector"], "library/verify");
    let roots = model["contentRoots"].as_array().unwrap();
    for (path, kind) in [
        ("library/tests", "testSource"),
        ("app/out/generated", "generatedSource"),
    ] {
        assert!(
            roots
                .iter()
                .any(|root| root["path"] == path && root["kind"] == kind),
            "{model}"
        );
    }
}

#[test]
fn synthetic_model_only_exposes_tasks_with_the_declared_execution_kind_as_runs() {
    let workspace = Workspace::new();
    let mut document = model(&workspace);
    let decoded = decode_model(&workspace, document.clone());
    let run = decoded["run"].as_array().unwrap();
    assert_eq!(run.len(), 1, "{decoded}");
    assert_eq!(run[0]["name"], "launch");
    assert_eq!(run[0]["execution"], "application");
    assert_eq!(run[0]["args"], json!(["--batch", "app/launch"]));
    assert_eq!(run[0]["cwd"], ".");
    document["tasks"][1]
        .as_object_mut()
        .unwrap()
        .remove("execution");
    let decoded = decode_model(&workspace, document);
    assert!(
        decoded["run"].as_array().is_none_or(Vec::is_empty),
        "{decoded}"
    );
}

#[test]
fn synthetic_task_plans_select_modules_and_use_opaque_model_properties() {
    let outcome = ok(
        build_task_plan_command::<Manifest>,
        json!({
            "root": ".", "task": "compile", "project": { "path": "app", "name": "app" },
            "options": { "offline": true },
        }),
    );
    assert_eq!(
        arguments(&outcome),
        ["--batch", "--offline", "--project", "app", "compile"]
    );
    for selector in ["verify", "library/verify", "opaque:group:task"] {
        let outcome = ok(
            build_task_plan_command::<Manifest>,
            json!({
                "root": ".", "task": "model-task", "project": { "path": ".", "name": "workspace" },
                "modelTask": { "name": "verify", "properties": { "selector": selector } },
            }),
        );
        assert_eq!(arguments(&outcome), ["--batch", selector]);
    }
}

#[test]
fn synthetic_custom_tasks_preserve_arguments_and_nested_project_selection() {
    for (path, expected) in [
        (".", vec!["--batch", "--offline", "inspect", "one argument"]),
        (
            "library",
            vec![
                "--batch",
                "--offline",
                "--directory",
                "library",
                "inspect",
                "one argument",
            ],
        ),
    ] {
        let outcome = ok(
            build_task_plan_command::<Manifest>,
            json!({
                "root": ".", "task": "custom", "project": { "path": path, "name": path },
                "options": { "offline": true }, "userArguments": ["inspect", "one argument"],
            }),
        );
        assert_eq!(arguments(&outcome), expected);
    }
}

#[test]
fn synthetic_test_plans_keep_filters_and_project_relative_report_globs() {
    for (case, selector) in [
        (None, "FixtureSuite"),
        (Some("works"), "FixtureSuite#works"),
    ] {
        let mut request = json!({
            "root": ".", "project": { "path": "library", "name": "library" }, "suite": "FixtureSuite",
        });
        if let Some(case) = case {
            request["case"] = json!(case);
        }
        let outcome = ok(build_test_plan_command::<Manifest>, request);
        assert_eq!(
            arguments(&outcome),
            [
                "--batch",
                "test",
                "--project",
                "library",
                "--select",
                selector
            ]
        );
        assert_eq!(
            outcome["plan"]["results"],
            json!({
                "decoder": "junitXml", "reports": ["library/out/tests/TEST-*.xml"],
            })
        );
    }
    let outcome = ok(
        build_test_plan_command::<Manifest>,
        json!({
            "root": ".", "projects": [".", "app", "library"],
        }),
    );
    assert_eq!(arguments(&outcome), ["--batch", "test"]);
    assert_eq!(
        outcome["plan"]["results"]["reports"],
        json!([
            "out/tests/TEST-*.xml",
            "app/out/tests/TEST-*.xml",
            "library/out/tests/TEST-*.xml",
        ])
    );
}

#[test]
fn synthetic_dependency_plans_distinguish_workspace_and_project_queries() {
    let workspace = Workspace::new();
    let plugin = workspace.path("plugins/bridge");
    let output = workspace.path("state/dependencies.json");
    for project in [None, Some("app")] {
        let mut request = json!({ "root": ".", "pluginDirectory": plugin, "outputFile": output });
        if let Some(project) = project {
            request["project"] = json!({ "path": project, "name": project });
        }
        let outcome = ok(build_dependency_plan_command::<Manifest>, request);
        let bridge = format!("{plugin}/fixture.bridge");
        let mut expected = vec!["--batch", "dependencies", &bridge, &output];
        if project.is_some() {
            expected.push("--project=app");
        }
        assert_eq!(arguments(&outcome), expected);
        assert_eq!(outcome["plan"]["decoder"], "litheDependencies");
    }
}

#[test]
fn synthetic_dependencies_preserve_scopes_properties_and_transitive_topology() {
    let workspace = Workspace::new();
    let dependency = json!({
        "name": "library", "group": "example", "version": "2.0", "scope": "compile",
        "properties": { "kind": "project", "selector": "opaque/library" },
        "children": [{ "name": "support", "version": "1.0", "scope": "test", "children": [] }],
    });
    for project in [None, Some("app")] {
        let dependencies = if project.is_some() {
            json!([dependency.clone()])
        } else {
            json!([
                { "name": "workspace", "children": [] },
                { "name": "app", "children": [dependency.clone()] },
            ])
        };
        let output = workspace.document(
            "state/dependencies.json",
            json!({
                "format": "litheDependencies", "version": 0, "dependencies": dependencies,
            }),
        );
        let mut request =
            json!({ "workspaceRoot": workspace.root(), "root": ".", "outputFile": output });
        if let Some(project) = project {
            request["project"] = json!(project);
        }
        let outcome = ok(build_decode_dependencies_command::<Manifest>, request);
        let tree = &outcome["dependencies"]["dependencies"];
        if project.is_none() {
            assert_eq!(strings(tree, "name"), ["workspace", "app"]);
        }
        assert_eq!(*tree, dependencies);
        assert_eq!(outcome["dependencies"]["truncated"], false);
    }
}

#[test]
fn synthetic_diagnostics_use_declared_patterns_and_workspace_relative_paths() {
    let workspace = Workspace::new();
    let outcome = ok(
        build_decode_diagnostics_command::<Manifest>,
        json!({
            "workspaceRoot": workspace.root(), "root": ".",
            "output": format!(
                "progress without a location\nERR {}|7|9|missing declaration\nWARN {}|3|1|old declaration\nnote://{}|4|unused value\n",
                workspace.path("library/source.fixture"), workspace.path("app/source.fixture"), workspace.path("app/config.fixture"),
            ),
        }),
    );
    assert_eq!(
        outcome["diagnostics"],
        json!([
            { "path": "library/source.fixture", "line": 7, "column": 9, "severity": "error", "message": "missing declaration" },
            { "path": "app/source.fixture", "line": 3, "column": 1, "severity": "warning", "message": "old declaration" },
        { "path": "app/config.fixture", "line": 4, "severity": "warning", "message": "unused value" },
        ])
    );
}
