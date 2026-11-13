//! Shared-fixture coverage for the language-tooling policy commands in
//! `lithe-lsp-policy`. Both platforms consume these fixtures, so a divergence
//! here is a cross-platform bug.

use serde_json::{json, Value};

fn execute(command: &str, payload: &Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"command": command, "payload": payload}).to_string(),
    ))
    .expect("Core should return JSON")
}

/// Every case yields its `expected` data, or fails with its `expectedError` code.
fn run_decision_fixture(command: &str, fixture: &str) {
    let fixture: Value = serde_json::from_str(fixture).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = execute(command, &case["request"]);
        if let Some(code) = case.get("expectedError") {
            assert_eq!(response["ok"], false, "{}: {response}", case["name"]);
            assert_eq!(&response["error"]["code"], code, "{}", case["name"]);
        } else {
            assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
            assert_eq!(response["data"], case["expected"], "{}", case["name"]);
        }
    }
}

/// Every case succeeds with its `response` data.
fn run_fixture(command: &str, fixture: &str) {
    let fixture: Value = serde_json::from_str(fixture).expect("fixture should be JSON");
    let cases = fixture["cases"]
        .as_array()
        .expect("fixture should list cases");
    assert!(!cases.is_empty(), "{command} fixture has no cases");
    for case in cases {
        let response = execute(command, &case["request"]);
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["response"], "{}", case["name"]);
    }
}

#[test]
fn declaration_or_usages_matches_shared_fixture() {
    run_decision_fixture(
        "lsp.declarationOrUsages",
        include_str!("../../../../fixtures/lsp/declaration-or-usages-v1.json"),
    );
}

#[test]
fn navigation_target_matches_shared_fixture() {
    run_decision_fixture(
        "lsp.classifyNavigationTarget",
        include_str!("../../../../fixtures/lsp/navigation-target-v1.json"),
    );
}

#[test]
fn workspace_edit_plan_matches_shared_fixture() {
    run_decision_fixture(
        "lsp.planWorkspaceEdit",
        include_str!("../../../../fixtures/lsp/workspace-edit-plan-v1.json"),
    );
}

#[test]
fn document_identity_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/lsp/document-identity-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = execute("lsp.documentIdentity", &case["request"]);
        if let Some(code) = case.get("error") {
            assert_eq!(&response["error"]["code"], code, "{case}");
        } else {
            assert_eq!(response["data"], case["expected"], "{case}: {response}");
        }
    }
}

#[test]
fn problems_query_matches_shared_fixture() {
    run_fixture(
        "lsp.problems.query",
        include_str!("../../../../fixtures/lsp/problems-query-v1.json"),
    );
}

#[test]
fn activation_policy_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/lsp/activation-policy-v1.json"
    ))
    .unwrap();
    // The fixture documents the budget; keep it pinned to the constants.
    assert_eq!(
        fixture["maxConsecutiveFailures"],
        lithe_lsp_policy::activation::MAX_CONSECUTIVE_FAILURES
    );
    assert_eq!(
        fixture["failureCooldownMs"],
        lithe_lsp_policy::activation::FAILURE_COOLDOWN_MS
    );
    run_fixture(
        "lsp.activationPolicy",
        include_str!("../../../../fixtures/lsp/activation-policy-v1.json"),
    );
}

#[test]
fn completion_merge_matches_shared_fixture() {
    run_fixture(
        "lsp.mergeCompletions",
        include_str!("../../../../fixtures/lsp/completion-merge-v1.json"),
    );
}

#[test]
fn malformed_requests_fail_with_invalid_request() {
    for (command, payload) in [
        (
            "lsp.problems.query",
            json!({"diagnostics": [{"fileKey": "a"}]}),
        ),
        (
            "lsp.problems.query",
            json!({"diagnostics": [], "groupBy": "type"}),
        ),
        ("lsp.activationPolicy", json!({"event": "automaticStart"})),
        (
            "lsp.activationPolicy",
            json!({"event": "crashed", "nowMs": 1}),
        ),
        ("lsp.mergeCompletions", json!({"lists": [{"items": []}]})),
    ] {
        let response = execute(command, &payload);
        assert_eq!(response["ok"], false, "{command}: {response}");
        assert_eq!(response["error"]["code"], "invalid_request", "{command}");
    }
}
