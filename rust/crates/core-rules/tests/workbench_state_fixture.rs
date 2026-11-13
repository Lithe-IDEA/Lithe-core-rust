//! Drives the shared workbench-state fixture through the rule dispatch that `lithe-core` forwards to.

use lithe_core_rules::{execute, RuleError, RuleErrorCode, COMMANDS};
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("../../../fixtures/editor/workbench-state-v1.json");
const COMMAND: &str = "editor.workbenchState.reduce";

fn call(payload: Value) -> Result<Value, RuleError> {
    execute(COMMAND, payload)
}

#[test]
fn command_is_owned_by_the_rules_crate() {
    assert!(COMMANDS.contains(&COMMAND));
}

#[test]
fn every_fixture_case_matches() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture["version"], 1);
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() >= 40, "fixture should keep its coverage");
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let data = call(json!({"state": case["state"], "action": case["action"]}))
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(data["state"], case["expected"]["state"], "{name}");
        assert_eq!(
            data["removedGroupIds"], case["expected"]["removedGroupIds"],
            "{name}"
        );
    }
}

#[test]
fn reducing_a_result_again_changes_nothing_for_normalizing_actions() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let again =
            call(json!({"state": case["expected"]["state"], "action": {"kind": "normalize"}}))
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        // Re-normalizing a reducer result is idempotent: the reducer never emits a state it
        // would repair.
        assert_eq!(again["state"], case["expected"]["state"], "{name}");
        assert_eq!(again["removedGroupIds"], json!([]), "{name}");
    }
}

#[test]
fn malformed_requests_are_invalid() {
    for payload in [
        json!({}),
        json!({"action": {"kind": "teleport"}}),
        json!({"action": {"kind": "open", "group": "g"}}),
        json!({"action": {"kind": "open", "group": "g", "member": "a", "extra": 1}}),
        json!({"state": {"groups": [{"members": []}]}, "action": {"kind": "normalize"}}),
        json!({"action": {"kind": "close", "group": "g", "member": "a", "emptyGroups": "nope"}}),
    ] {
        let error = call(payload.clone()).expect_err(&payload.to_string());
        assert_eq!(error.code, RuleErrorCode::InvalidRequest, "{payload}");
    }
}
