//! Public-dispatch coverage for the shared source-backed settings fixture.

use serde_json::{json, Value};

#[test]
fn native_source_settings_fixture_is_available_through_the_public_boundary() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/plugins/settings-sources.json"
    )))
    .unwrap();
    let response: Value = serde_json::from_str(&crate::execute_json(
        &json!({"command":"plugins.settings.sourceState","payload":{"result":fixture["snapshot"]}})
            .to_string(),
    ))
    .unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["status"], "ok", "{response}");
    assert_eq!(
        response["data"]["state"]["inputs"]["root"]["target"]["inherit"],
        true
    );
}

#[test]
fn source_editor_actions_require_explicit_host_capability_and_keep_their_binding() {
    let manifest: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/settings/source-editor.plugin.json"
    )))
    .unwrap();
    let run = |command: &str, payload: Value| -> Value {
        serde_json::from_str(&crate::execute_json(
            &json!({"command":command,"payload":payload}).to_string(),
        ))
        .unwrap()
    };
    let old_host = json!({"backend":[0],"languages":[0],"settings":[0]});
    let refused = run(
        "plugins.declarative.validate",
        json!({"manifest":manifest,"hostCapabilities":old_host}),
    );
    assert_eq!(
        refused["data"]["error"]["code"], "plugin.api.unsupported",
        "{refused}"
    );
    let plan = run(
        "plugins.declarative.settingsActionPlan",
        json!({
            "manifest":manifest,"owner":{"pluginID":"dev.lithe.test.source-settings","version":"1.0.0"},
            "actionID":"compiler","enabled":true,"confirmed":false,"workspaceAvailable":true
        }),
    );
    assert_eq!(plan["data"]["plan"]["sourceEditor"], true, "{plan}");
    assert_eq!(plan["data"]["plan"]["function"], "compiler.settings");
    assert_eq!(plan["data"]["plan"]["params"], json!({}));
}
