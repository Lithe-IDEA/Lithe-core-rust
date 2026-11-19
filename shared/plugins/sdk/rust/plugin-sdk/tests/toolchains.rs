//! Exercises toolchain queries over the SDK's actual framed transport.

mod common;

use common::Host;
use lithe_plugin_sdk::toolchains::{
    ToolchainInstallation, ToolchainQueryError, ToolchainQueryResult,
};
use lithe_plugin_sdk::Backend;
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

fn query(provider: &str) -> Value {
    json!({
        "operationID": "query-1", "generation": 3, "providerID": provider,
        "authority": {"kind": "local"}, "context": null, "workspace": null,
        "tools": ["fixture"],
    })
}

fn initialize(host: &mut Host, providers: Value, version: u64) -> Value {
    host.request(
        0,
        "initialize",
        json!({"capabilities": {"experimental": {
            "litheLogging": {"versions": [0]},
            "litheToolchains": {"versions": [version], "providers": providers}
        }}}),
    );
    host.answer(0).0["result"]["capabilities"]["experimental"]["litheToolchains"].clone()
}

fn finish(mut host: Host) {
    host.request("shutdown", "shutdown", Value::Null);
    assert_eq!(host.answer("shutdown").0["result"], Value::Null);
    host.notify("exit", Value::Null);
    assert_eq!(host.exit_code(), 0);
}

#[test]
fn negotiates_only_offered_providers_and_echoes_query_identity() {
    let mut backend = Backend::new("test-backend");
    backend
        .toolchain_provider("fixture", |request, cancellation| {
            assert_eq!(request.tools, ["fixture"]);
            cancellation.check()?;
            Ok(ToolchainQueryResult {
                installations: vec![ToolchainInstallation {
                    tool: "fixture".into(),
                    version: "2.0".into(),
                    home: "/example/install".into(),
                }],
                ..ToolchainQueryResult::default()
            })
        })
        .unwrap();
    backend
        .toolchain_provider("not-offered", |_, _| panic!("unoffered handler ran"))
        .unwrap();
    let mut host = Host::start(backend);
    assert_eq!(
        initialize(&mut host, json!(["fixture", "unknown"]), 0),
        json!({
            "version": 0, "providers": ["fixture"]
        })
    );
    host.request("call-1", "toolchains/query", query("fixture"));
    let reply = host.answer("call-1").0;
    assert_eq!(
        reply["result"],
        json!({
            "operationID": "query-1", "generation": 3, "providerID": "fixture",
            "installations": [{"tool": "fixture", "version": "2.0", "home": "/example/install"}],
            "statements": [], "watch": []
        })
    );
    host.request(2, "toolchains/query", query("not-offered"));
    assert_eq!(
        host.answer(2).0["error"]["data"]["code"],
        "plugin.toolchain.methodUnavailable"
    );
    let mut malformed = query("fixture");
    malformed["tools"] = json!(["fixture", "fixture"]);
    host.request(3, "toolchains/query", malformed);
    assert_eq!(host.answer(3).0["error"]["code"], -32602);
    finish(host);
}

#[test]
fn reply_budget_includes_the_json_rpc_envelope() {
    const LIMIT: usize = 1024 * 1024;
    let empty_reply = json!({
        "jsonrpc": "2.0", "id": "ok", "result": {
            "operationID": "query-1", "generation": 3, "providerID": "fixture",
            "installations": [], "statements": [], "watch": ["/fixture/"]
        }
    });
    let watch_path = format!(
        "/fixture/{}",
        "x".repeat(LIMIT - empty_reply.to_string().len())
    );
    let mut backend = Backend::new("test-backend");
    backend
        .toolchain_provider("fixture", move |_, _| {
            Ok(ToolchainQueryResult {
                watch: vec![watch_path.clone()],
                ..ToolchainQueryResult::default()
            })
        })
        .unwrap();
    let mut host = Host::start(backend);
    initialize(&mut host, json!(["fixture"]), 0);

    host.request("ok", "toolchains/query", query("fixture"));
    let at_limit = host.answer("ok").0;
    assert!(at_limit.get("result").is_some());
    assert_eq!(at_limit.to_string().len(), LIMIT);

    // The same result with a longer request ID crosses the reply limit.
    host.request("big", "toolchains/query", query("fixture"));
    let over_limit = host.answer("big").0;
    assert_eq!(over_limit["error"]["code"], -32075);
    assert_eq!(
        over_limit["error"]["data"],
        json!({"code": "plugin.toolchain.limitExceeded", "operationID": "query-1", "providerID": "fixture"})
    );
    finish(host);
}

#[test]
fn declines_an_unoffered_protocol_version() {
    let mut backend = Backend::new("test-backend");
    backend
        .toolchain_provider("fixture", |_, _| panic!("unnegotiated handler ran"))
        .unwrap();
    let mut host = Host::start(backend);
    assert!(initialize(&mut host, json!(["fixture"]), 1).is_null());
    host.request(1, "toolchains/query", query("fixture"));
    assert_eq!(host.answer(1).0["error"]["code"], -32601);
    finish(host);
}

#[test]
fn cancellation_returns_the_protocol_error_with_the_original_identity() {
    let mut backend = Backend::new("test-backend");
    let (started, entered) = mpsc::channel();
    backend
        .toolchain_provider("fixture", move |_, cancellation| {
            started.send(()).unwrap();
            cancellation.wait_timeout(Duration::from_secs(3));
            cancellation.check()?;
            Err(ToolchainQueryError::Timeout)
        })
        .unwrap();
    let mut host = Host::start(backend);
    initialize(&mut host, json!(["fixture"]), 0);
    host.request("query-1", "toolchains/query", query("fixture"));
    entered
        .recv_timeout(Duration::from_secs(3))
        .expect("provider started");
    host.notify("$/cancelRequest", json!({"id": "query-1"}));
    assert_eq!(
        host.answer("query-1").0["error"],
        json!({
            "code": -32800, "message": "The toolchain query was cancelled.",
            "data": {"code": "plugin.toolchain.cancelled", "operationID": "query-1", "providerID": "fixture"}
        })
    );
    finish(host);
}
