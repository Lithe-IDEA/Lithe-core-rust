//! Exercises build-provider requests over the SDK's actual framed transport.

mod common;

use common::Host;
use lithe_plugin_sdk::{Backend, PluginError};
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

fn initialize(host: &mut Host, build: Option<Value>) -> Value {
    let mut experimental = json!({ "litheLogging": { "versions": [0] } });
    if let Some(versions) = build {
        experimental["litheBuild"] = json!({ "versions": versions });
    }
    host.request(
        0,
        "initialize",
        json!({ "rootUri": "file:///tmp/workspace", "capabilities": { "experimental": experimental } }),
    );
    host.answer(0).0["result"]["capabilities"]["experimental"].clone()
}

fn model_backend() -> Backend {
    let mut backend = Backend::new("test-build");
    backend
        .build_model(|_, _| Ok(json!({ "root": ".", "projects": [] })))
        .build_run_entrypoints(|_, _| Ok(json!({ "entrypoints": [] })));
    backend
}

fn finish(mut host: Host) {
    host.request("shutdown", "shutdown", Value::Null);
    assert_eq!(host.answer("shutdown").0["result"], Value::Null);
    host.notify("exit", Value::Null);
    assert_eq!(host.exit_code(), 0);
}

fn progress(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|frame| frame["method"] == "lithe/build/progress")
        .map(|frame| &frame["params"])
        .collect()
}

#[test]
fn negotiates_the_newest_offered_build_version() {
    let mut host = Host::start(model_backend());
    let experimental = initialize(&mut host, Some(json!([0, 1, 7])));
    assert_eq!(experimental["litheBuild"], json!({ "version": 1 }));
    host.request(1, "lithe/build/runEntrypoints", json!({ "root": "." }));
    assert_eq!(
        host.answer(1).0["result"],
        json!({ "format": "litheRunEntrypoints", "version": 0, "entrypoints": [] })
    );
    finish(host);
}

#[test]
fn a_version_zero_host_does_not_route_run_entrypoints() {
    let mut host = Host::start(model_backend());
    assert_eq!(
        initialize(&mut host, Some(json!([0])))["litheBuild"],
        json!({ "version": 0 })
    );
    host.request(1, "lithe/build/model", json!({ "root": "." }));
    let (answer, facts) = host.answer(1);
    assert_eq!(
        answer["result"],
        json!({ "format": "litheBuildModel", "version": 0, "root": ".", "projects": [] })
    );
    assert!(facts
        .iter()
        .any(|fact| fact["params"]["kind"] == "operationFinished"
            && fact["params"]["operation"] == "lithe/build/model"));
    host.request(2, "lithe/build/runEntrypoints", json!({ "root": "." }));
    assert_eq!(host.answer(2).0["error"]["code"], -32601);
    finish(host);
}

#[test]
fn no_build_provider_without_handlers_or_an_offer() {
    let mut host = Host::start(Backend::new("test-build"));
    assert!(initialize(&mut host, Some(json!([0, 1])))
        .get("litheBuild")
        .is_none());
    host.request(1, "lithe/build/model", json!({ "root": "." }));
    assert_eq!(host.answer(1).0["error"]["code"], -32601);
    finish(host);

    let mut host = Host::start(model_backend());
    assert!(initialize(&mut host, None).get("litheBuild").is_none());
    host.request(1, "lithe/build/model", json!({ "root": "." }));
    assert_eq!(host.answer(1).0["error"]["code"], -32601);
    finish(host);
}

#[test]
fn a_handler_without_its_request_declared_is_not_routed() {
    let mut host = Host::start(model_backend());
    initialize(&mut host, Some(json!([1])));
    host.request(1, "lithe/build/dependencies", json!({ "root": "." }));
    assert_eq!(host.answer(1).0["error"]["code"], -32601);
    finish(host);
}

#[test]
fn tool_runs_need_object_params_and_their_operation_id() {
    let mut backend = Backend::new("test-build");
    backend.build_execute(|_, _| panic!("an invalid request ran"));
    let mut host = Host::start(backend);
    initialize(&mut host, Some(json!([1])));
    host.request(1, "lithe/build/execute", json!({ "root": "." }));
    host.request(2, "lithe/build/execute", json!(["root"]));
    for id in [1, 2] {
        let error = host.answer(id).0["error"].clone();
        assert_eq!(error["code"], -32602);
        assert_eq!(
            error["message"],
            "A build request needs object params and its wire operationID."
        );
    }
    finish(host);
}

#[test]
fn reports_progress_and_streams_output_until_the_request_ends() {
    let (sender, receiver) = mpsc::channel();
    let mut backend = Backend::new("test-build");
    backend.build_execute(move |params, context| {
        assert_eq!(params["task"], "build");
        assert_eq!(context.method(), "lithe/build/execute");
        assert_eq!(context.operation_id(), Some("op-build"));
        context.progress(&"m".repeat(5000), Some(250));
        let text = format!("{}é{}", "a".repeat(65_535), "b".repeat(10));
        context.output(&text);
        context.output("");
        let _ = sender.send(context.backend());
        Ok(json!({ "exitCode": 0, "output": text }))
    });
    let mut host = Host::start(backend);
    initialize(&mut host, Some(json!([1])));
    host.request(
        1,
        "lithe/build/execute",
        json!({ "operationID": "op-build", "root": ".", "task": "build" }),
    );
    let (answer, before) = host.answer(1);
    assert_eq!(answer["result"]["exitCode"], 0);
    let reports = progress(&before);
    assert_eq!(reports.len(), 3, "{reports:?}");
    assert!(reports
        .iter()
        .all(|report| report["operationID"] == "op-build"));
    assert_eq!(reports[0]["message"].as_str().unwrap().len(), 4096);
    assert_eq!(reports[0]["percentage"], 100);
    // The 2-byte `é` straddles the 64 KiB boundary, so the first chunk stops before it.
    assert_eq!(reports[1]["output"], "a".repeat(65_535));
    assert_eq!(reports[2]["output"], format!("é{}", "b".repeat(10)));
    let handle = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(handle.build_version(), Some(1));
    finish(host);
}

#[test]
fn requests_that_only_read_the_build_send_no_progress() {
    let mut backend = Backend::new("test-build");
    backend.build_model(|_, context| {
        assert_eq!(context.operation_id(), None);
        context.progress("reading", Some(10));
        context.output("text");
        Ok(json!({ "root": ".", "projects": [] }))
    });
    let mut host = Host::start(backend);
    initialize(&mut host, Some(json!([0])));
    host.request(
        1,
        "lithe/build/model",
        json!({ "root": ".", "operationID": "op-model" }),
    );
    let (answer, before) = host.answer(1);
    assert!(answer.get("result").is_some(), "{answer}");
    assert!(progress(&before).is_empty(), "{before:?}");
    finish(host);
}

#[test]
fn results_of_another_shape_fail_as_internal_errors() {
    let mut backend = Backend::new("test-build");
    backend
        .build_model(|_, _| Ok(json!({ "format": "litheDependencies", "projects": [] })))
        .build_prepare_debug(|_, _| {
            Ok(json!({
                "program": "target/debug/app", "arguments": [], "workingDirectory": ".",
                "environment": {}, "adapter": "native"
            }))
        })
        .build_test(|_, _| Ok(json!({ "exitCode": 0 })));
    let mut host = Host::start(backend);
    initialize(&mut host, Some(json!([1])));
    host.request(1, "lithe/build/model", json!({ "root": "." }));
    host.request(
        2,
        "lithe/build/prepareDebug",
        json!({ "operationID": "op-2", "root": "." }),
    );
    host.request(
        3,
        "lithe/build/test",
        json!({ "operationID": "op-3", "root": "." }),
    );
    // Each request runs on a thread of its own, so the answers come in any order.
    let mut failures = Vec::new();
    let mut answered = 0;
    while answered < 3 {
        let frame = host.next();
        if frame.get("method").is_none() {
            assert_eq!(frame["error"]["code"], -32603, "{frame}");
            answered += 1;
        }
        failures.push(frame);
    }
    let facts: Vec<_> = failures
        .iter()
        .filter(|frame| frame["method"] == "lithe/log")
        .map(|frame| frame["params"].clone())
        .collect();
    for operation in [
        "lithe/build/model",
        "lithe/build/prepareDebug",
        "lithe/build/test",
    ] {
        assert!(
            facts.iter().any(|fact| fact["operation"] == operation
                && fact["kind"] == "operationFailed"
                && fact["code"] == "internal_error"),
            "{operation}: {facts:?}"
        );
    }
    finish(host);
}

#[test]
fn plugin_errors_reach_the_host_with_their_code() {
    let mut backend = Backend::new("test-build");
    backend.build_prepare_run(|_, _| {
        Err(PluginError::new("cargo.unavailable", "Cargo was not found.").into())
    });
    let mut host = Host::start(backend);
    initialize(&mut host, Some(json!([1])));
    host.request(
        1,
        "lithe/build/prepareRun",
        json!({ "operationID": "op-run", "root": "." }),
    );
    let (answer, before) = host.answer(1);
    assert_eq!(answer["error"]["code"], -32000);
    assert_eq!(answer["error"]["data"]["code"], "cargo.unavailable");
    assert!(before.iter().any(
        |frame| frame["params"]["operation"] == "lithe/build/prepareRun"
            && frame["params"]["code"] == "cargo.unavailable"
            && frame["params"]["operationId"] == "op-run"
    ));
    finish(host);
}

#[test]
fn a_cancelled_request_answers_cancelled_and_reports_nothing_more() {
    let (started, running) = mpsc::channel();
    let mut backend = Backend::new("test-build");
    backend.build_execute(move |_, context| {
        let _ = started.send(());
        assert!(context.cancellation().wait_timeout(Duration::from_secs(10)));
        context.progress("after cancel", None);
        context.output("after cancel");
        Ok(json!({ "exitCode": 0, "output": "" }))
    });
    let mut host = Host::start(backend);
    initialize(&mut host, Some(json!([1])));
    host.request(
        7,
        "lithe/build/execute",
        json!({ "operationID": "op-7", "root": "." }),
    );
    running.recv_timeout(Duration::from_secs(10)).unwrap();
    host.notify("$/cancelRequest", json!({ "id": 7 }));
    let (answer, before) = host.answer(7);
    assert_eq!(answer["error"]["code"], -32800);
    assert!(progress(&before).is_empty(), "{before:?}");
    finish(host);
}

#[test]
fn shutdown_cancels_running_build_requests() {
    let (started, running) = mpsc::channel();
    let mut backend = Backend::new("test-build");
    backend.build_prepare_debug(move |_, context| {
        let _ = started.send(());
        context.cancellation().wait_timeout(Duration::from_secs(10));
        context.check_cancelled()?;
        Ok(json!({}))
    });
    let mut host = Host::start(backend);
    initialize(&mut host, Some(json!([1])));
    host.request(
        1,
        "lithe/build/prepareDebug",
        json!({ "operationID": "op-1", "root": "." }),
    );
    running.recv_timeout(Duration::from_secs(10)).unwrap();
    host.request("shutdown", "shutdown", Value::Null);
    // `shutdown` answers once it cancelled the request, which may answer later.
    let (mut call, mut shutdown) = (None, None);
    while call.is_none() || shutdown.is_none() {
        let frame = host.next();
        if frame["id"] == 1 {
            call = Some(frame);
        } else if frame["id"] == "shutdown" {
            shutdown = Some(frame);
        }
    }
    assert_eq!(call.unwrap()["error"]["code"], -32800);
    assert_eq!(shutdown.unwrap()["result"], Value::Null);
    host.notify("exit", Value::Null);
    assert_eq!(host.exit_code(), 0);
}
