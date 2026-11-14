//! `git.normalizeChangeTargets` against the shared fixture.

use serde_json::{json, Value};

fn call(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"targets", "command":"git.normalizeChangeTargets", "payload":payload})
            .to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_fixture_cases_keep_only_operation_targets() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/git/change-targets-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(
            response["data"]["indices"], case["indices"],
            "{}",
            case["name"]
        );
    }
    for case in fixture["invalid"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], false, "{}", case["name"]);
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "{}",
            case["name"]
        );
    }
}
