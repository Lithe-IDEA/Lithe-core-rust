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

#[test]
fn editor_projection_owns_scope_reset_values_and_section_grouping() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-documents.json"
    )))
    .unwrap();
    let mut manifest = fixture["manifest"].clone();
    manifest["settings"][0]["section"] = json!("Server");
    manifest["settings"][1]["section"] = json!("General");
    let result = execute(
        "plugins.declarative.settingsState",
        json!({"manifest": manifest,
        "globalDocument":"{\"dev.example.settings\":{\"flag\":false,\"count\":\"invalid\"}}",
        "projectDocument":"{\"dev.example.settings\":{\"flag\":true}}"}),
    );
    assert_eq!(result["data"]["status"], "ok", "{result}");
    let state = &result["data"]["state"];
    assert_eq!(state["metadata"]["displayName"], "Settings Fixture");
    assert_eq!(state["metadata"]["version"], "1.0.0");
    assert_eq!(state["sections"][0]["title"], "");
    assert_eq!(state["sections"][1]["title"], "General");
    assert_eq!(state["sections"][2]["title"], "Server");
    let rows = state["rows"].as_array().unwrap();
    let flag = rows
        .iter()
        .position(|row| row["declaration"]["id"] == "flag")
        .unwrap();
    let count = rows
        .iter()
        .position(|row| row["declaration"]["id"] == "count")
        .unwrap();
    assert_eq!(state["sections"][1]["rows"], json!([count]));
    assert_eq!(state["sections"][2]["rows"], json!([flag]));
    let project = &state["editors"]["project"];
    assert_eq!(project[flag]["value"], true);
    assert_eq!(project[flag]["input"]["on"], true);
    assert_eq!(project[flag]["resetInput"]["on"], false);
    assert_eq!(project[flag]["resetValue"], false);
    assert_eq!(project[flag]["editable"], true);
    assert_eq!(project[flag]["overridden"], true);
    assert_eq!(project[count]["editable"], false);
    assert_eq!(project[count]["value"], 3);
    assert_eq!(project[count]["input"]["text"], "3");
    assert_eq!(project[count]["invalidStored"], true);
    let global = &state["editors"]["global"];
    assert_eq!(global[flag]["value"], false);
    assert_eq!(global[flag]["resetValue"], true);
    assert_eq!(global[flag]["source"], "global");
    assert_eq!(global[count]["invalidStored"], true);
}

#[test]
fn setting_action_plans_match_shared_fixture_without_starting_a_process() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-actions.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut declared = manifest();
        if let Some(confirmation) = case.get("confirmation") {
            declared["settingActions"][0]["confirm"] = confirmation.clone();
        }
        let mut input = case["input"].clone();
        input["manifest"] = declared;
        let response = execute("plugins.declarative.settingsActionPlan", input);
        if let Some(code) = case.get("error") {
            assert_eq!(
                &response["data"]["error"]["code"], code,
                "{case}: {response}"
            );
        } else {
            assert_eq!(response["data"]["plan"], case["plan"], "{case}: {response}");
        }
    }
}

#[test]
fn settings_surfaces_shared_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/fixtures/plugins/settings-surfaces.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = execute(case["command"].as_str().unwrap(), case["input"].clone());
        assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
        let data = &response["data"];
        assert_eq!(
            data["status"], case["status"],
            "{}: {response}",
            case["name"]
        );
        if let Some(count) = case["pages"].as_u64() {
            assert_eq!(
                data["pages"].as_array().unwrap().len() as u64,
                count,
                "{response}"
            );
        }
        if !case["delivery"].is_null() {
            assert_eq!(data["delivery"], case["delivery"], "{response}");
        }
        if !case["rowIDs"].is_null() {
            assert_eq!(
                json!(data["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["id"].clone())
                    .collect::<Vec<_>>()),
                case["rowIDs"],
                "{response}"
            );
        }
    }
}

#[test]
fn settings_navigation_reserves_capacity_for_host_rows() {
    let mut items = vec![
        json!({"id":"languages","title":"Languages","parent":null,"group":false,"expanded":false}),
    ];
    for index in 0..512 {
        items.push(json!({"id":format!("language:{index}"),"title":format!("Language {index}"),"parent":"languages","group":false,"expanded":false}));
    }
    let response = execute(
        "plugins.settings.navigation",
        json!({"items":items,"query":""}),
    );
    assert_eq!(response["data"]["status"], "ok", "{response}");
    assert_eq!(response["data"]["rows"].as_array().unwrap().len(), 513);
}
