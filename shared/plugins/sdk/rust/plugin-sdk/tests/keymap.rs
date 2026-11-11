//! Shortcut providers negotiate independently of plugin functions.
mod common;

use common::Host;
use lithe_plugin_sdk::Backend;
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn negotiates_only_offered_providers_and_invokes_typed_domain_request() {
    let mut backend = Backend::new("test-backend");
    backend
        .keymap_import_provider("first", |request, _context| {
            assert_eq!(request.provider_id(), "first");
            assert_eq!(request.snapshot_id(), "snapshot-1");
            Ok(json!({"bindings": []}))
        })
        .unwrap();
    backend
        .keymap_import_provider("not-offered", |_request, _context| {
            Ok(json!({"bindings": []}))
        })
        .unwrap();
    let mut host = Host::start(backend);
    host.request(
        0,
        "initialize",
        json!({"capabilities": {"experimental": {
            "litheLogging": {"versions": [0]},
            "litheKeymapImport": {"versions": [0], "providers": ["first", "unknown"]}
        }}}),
    );
    let result = host.answer(0).0["result"].clone();
    assert_eq!(
        result["capabilities"]["experimental"]["litheKeymapImport"],
        json!({"version":0,"providers":["first"]})
    );
    host.request(
        1,
        "lithe/keymap/import",
        json!({"providerID":"first", "snapshotID":"snapshot-1"}),
    );
    let (answer, before) = host.answer(1);
    assert_eq!(answer["result"], json!({"bindings":[]}));
    // The fact names the wire method Core accepts, never the provider-scoped call id.
    let facts: Vec<_> = before
        .iter()
        .filter(|frame| frame["method"] == "lithe/log")
        .collect();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0]["params"]["kind"], "operationFinished");
    assert_eq!(facts[0]["params"]["operation"], "lithe/keymap/import");
    host.request(
        2,
        "lithe/keymap/import",
        json!({"providerID":"not-offered", "snapshotID":"snapshot-1"}),
    );
    assert!(host.answer(2).0["error"].is_object());
    host.request(
        3,
        "lithe/keymap/import",
        json!({"providerID":"first", "snapshotID":"../private"}),
    );
    assert!(host.answer(3).0["error"].is_object());
    host.request(4, "shutdown", Value::Null);
    assert_eq!(host.answer(4).0["result"], Value::Null);
    host.notify("exit", Value::Null);
    assert_eq!(host.exit_code(), 0);
}

#[test]
fn cancellation_reaches_a_running_provider() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (cancelled_tx, cancelled_rx) = mpsc::channel();
    let mut backend = Backend::new("test-backend");
    backend
        .keymap_import_provider("first", move |_request, context| {
            let (release_tx, release_rx) = mpsc::channel();
            let cancelled_tx = cancelled_tx.clone();
            context.cancellation().on_cancel(move || {
                let _ = cancelled_tx.send(());
                let _ = release_tx.send(());
            });
            entered_tx.send(()).unwrap();
            release_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("provider cancellation");
            context.check_cancelled()?;
            Ok(json!({"bindings": []}))
        })
        .unwrap();
    let mut host = Host::start(backend);
    host.request(0, "initialize", json!({"capabilities":{"experimental":{"litheLogging":{"versions":[0]},"litheKeymapImport":{"versions":[0],"providers":["first"]}}}}));
    host.answer(0);
    host.request(
        1,
        "lithe/keymap/import",
        json!({"providerID":"first", "snapshotID":"snapshot-1"}),
    );
    entered_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("provider started");
    host.notify("$/cancelRequest", json!({"id":1}));
    cancelled_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("provider cancelled");
    assert_eq!(host.answer(1).0["error"]["code"], -32800);
    host.close();
}

#[test]
fn provider_lists_and_reads_selected_files_through_scoped_host() {
    let mut backend = Backend::new("test-backend");
    backend
        .keymap_import_provider("first", |request, context| {
            let files = request.list_inputs(context.host())?;
            let bytes = request.read_chunk(context.host(), &files[0].id, 0, 4)?;
        assert_eq!(String::from_utf8(bytes).unwrap(), "data");
        Ok(json!({"bindings": [{"commandID": "find-next", "keys": [{"key": "f3", "modifiers": []}]}]}))
        })
        .unwrap();
    let mut host = Host::start(backend);
    host.request(
        0,
        "initialize",
        json!({"capabilities":{"experimental":{
            "litheLogging":{"versions":[0]}, "litheFunctions":{"versions":[0]},
            "litheKeymapImport":{"versions":[0],"providers":["first"]}
        }}}),
    );
    host.answer(0);
    host.request(
        1,
        "lithe/keymap/import",
        json!({"providerID":"first", "snapshotID":"snapshot-1"}),
    );
    let list = host.next();
    assert_eq!(list["method"], "lithe/host/keymapImport/listInputs");
    host.send(json!({"jsonrpc":"2.0", "id":list["id"], "result":{"files":[{"id":"file-0", "path":"keys.json", "byteCount":4}]}}));
    let read = host.next();
    assert_eq!(read["method"], "lithe/host/keymapImport/readChunk");
    assert_eq!(read["params"]["fileID"], "file-0");
    host.send(json!({"jsonrpc":"2.0", "id":read["id"], "result":{"data":"ZGF0YQ=="}}));
    assert_eq!(
        host.answer(1).0["result"],
        json!({"bindings": [{"commandID": "find-next", "keys": [{"key": "f3", "modifiers": []}]}]})
    );
    host.close();
}
