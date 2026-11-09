//! Consumer-facing command regressions for plugin settings document ownership.

use serde_json::{json, Value};

fn manifest() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/contracts/plugins/v1/fixtures/settings/settings-v2.plugin.json"
    )))
    .unwrap()
}

fn execute(command: &str, payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({
            "command": command, "payload": payload
        })
        .to_string(),
    ))
    .unwrap()
}

#[test]
fn settings_state_dispatches_with_core_owned_sources_and_normalized_scope() {
    let result = execute(
        "plugins.declarative.settingsState",
        json!({
            "manifest": manifest(),
            "globalDocument": "{\"dev.lithe.fixture.settings-v2\":{\"dialyzer\":false}}",
            "projectDocument": "{\"dev.lithe.fixture.settings-v2\":{\"mixEnv\":\"test\"}}"
        }),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["data"]["status"], "ok", "{result}");
    let state = &result["data"]["state"];
    assert_eq!(state["pluginID"], "dev.lithe.fixture.settings-v2");
    let rows = state["rows"].as_array().unwrap();
    let dialyzer = rows
        .iter()
        .find(|row| row["declaration"]["id"] == "dialyzer")
        .unwrap();
    assert_eq!(dialyzer["effective"]["value"], false);
    assert_eq!(dialyzer["effective"]["source"], "global");
    let checks = rows
        .iter()
        .find(|row| row["declaration"]["id"] == "checks")
        .unwrap();
    assert_eq!(checks["scope"], "global");
    assert_eq!(checks["effect"], "nextRun");
    let mix = rows
        .iter()
        .find(|row| row["declaration"]["id"] == "mixEnv")
        .unwrap();
    assert_eq!(mix["global"]["value"], "dev");
    assert_eq!(mix["effective"]["value"], "test");
    assert_eq!(mix["effective"]["source"], "project");
}

#[test]
fn complete_edit_fixture_preserves_types_siblings_and_atomic_rejection() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-documents.json"
    )))
    .unwrap();
    for case in fixture["editCases"].as_array().unwrap() {
        let result = execute(
            "plugins.declarative.editSettings",
            json!({
                "manifest": fixture["manifest"], "scope":case["scope"],
                "document":case["document"], "edits":case["edits"]
            }),
        );
        assert_eq!(result["ok"], true, "case {}: {result}", case["name"]);
        let outcome = &result["data"];
        if let Some(code) = case["errorCode"].as_str() {
            assert_eq!(
                outcome["status"], "rejected",
                "case {}: {result}",
                case["name"]
            );
            assert_eq!(
                outcome["error"]["code"], code,
                "case {}: {result}",
                case["name"]
            );
            assert!(
                outcome.get("edit").is_none(),
                "rejected edits must not expose partial content"
            );
        } else {
            assert_eq!(outcome["status"], "ok", "case {}: {result}", case["name"]);
            let text = outcome["edit"]["document"].as_str().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(text).unwrap(),
                case["expected"],
                "case {}",
                case["name"]
            );
            assert_eq!(
                outcome["edit"]["effects"], case["effects"],
                "case {}",
                case["name"]
            );
            assert_eq!(
                outcome["edit"]["delete"],
                case["delete"].as_bool().unwrap_or(false)
            );
            if case["unchanged"] == true {
                assert_eq!(outcome["edit"]["changed"], false);
                assert_eq!(text, case["document"].as_str().unwrap());
            }
        }
    }
}

#[test]
fn missing_files_are_empty_but_empty_existing_documents_are_rejected() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-documents.json"
    )))
    .unwrap();
    let valid = execute(
        "plugins.declarative.settingsState",
        json!({"manifest": fixture["manifest"]}),
    );
    assert_eq!(valid["data"]["status"], "ok", "{valid}");
    let invalid = execute(
        "plugins.declarative.settingsState",
        json!({"manifest": fixture["manifest"], "projectDocument":""}),
    );
    assert_eq!(
        invalid["data"]["error"]["code"], "plugin.settings.invalidDocument",
        "{invalid}"
    );
}

#[test]
fn invalid_stored_values_fall_back_without_hiding_raw_executable_overrides() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-documents.json"
    )))
    .unwrap();
    let result = execute(
        "plugins.declarative.settingsState",
        json!({"manifest": fixture["manifest"],
        "globalDocument": "{\"dev.example.settings\":{\"flag\":\"false\",\"program\":\"relative\",\"dormant\":true}}",
        "projectDocument": "{\"dev.example.settings\":{\"count\":9}}"}),
    );
    let state = &result["data"]["state"];
    let rows = state["rows"].as_array().unwrap();
    let flag = rows
        .iter()
        .find(|row| row["declaration"]["id"] == "flag")
        .unwrap();
    assert_eq!(flag["effective"]["value"], true);
    assert_eq!(flag["effective"]["source"], "default");
    let count = rows
        .iter()
        .find(|row| row["declaration"]["id"] == "count")
        .unwrap();
    assert_eq!(count["effective"]["value"], 3);
    assert_eq!(state["settings"]["program"], "relative");
    assert!(state["settings"].get("dormant").is_none());
}

#[test]
fn document_size_and_batch_limits_reject_before_returning_replacement_text() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-documents.json"
    )))
    .unwrap();
    let oversized = execute(
        "plugins.declarative.editSettings",
        json!({"manifest":fixture["manifest"],
        "scope":"global", "document":" ".repeat(1024*1024+1), "edits":[{"id":"flag","value":false}]}),
    );
    assert_eq!(
        oversized["data"]["error"]["code"],
        "plugin.settings.invalidDocument"
    );
    let edits: Vec<_> = (0..33)
        .map(|_| json!({"id":"flag","value":false}))
        .collect();
    let oversized = execute(
        "plugins.declarative.editSettings",
        json!({"manifest":fixture["manifest"],
        "scope":"global", "edits":edits}),
    );
    assert_eq!(
        oversized["data"]["error"]["code"],
        "plugin.settings.invalidEdit"
    );
}

#[test]
fn plugins_without_setting_declarations_do_not_request_native_document_reads() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-documents.json"
    )))
    .unwrap();
    let mut manifest = fixture["manifest"].clone();
    manifest.as_object_mut().unwrap().remove("settings");
    manifest["requires"]
        .as_object_mut()
        .unwrap()
        .remove("settings");
    let result = execute(
        "plugins.declarative.settingsState",
        json!({"manifest": manifest}),
    );
    assert_eq!(result["data"]["status"], "ok", "{result}");
    assert_eq!(result["data"]["state"]["storage"]["readRequired"], false);
    assert_eq!(result["data"]["state"]["rows"], json!([]));
}
