//! `plugins.installed.select` against the shared installed-version fixture.

use serde_json::{json, Value};

#[test]
fn shared_installed_version_fixture_uses_the_existing_numeric_comparator() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/plugins/installed-versions.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response: Value = serde_json::from_str(&crate::execute_json(&json!({
            "command":"plugins.installed.select", "payload":{
                "entries":case["entries"], "includeDisabled":case["includeDisabled"].as_bool().unwrap_or(false)
            }
        }).to_string())).unwrap();
        assert_eq!(response["ok"], true, "case {}: {response}", case["name"]);
        assert_eq!(
            response["data"]["selected"], case["selected"],
            "case {}",
            case["name"]
        );
        assert_eq!(
            response["data"]["rejected"], case["rejected"],
            "case {}",
            case["name"]
        );
    }
}
