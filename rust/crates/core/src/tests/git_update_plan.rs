//! `git.updatePlan` against the shared fixture.

use serde_json::{json, Value};

fn call(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"update", "command":"git.updatePlan", "payload":payload}).to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_fixture_steps_are_the_only_update_rules() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../fixtures/git/update-plan-v1.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["plan"], "{}", case["name"]);
    }
    for case in fixture["invalid"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], false, "{}", case["name"]);
    }
}
