//! Bridge and loading rules of plugin pages, against the shared web-panel fixture.

use super::*;

fn manifest() -> Value {
    serde_json::from_str(include_str!(
        "../../../../shared/contracts/plugins/v1/fixtures/ui/web-panel.plugin.json"
    ))
    .expect("fixture")
}

fn theme() -> Value {
    json!({ "theme": "dark", "variables": { "--lithe-background": "#1e1f22" } })
}

fn open(build_systems: bool) -> u64 {
    let opened = open_command(json!({
        "manifest": manifest(),
        "windowID": "dev.lithe.fixture.web-panel.window",
        "platform": "macos",
        "locale": "zh-CN",
        "theme": theme(),
        "buildSystems": build_systems,
    }))
    .expect("open");
    assert_eq!(
        opened["url"],
        "lithe-plugin://dev.lithe.fixture.web-panel/ui/index.html"
    );
    assert_eq!(opened["bridgeVersion"], 0);
    opened["page"].as_u64().expect("page")
}

fn request(page: u64, id: u64, method: &str, params: Value) -> Value {
    request_command(json!({
        "page": page,
        "message": json!({ "id": id, "method": method, "params": params }).to_string(),
    }))
    .expect("request")
}

fn refusal_code(outcome: &Value) -> String {
    let reply: Value =
        serde_json::from_str(outcome["reply"].as_str().expect("reply")).expect("json");
    reply["error"]["code"].as_str().expect("code").to_owned()
}

#[test]
fn only_a_declared_web_window_opens() {
    let text_window = open_command(json!({
        "manifest": manifest(),
        "windowID": "dev.lithe.fixture.web-panel.notes",
        "platform": "macos",
        "locale": "en",
        "theme": theme(),
    }));
    assert!(text_window.is_err(), "a text window has no page");
    let bad_theme = open_command(json!({
        "manifest": manifest(),
        "windowID": "dev.lithe.fixture.web-panel.window",
        "platform": "macos",
        "locale": "en",
        "theme": { "theme": "dark", "variables": { "color": "red" } },
    }));
    assert!(bad_theme.is_err(), "only --lithe-* variables");
}

#[test]
fn resources_stay_inside_the_package() {
    let page = open(false);
    let resource =
        |path: &str| resource_command(json!({ "page": page, "path": path })).expect("resource");
    let bootstrap = resource("/.lithe-host/bridge.js");
    assert_eq!(bootstrap["kind"], "bootstrap");
    let script = bootstrap["body"].as_str().expect("script");
    assert!(script.contains("\"pluginID\":\"dev.lithe.fixture.web-panel\""));
    assert!(script.contains("version: 0"));
    assert!(!script.contains("__LITHE_"));
    let entry = resource("/ui/index.html");
    assert_eq!(entry["kind"], "file");
    assert_eq!(entry["segments"], json!(["ui", "index.html"]));
    assert_eq!(entry["document"], true);
    assert_eq!(
        resource("ui/app.JS")["mime"],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(resource("ui/app.JS")["document"], false);
    for escape in [
        "",
        "/",
        "../plugin.json",
        "ui/../../x",
        "ui//a.js",
        "ui/./a.js",
        "a\\b",
        "C:/x",
        "ui/\u{7}",
        ".lithe-host/other.js",
    ] {
        assert_eq!(resource(escape)["kind"], "refused", "{escape:?}");
    }
    close_command(json!({ "page": page })).expect("close");
    assert!(resource_command(json!({ "page": page, "path": "ui/index.html" })).is_err());
}

#[test]
fn the_policy_and_bridge_precede_the_package_markup() {
    let head = "<X>";
    assert_eq!(
        inject_head("<!DOCTYPE html><html><head><title>t</title>", head),
        "<!DOCTYPE html><html><head><X><title>t</title>"
    );
    assert_eq!(
        inject_head("<html lang=en><body><script src=a.js></script>", head),
        "<html lang=en><X><body><script src=a.js></script>"
    );
    assert_eq!(
        inject_head("<!doctype html><script src=a.js></script>", head),
        "<!doctype html><X><script src=a.js></script>"
    );
    assert_eq!(
        inject_head("<script src=a.js></script>", head),
        "<X><script src=a.js></script>"
    );
    // `<header>` is not `<head>`, and a head after the body does not count.
    assert_eq!(
        inject_head("<header>h</header>", head),
        "<X><header>h</header>"
    );
    assert_eq!(
        inject_head("<body>b</body><head>", head),
        "<X><body>b</body><head>"
    );

    let page = open(false);
    let document =
        document_command(json!({ "page": page, "html": "<head></head>" })).expect("document");
    let html = document["html"].as_str().expect("html");
    assert!(html.starts_with("<head><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'self' lithe-plugin://dev.lithe.fixture.web-panel;"));
    assert!(html.contains("connect-src 'self' lithe-plugin://dev.lithe.fixture.web-panel;"));
    assert!(html.contains("<script src=\"/.lithe-host/bridge.js\"></script></head>"));
    close_command(json!({ "page": page })).expect("close");
}

#[test]
fn unknown_and_malformed_requests_are_refused() {
    let page = open(false);
    assert_eq!(
        refusal_code(&request(page, 1, "shell.exec", json!({}))),
        "plugin.web.unknownMethod"
    );
    assert_eq!(
        refusal_code(&request(page, 2, "build.instances", json!({}))),
        "plugin.web.unknownMethod"
    );
    assert_eq!(
        refusal_code(&request(page, 3, "ui.getState", json!({ "extra": 1 }))),
        "plugin.web.invalidRequest"
    );
    assert_eq!(
        refusal_code(&request(
            page,
            4,
            "functions.invoke",
            json!({ "instance": "x", "function": "f" })
        )),
        "plugin.web.forbidden"
    );
    assert_eq!(
        refusal_code(&request(
            page,
            5,
            "workspace.openFile",
            json!({ "path": "src/../../etc/passwd" })
        )),
        "plugin.web.forbidden"
    );
    assert_eq!(
        refusal_code(&request(
            page,
            6,
            "workspace.openFile",
            json!({ "path": "a", "line": 0 })
        )),
        "plugin.web.invalidRequest"
    );
    let unanswerable =
        request_command(json!({ "page": page, "message": "{\"method\":\"ui.getState\"}" }))
            .expect("request");
    assert!(unanswerable["ignored"].is_string());
    let reply: Value = serde_json::from_str(
        request(page, 7, "nope", json!({}))["reply"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        reply["id"], 7,
        "a refusal answers the request that caused it"
    );
    close_command(json!({ "page": page })).expect("close");
}

#[test]
fn state_is_bounded_and_null_clears_it() {
    let page = open(false);
    let set = request(page, 1, "ui.setState", json!({ "state": { "tab": 2 } }));
    assert_eq!(set["action"]["params"]["state"], "{\"tab\":2}");
    let clear = request(page, 2, "ui.setState", json!({ "state": null }));
    assert_eq!(clear["action"]["params"], json!({}));
    let missing = request(page, 3, "ui.setState", json!({}));
    assert_eq!(refusal_code(&missing), "plugin.web.invalidRequest");
    let large = request(
        page,
        4,
        "ui.setState",
        json!({ "state": "x".repeat(STATE_LIMIT) }),
    );
    assert_eq!(refusal_code(&large), "plugin.web.invalidRequest");
    close_command(json!({ "page": page })).expect("close");
}

#[test]
fn call_ids_are_never_reused_and_calls_are_bounded() {
    let page = open(false);
    let invoke = request(
        page,
        1,
        "functions.invoke",
        json!({ "function": "db.query", "call": "c1", "params": { "sql": "1" } }),
    );
    let action = &invoke["action"];
    assert_eq!(action["params"]["call"], "c1");
    assert_eq!(action["params"]["progress"], true);
    assert_eq!(action["params"]["params"], json!({ "sql": "1" }));
    // Progress reaches the page while the call runs.
    let progress = event_command(json!({ "page": page, "event": "functions.progress", "payload": { "call": "c1", "function": "db.query" } })).expect("event");
    assert!(progress["message"].is_string());
    let cancel = request(page, 2, "functions.cancel", json!({ "call": "c1" }));
    assert_eq!(cancel["action"]["params"]["active"], true);
    let replied = reply_command(json!({ "page": page, "id": 1, "method": "functions.invoke", "call": "c1", "error": { "code": "requestCancelled", "message": "x" } })).expect("reply");
    let message: Value = serde_json::from_str(replied["message"].as_str().unwrap()).unwrap();
    assert_eq!(
        message,
        json!({ "id": 1, "error": { "code": "requestCancelled", "message": "x" } })
    );
    // Ended: cancel is a no-op, progress is dropped, the ID stays used.
    assert_eq!(
        request(page, 3, "functions.cancel", json!({ "call": "c1" }))["action"]["params"]["active"],
        false
    );
    let late = event_command(
        json!({ "page": page, "event": "functions.progress", "payload": { "call": "c1" } }),
    )
    .expect("event");
    assert!(late["message"].is_null());
    assert_eq!(
        refusal_code(&request(
            page,
            4,
            "functions.invoke",
            json!({ "function": "f", "call": "c1" })
        )),
        "plugin.web.invalidRequest"
    );

    // A call without an ID gets a host-made one that progress never reaches.
    let anonymous = request(page, 5, "functions.invoke", json!({ "function": "f" }));
    let host_call = anonymous["action"]["params"]["call"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(anonymous["action"]["params"]["progress"], false);
    let dropped = event_command(
        json!({ "page": page, "event": "functions.progress", "payload": { "call": host_call } }),
    )
    .expect("event");
    assert!(dropped["message"].is_null());
    for id in 0..(CALL_LIMIT as u64 - 1) {
        assert!(request(
            page,
            10 + id,
            "functions.invoke",
            json!({ "function": "f" })
        )["action"]
            .is_object());
    }
    assert_eq!(
        refusal_code(&request(
            page,
            99,
            "functions.invoke",
            json!({ "function": "f" })
        )),
        "plugin.build.unavailable"
    );
    let closed = close_command(json!({ "page": page })).expect("close");
    assert_eq!(closed["cancelCalls"].as_array().unwrap().len(), CALL_LIMIT);
    // Answers after close are dropped.
    let after = reply_command(json!({ "page": page, "id": 5, "method": "functions.invoke", "call": host_call, "result": { "value": 1 } })).expect("reply");
    assert!(after["message"].is_null());
}

#[test]
fn one_file_dialog_at_a_time() {
    let page = open(false);
    let open_dialog = request(
        page,
        1,
        "dialog.openFile",
        json!({ "title": "Import", "extensions": ["sql", "csv"] }),
    );
    assert_eq!(
        open_dialog["action"]["params"]["extensions"],
        json!(["sql", "csv"])
    );
    assert_eq!(
        refusal_code(&request(page, 2, "dialog.saveFile", json!({}))),
        "plugin.web.invalidRequest"
    );
    reply_command(
        json!({ "page": page, "id": 1, "method": "dialog.openFile", "result": { "path": null } }),
    )
    .expect("reply");
    let save = request(
        page,
        3,
        "dialog.saveFile",
        json!({ "suggestedName": "dump.sql" }),
    );
    assert_eq!(save["action"]["params"]["suggestedName"], "dump.sql");
    reply_command(
        json!({ "page": page, "id": 3, "method": "dialog.saveFile", "result": { "path": null } }),
    )
    .expect("reply");
    for bad in [
        json!({ "suggestedName": "../x" }),
        json!({ "extensions": [".sql"] }),
        json!({ "extensions": vec!["a"; 17] }),
    ] {
        assert_eq!(
            refusal_code(&request(page, 4, "dialog.saveFile", bad.clone())),
            "plugin.web.invalidRequest",
            "{bad}"
        );
    }
    assert_eq!(
        refusal_code(&request(
            page,
            5,
            "dialog.openFile",
            json!({ "suggestedName": "x" })
        )),
        "plugin.web.invalidRequest"
    );
    close_command(json!({ "page": page })).expect("close");
}

#[test]
fn theme_changes_reach_later_bridge_scripts() {
    let page = open(false);
    let changed = event_command(json!({ "page": page, "event": "ui.themeChanged", "payload": { "theme": "light", "variables": {} } })).expect("event");
    let message: Value = serde_json::from_str(changed["message"].as_str().unwrap()).unwrap();
    assert_eq!(message["event"], "ui.themeChanged");
    assert_eq!(message["payload"]["theme"], "light");
    let script = resource_command(json!({ "page": page, "path": ".lithe-host/bridge.js" }))
        .expect("resource");
    assert!(script["body"]
        .as_str()
        .unwrap()
        .contains("let theme = {\"theme\":\"light\""));
    assert!(event_command(json!({ "page": page, "event": "made.up", "payload": {} })).is_err());
    close_command(json!({ "page": page })).expect("close");
}

#[test]
fn build_methods_pass_to_a_host_that_serves_them() {
    let page = open(true);
    let instances = request(page, 1, "build.instances", json!({}));
    assert_eq!(instances["action"]["method"], "build.instances");
    let with_instance = request(
        page,
        2,
        "functions.list",
        json!({ "instance": "maven:root" }),
    );
    assert_eq!(with_instance["action"]["params"]["instance"], "maven:root");
    close_command(json!({ "page": page })).expect("close");
}
