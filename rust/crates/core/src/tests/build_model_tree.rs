//! `build.modelTree`: the nested project tree every host's build tool window
//! renders, checked against the shared fixture.

use crate::build::build_model_tree_command;
use crate::protocol::ErrorCode;
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
use serde_json::{json, Value};
use std::collections::BTreeSet;

const FIXTURE: &str = include_str!("../../../../fixtures/build/model-tree-v1.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

fn tree(manifest: &Value, build_system: &Value, request: &Value) -> Value {
    let result = build_model_tree_command::<Manifest>(
        json!({"manifest": manifest, "buildSystemID": build_system, "request": request}),
    )
    .unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    result["tree"].clone()
}

/// Every key once, so a host can keep open nodes by key.
fn assert_unique_keys(nodes: &Value, keys: &mut BTreeSet<String>) {
    for node in nodes.as_array().into_iter().flatten() {
        let key = node["key"].as_str().unwrap().to_string();
        assert!(keys.insert(key.clone()), "repeated key {key:?}");
        assert_unique_keys(&node["children"], keys);
    }
}

fn count(nodes: &Value) -> usize {
    nodes
        .as_array()
        .into_iter()
        .flatten()
        .map(|node| 1 + count(&node["children"]))
        .sum()
}

#[test]
fn model_tree_nests_projects_and_files_their_tasks() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let actual = tree(
            &fixture["manifest"],
            &case["buildSystemID"],
            &case["request"],
        );
        assert_eq!(actual, case["expected"], "{}", case["name"]);
        let mut keys = BTreeSet::new();
        if let Some(project) = actual.get("project") {
            keys.insert(project["key"].as_str().unwrap().to_string());
        }
        assert_unique_keys(&actual["nodes"], &mut keys);
    }
}

#[test]
fn model_tree_keeps_every_node_when_the_root_project_matches() {
    let fixture = fixture();
    let case = &fixture["cases"][0];
    let mut request = case["request"].clone();
    request["query"] = json!(" APP ");
    let mut expected = case["expected"].clone();
    expected["project"]["matched"] = json!(true);
    assert_eq!(
        tree(&fixture["manifest"], &case["buildSystemID"], &request),
        expected
    );
}

#[test]
fn model_tree_refuses_malformed_requests() {
    let fixture = fixture();
    for case in fixture["rejected"].as_array().unwrap() {
        let error = build_model_tree_command::<Manifest>(json!({
            "manifest": fixture["manifest"],
            "buildSystemID": case["buildSystemID"],
            "request": case["request"],
        }))
        .expect_err(case["name"].as_str().unwrap());
        assert!(
            matches!(error.code, ErrorCode::InvalidRequest),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn model_tree_bounds_the_model_and_cuts_a_huge_tree() {
    let fixture = fixture();
    let project = |at: usize| json!({"id": format!("nested:p{at}"), "path": format!("p{at}"), "name": format!("p{at}")});
    let too_many: Vec<Value> = (0..2_001).map(project).collect();
    let error = build_model_tree_command::<Manifest>(json!({
        "manifest": fixture["manifest"],
        "buildSystemID": "nested",
        "request": {"model": {"projects": too_many}},
    }))
    .unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));

    // Every task its own four-level folder path: far more nodes than a tree holds.
    let projects: Vec<Value> = (0..2_000).map(project).collect();
    let tasks: Vec<Value> = (0..10_000)
        .map(|at| {
            json!({"id": format!("nested:t{at}"), "projectID": format!("nested:p{}", at % 2_000),
                "name": format!("t{at}"), "group": "build", "category": [format!("a{at}"), "b", "c", "d"]})
        })
        .collect();
    let cut = tree(
        &fixture["manifest"],
        &json!("nested"),
        &json!({"model": {"projects": projects, "tasks": tasks}}),
    );
    assert_eq!(cut["truncated"], true);
    assert!(count(&cut["nodes"]) <= 50_000, "{}", count(&cut["nodes"]));
}
