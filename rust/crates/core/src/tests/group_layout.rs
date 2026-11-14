//! `editor.groupLayout.reduce` and `editor.groupLayout.dropTargets` against the shared editor
//! group layout fixture.

use serde_json::{json, Value};

fn run(command: &str, request: &Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"command": command, "payload": request}).to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_group_layout_fixture_runs_every_case() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/editor/group-layout-v1.json"
    )))
    .unwrap();
    for (section, command) in [
        ("reduce", "editor.groupLayout.reduce"),
        ("dropTargets", "editor.groupLayout.dropTargets"),
    ] {
        for case in fixture[section]["cases"].as_array().unwrap() {
            let response = run(command, &case["request"]);
            assert_eq!(response["ok"], true, "case {}: {response}", case["name"]);
            assert_eq!(response["data"], case["response"], "case {}", case["name"]);
        }
        for case in fixture[section]["invalid"].as_array().unwrap() {
            let response = run(command, &case["request"]);
            assert_eq!(response["ok"], false, "case {}: {response}", case["name"]);
            assert_eq!(
                response["error"]["code"], "invalid_request",
                "case {}",
                case["name"]
            );
        }
    }
}
