//! Drives the shared double-Shift fixture through the rule dispatch that `lithe-core` forwards to.

use lithe_core_rules::{execute, keymap::THRESHOLD_SECONDS, RuleErrorCode};
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("../../fixtures/keymap/modifier-gesture-v1.json");

fn step(state: &Value, event: Value, timestamp: f64) -> Value {
    execute(
        "keymap.modifierGestureStep",
        json!({"state": state, "event": event, "timestamp": timestamp}),
    )
    .unwrap()
}

#[test]
fn threshold_matches_the_fixture() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(
        fixture["thresholdSeconds"].as_f64(),
        Some(THRESHOLD_SECONDS)
    );
}

#[test]
fn every_fixture_sequence_matches_step_by_step() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    for sequence in fixture["sequences"].as_array().unwrap() {
        let name = sequence["name"].as_str().unwrap();
        // The host owns the state; start from the idle default exactly as a host does.
        let mut state = json!({});
        for (index, entry) in sequence["steps"].as_array().unwrap().iter().enumerate() {
            let (event, timestamp) = match entry["event"].as_str().unwrap() {
                "keyDown" => (json!({"kind": "keyDown"}), 0.0),
                "reset" => (json!({"kind": "reset"}), 0.0),
                _ => (
                    json!({
                        "kind": "flags",
                        "shift": entry["shift"],
                        "otherModifiers": entry["otherModifiers"],
                    }),
                    entry["timestamp"].as_f64().unwrap(),
                ),
            };
            let data = step(&state, event, timestamp);
            if let Some(expected) = entry.get("expected") {
                assert_eq!(&data["triggered"], expected, "{name} step {index}");
            }
            state = data["state"].clone();
        }
    }
}

#[test]
fn malformed_requests_use_core_error_codes() {
    let unknown = execute("keymap.nope", json!({})).unwrap_err();
    assert_eq!(unknown.code, RuleErrorCode::NotSupported);

    let bad_payload = execute(
        "keymap.modifierGestureStep",
        json!({"event": {"kind": "flags"}}),
    )
    .unwrap_err();
    assert_eq!(bad_payload.code, RuleErrorCode::InvalidRequest);
}
