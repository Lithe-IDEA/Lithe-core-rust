//! The `editor.*` commands through the JSON dispatcher: shared fixtures for
//! navigation input, text edit plans and navigation history, and the request
//! envelope for malformed payloads.

use serde_json::{json, Value};

#[test]
fn navigation_input_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/editor/navigation-input-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response: Value = serde_json::from_str(&crate::execute_json(
            &json!({"command":"editor.parseNavigationInput", "payload":{"text":case["input"]}})
                .to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(
            response["data"]["target"], case["target"],
            "{}",
            case["name"]
        );
    }
}

/// Plugin edit plans follow the shared fixture, and applying a plan's
/// document-separator text in reverse order yields the fixture's result.
#[test]
fn text_edit_plan_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/editor/text-edit-plan-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut payload = json!({"text": case["text"], "edits": case["edits"]});
        if let Some(positions) = case.get("positions") {
            payload["positions"] = positions.clone();
        }
        let response: Value = serde_json::from_str(&crate::execute_json(
            &json!({"command":"editor.planTextEdits", "payload": payload}).to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["expected"], "{}", case["name"]);
        if let Some(expected) = case["expectedText"].as_str() {
            let mut units: Vec<u16> = case["text"].as_str().unwrap().encode_utf16().collect();
            for edit in response["data"]["edits"].as_array().unwrap().iter().rev() {
                let offset = edit["utf16Offset"].as_u64().unwrap() as usize;
                let length = edit["utf16Length"].as_u64().unwrap() as usize;
                let replacement: Vec<u16> = edit["text"].as_str().unwrap().encode_utf16().collect();
                units.splice(offset..offset + length, replacement);
            }
            assert_eq!(
                String::from_utf16(&units).unwrap(),
                expected,
                "{}",
                case["name"]
            );
        }
    }
}

#[test]
fn text_edit_plan_rejects_malformed_requests() {
    for payload in [
        json!({"edits": []}),
        json!({"text": "a", "edits": [{"range": {"start": {"line": -1, "column": 1}, "end": {"line": 1, "column": 1}}, "text": ""}]}),
        json!({"text": "a", "unknown": true}),
    ] {
        let response: Value = serde_json::from_str(&crate::execute_json(
            &json!({"command":"editor.planTextEdits", "payload": payload}).to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], false, "{payload}");
        assert_eq!(response["error"]["code"], "invalid_request");
    }
}

fn navigation_history(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"command":"editor.navigationHistory.reduce", "payload":payload}).to_string(),
    ))
    .unwrap()
}

#[test]
fn navigation_history_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/editor/navigation-history-v1.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["maxEntries"],
        lithe_editor::navigation_history::MAX_HISTORY_ENTRIES
    );
    for case in fixture["cases"].as_array().unwrap() {
        let response = navigation_history(case["request"].clone());
        if let Some(code) = case.get("expectedError") {
            assert_eq!(response["ok"], false, "{}: {response}", case["name"]);
            assert_eq!(response["error"]["code"], *code, "{}", case["name"]);
            continue;
        }
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        assert_eq!(response["data"], case["expected"], "{}", case["name"]);
    }
}

// Both stacks are bounded so a long editing session cannot grow history (or
// the IPC payload that carries it) without limit; the oldest entries go first.
#[test]
fn navigation_history_keeps_only_the_newest_entries() {
    let limit = lithe_editor::navigation_history::MAX_HISTORY_ENTRIES as u64;
    let back: Vec<Value> = (0..limit)
        .map(|line| json!({"document":"a", "line":line, "column":0}))
        .collect();
    let response = navigation_history(json!({
        "state": {"back": back, "forward": []},
        "event": {"type":"record", "departure":{"document":"b", "line":0, "column":0}}
    }));
    let back = response["data"]["state"]["back"].as_array().unwrap();
    assert_eq!(back.len() as u64, limit);
    assert_eq!(back[0]["line"], 1);
    assert_eq!(back[back.len() - 1]["document"], "b");

    let oversized: Vec<Value> = (0..limit + 5)
        .map(|line| json!({"document":"a", "line":line, "column":0}))
        .collect();
    let response = navigation_history(json!({
        "state": {"back": [], "forward": oversized},
        "event": {"type":"back", "current":null}
    }));
    let forward = response["data"]["state"]["forward"].as_array().unwrap();
    assert_eq!(forward.len() as u64, limit);
    assert_eq!(forward[0]["line"], 5);
}
