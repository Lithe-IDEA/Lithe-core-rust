//! `plugins.installed.lifecycle` against the shared plugin lifecycle fixture.

use serde_json::{json, Value};

fn lifecycle(request: &Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"command":"plugins.installed.lifecycle", "payload":request}).to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_plugin_lifecycle_fixture_plans_every_action() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/plugins/installed-lifecycle.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = lifecycle(&case["request"]);
        assert_eq!(response["ok"], true, "case {}: {response}", case["name"]);
        assert_eq!(response["data"], case["response"], "case {}", case["name"]);
    }
    for case in fixture["invalid"].as_array().unwrap() {
        let response = lifecycle(&case["request"]);
        assert_eq!(response["ok"], false, "case {}: {response}", case["name"]);
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "case {}",
            case["name"]
        );
    }
}
