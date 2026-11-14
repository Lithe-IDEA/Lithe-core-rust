//! `git.historyWindow` against the shared fixture and its window bound.

use serde_json::{json, Value};

fn call(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"window", "command":"git.historyWindow", "payload":payload}).to_string(),
    ))
    .unwrap()
}

#[test]
fn shared_fixture_windows_keep_depth_and_selection() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/git/history-window-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = call(case["request"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["response"], "{}", case["name"]);
    }
    for case in fixture["invalid"].as_array().unwrap() {
        assert_eq!(
            call(case["request"].clone())["ok"],
            false,
            "{}",
            case["name"]
        );
    }
}

#[test]
fn merge_never_grows_past_the_window_bound() {
    let window = (0..4_999)
        .map(|index| format!("w{index}"))
        .collect::<Vec<_>>();
    let response = call(json!({"operation":"mergePage", "window":window,
        "page":["a","b","c"], "pageHasMore":true}));
    assert_eq!(response["data"], json!({"append":[0], "hasMore":false}));
}
