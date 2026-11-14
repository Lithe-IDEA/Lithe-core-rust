//! Execution and run-configuration commands answered through the dispatcher.

use serde_json::Value;

#[test]
fn argument_text_command_formats_a_batch_from_the_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/run-configuration/argument-text.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let argument_lists: Vec<_> = cases.iter().map(|case| case["values"].clone()).collect();
    let expected: Vec<_> = cases.iter().map(|case| case["text"].clone()).collect();
    let request = serde_json::json!({
        "id": "argument-text", "command": "runConfig.argumentText",
        "payload": {"argumentLists": argument_lists}
    });
    let response: Value = serde_json::from_str(&crate::execute_json(&request.to_string())).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"], Value::Array(expected));
}

#[test]
fn context_selection_command_matches_shared_provider_neutral_cases() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/run-configuration/context-selection.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let request = serde_json::json!({
            "id": "context-selection", "command": "runConfig.selectContextConfiguration",
            "payload": case["request"]
        });
        let response: Value =
            serde_json::from_str(&crate::execute_json(&request.to_string())).unwrap();
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(
            response["data"]["configurationId"], case["expected"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn search_path_command_follows_the_shared_order_and_identity_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/execution/search-path-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let request = serde_json::json!({"id":"search-path", "command":"execution.resolveSearchPath", "payload":case["request"]});
        let response: Value =
            serde_json::from_str(&crate::execute_json(&request.to_string())).unwrap();
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["expected"], "{}", case["name"]);
    }
}

#[test]
fn environment_command_follows_the_shared_precedence_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/execution/launch-environment-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let request = serde_json::json!({"id":"environment", "command":"execution.resolveEnvironment", "payload":case["request"]});
        let response: Value =
            serde_json::from_str(&crate::execute_json(&request.to_string())).unwrap();
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["expected"], "{}", case["name"]);
    }
}
