//! `git.graphProjection` against the shared fixture, including color stability across pages.

use serde_json::{json, Value};

fn call(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"graph", "command":"git.graphProjection", "payload":payload}).to_string(),
    ))
    .unwrap()
}

fn shared_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../fixtures/git/graph-projection-v1.json"
    ))
    .unwrap()
}

#[test]
fn shared_fixture_projects_the_exact_response() {
    let fixture = shared_fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() >= 20, "the fixture must keep its scenarios");
    for case in cases {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["response"], "{}", case["name"]);
    }
}

#[test]
fn shared_fixture_keeps_a_named_branch_color_across_pages() {
    let fixture = shared_fixture();
    let cases = fixture["cases"].as_array().unwrap();
    for sequence in fixture["stability"].as_array().unwrap() {
        let expected = &sequence["expectedColorId"];
        assert!(!expected.is_null());
        for step in sequence["steps"].as_array().unwrap() {
            let case = cases
                .iter()
                .find(|case| case["name"] == step["case"])
                .unwrap();
            let response = call(case["request"].clone());
            let row = response["data"]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["commitHash"] == step["hash"])
                .unwrap();
            assert_eq!(
                &row["colorId"], expected,
                "{} / {}",
                sequence["name"], step["case"]
            );
        }
    }
}

#[test]
fn shared_fixture_rejects_invalid_requests() {
    for case in shared_fixture()["invalid"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], false, "{}", case["name"]);
        assert_eq!(
            response["error"]["code"], "invalid_request",
            "{}",
            case["name"]
        );
    }
}
