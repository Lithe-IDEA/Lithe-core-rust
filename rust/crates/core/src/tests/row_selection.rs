//! `workspace.rowSelection.reduce` against the shared row selection fixture.

use serde_json::{json, Value};

fn run(request: &Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"command": "workspace.rowSelection.reduce", "payload": request}).to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_row_selection_fixture_runs_every_case() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/workspace/row-selection-v1.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = run(&case["request"]);
        assert_eq!(response["ok"], true, "case {}: {response}", case["name"]);
        assert_eq!(response["data"], case["response"], "case {}", case["name"]);
    }
    for case in fixture["invalid"].as_array().unwrap() {
        let response = run(&case["request"]);
        assert_eq!(response["ok"], false, "case {}: {response}", case["name"]);
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "case {}",
            case["name"]
        );
    }
}
