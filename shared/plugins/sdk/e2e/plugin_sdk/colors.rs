//! Color decoration registration and results through each SDK's generic host API.

use super::{core, Fixture, Session, WAIT};
use serde_json::{json, Value};
use std::fs;

/// Starts a sample with only the editor provider grant, forwards its generic
/// registration through Core, and decodes a versioned color decoration answer.
pub fn run(label: &str, executable: &str, prefix: &[String]) {
    let fixture = Fixture::new(&format!("colors-{label}"));
    let workspace = fixture.workspace();
    let state = fixture.root.join("state");
    fs::create_dir_all(&workspace).expect("fixture workspace");
    fs::create_dir_all(&state).expect("fixture state");
    let uri = core(
        "lsp.documentUri",
        json!({"path": workspace, "platform":"auto"}),
    );
    let state_uri = lithe_document_uri::DocumentPathPlatform::Auto
        .document_uri(&state.to_string_lossy())
        .expect("state URI");
    let state_argument = lithe_document_uri::file_path_from_uri(&state_uri);
    let mut arguments: Vec<Value> = prefix.iter().map(|argument| json!(argument)).collect();
    arguments.extend([json!("--state"), json!(state_argument)]);
    let plugin_id = format!("dev.lithe.test.colors.{label}");
    let started = core(
        "lsp.startServer",
        json!({
            "providerId":format!("pluginBackend:{plugin_id}"), "pluginId":plugin_id,
            "executablePath":executable, "arguments":arguments,
            "rootUri":uri["uri"], "workingDirectory":workspace,
            "hostServices":["lithe/host/editor/registerProviders"],
            "hostOptions":{"editor":{"actionIcons":["run"],"actionSourceBytes":131072,
                "providerTypes":["colorDecorations"]}},
            "initializeTimeoutMilliseconds":WAIT.as_millis() as u64,
            "requestTimeoutMilliseconds":WAIT.as_millis() as u64,
            "shutdownTimeoutMilliseconds":5000
        }),
    );
    let mut session = Session {
        id: started["sessionId"].as_str().expect("session ID").into(),
        plugin_id: plugin_id.clone(),
        events: Vec::new(),
    };
    let state = session.await_event("color fixture initialization", |event| {
        event["type"] == "stateChanged"
            && matches!(event["state"].as_str(), Some("ready" | "failed"))
    });
    assert_eq!(
        state["state"], "ready",
        "color fixture failed: {:#?}",
        session.events
    );
    let colors = json!([{"id":"accent-1","light":"#8B2050","dark":"#EE8CAA"}]);
    let registration = json!({"context":{"contextID":"colors","generation":1},
        "id":"color-fixture","selector":{"fileExtensions":["txt"]},
        "providers":{"colorDecorations":{"function":"fixture.colors","colorLegend":colors}}});
    session.invoke(
        "colors-register",
        "fixture.colors",
        json!({"registration":registration}),
    );
    let request = session.await_event("color registration host request", |event| {
        event["type"] == "hostRequest" && event["method"] == "lithe/host/editor/registerProviders"
    });
    let mut params = request["result"]["params"].clone();
    assert_eq!(params, registration);
    params
        .as_object_mut()
        .expect("registration")
        .remove("context");
    let normalized = core(
        "plugins.editorProviders.decode",
        json!({
        "kind":"registration","types":["colorDecorations"],"value":params}),
    );
    assert_eq!(
        normalized["registration"]["providers"]["colorDecorations"]["budgetMs"],
        2000
    );
    core(
        "lsp.hostResponse",
        json!({"sessionId":session.id,
        "hostRequestID":request["result"]["hostRequestID"],"result":{"subscriptionID":"colors-1"}}),
    );
    assert_eq!(
        session.completion("colors-register")["result"]["value"],
        json!({"subscriptionID":"colors-1"})
    );

    let value = json!({"decorations":[{"range":{"start":{"line":0,"character":2},
        "end":{"line":0,"character":4}},"colorId":"accent-1"}]});
    let answer = session.call(
        "colors-result",
        "fixture.colors",
        json!({
        "document":{"documentID":"doc-1","version":3},"value":value}),
    );
    let result = core(
        "plugins.editorProviders.decode",
        json!({"kind":"result",
        "type":"colorDecorations","colorLegend":colors,"value":answer["result"]["value"]}),
    );
    assert_eq!(result["result"], value);
    let merged = core(
        "plugins.editorProviders.decode",
        json!({"kind":"mergeColorDecorations",
        "value":{"sources":[{"sourceId":format!("{plugin_id}/color-fixture"),
            "colorLegend":colors,"value":result["result"]}]}}),
    );
    assert_eq!(merged["decorations"][0]["dark"], "#EE8CAA");
    assert_eq!(
        merged["decorations"][0]["range"],
        value["decorations"][0]["range"]
    );
    session.stop();
}
