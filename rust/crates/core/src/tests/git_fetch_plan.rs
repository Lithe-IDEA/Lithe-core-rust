//! `git.fetchPlan` against the shared fixture, and option validation before any repository access.

use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../../fixtures/git/fetch-plan-v1.json")).unwrap()
}

fn request(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({
            "id": "fetch-plan-test", "command": "git.fetchPlan", "payload": payload
        })
        .to_string(),
    ))
    .unwrap()
}

#[test]
fn fetch_plans_match_shared_choices_without_accessing_a_repository() {
    for case in fixture()["cases"].as_array().unwrap() {
        let response = request(json!({"options": case["options"]}));
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["data"]["options"], case["expectedOptions"]);
        assert_eq!(response["data"]["arguments"], case["arguments"]);
    }
}

#[test]
fn fetch_rejects_sources_and_arguments_in_the_remote_name_field() {
    for remote in fixture()["invalidRemotes"].as_array().unwrap() {
        let response = request(json!({"options": {"remote": remote}}));
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "{remote}: {response}"
        );
    }
}

#[test]
fn fetch_rejects_unknown_settings_instead_of_silently_ignoring_them() {
    for options in [
        json!({"prune": "false"}),
        json!({"submodules": "sometimes"}),
        json!({"arguments": ["--force"]}),
        json!({"credentialHelper": ""}),
    ] {
        assert_eq!(
            request(json!({"options": options}))["error"]["code"],
            "invalid_request"
        );
    }
    // Misplaced options must fail before repository lookup or any mutation.
    let response: Value = serde_json::from_str(&crate::execute_json(&json!({
        "command": "git.write", "payload": {"root": "", "operation": "commit", "fetchOptions": {}}
    }).to_string())).unwrap();
    assert_eq!(
        response["error"]["message"],
        "Fetch options require a fetch operation"
    );
}
