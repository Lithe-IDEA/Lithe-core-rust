//! The `diagnostics.*` commands through the JSON dispatcher: request shapes,
//! the domain error envelope and the stable error codes the hosts rely on.
//! The logic itself is tested in `lithe-diagnostics`.

use crate::execute_json;
use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../fixtures/diagnostics/logging-v2.json"
    ))
    .unwrap()
}

#[test]
fn collection_plan_dispatches_with_the_domain_envelope() {
    let request = json!({"version":1,"command":"diagnostics.planLogCollection","payload":{
        "files":[],"currentSession":"","session":"all"}});
    let response: Value = serde_json::from_str(&execute_json(&request.to_string())).unwrap();
    assert_eq!(response["data"]["result"]["reads"], json!([]));
}

#[test]
fn diagnostics_v2_commands_dispatch_with_stable_domain_errors() {
    let cases = [
        ("diagnostics.encodeLogBatch", json!({"events":[]})),
        ("diagnostics.queryLogsV2", json!({"sources":[]})),
        ("diagnostics.parseLogFilter", json!({"expression":""})),
        (
            "diagnostics.composeLogFilter",
            json!({"expression":"","term":{"kind":"text","value":"test"}}),
        ),
        (
            "diagnostics.formatLogRecords",
            json!({"lines":[],"format":"json"}),
        ),
        (
            "diagnostics.planLogRetention",
            json!({"currentSession":"", "now":"2026-11-03T00:00:00Z"}),
        ),
        (
            "diagnostics.resolveLogChannel",
            json!({"channel":"vcs/git"}),
        ),
    ];
    for (command, payload) in cases {
        let response: Value = serde_json::from_str(&execute_json(
            &json!({"id":"fixture", "version":1, "command":command, "payload":payload}).to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        assert!(response["data"].get("error").is_none(), "{response}");
    }
    let response: Value = serde_json::from_str(&execute_json(&json!({"version":1,"command":"diagnostics.resolveLogChannel","payload":{"channel":"vcs/.."}}).to_string())).unwrap();
    assert_eq!(response["data"]["error"]["code"], -32033);
}

#[test]
fn shared_window_commands_cover_dispatch_shapes_and_unique_errors() {
    let fixture = fixture();
    let mut seen_codes = std::collections::BTreeSet::new();
    let mut seen_commands = std::collections::BTreeSet::new();
    for case in fixture["windowCommands"].as_array().unwrap() {
        let mut payload = case["request"].clone();
        if let Some(count) = case["repeatCount"].as_u64() {
            payload["lines"] = json!(vec![case["repeatLine"].as_str().unwrap(); count as usize]);
        }
        let response: Value = serde_json::from_str(&execute_json(
            &json!({"version":1,"command":case["command"],"payload":payload}).to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        if let Some(code) = case["code"].as_i64() {
            assert_eq!(
                response["data"]["error"]["code"], code,
                "{case}: {response}"
            );
            assert!(fixture["errorCodes"][code.to_string()].is_string());
            seen_codes.insert(code);
        } else {
            // parse returns diagnostics directly; other pure v2 commands use the domain envelope.
            let actual = if case["command"] == "diagnostics.parseLogFilter"
                || case["command"] == "diagnostics.buildManifest"
            {
                &response["data"]
            } else {
                &response["data"]["result"]
            };
            assert_eq!(actual, &case["expected"], "{case}: {response}");
        }
        seen_commands.insert(case["command"].as_str().unwrap());
    }
    for command in [
        "diagnostics.parseLogFilter",
        "diagnostics.composeLogFilter",
        "diagnostics.formatLogRecords",
    ] {
        assert!(seen_commands.contains(command));
    }
    for code in [-32060, -32061, -32062, -32063] {
        assert!(seen_codes.contains(&code));
    }
}
