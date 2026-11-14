//! Shortcut import through the dispatcher and the host manifest: binding
//! plans, provider replies, provider declarations and source plans.

use lithe_plugin_keymap_import::plan_sources_command;
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
use serde_json::{json, Value};

#[test]
fn import_binding_plans_follow_shared_fixtures() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/keymap/import-plan-v1.json"
    ))
    .unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let request = json!({"id": "fixture", "command": "keymap.planImportBindings", "payload": case["request"]});
        let response: Value =
            serde_json::from_str(&crate::execute_json(&request.to_string())).unwrap();
        if let Some(expected) = case.get("expected") {
            assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
            assert_eq!(&response["data"], expected, "{}", case["name"]);
        } else {
            assert_eq!(response["ok"], false, "{}", case["name"]);
            assert!(response.get("data").is_none());
        }
    }
}

#[test]
fn dispatcher_returns_the_validated_reply_without_partial_results() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/keymap/import-provider-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let request =
            json!({"id": "fixture", "command": "keymap.decodeImport", "payload": case["request"]});
        let response: Value =
            serde_json::from_str(&crate::execute_json(&request.to_string())).unwrap();
        assert_eq!(response["id"], "fixture");
        match case.get("expected") {
            Some(expected) => {
                assert_eq!(response["ok"], true, "{}", case["name"]);
                assert_eq!(&response["data"], expected, "{}", case["name"]);
            }
            None => {
                assert_eq!(response["ok"], false, "{}", case["name"]);
                assert_eq!(response["error"]["code"], "invalid_request");
                assert!(response.get("data").is_none());
            }
        }
    }
}

#[test]
fn import_provider_manifest_is_generic_bounded_and_permission_checked() {
    let plugin: Value = serde_json::from_str(include_str!(
        "../../../../../shared/plugins/contract/v1/fixtures/keymap-import/keymap-import-provider.plugin.json"
    ))
    .unwrap();
    let validate = lithe_plugin_manifest::validate_manifest_value;
    assert!(validate(plugin.clone()).is_ok());
    let host_without_importers = json!({"backend": [0]});
    let host_with_importers = json!({"backend": [0], "keymapImportProviders": [0]});
    let command = lithe_plugin_manifest::validate_command;
    let rejected = command(json!({
        "manifest": plugin,
        "hostCapabilities": host_without_importers
    }))
    .unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["error"]["code"], "plugin.api.unsupported");
    let accepted = command(json!({
        "manifest": plugin,
        "hostCapabilities": host_with_importers
    }))
    .unwrap();
    assert_eq!(accepted["status"], "ok");

    let mut duplicate = plugin.clone();
    duplicate["keymapImportProviders"][1]["id"] = json!("first");
    assert!(validate(duplicate).is_err());
    let mut malformed = plugin.clone();
    malformed["keymapImportProviders"][0]["id"] = json!("../private");
    assert!(validate(malformed).is_err());
    let mut traversal = plugin.clone();
    traversal["keymapImportProviders"][0]["inputs"][0]["path"] = json!("../credentials");
    assert!(validate(traversal).is_err());
    let mut wildcard = plugin.clone();
    wildcard["keymapImportProviders"][1]["inputs"][1]["extension"] = json!("*");
    assert!(validate(wildcard).is_err());
    let mut duplicate_inputs = plugin.clone();
    duplicate_inputs["keymapImportProviders"][0]["inputs"] = json!([
        {"kind": "file", "path": "bindings.json"},
        {"kind": "file", "path": "bindings.json"}
    ]);
    assert!(validate(duplicate_inputs).is_err());
    let mut no_recognition = plugin.clone();
    no_recognition["keymapImportProviders"][0]["recognition"] = json!([]);
    assert!(validate(no_recognition).is_err());
    let mut duplicate_recognition = plugin.clone();
    duplicate_recognition["keymapImportProviders"][0]["recognition"] = json!([
        {"kind": "file", "path": "bindings.json"}, {"kind": "file", "path": "bindings.json"}
    ]);
    assert!(validate(duplicate_recognition).is_err());
    let mut bad_root = plugin.clone();
    bad_root["keymapImportProviders"][0]["recognition"][1]["components"] = json!(["..", "User"]);
    assert!(validate(bad_root).is_err());
    let mut bad_pattern = plugin.clone();
    bad_pattern["keymapImportProviders"][0]["recognition"][1]["components"] =
        json!(["First*", "User"]);
    assert!(validate(bad_pattern).is_err());
    let mut too_broad = plugin.clone();
    too_broad["keymapImportProviders"][0]["recognition"][1]["components"] = json!(["*", "*"]);
    assert!(validate(too_broad).is_err());
    let mut unanchored = plugin.clone();
    unanchored["keymapImportProviders"][0]["recognition"][1]["components"] = json!(["*", "User"]);
    assert!(validate(unanchored).is_err());
    let mut international = plugin.clone();
    international["keymapImportProviders"][0]["displayName"] = json!("编".repeat(64));
    assert!(validate(international.clone()).is_ok());
    international["keymapImportProviders"][0]["displayName"] = json!("编".repeat(65));
    assert!(validate(international).is_err());
    let mut no_grant = plugin.clone();
    no_grant["permissions"] = json!(["backend.launch"]);
    assert_eq!(
        validate(no_grant).unwrap_err().code,
        "plugin.permission.undeclared"
    );
    let mut no_source_grant = plugin.clone();
    no_source_grant["permissions"] = json!(["backend.launch", "keymaps.provide"]);
    assert_eq!(
        validate(no_source_grant).unwrap_err().code,
        "plugin.permission.undeclared"
    );
    let mut unknown_backend_version = plugin.clone();
    unknown_backend_version["requires"]["backend"] = json!(1);
    assert_eq!(
        validate(unknown_backend_version).unwrap_err().code,
        "plugin.api.unsupported"
    );
    let mut unsafe_launch = plugin.clone();
    unsafe_launch["backend"]["launch"] =
        json!({"kind": "runtime", "tool": "node", "entry": "main.mjs"});
    assert!(validate(unsafe_launch).is_err());
    let mut too_many = plugin;
    too_many["keymapImportProviders"] = json!((0..9)
        .map(|i| json!({"id": format!("provider-{i}"), "displayName": "Provider"}))
        .collect::<Vec<_>>());
    assert!(validate(too_many).is_err());
}

#[test]
fn source_plans_only_expose_inputs_of_the_requested_provider() {
    let plugin: Value = serde_json::from_str(include_str!(
        "../../../../../shared/plugins/contract/v1/fixtures/keymap-import/keymap-import-provider.plugin.json"
    ))
    .unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/keymap/source-plans-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let actual = plan_sources_command::<Manifest>(
            json!({"manifest": plugin, "providerID": case["providerID"]}),
        );
        if case["expected"].is_null() {
            assert!(actual.is_err());
        } else {
            let actual = actual.unwrap();
            assert_eq!(actual["inputs"], case["expected"]["inputs"]);
            assert_eq!(actual["recognition"], case["expected"]["recognition"]);
            assert_eq!(actual["limits"], fixture["limits"]);
        }
    }
    let mut invalid = plugin;
    invalid["keymapImportProviders"][0]["inputs"][0]["path"] = json!("../credentials");
    assert!(
        plan_sources_command::<Manifest>(json!({"manifest": invalid, "providerID": "second"}))
            .is_err()
    );
    let mut invalid: Value = serde_json::from_str(include_str!(
        "../../../../../shared/plugins/contract/v1/fixtures/keymap-import/keymap-import-provider.plugin.json"
    ))
    .unwrap();
    invalid["keymapImportProviders"][1]["recognition"][0]["path"] = json!("../private");
    assert!(
        plan_sources_command::<Manifest>(json!({"manifest": invalid, "providerID": "first"}))
            .is_err()
    );
}
