//! Managed-runtime plans and resolver answers through the public JSON boundary.

use serde_json::{json, Value};

fn execute(command: &str, payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"command":command,"payload":payload}).to_string(),
    ))
    .unwrap()
}

#[test]
fn managed_runtime_fixture_crosses_the_public_command_boundary() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/plugins/managed-runtimes.json"
    )))
    .unwrap();
    let plan = execute(
        "plugins.managedRuntimes.plan",
        json!({"manifest":fixture["manifest"],"platform":fixture["platform"]}),
    );
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["data"]["status"], "ok", "{plan}");
    assert_eq!(plan["data"]["plan"]["dependencies"][0]["id"], "engine");
    let resolved = execute(
        "plugins.managedRuntimes.decodeResolution",
        json!({"manifest":fixture["manifest"],"platform":fixture["platform"],"result":fixture["resolution"]}),
    );
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(resolved["data"]["resolution"], fixture["resolution"]);
    let malformed = execute("plugins.managedRuntimes.plan", json!({}));
    assert_eq!(malformed["ok"], false, "{malformed}");
    assert_eq!(malformed["error"]["code"], "invalid_request");
}
