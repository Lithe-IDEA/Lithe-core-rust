//! End-to-end scenario of a plugin backend (`backend` in `plugin.json`),
//! run against Rust Core through the same JSON commands a host sends.
//!
//! The test plays the host: it expands the backend launch plan, starts the
//! sample under the plugin's backend provider ID with the granted host
//! services, and answers Core's `hostRequest` events from an in-memory
//! credential store the way the macOS host answers them from the Keychain.
//! The scenario drives functions with progress, a plugin failure, an
//! unexpected failure, cancellation, runtime registration, credentials, the
//! workspace, window and terminal host services, and state across a restart,
//! where the backend is granted no host service.

use super::{core, listing, Fixture, Session};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

/// Runs the scenario against the installed package at `package`, whose
/// `plugin.json` declares the sample backend.
pub fn run_backend_scenario(package: &Path) {
    let manifest: Value =
        serde_json::from_slice(&fs::read(package.join("plugin.json")).expect("sample manifest"))
            .expect("sample manifest JSON");
    let plugin_id = manifest["id"].as_str().expect("plugin ID").to_string();
    let package_before = listing(package);
    let validated = core(
        "plugins.declarative.validate",
        json!({ "manifest": manifest }),
    );
    assert_eq!(validated["status"], "ok", "{validated}");

    let fixture = Fixture::new("backend");
    let state = fixture.root.join("state");
    let plan = launch_plan(&manifest, package, &state);
    assert_eq!(
        plan["backend"]["hostServices"],
        json!([
            "credentials",
            "lithe/host/terminal/close",
            "lithe/host/terminal/create",
            "lithe/host/terminal/dispose",
            "lithe/host/terminal/onExit",
            "lithe/host/terminal/reopen",
            "lithe/host/terminal/show",
            "lithe/host/window/showError",
            "lithe/host/window/showInformation",
            "lithe/host/window/showWarning",
            "lithe/host/workspace/watch",
            "workspace"
        ])
    );
    assert_eq!(plan["requiredDirectories"], json!(["state"]));
    fs::create_dir_all(&state).expect("host state directory");

    let mut session = start(&plugin_id, package, &plan, &state);
    assert_eq!(
        session.functions(),
        [
            "fail.unexpectedly",
            "files.nextChange",
            "note.read",
            "note.save",
            "secret.check",
            "secret.forget",
            "secret.remember",
            "terminal.cycle",
            "work.wait"
        ]
    );

    // A function with progress, which registers note.clear on first use.
    let saved = session.call(
        "save",
        "note.save",
        json!({ "name": "first", "text": "héllo" }),
    );
    assert_eq!(
        saved["result"]["value"],
        json!({ "name": "first", "bytes": 6 }),
        "{saved}"
    );
    let progress: Vec<_> = session
        .events
        .iter()
        .filter(|event| event["type"] == "requestProgress" && event["operationId"] == "save")
        .map(|event| event["result"]["message"].clone())
        .collect();
    assert_eq!(progress, ["Saving", "Saved"]);
    session.await_event("note.clear to be registered", |event| {
        event["type"] == "functionsChanged"
            && event["result"]["functions"]
                .as_array()
                .is_some_and(|functions| functions.contains(&json!("note.clear")))
    });
    let read = session.call("read", "note.read", json!({ "name": "first" }));
    assert_eq!(read["result"]["value"], json!({ "text": "héllo" }));

    // A plugin failure keeps Lithe's stable code and forwards the plugin's own.
    let missing = session.call("missing", "note.read", json!({ "name": "other" }));
    assert_eq!(
        missing["error"]["code"], "plugin.function.failed",
        "{missing}"
    );
    assert_eq!(missing["error"]["pluginErrorCode"], "sample.noteMissing");

    // An unexpected failure reaches the host without its details.
    let failed = session.call("fail", "fail.unexpectedly", json!({}));
    assert_eq!(
        failed["error"]["code"], "plugin.function.failed",
        "{failed}"
    );
    assert!(failed["error"].get("pluginErrorCode").is_none(), "{failed}");
    assert!(
        !failed.to_string().contains(&*state.to_string_lossy()),
        "a machine path reached the host: {failed}"
    );

    super::assert_log_facts(
        &plugin_id,
        &[
            ("save", "plugins.operation.finished", "INFO"),
            ("fail", "plugins.operation.failed", "ERROR"),
        ],
    );

    // Credentials travel as host requests and never touch the state directory.
    let mut store = BTreeMap::new();
    let remembered = serve_host(
        &mut session,
        &mut store,
        "remember",
        "secret.remember",
        json!({ "key": "sample:password", "secret": "s3cr3t" }),
    );
    assert_eq!(remembered["result"]["value"], json!({}), "{remembered}");
    assert_eq!(
        store,
        BTreeMap::from([("sample:password".to_string(), "s3cr3t".to_string())])
    );
    for (operation, secret, matches) in [("check", "s3cr3t", true), ("check-wrong", "nope", false)]
    {
        let checked = serve_host(
            &mut session,
            &mut store,
            operation,
            "secret.check",
            json!({ "key": "sample:password", "secret": secret }),
        );
        assert_eq!(
            checked["result"]["value"],
            json!({ "stored": true, "matches": matches }),
            "{checked}"
        );
    }
    let forgotten = serve_host(
        &mut session,
        &mut store,
        "forget",
        "secret.forget",
        json!({ "key": "sample:password" }),
    );
    assert_eq!(forgotten["result"]["value"], Value::Null, "{forgotten}");
    assert!(store.is_empty());
    let requests: Vec<_> = session
        .events
        .iter()
        .filter(|event| event["type"] == "hostRequest")
        .map(|event| (event["method"].clone(), event["result"]["service"].clone()))
        .collect();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert!(requests.iter().all(|(_, service)| service == "credentials"));

    // Cancelling a running call reaches the handler.
    session.invoke("wait", "work.wait", json!({}));
    wait_for_file(&mut session, &state.join("waiting"));
    core(
        "lsp.cancelOperation",
        json!({ "sessionId": session.id, "operationId": "wait" }),
    );
    assert_eq!(
        session.completion("wait")["error"]["code"],
        "requestCancelled"
    );
    wait_for_file(&mut session, &state.join("cancelled"));

    run_host_services(&mut session);
    run_terminal_services(&mut session);

    // State survives a restart. Without credentials.store the host grants no
    // host service, and Core refuses the request before the host sees it.
    session.stop();
    drop(session);
    let mut ungranted = manifest.clone();
    ungranted["permissions"] = json!(["backend.launch", "ui.contribute", "ui.web"]);
    let plan = launch_plan(&ungranted, package, &state);
    assert_eq!(plan["backend"]["hostServices"], json!([]));
    let mut restarted = start(&plugin_id, package, &plan, &state);
    assert!(restarted.functions().contains(&"note.clear".to_string()));
    let read = restarted.call("read-again", "note.read", json!({ "name": "first" }));
    assert_eq!(read["result"]["value"], json!({ "text": "héllo" }));
    let refused = restarted.call(
        "remember-ungranted",
        "secret.remember",
        json!({ "key": "sample:password", "secret": "s3cr3t" }),
    );
    assert_eq!(
        refused["error"]["pluginErrorCode"], "sample.credentialsUnavailable",
        "{refused}"
    );
    // The capabilities Core reports leave out the watch, so the sample
    // refuses before asking the host.
    invoke_in(
        &restarted,
        "watch-ungranted",
        "files.nextChange",
        &workspace(),
    );
    let unwatched = restarted.completion("watch-ungranted");
    assert_eq!(
        unwatched["error"]["pluginErrorCode"], "sample.watchUnavailable",
        "{unwatched}"
    );
    assert!(
        !restarted
            .events
            .iter()
            .any(|event| event["type"] == "hostRequest"),
        "an ungranted request reached the host"
    );
    assert_eq!(
        restarted.call("clear", "note.clear", json!({}))["result"]["value"],
        Value::Null
    );
    let cleared = restarted.call("read-cleared", "note.read", json!({ "name": "first" }));
    assert_eq!(cleared["error"]["pluginErrorCode"], "sample.noteMissing");
    restarted.stop();
    drop(restarted);

    assert!(
        listing(package) == package_before,
        "the plugin package changed at runtime"
    );
    let left: BTreeSet<_> = fs::read_dir(&state)
        .expect("state directory")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        left,
        BTreeSet::from(["cancelled".to_string(), "waiting".to_string()])
    );
}

/// The workspace context of the window the scenario's pages and command
/// run in.
fn workspace() -> Value {
    json!({ "contextID": "w-sample", "generation": 1 })
}

/// Workspace host services through Core: a call's workspace context,
/// capability discovery Core answers itself, a file watch whose event arrives
/// as a host notification, a typed host failure, a cancelled call cancelling
/// its host request on the wire, and notifications whose handlers send host
/// requests of their own.
fn run_host_services(session: &mut Session) {
    let context = workspace();
    let folders = json!({
        "folders": [{ "rootID": "r-sample", "name": "orders", "path": "/work/orders" }]
    });

    // Without a context the sample asks the host nothing.
    let nowhere = session.call("watch-nowhere", "files.nextChange", json!({}));
    assert_eq!(
        nowhere["error"]["pluginErrorCode"], "sample.noWorkspace",
        "{nowhere}"
    );

    // The watch's event reaches the call through subscriptions/notify.
    invoke_in(session, "watch", "files.nextChange", &context);
    let request = take_host_request(session, "lithe/host/workspace/getFolders");
    assert_eq!(request["result"]["params"], json!({ "context": context }));
    respond(session, &request, Ok(folders.clone()));
    let request = take_host_request(session, "lithe/host/workspace/watch");
    assert_eq!(
        request["result"]["params"],
        json!({
            "context": context,
            "resource": { "rootID": "r-sample", "path": "" },
            "recursive": true
        })
    );
    respond(
        session,
        &request,
        Ok(json!({ "subscriptionID": "watch-1" })),
    );
    assert!(notify(
        session,
        "lithe/host/subscriptions/notify",
        json!({
            "subscriptionID": "watch-1",
            "sequence": 1,
            "event": { "rootID": "r-sample", "changedPaths": ["notes.txt"], "requiresRescan": false }
        })
    ));
    let request = take_host_request(session, "lithe/host/subscriptions/dispose");
    assert_eq!(
        request["result"]["params"],
        json!({ "subscriptionID": "watch-1" })
    );
    respond(session, &request, Ok(Value::Null));
    let changed = session.completion("watch");
    assert_eq!(
        changed["result"]["value"],
        json!({ "changedPaths": ["notes.txt"] }),
        "{changed}"
    );

    // A host failure keeps its code, and the sample turns it into its own.
    invoke_in(session, "watch-stale", "files.nextChange", &context);
    let request = take_host_request(session, "lithe/host/workspace/getFolders");
    respond(
        session,
        &request,
        Err(json!({ "message": "The workspace changed.", "code": "plugin.host.contextExpired" })),
    );
    let stale = session.completion("watch-stale");
    assert_eq!(
        stale["error"]["pluginErrorCode"], "sample.workspaceChanged",
        "{stale}"
    );

    // Cancelling the call cancels its unanswered host request on the wire.
    invoke_in(session, "watch-cancel", "files.nextChange", &context);
    let request = take_host_request(session, "lithe/host/workspace/getFolders");
    respond(session, &request, Ok(folders.clone()));
    let pending = take_host_request(session, "lithe/host/workspace/watch");
    core(
        "lsp.cancelOperation",
        json!({ "sessionId": session.id, "operationId": "watch-cancel" }),
    );
    assert_eq!(
        session.completion("watch-cancel")["error"]["code"],
        "requestCancelled"
    );
    let cancelled = pending["result"]["hostRequestID"].clone();
    session.await_event("the watch request to be cancelled", |event| {
        event["type"] == "hostRequestCancelled" && event["result"]["hostRequestID"] == cancelled
    });

    // Notifications reach their handlers, which ask the host for more.
    assert!(notify(
        session,
        "lithe/host/workspace/didChangeContexts",
        json!({ "revision": 2 })
    ));
    let request = take_host_request(session, "lithe/host/workspace/listContexts");
    respond(
        session,
        &request,
        Ok(json!({
            "revision": 2,
            "contexts": [{
                "contextID": "w-sample",
                "generation": 1,
                "name": "orders",
                "authority": { "kind": "local" },
                "folders": folders["folders"]
            }]
        })),
    );
    assert!(notify(
        session,
        "lithe/host/commands/didRun",
        json!({ "commandID": "org.example.sample-backend.greet", "context": context })
    ));
    let request = take_host_request(session, "lithe/host/workspace/getFolders");
    assert_eq!(request["result"]["params"], json!({ "context": context }));
    respond(session, &request, Ok(folders));
    let request = take_host_request(session, "lithe/host/window/showInformation");
    assert_eq!(
        request["result"]["params"],
        json!({ "context": context, "text": "Sample Backend sees orders." })
    );
    respond(session, &request, Ok(json!({ "selectedIndex": null })));

    // Core answered capability discovery without the host.
    assert!(
        !session
            .events
            .iter()
            .any(|event| event["type"] == "hostRequest"
                && event["method"] == "lithe/host/getCapabilities"),
        "the host was asked for capabilities"
    );
}

/// This scenario plays the native host at the protocol boundary. Real PTY
/// rendering and history restoration are covered by Lithe's native tests.
fn run_terminal_services(session: &mut Session) {
    invoke_in(session, "terminal-cycle", "terminal.cycle", &workspace());
    let create = take_host_request(session, "lithe/host/terminal/create");
    assert_eq!(create["result"]["params"]["retainedContextBytes"], 4096);
    assert_eq!(
        create["result"]["params"]["arguments"],
        json!(["--interactive"])
    );
    respond(
        session,
        &create,
        Ok(json!({ "terminalID":"terminal-1", "generation":1 })),
    );
    let listen = take_host_request(session, "lithe/host/terminal/onExit");
    respond(
        session,
        &listen,
        Ok(json!({ "subscriptionID":"terminal-sub" })),
    );
    let close = take_host_request(session, "lithe/host/terminal/close");
    respond(session, &close, Ok(json!({ "retainedContextBytes":1024 })));
    assert!(notify(
        session,
        "lithe/host/subscriptions/notify",
        json!({
            "subscriptionID":"terminal-sub", "sequence":1,
            "event": { "type":"exit", "terminalID":"terminal-1", "generation":1, "exitCode":null, "reason":"closed" }
        })
    ));
    let reopen = take_host_request(session, "lithe/host/terminal/reopen");
    assert_eq!(reopen["result"]["params"]["terminalID"], "terminal-1");
    respond(
        session,
        &reopen,
        Ok(json!({ "terminalID":"terminal-1", "generation":2, "restoredContextBytes":1024 })),
    );
    let dispose = take_host_request(session, "lithe/host/terminal/dispose");
    respond(session, &dispose, Ok(Value::Null));
    let subscription = take_host_request(session, "lithe/host/subscriptions/dispose");
    respond(session, &subscription, Ok(Value::Null));
    let result = session.completion("terminal-cycle");
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["value"]["reopened"]["generation"], 2);
}

/// Invokes `function` as a page of the window showing `context` does.
fn invoke_in(session: &Session, operation: &str, function: &str, context: &Value) {
    core(
        "lsp.functionRequest",
        json!({
            "sessionId": session.id,
            "operationId": operation,
            "function": { "pluginID": session.plugin_id, "functionID": function },
            "params": {},
            "context": context,
        }),
    );
}

/// Waits for a `hostRequest` for `method` and takes it from the events, so
/// the next wait finds the next one.
fn take_host_request(session: &mut Session, method: &str) -> Value {
    let request = session.await_event(&format!("a {method} host request"), |event| {
        event["type"] == "hostRequest" && event["method"] == method
    });
    let id = request["result"]["hostRequestID"].clone();
    session.events.retain(|event| {
        !(event["type"] == "hostRequest" && event["result"]["hostRequestID"] == id)
    });
    request
}

/// Answers a host request as the host does: its result, or its failure.
fn respond(session: &Session, request: &Value, outcome: Result<Value, Value>) {
    let mut response = json!({
        "sessionId": session.id,
        "hostRequestID": request["result"]["hostRequestID"],
    });
    match outcome {
        Ok(result) => response["result"] = result,
        Err(error) => response["error"] = error,
    }
    core("lsp.hostResponse", response);
}

/// Sends a host notification through Core; whether it reached the backend.
fn notify(session: &Session, method: &str, params: Value) -> bool {
    core(
        "lsp.hostNotify",
        json!({ "sessionId": session.id, "method": method, "params": params }),
    )["delivered"]
        == true
}

fn launch_plan(manifest: &Value, package: &Path, state: &Path) -> Value {
    let plan = core(
        "plugins.declarative.languageServerLaunchPlan",
        json!({
            "manifest": manifest,
            "backend": true,
            "request": {
                "pluginDirectory": package,
                "stateDirectory": state,
                "architecture": if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" },
            }
        }),
    );
    assert_eq!(plan["status"], "ok", "{plan}");
    assert_eq!(plan["plan"]["launch"]["kind"], "packaged");
    plan["plan"].clone()
}

/// Starts the backend as the host does: the packaged executable, under the
/// plugin's backend provider ID, with the state directory as root and
/// working directory and the plan's host services.
fn start(plugin_id: &str, package: &Path, plan: &Value, state: &Path) -> Session {
    let executable = package.join(plan["launch"]["path"].as_str().expect("packaged path"));
    let started = core(
        "lsp.startServer",
        json!({
            "providerId": format!("pluginBackend:{plugin_id}"),
            "pluginId": plugin_id,
            "executablePath": executable,
            "arguments": plan["arguments"],
            "rootUri": format!("file://{}/", state.display()),
            "workingDirectory": state,
            "hostServices": plan["backend"]["hostServices"],
            "initializeTimeoutMilliseconds": super::WAIT.as_millis() as u64,
            "requestTimeoutMilliseconds": super::WAIT.as_millis() as u64,
            "shutdownTimeoutMilliseconds": 5000,
        }),
    );
    let mut session = Session {
        id: started["sessionId"]
            .as_str()
            .expect("session ID")
            .to_string(),
        plugin_id: plugin_id.to_string(),
        events: Vec::new(),
    };
    session.await_event("the backend to become ready", |event| {
        event["type"] == "stateChanged" && event["state"] == "ready"
    });
    session
}

/// Invokes `function` and answers every host request of the call from
/// `store` until the call ends.
fn serve_host(
    session: &mut Session,
    store: &mut BTreeMap<String, String>,
    operation: &str,
    function: &str,
    params: Value,
) -> Value {
    let request_id = |event: &Value| {
        event["result"]["hostRequestID"]
            .as_str()
            .map(str::to_string)
    };
    let mut answered: BTreeSet<_> = session
        .events
        .iter()
        .filter(|event| event["type"] == "hostRequest")
        .filter_map(request_id)
        .collect();
    session.invoke(operation, function, params);
    let deadline = Instant::now() + super::WAIT;
    loop {
        let pending: Vec<_> = session
            .events
            .iter()
            .filter(|event| {
                event["type"] == "hostRequest"
                    && request_id(event).is_some_and(|id| !answered.contains(&id))
            })
            .cloned()
            .collect();
        for request in pending {
            let id = request_id(&request).expect("host request ID");
            let params = &request["result"]["params"];
            let key = params["key"].as_str().expect("credential key").to_string();
            let result = match request["method"].as_str() {
                Some("lithe/host/credentials/get") => {
                    json!({ "secret": store.get(&key) })
                }
                Some("lithe/host/credentials/set") => {
                    store.insert(key, params["secret"].as_str().expect("secret").to_string());
                    Value::Null
                }
                Some("lithe/host/credentials/delete") => {
                    store.remove(&key);
                    Value::Null
                }
                other => panic!("unexpected host request {other:?}"),
            };
            core(
                "lsp.hostResponse",
                json!({ "sessionId": session.id, "hostRequestID": id, "result": result }),
            );
            answered.insert(id);
        }
        if let Some(completed) = session
            .events
            .iter()
            .find(|event| event["operationId"] == operation && event["type"] == "requestCompleted")
        {
            return completed.clone();
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {operation}; events: {:#?}",
            session.events
        );
        let drained = core(
            "lsp.waitEvents",
            json!({ "sessionId": session.id, "timeoutMilliseconds": remaining.as_millis().min(200) as u64 }),
        );
        session
            .events
            .extend(drained["events"].as_array().cloned().unwrap_or_default());
    }
}

/// Waits for the backend to create `path`, draining events meanwhile. The
/// file is the only signal a handler blocked in the backend process gives.
fn wait_for_file(session: &mut Session, path: &Path) {
    let deadline = Instant::now() + super::WAIT;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "the backend never created {}",
            path.display()
        );
        let drained = core(
            "lsp.waitEvents",
            json!({ "sessionId": session.id, "timeoutMilliseconds": 50 }),
        );
        session
            .events
            .extend(drained["events"].as_array().cloned().unwrap_or_default());
    }
}
