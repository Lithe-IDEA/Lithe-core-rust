//! Drives a Rust SDK toolchain query over Core's actual transport and decoder.

use super::{core, Fixture, Session, WAIT};
use serde_json::json;
use std::fs;

pub fn run(executable: &str) {
    let fixture = Fixture::new("rust-toolchain");
    let workspace = fixture.workspace();
    let state = fixture.root.join("state");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&state).unwrap();
    let plugin_id = "org.example.sdk-toolchain";
    let manifest = json!({
        "schemaVersion": 0, "pluginAPIVersion": 0,
        "id": plugin_id, "displayName": "SDK toolchain fixture", "version": "1.0.0",
        "requires": {"backend": 0, "toolchainProviders": 0},
        "vendor": {"id": "org.example", "displayName": "Fixture publisher", "signatureRequirement": "publisherPackage"},
        "permissions": ["backend.launch", "toolchains.provide"],
        "toolchainProviders": [{"id": "fixture", "displayName": "SDK fixture", "tools": ["fixture-runtime"]}],
        "backend": {"launch": {"kind": "packaged", "path": "bin/sample-backend"}, "arguments": ["--state", "${stateDir}"]},
    });
    let registry = json!({"plugins": [{"manifest": manifest}]});
    let validated = core("toolchains.registry", registry.clone());
    assert_eq!(validated["status"], "ok", "{validated}");
    assert_eq!(
        validated["registry"]["providers"][0]["provider"]["id"],
        "fixture"
    );
    let started = core(
        "lsp.startServer",
        json!({
            "providerId": format!("pluginBackend:{plugin_id}"), "pluginId": plugin_id,
            "executablePath": executable, "arguments": ["--state", state],
            "rootUri": format!("file://{}/", workspace.display()), "workingDirectory": workspace,
            "toolchainProviders": ["fixture"], "initializeTimeoutMilliseconds": WAIT.as_millis() as u64,
            "requestTimeoutMilliseconds": WAIT.as_millis() as u64, "shutdownTimeoutMilliseconds": 5000,
        }),
    );
    let mut session = Session {
        id: started["sessionId"].as_str().expect("session ID").into(),
        plugin_id: plugin_id.into(),
        events: Vec::new(),
    };
    let negotiated = session.await_event("toolchain provider negotiation", |event| {
        event["type"] == "toolchainProviders"
    });
    assert_eq!(negotiated["result"]["providers"], json!(["fixture"]));
    session.await_event("session ready", |event| {
        event["type"] == "stateChanged" && event["state"] == "ready"
    });

    let operation = "sdk-toolchain-query";
    let params = json!({
        "operationID": operation, "generation": 3, "providerID": "fixture",
        "authority": {"kind": "local"}, "context": null, "workspace": null,
        "tools": ["fixture-runtime"],
    });
    core(
        "lsp.toolchainRequest",
        json!({
            "sessionId": session.id, "operationId": operation, "params": params,
            "timeoutMilliseconds": 5000,
        }),
    );
    let completed = session.completion(operation);
    assert!(completed.get("error").is_none(), "{completed}");
    let wire = &completed["result"]["value"];
    let decoded = core(
        "toolchains.decodeReply",
        json!({
            "registry": registry, "pluginID": plugin_id, "currentGeneration": 3,
            "request": {"jsonrpc": "2.0", "id": wire["id"], "method": "toolchains/query", "params": params},
            "response": wire["response"], "platform": "macos", "filesystem": [],
        }),
    );
    assert_eq!(decoded["status"], "ok", "{decoded}");
    assert_eq!(decoded["snapshot"]["installations"], json!([]));
    assert_eq!(decoded["snapshot"]["statements"], json!([]));
    assert_eq!(decoded["snapshot"]["watch"], json!([]));
    session.stop();
}
