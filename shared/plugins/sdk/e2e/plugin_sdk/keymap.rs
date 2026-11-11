//! The same selected-source import over Core for each official backend SDK.
use super::{core, Fixture, Session, WAIT};
use serde_json::{json, Value};
use std::fs;

pub fn run(label: &str, executable: &str, prefix: &[String]) {
    let fixture = Fixture::new(label);
    let workspace = fixture.workspace();
    let state = fixture.root.join("state");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&state).unwrap();
    let mut arguments: Vec<Value> = prefix.iter().map(|arg| json!(arg)).collect();
    arguments.extend([json!("--state"), json!(state)]);
    let plugin_id = format!("org.example.sdk-keymap-{label}");
    let started = core(
        "lsp.startServer",
        json!({
            "providerId": format!("pluginBackend:{plugin_id}"), "pluginId": plugin_id,
            "executablePath": executable, "arguments": arguments,
            "rootUri": format!("file://{}/", workspace.display()), "workingDirectory": workspace,
            "hostServices": ["lithe/host/keymapImport/listInputs", "lithe/host/keymapImport/readChunk"],
            "keymapImportProviders": ["fixture"], "initializeTimeoutMilliseconds": WAIT.as_millis() as u64,
            "requestTimeoutMilliseconds": WAIT.as_millis() as u64, "shutdownTimeoutMilliseconds": 5000
        }),
    );
    let mut session = Session {
        id: started["sessionId"].as_str().unwrap().into(),
        plugin_id,
        events: Vec::new(),
    };
    session.await_event("keymap provider negotiation", |event| {
        event["type"] == "keymapImportProviders"
    });
    session.await_event("session ready", |event| {
        event["type"] == "stateChanged" && event["state"] == "ready"
    });
    core(
        "lsp.keymapImportRequest",
        json!({"sessionId":session.id, "operationId":"keymap-e2e",
        "providerID":"fixture", "snapshotID":"snapshot-1", "timeoutMilliseconds":5000}),
    );
    for (method, result) in [
        (
            "lithe/host/keymapImport/listInputs",
            json!({"files":[{"id":"file-0","path":"keys.json","byteCount":4}]}),
        ),
        (
            "lithe/host/keymapImport/readChunk",
            json!({"data":"ZGF0YQ=="}),
        ),
    ] {
        let request = session.await_event(method, |event| {
            event["type"] == "hostRequest" && event["method"] == method
        });
        assert_eq!(request["result"]["params"]["snapshotID"], "snapshot-1");
        core(
            "lsp.hostResponse",
            json!({"sessionId":session.id, "hostRequestID":request["result"]["hostRequestID"], "result":result}),
        );
    }
    let completed = session.completion("keymap-e2e");
    let reply = completed["result"]["value"]["response"]["result"].clone();
    let normalized = core(
        "keymap.decodeImport",
        json!({"commandIDs":["find-next"], "reply":reply}),
    );
    assert_eq!(
        normalized,
        json!({"bindings":[{"commandID":"find-next", "keys":[{"key":"f3", "modifiers":[]}]}]})
    );
    session.stop();
}
