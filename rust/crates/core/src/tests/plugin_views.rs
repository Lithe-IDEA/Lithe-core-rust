//! Plugin views through Core's JSON entry point: the manifest declares where a
//! view is drawn, and `plugins.ui.applyResult` checks each backend result
//! against the shared fixture every host and SDK reads.

use lithe_plugin_manifest::validate_command;
use serde_json::{json, Value};

const MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../shared/plugins/contract/v1/fixtures/ui/view-panel.plugin.json"
));
const RESULTS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../shared/plugins/contract/v1/fixtures/ui/view-results.json"
));

fn manifest() -> Value {
    serde_json::from_str(MANIFEST).expect("view panel fixture")
}

fn validate(manifest: Value) -> Value {
    validate_command(json!({ "manifest": manifest })).expect("payload is well formed")
}

fn apply_result(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id": "view-test", "command": "plugins.ui.applyResult", "payload": payload})
            .to_string(),
    ))
    .expect("Core response")
}

#[test]
fn shared_view_results_match_the_fixture() {
    let fixture: Value = serde_json::from_str(RESULTS).expect("view results fixture");
    assert_eq!(fixture["manifest"], "view-panel.plugin.json");
    for case in fixture["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().expect("name");
        let mut payload = json!({
            "manifest": manifest(),
            "surface": case["surface"],
            "result": case["result"],
        });
        if let Some(view) = case.get("view") {
            payload["view"] = view.clone();
        }
        let response = apply_result(payload);
        assert_eq!(response["ok"], true, "{name}: {response}");
        let outcome = &response["data"];
        let expected = &case["expected"];
        assert_eq!(outcome["status"], expected["status"], "{name}: {outcome}");
        if expected["status"] == "ok" {
            let view = match &expected["view"] {
                Value::String(pointer) if pointer == "result.view" => &case["result"]["view"],
                view => view,
            };
            assert_eq!(&outcome["view"]["view"], view, "{name}");
            assert_eq!(outcome["view"]["changed"], expected["changed"], "{name}");
        } else {
            assert_eq!(outcome["error"]["code"], "plugin.ui.invalidView", "{name}");
            assert_eq!(
                outcome["error"]["field"], expected["field"],
                "{name}: {outcome}"
            );
        }
    }
}

#[test]
fn a_malformed_apply_request_is_a_core_failure() {
    let response = apply_result(json!({"manifest": manifest(), "result": {}}));
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_request");
}

/// The manifest side: a view needs a backend, `ui` needs a host that offers
/// it, and a window body is exactly one of text, web and view.
#[test]
fn view_declarations_need_a_backend_and_one_body() {
    let manifest = manifest();
    let result = validate(manifest.clone());
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(
        result["plugin"]["ui"]["toolWindows"][0]["view"],
        json!({"function": "explorer.draw"})
    );
    assert_eq!(result["plugin"]["settingRegions"][0]["id"], "overview");

    let without_ui = validate_command(json!({
        "manifest": manifest, "hostCapabilities": {"backend": [0], "settings": [0]}
    }))
    .expect("payload is well formed");
    assert_eq!(without_ui["error"]["code"], "plugin.api.unsupported");
    assert_eq!(without_ui["error"]["field"], "requires.ui");

    let mut mixed = manifest.clone();
    mixed["ui"]["toolWindows"][0]["text"] = json!("cannot mix body types");
    let rejected = validate(mixed);
    assert_eq!(
        rejected["error"]["field"], "ui.toolWindows[0]",
        "{rejected}"
    );

    for (pointer, value) in [
        ("/ui/toolWindows/0/view/function", json!("Explorer Draw")),
        (
            "/ui/toolWindows/0/view/source",
            json!({"buildSystemID": "sample"}),
        ),
        ("/settingRegions/0/view/function", json!("")),
        ("/settingRegions/0/keywords", json!(["graph", "Graph"])),
        ("/settingRegions/0/id", json!("Overview")),
    ] {
        let mut invalid = manifest.clone();
        let (parent, key) = pointer.rsplit_once('/').expect("pointer");
        invalid
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .expect(parent)
            .insert(key.to_owned(), value);
        assert_eq!(validate(invalid)["status"], "rejected", "{pointer}");
    }

    let mut without_backend = manifest.clone();
    without_backend
        .as_object_mut()
        .expect("manifest")
        .remove("backend");
    without_backend["permissions"] = json!(["ui.contribute"]);
    without_backend["requires"] = json!({"settings": 0, "ui": 0});
    let rejected = validate(without_backend);
    assert_eq!(rejected["status"], "rejected", "{rejected}");
}

/// A region with only keywords needs no backend, and a region's view counts
/// as UI: it needs `ui.contribute` and the `ui` capability.
#[test]
fn keyword_regions_need_no_backend_and_view_regions_are_ui() {
    let keywords_only = json!({
        "schemaVersion": 0,
        "id": "dev.lithe.fixture.keywords",
        "displayName": "Keywords",
        "version": "1.0.0",
        "pluginAPIVersion": 0,
        "requires": {"languageServers": 0, "languages": 0, "settings": 0},
        "vendor": {"id": "org.example", "displayName": "Example", "signatureRequirement": "publisherPackage"},
        "permissions": ["languageServer.launch"],
        "activationEvents": ["onLanguage:fictional"],
        "languages": [{"id": "fictional", "displayName": "Fictional", "fileExtensions": ["fictional"]}],
        "languageServers": [{"languageID": "fictional",
            "launch": {"kind": "systemCommand", "commands": ["fictional-ls"]}}],
        "settings": [{"id": "offline", "type": "boolean", "title": "Offline", "default": false}],
        "settingRegions": [{"id": "search", "keywords": ["airplane mode"]}]
    });
    let result = validate(keywords_only);
    assert_eq!(result["status"], "ok", "{result}");

    let mut region_only = manifest();
    region_only.as_object_mut().expect("manifest").remove("ui");
    region_only["activationEvents"] = json!([]);
    assert_eq!(validate(region_only.clone())["status"], "ok");
    region_only["permissions"] = json!(["backend.launch"]);
    assert_eq!(validate(region_only.clone())["status"], "rejected");
    region_only["permissions"] = json!(["backend.launch", "ui.contribute"]);
    region_only["requires"] = json!({"backend": 0, "settings": 0});
    assert_eq!(validate(region_only)["status"], "rejected");
}
