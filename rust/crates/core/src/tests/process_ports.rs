//! `process.listeningPorts` against the shared listening-ports fixture.

use serde_json::{json, Value};

fn run(request: &Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"command": "process.listeningPorts", "payload": request}).to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_listening_ports_fixture_runs_every_case() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/process-lifecycle/listening-ports-v1.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = run(&case["request"]);
        assert_eq!(response["ok"], true, "case {}: {response}", case["name"]);
        assert_eq!(response["data"], case["expected"], "case {}", case["name"]);
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
