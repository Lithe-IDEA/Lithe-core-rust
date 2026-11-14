//! `git.referenceTree` against the shared fixture.

use serde_json::{json, Value};

fn call(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"tree", "command":"git.referenceTree", "payload":payload}).to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_fixture_trees_have_one_grouping_and_order() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/git/reference-tree-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"]["nodes"], case["nodes"], "{}", case["name"]);
    }
}

#[test]
fn rejects_unknown_fields() {
    let response = call(json!({"references":[{"name":"main","kind":"local"}]}));
    assert_eq!(response["ok"], false);
}
