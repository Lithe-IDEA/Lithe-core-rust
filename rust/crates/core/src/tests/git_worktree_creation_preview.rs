//! `git.worktreeCreationPreview` against the shared fixture.

use serde_json::{json, Value};

fn call(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"worktree", "command":"git.worktreeCreationPreview", "payload":payload})
            .to_string(),
    ))
    .unwrap()
}

/// The fixture writes paths for Unix; on Windows an absolute one moves to drive `C:`.
fn native(value: &mut Value, fields: &[&str]) {
    for field in fields {
        if let Some(path) = value[*field].as_str().filter(|path| path.starts_with('/')) {
            value[*field] = if cfg!(windows) {
                Value::String(format!("C:{}", path.replace('/', "\\")))
            } else {
                Value::String(path.to_string())
            };
        }
    }
}

#[test]
fn shared_fixture_derives_choices_destination_and_readiness() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/git/worktree-creation-preview-v1.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let mut request = case["request"].clone();
        native(&mut request, &["root", "destination", "lastSuggestion"]);
        let mut expected = case["response"].clone();
        native(&mut expected, &["suggestedDestination", "destination"]);
        let response = call(request);
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], expected, "{}", case["name"]);
    }
}

#[test]
fn shared_fixture_rejects_invalid_requests() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/git/worktree-creation-preview-v1.json"
    ))
    .unwrap();
    for case in fixture["invalidRequests"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], false, "{}", case["name"]);
        assert_eq!(response["error"]["code"], case["code"], "{}", case["name"]);
    }
}
