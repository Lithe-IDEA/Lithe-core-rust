//! Host services of the plugin-function protocol, driven by a scripted host
//! and the contract fixture; mirrors `sdk/node/test/host.test.mjs`.

mod common;

use common::{Host, WAIT};
use lithe_plugin_sdk::{
    is_credential_key, Backend, Cancellation, FunctionError, SdkError, WorkspaceContext,
    HOST_ERROR_CODES,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(200);

fn fixture() -> Value {
    // Read at run time: the packaged crate has no contract directory.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../contract/v1/fixtures/backend/function-protocol.json");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("the fixture is JSON")
}

fn cases(fixture: &Value, key: &str) -> Vec<Value> {
    fixture[key].as_array().expect("fixture cases").clone()
}

fn context() -> WorkspaceContext {
    WorkspaceContext::new("w1", 2).unwrap()
}

/// Runs blocking SDK work off the test thread.
fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
}

fn done<T>(receiver: &mpsc::Receiver<T>) -> T {
    receiver
        .recv_timeout(WAIT)
        .expect("the host request to finish in time")
}

fn code<T: std::fmt::Debug>(result: Result<T, SdkError>) -> String {
    match result {
        Ok(value) => panic!("the host request succeeded: {value:?}"),
        Err(error) => error.code().to_string(),
    }
}

/// A host whose session negotiated plugin functions (or did not), and every
/// frame the backend sent.
struct Session {
    host: Host,
    log: Vec<Value>,
    unread: VecDeque<Value>,
}

impl Session {
    /// A session whose host offered `litheFunctions` version 0.
    fn start(backend: Backend) -> Self {
        Self::begin(backend, json!([0]))
    }

    /// A session whose host offered no plugin functions.
    fn without_functions(backend: Backend) -> Self {
        Self::begin(backend, Value::Null)
    }

    fn begin(backend: Backend, offer: Value) -> Self {
        let mut host = Host::start(backend);
        host.initialize(offer);
        Self {
            host,
            log: Vec::new(),
            unread: VecDeque::new(),
        }
    }

    fn pull(&mut self, timeout: Duration) -> Option<Value> {
        let frame = self.host.next_within(timeout)?;
        self.log.push(frame.clone());
        Some(frame)
    }

    /// The next frame the test has not taken that `matches`.
    fn take_where(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.unread.iter().position(&matches) {
            return self.unread.remove(index).unwrap();
        }
        loop {
            let frame = self
                .pull(WAIT)
                .unwrap_or_else(|| panic!("no matching frame; unread: {:?}", self.unread));
            if matches(&frame) {
                return frame;
            }
            self.unread.push_back(frame);
        }
    }

    /// The next host request with `method`.
    fn take(&mut self, method: &str) -> Value {
        self.take_where(|frame| frame["method"] == method && frame.get("id").is_some())
    }

    /// The backend's answer to request `id`.
    fn response(&mut self, id: impl Into<Value>) -> Value {
        let id = id.into();
        self.take_where(|frame| frame["id"] == id && frame.get("method").is_none())
    }

    /// Every frame with `method` the backend sent, once it went quiet.
    fn sent(&mut self, method: &str) -> Vec<Value> {
        while let Some(frame) = self.pull(QUIET) {
            self.unread.push_back(frame);
        }
        self.log
            .iter()
            .filter(|frame| frame["method"] == method)
            .cloned()
            .collect()
    }

    fn reply(&mut self, request: &Value, result: Value) {
        self.host
            .send(json!({ "jsonrpc": "2.0", "id": request["id"], "result": result }));
    }

    fn refuse(&mut self, request: &Value, error: Value) {
        self.host
            .send(json!({ "jsonrpc": "2.0", "id": request["id"], "error": error }));
    }

    fn close(self) -> i32 {
        self.host.close()
    }
}

#[test]
fn the_sdk_answers_plugin_functions_only_when_the_host_offers_version_zero() {
    for (offered, negotiated) in [
        (json!([0]), true),
        (json!([0, 99]), true),
        (json!([1]), false),
        (json!([99]), false),
        (json!([]), false),
    ] {
        let mut backend = Backend::new("test-backend");
        backend
            .function("query.run", |_, _| Ok(Value::Null))
            .unwrap();
        let handle = backend.handle();
        let mut host = Host::start(backend);
        let result = host.initialize(offered.clone());
        let answer = &result["capabilities"]["experimental"]["litheFunctions"];
        if negotiated {
            assert_eq!(
                answer,
                &json!({ "version": 0, "functions": [{ "id": "query.run" }] })
            );
            assert_eq!(handle.functions_version(), Some(0));
        } else {
            assert_eq!(answer, &Value::Null, "{offered}");
            assert_eq!(handle.functions_version(), None, "{offered}");
        }
        assert_eq!(handle.host().negotiated(), negotiated, "{offered}");
        host.close();
    }
}

#[test]
fn credentials_round_trip_through_the_host_with_the_contract_shapes() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);

    let credentials = host.clone();
    let reading = background(move || credentials.credentials().get("connection.prod"));
    let get = session.take("lithe/host/credentials/get");
    assert_eq!(get["params"], json!({ "key": "connection.prod" }));
    session.reply(&get, json!({ "secret": "s3cret" }));
    assert_eq!(done(&reading).unwrap(), Some("s3cret".to_string()));

    let credentials = host.clone();
    let missing = background(move || credentials.credentials().get("connection.prod"));
    let get = session.take("lithe/host/credentials/get");
    session.reply(&get, Value::Null);
    assert_eq!(done(&missing).unwrap(), None);

    let credentials = host.clone();
    let malformed = background(move || credentials.credentials().get("connection.prod"));
    let get = session.take("lithe/host/credentials/get");
    session.reply(&get, json!({}));
    assert_eq!(code(done(&malformed)), "sdk.host.invalidResult");

    let credentials = host.clone();
    let writing = background(move || {
        credentials
            .credentials()
            .set("connection.prod:password", "s3cret")
    });
    let set = session.take("lithe/host/credentials/set");
    assert_eq!(
        set["params"],
        json!({ "key": "connection.prod:password", "secret": "s3cret" })
    );
    session.reply(&set, Value::Null);
    done(&writing).unwrap();

    let credentials = host.clone();
    let deleting = background(move || credentials.credentials().delete("connection.prod"));
    let delete = session.take("lithe/host/credentials/delete");
    assert_eq!(delete["params"], json!({ "key": "connection.prod" }));
    session.reply(&delete, Value::Null);
    done(&deleting).unwrap();

    // Malformed arguments fail locally and never reach the host.
    let credentials = host.credentials();
    for key in ["", "has space", &"k".repeat(129)] {
        assert_eq!(
            code(credentials.get(key)),
            "sdk.credentials.invalidKey",
            "{key:?}"
        );
    }
    let secret = "x".repeat(16 * 1024 + 1);
    assert_eq!(
        code(credentials.set("key", &secret)),
        "sdk.credentials.invalidSecret"
    );
    session.host.assert_quiet();
    session.close();
}

#[test]
fn local_key_and_context_checks_agree_with_the_fixture_cases_core_refuses_or_forwards() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let (mut keys, mut contexts) = (0, 0);
    for case in cases(&fixture(), "hostRequests") {
        let name = case["name"].as_str().unwrap();
        let method = case["method"].as_str().unwrap();
        if method.starts_with("lithe/host/credentials/") {
            let key = case["params"]["key"].as_str().unwrap();
            if case["expected"].get("forward").is_some() {
                assert!(is_credential_key(key), "{name}");
            } else if name.contains("key") {
                assert_eq!(
                    code(host.credentials().get(key)),
                    "sdk.credentials.invalidKey",
                    "{name}"
                );
                keys += 1;
            }
        }
        if [
            "context without",
            "context ID outside",
            "generation is positive",
        ]
        .iter()
        .any(|pattern| name.contains(pattern))
        {
            let error = WorkspaceContext::from_value(&case["params"]["context"]).unwrap_err();
            assert_eq!(error.code(), "sdk.host.invalidContext", "{name}");
            contexts += 1;
        }
    }
    assert_eq!((keys, contexts), (2, 3));
    session.host.assert_quiet();

    // A full context from listContexts is passed on as its reference.
    let full = json!({ "contextID": "w1", "generation": 2, "name": "orders", "folders": [] });
    let workspace = WorkspaceContext::from_value(&full).unwrap();
    let asking = {
        let host = host.clone();
        background(move || host.workspace().get_folders(&workspace))
    };
    let request = session.take("lithe/host/workspace/getFolders");
    assert_eq!(
        request["params"],
        json!({ "context": { "contextID": "w1", "generation": 2 } })
    );
    session.reply(&request, json!({ "folders": [] }));
    assert_eq!(done(&asking).unwrap(), json!({ "folders": [] }));
    session.close();
}

#[test]
fn host_refusals_keep_the_code_number_and_details_core_sent() {
    let fixture = fixture();
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let mut answer = |wire: Value| -> SdkError {
        let pending = {
            let host = host.clone();
            background(move || host.request("lithe/host/workspace/listContexts", Value::Null))
        };
        let request = session.take("lithe/host/workspace/listContexts");
        session.refuse(&request, wire);
        done(&pending).expect_err("a refusal")
    };
    let mut refusals = 0;
    for case in cases(&fixture, "hostRequests") {
        let expected = &case["expected"];
        let Some(expected_code) = expected["code"].as_str() else {
            continue;
        };
        let mut data = json!({ "code": expected_code });
        if !expected["details"].is_null() {
            data["details"] = expected["details"].clone();
        }
        let error =
            answer(json!({ "code": expected["rpcCode"], "message": "Refused.", "data": data }));
        assert_eq!(error.code(), expected_code, "{}", case["name"]);
        assert_eq!(
            error.rpc_code(),
            expected["rpcCode"].as_i64(),
            "{}",
            case["name"]
        );
        assert_eq!(
            error.details(),
            expected["details"].as_object(),
            "{}",
            case["name"]
        );
        assert_eq!(error.message(), "Refused.");
        refusals += 1;
    }
    assert!(refusals > 0);
    let failures = cases(&fixture, "hostFailures");
    assert!(!failures.is_empty());
    for case in failures {
        let expected = &case["expected"];
        let number = expected["rpcCode"].as_i64().unwrap();
        let known = HOST_ERROR_CODES
            .iter()
            .find(|(code, _)| expected["code"] == *code)
            .map(|(_, number)| *number);
        assert_eq!(known, Some(number), "{}", case["name"]);
        // The number alone names the code.
        let error = answer(json!({ "code": number, "message": "Failed." }));
        assert_eq!(error.code(), expected["code"], "{}", case["name"]);
    }
    let later = answer(json!({
        "code": -32050, "message": "Later code.", "data": { "details": ["not", "an", "object"] }
    }));
    assert_eq!(later.code(), "plugin.host.failed");
    assert_eq!(later.details(), None);
    session.close();
}

#[test]
fn without_plugin_functions_the_host_offers_no_service_and_host_requests_stay_within_lithe_host() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let session = Session::without_functions(backend);
    let unavailable = host.credentials().get("connection.prod").unwrap_err();
    assert_eq!(unavailable.code(), "plugin.host.unavailable");
    assert_eq!(unavailable.rpc_code(), Some(-32601));
    assert_eq!(code(host.capabilities()), "plugin.host.unavailable");
    assert!(!host.supports("lithe/host/credentials/get").unwrap());
    // The relay is a host service too, and so is the generic request.
    let call = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"
    });
    let functions = host.functions();
    let providers = functions.providers("languageImport.settings");
    let invoked = functions.invoke(&context(), call);
    let generic = host.request("lithe/host/functions/invoke", json!({}));
    for result in [providers, invoked, generic] {
        let error = result.unwrap_err();
        assert_eq!(error.code(), "plugin.host.unavailable");
        assert_eq!(error.rpc_code(), Some(-32601));
    }
    assert_eq!(
        code(host.request("lithe/function/invoke", json!({}))),
        "sdk.host.method"
    );
    session.host.assert_quiet();
    session.close();

    let idle = Backend::new("idle").handle().host();
    assert_eq!(code(idle.credentials().get("key")), "sdk.session.notReady");
}

#[test]
fn capabilities_are_asked_for_once_and_supports_reads_them() {
    let answer = cases(&fixture(), "hostRequests")
        .into_iter()
        .find_map(|case| case["expected"].get("answer").cloned())
        .expect("a capabilities answer");
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let first = {
        let host = host.clone();
        background(move || host.supports("lithe/host/workspace/getFolders"))
    };
    let second = {
        let host = host.clone();
        background(move || host.supports("lithe/host/workspace/readFile"))
    };
    let request = session.take("lithe/host/getCapabilities");
    assert!(request.get("params").is_none(), "{request}");
    session.reply(&request, answer);
    assert!(done(&first).unwrap());
    assert!(!done(&second).unwrap());
    assert_eq!(
        host.capabilities().unwrap()["limits"]["pendingHostRequests"],
        16
    );
    assert_eq!(session.sent("lithe/host/getCapabilities").len(), 1);
    session.close();
}

#[test]
fn a_call_carries_its_workspace_context_to_the_handler() {
    let mut checked = 0;
    for case in cases(&fixture(), "invocationContexts") {
        let params = &case["expected"]["params"];
        if params.is_null() {
            continue;
        }
        let (sender, seen) = mpsc::channel();
        let mut backend = Backend::new("test-backend");
        backend
            .function(params["functionID"].as_str().unwrap(), move |_, context| {
                let context = context.workspace_context().map(WorkspaceContext::to_value);
                sender.send(context.unwrap_or(Value::Null)).unwrap();
                Ok(Value::Null)
            })
            .unwrap();
        let mut session = Session::start(backend);
        session
            .host
            .request(9, "lithe/function/invoke", params.clone());
        let response = session.response(9);
        assert!(
            response.get("error").is_none(),
            "{}: {response}",
            case["name"]
        );
        assert_eq!(
            seen.recv_timeout(WAIT).unwrap(),
            params["context"],
            "{}",
            case["name"]
        );
        session.close();
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn cancelling_a_call_cancels_its_host_requests_at_core() {
    let (sender, outcome) = mpsc::channel();
    let mut backend = Backend::new("test-backend");
    backend
        .function("secret.read", move |_, context| {
            let read = context.host().credentials().get("connection.prod");
            let failure = read.as_ref().err().map(|error| error.code().to_string());
            sender.send(failure).unwrap();
            Ok(json!(read?))
        })
        .unwrap();
    let mut session = Session::start(backend);
    session.host.request(
        41,
        "lithe/function/invoke",
        json!({ "functionID": "secret.read", "operationID": "c" }),
    );
    let request = session.take("lithe/host/credentials/get");
    session.host.notify("$/cancelRequest", json!({ "id": 41 }));
    assert_eq!(session.response(41)["error"]["code"], -32800);
    // The host request fails with the call's cancellation.
    assert_eq!(
        outcome.recv_timeout(WAIT).unwrap().as_deref(),
        Some("sdk.call.cancelled")
    );
    let cancels: Vec<_> = session
        .sent("$/cancelRequest")
        .into_iter()
        .map(|frame| frame["params"].clone())
        .collect();
    assert_eq!(cancels, [json!({ "id": request["id"] })]);
    // Core's late answer is dropped; the session continues.
    session.refuse(
        &request,
        json!({ "code": -32800, "message": "Cancelled.", "data": { "code": "plugin.host.cancelled" } }),
    );
    assert_eq!(session.close(), 1);
}

#[test]
fn a_cancelled_token_ends_its_request_which_is_never_sent_again() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let stop = Cancellation::new();
    let cancellable = host.with_cancellation(&stop);
    let edit = json!({ "documentID": "d1", "expectedVersion": 4, "edits": [] });
    let pending = {
        let (cancellable, edit) = (cancellable.clone(), edit.clone());
        background(move || cancellable.editor().apply_edits(&context(), edit))
    };
    let request = session.take("lithe/host/editor/applyEdits");
    let mut with_context = edit;
    with_context["context"] = context().to_value();
    assert_eq!(request["params"], with_context);
    stop.cancel();
    assert_eq!(code(done(&pending)), "sdk.call.cancelled");
    let cancel = session.take_where(|frame| frame["method"] == "$/cancelRequest");
    assert_eq!(cancel["params"], json!({ "id": request["id"] }));
    session.reply(
        &request,
        json!({ "documentID": "d1", "version": 5, "applied": true }),
    );
    assert_eq!(
        code(cancellable.request("lithe/host/editor/applyEdits", json!({}))),
        "sdk.call.cancelled"
    );
    assert_eq!(session.sent("lithe/host/editor/applyEdits").len(), 1);
    session.close();
}

#[test]
fn at_most_sixteen_host_requests_are_unanswered_later_ones_wait_for_a_free_slot() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let calls: Vec<_> = (0..18)
        .map(|_| {
            let host = host.clone();
            background(move || host.workspace().list_contexts())
        })
        .collect();
    let first = session.sent("lithe/host/workspace/listContexts");
    assert_eq!(first.len(), 16);
    let answered = &first[0];
    session.reply(answered, json!({ "revision": 1, "contexts": [] }));
    assert_eq!(session.sent("lithe/host/workspace/listContexts").len(), 17);
    // Ending the session fails the queued request too.
    session.close();
    let mut outcomes: Vec<_> = calls
        .iter()
        .map(|call| match done(call) {
            Ok(result) => result.to_string(),
            Err(error) => error.code().to_string(),
        })
        .collect();
    outcomes.sort();
    let mut expected = vec!["sdk.session.closed".to_string(); 17];
    expected.push(json!({ "revision": 1, "contexts": [] }).to_string());
    expected.sort();
    assert_eq!(outcomes, expected);
}

#[test]
fn subscription_events_arrive_in_order_also_when_sent_before_the_answer() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let watched = json!({ "resource": { "rootID": "r1", "path": "" }, "recursive": true });
    let watching = {
        let (host, watched) = (host.clone(), watched.clone());
        background(move || host.workspace().watch(&context(), watched))
    };
    let request = session.take("lithe/host/workspace/watch");
    let mut with_context = watched;
    with_context["context"] = context().to_value();
    assert_eq!(request["params"], with_context);
    let notify = |session: &mut Session, sequence: u64| {
        session.host.notify(
            "lithe/host/subscriptions/notify",
            json!({
                "subscriptionID": "s1", "sequence": sequence,
                "event": { "rootID": "r1", "changedPaths": [format!("f{sequence}")], "requiresRescan": false }
            }),
        );
    };
    notify(&mut session, 1);
    session.reply(&request, json!({ "subscriptionID": "s1" }));
    let subscription = Arc::new(done(&watching).unwrap());
    notify(&mut session, 2);
    notify(&mut session, 4);
    let seen: Vec<_> = (0..3)
        .map(|_| {
            let event = subscription.recv_timeout(WAIT).expect("an event");
            json!([
                event.subscription_id(),
                event.sequence(),
                event.gap(),
                event.event()["changedPaths"]
            ])
        })
        .collect();
    assert_eq!(
        seen,
        [
            json!(["s1", 1, false, ["f1"]]),
            json!(["s1", 2, false, ["f2"]]),
            json!(["s1", 4, true, ["f4"]]),
        ]
    );
    assert_eq!(subscription.id(), "s1");
    assert!(!subscription.is_disposed());

    let disposing = {
        let subscription = Arc::clone(&subscription);
        background(move || subscription.dispose())
    };
    let dispose = session.take("lithe/host/subscriptions/dispose");
    assert!(subscription.is_disposed());
    assert_eq!(dispose["params"], json!({ "subscriptionID": "s1" }));
    notify(&mut session, 5);
    session.reply(&dispose, Value::Null);
    done(&disposing).unwrap();
    // Event 5 came after the disposal; it was dropped.
    assert_eq!(
        subscription.recv_timeout(QUIET),
        Err(RecvTimeoutError::Disconnected)
    );
    assert_eq!(subscription.recv(), None);
    session.close();
}

#[test]
fn commands_context_changes_and_later_host_notifications_reach_their_handlers() {
    let delivered: Vec<_> = cases(&fixture(), "hostNotifications")
        .into_iter()
        .filter(|case| {
            case["expected"]["delivered"] == true
                && case["method"] != "lithe/host/subscriptions/notify"
        })
        .collect();
    let mut expected: BTreeSet<String> = delivered
        .iter()
        .map(|case| {
            let params = &case["params"];
            let entry = match case["method"].as_str().unwrap() {
                "lithe/host/commands/didRun" => {
                    json!(["command", params["commandID"], params["context"]])
                }
                "lithe/host/workspace/didChangeContexts" => json!(["contexts", params]),
                method => json!(["notification", method, params]),
            };
            entry.to_string()
        })
        .collect();
    expected.insert(json!(["later", { "value": 1 }]).to_string());
    // Every delivered case is told apart by its method and params.
    assert_eq!(expected.len(), delivered.len() + 1);

    let (sender, seen) = mpsc::channel::<Value>();
    let mut backend = Backend::new("test-backend");
    let (commands, contexts, later) = (sender.clone(), sender.clone(), sender.clone());
    backend
        .on_command("db.refresh", move |command_id, context| {
            let context = context.map_or(Value::Null, |context| context.to_value());
            commands
                .send(json!(["command", command_id, context]))
                .unwrap();
            Ok(())
        })
        .unwrap()
        .on_contexts_changed(move |params| {
            contexts.send(json!(["contexts", params])).unwrap();
            Ok(())
        })
        .on_host_notification("lithe/host/later/didSomething", move |params| {
            later.send(json!(["later", params])).unwrap();
            Ok(())
        })
        .unwrap();
    // The other delivered methods (run exits, language server changes and
    // forwarded notifications) use the generic registration, once each.
    let routed = [
        "lithe/host/commands/didRun",
        "lithe/host/workspace/didChangeContexts",
    ];
    let generic: BTreeSet<&str> = delivered
        .iter()
        .filter_map(|case| case["method"].as_str())
        .filter(|method| !routed.contains(method))
        .collect();
    assert_eq!(generic.len(), 3);
    for method in generic {
        let sender = sender.clone();
        let method_name = method.to_string();
        backend
            .on_host_notification(method, move |params| {
                sender
                    .send(json!(["notification", method_name, params]))
                    .unwrap();
                Ok(())
            })
            .unwrap();
    }
    let refused = |result: Result<&mut Backend, SdkError>| {
        result.err().expect("a refusal").code().to_string()
    };
    let reserved = backend.on_host_notification("lithe/host/commands/didRun", |_| Ok(()));
    assert_eq!(refused(reserved), "sdk.method.reserved");
    let invalid = backend.on_host_notification("workspace/didChange", |_| Ok(()));
    assert_eq!(refused(invalid), "sdk.method.invalid");
    let command = backend.on_command("DB_Refresh", |_, _| Err(FunctionError::Cancelled));
    assert_eq!(refused(command), "sdk.command.invalidID");

    let mut session = Session::start(backend);
    for case in &delivered {
        session
            .host
            .notify(case["method"].as_str().unwrap(), case["params"].clone());
    }
    // A notification this SDK does not know is ignored, as the contract asks.
    session
        .host
        .notify("lithe/host/unknown/ignored", json!({ "anything": true }));
    session
        .host
        .notify("lithe/host/later/didSomething", json!({ "value": 1 }));
    // Handlers run on their own threads, so only the set is fixed.
    let received: BTreeSet<String> = (0..expected.len())
        .map(|_| seen.recv_timeout(WAIT).expect("a notification").to_string())
        .collect();
    assert_eq!(received, expected);
    assert!(seen.recv_timeout(QUIET).is_err());
    session.host.assert_quiet();
    session.close();
}

#[test]
fn native_terminal_methods_keep_context_and_literal_arguments() {
    let backend = Backend::new("terminal-test");
    let handle = backend.handle();
    let mut session = Session::start(backend);
    let host = handle.host();
    let creating = background(move || {
        host.terminal().create(
            &context(),
            json!({
                "executable": "example-client", "arguments": ["literal;$(value)", "space value"]
            }),
        )
    });
    let create = session.take("lithe/host/terminal/create");
    assert_eq!(create["params"]["context"], context().to_value());
    assert_eq!(
        create["params"]["arguments"],
        json!(["literal;$(value)", "space value"])
    );
    session.reply(&create, json!({ "terminalID": "owned" }));
    assert_eq!(done(&creating).unwrap()["terminalID"], "owned");
    for method in ["show", "close", "reopen", "dispose"] {
        let host = handle.host();
        let operation = background(move || match method {
            "show" => host.terminal().show(&context(), "owned"),
            "close" => host.terminal().close(&context(), "owned", Some(4096)),
            "reopen" => host.terminal().reopen(&context(), "owned"),
            _ => host.terminal().dispose(&context(), "owned"),
        });
        let request = session.take(&format!("lithe/host/terminal/{method}"));
        assert_eq!(request["params"]["terminalID"], "owned");
        session.reply(&request, Value::Null);
        assert_eq!(done(&operation).unwrap(), Value::Null);
    }
    session.close();
}

#[test]
fn a_relayed_call_names_the_plugin_that_called_it_and_a_host_call_names_none() {
    let mut checked = 0;
    for case in cases(&fixture(), "invocationCallers") {
        let params = &case["expected"]["params"];
        let (sender, seen) = mpsc::channel::<Option<String>>();
        let mut backend = Backend::new("test-backend");
        backend
            .function(params["functionID"].as_str().unwrap(), move |_, context| {
                let caller = context.caller().map(|caller| caller.plugin_id().into());
                sender.send(caller).unwrap();
                Ok(Value::Null)
            })
            .unwrap();
        let mut session = Session::start(backend);
        session
            .host
            .request(9, "lithe/function/invoke", params.clone());
        let response = session.response(9);
        assert!(
            response.get("error").is_none(),
            "{}: {response}",
            case["name"]
        );
        assert_eq!(
            seen.recv_timeout(WAIT).unwrap(),
            params["caller"]["pluginID"].as_str().map(str::to_string),
            "{}",
            case["name"]
        );
        session.close();
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn a_caller_that_is_not_exactly_a_plugin_id_is_not_reported() {
    let (sender, seen) = mpsc::channel::<Option<String>>();
    let mut backend = Backend::new("test-backend");
    backend
        .function("languageImport.settings", move |_, context| {
            let caller = context.caller().map(|caller| caller.plugin_id().into());
            sender.send(caller).unwrap();
            Ok(Value::Null)
        })
        .unwrap();
    let mut session = Session::start(backend);
    let python = "dev.lithe.fixture.python";
    let attempts = [
        (json!({ "pluginID": python }), Some(python)),
        (json!({ "pluginID": "" }), None),
        (json!({ "pluginID": 7 }), None),
        (json!({}), None),
        (json!({ "pluginID": python, "extra": true }), None),
        (json!(python), None),
        (Value::Null, None),
    ];
    for (id, (caller, expected)) in (10..).zip(attempts) {
        let extra = json!({ "caller": caller });
        session
            .host
            .invoke_with(id, "languageImport.settings", json!({}), extra);
        assert_eq!(session.response(id)["result"], Value::Null);
        let reported = seen.recv_timeout(WAIT).unwrap();
        assert_eq!(reported.as_deref(), expected, "{caller}");
    }
    session.close();
}

#[test]
fn relay_requests_carry_the_fixture_params_and_only_the_fields_the_methods_define() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let mut forwarded = 0;
    for case in cases(&fixture(), "hostRequests") {
        if case["expected"]["relay"] != true {
            continue;
        }
        let method = case["method"].as_str().unwrap().to_string();
        let answer = if method.ends_with("/providers") {
            json!({ "providers": [{ "pluginID": "dev.lithe.fixture.go", "version": 1 }] })
        } else {
            json!({ "value": { "settings": [] } })
        };
        let sending = {
            let host = host.clone();
            let (method, params) = (method.clone(), case["params"].clone());
            background(move || {
                if method.ends_with("/providers") {
                    let function_id = params["functionID"].as_str().unwrap();
                    host.functions().providers(function_id)
                } else {
                    let context = WorkspaceContext::from_value(&params["context"]).unwrap();
                    host.functions().invoke(&context, params)
                }
            })
        };
        let request = session.take(&method);
        assert_eq!(request["params"], case["params"], "{}", case["name"]);
        session.reply(&request, answer.clone());
        assert_eq!(done(&sending).unwrap(), answer, "{}", case["name"]);
        forwarded += 1;
    }
    assert_eq!(forwarded, 3);

    // Fields the method does not define are not sent: Core would refuse a
    // top-level `caller`, and the call's context is the argument.
    let call = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings",
        "caller": { "pluginID": "dev.lithe.fixture.other" },
        "context": { "contextID": "other", "generation": 9 }, "extra": true
    });
    let sending = {
        let host = host.clone();
        background(move || host.functions().invoke(&context(), call))
    };
    let request = session.take("lithe/host/functions/invoke");
    let expected = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings",
        "context": context().to_value()
    });
    assert_eq!(request["params"], expected);
    session.reply(&request, json!({ "value": null }));
    done(&sending).unwrap();
    session.close();
}

#[test]
fn a_relayed_call_from_a_handler_belongs_to_the_call_it_serves_unless_it_names_another() {
    let mut backend = Backend::new("test-backend");
    backend
        .function("project.collect", |params, served| {
            let value = served.host().functions().invoke(&context(), params)?;
            Ok(value)
        })
        .unwrap();
    let mut session = Session::start(backend);
    let call = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"
    });

    session.host.invoke(5, "project.collect", call.clone());
    let request = session.take("lithe/host/functions/invoke");
    assert_eq!(request["params"]["parentOperationID"], "op-5");
    session.reply(&request, json!({ "value": { "files": 3 } }));
    let response = session.response(5);
    assert_eq!(response["result"], json!({ "value": { "files": 3 } }));

    let mut named = call.clone();
    named["parentOperationID"] = json!("op-elsewhere");
    session.host.invoke(6, "project.collect", named);
    let request = session.take("lithe/host/functions/invoke");
    assert_eq!(request["params"]["parentOperationID"], "op-elsewhere");
    session.reply(&request, json!({ "value": null }));
    let response = session.response(6);
    assert_eq!(response["result"], json!({ "value": null }));

    // A parent the contract would refuse never leaves the SDK.
    let mut invalid = call;
    invalid["parentOperationID"] = json!("");
    session.host.invoke(7, "project.collect", invalid);
    assert!(session.response(7).get("error").is_some());
    assert_eq!(session.sent("lithe/host/functions/invoke").len(), 2);
    session.close();
}

#[test]
fn relay_refusals_keep_the_code_number_and_the_providers_own_code() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let call = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"
    });
    let mut refused = |wire: Value| -> SdkError {
        let pending = {
            let (host, call) = (host.clone(), call.clone());
            background(move || host.functions().invoke(&context(), call))
        };
        let request = session.take("lithe/host/functions/invoke");
        session.refuse(&request, wire);
        done(&pending).expect_err("a refusal")
    };
    let relay_errors = cases(&fixture(), "relayErrors");
    assert_eq!(relay_errors.len(), 15);
    for case in relay_errors {
        let expected = case["code"].as_str().unwrap();
        let number = case["rpcCode"].as_i64().unwrap();
        let data = json!({ "code": expected });
        let named = refused(json!({ "code": number, "message": "Refused.", "data": data }));
        assert_eq!(named.code(), expected, "{}", case["name"]);
        assert_eq!(named.rpc_code(), Some(number), "{}", case["name"]);
        // The number alone names the code.
        let numbered = refused(json!({ "code": number, "message": "Refused." }));
        assert_eq!(numbered.code(), expected, "{}", case["name"]);
        assert_eq!(numbered.rpc_code(), Some(number), "{}", case["name"]);
    }
    let failed = refused(json!({
        "code": -32070, "message": "The provider failed.",
        "data": {
            "code": "plugin.function.failed", "details": { "pluginErrorCode": "db.locked" }
        }
    }));
    assert_eq!(failed.code(), "plugin.function.failed");
    assert_eq!(failed.details().unwrap()["pluginErrorCode"], "db.locked");
    session.close();
}

#[test]
fn an_invalid_relayed_call_fails_locally_and_nothing_is_sent() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let session = Session::start(backend);
    let functions = host.functions();
    let valid = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"
    });
    let attempts = [
        ("pluginID", json!("")),
        ("pluginID", json!(7)),
        ("functionID", json!("Not-Valid")),
        ("params", json!([1])),
        ("timeoutMilliseconds", json!(0)),
        ("timeoutMilliseconds", json!(-5)),
        ("timeoutMilliseconds", json!(1.5)),
        ("timeoutMilliseconds", json!("5")),
        ("parentOperationID", json!(17)),
        ("parentOperationID", json!("")),
        ("parentOperationID", json!("p".repeat(129))),
    ];
    for (field, bad) in attempts {
        let label = format!("{field}: {bad}");
        let mut call = valid.clone();
        call[field] = bad;
        let result = functions.invoke(&context(), call);
        assert_eq!(code(result), "sdk.functions.invalidCall", "{label}");
    }
    let incomplete = [
        json!([]),
        json!({ "functionID": "languageImport.settings" }),
        json!({ "pluginID": "dev.lithe.fixture.go" }),
    ];
    for call in incomplete {
        let label = call.to_string();
        let result = functions.invoke(&context(), call);
        assert_eq!(code(result), "sdk.functions.invalidCall", "{label}");
    }
    for bad in ["", "Not-Valid", "UPPER", "a..b"] {
        let result = functions.providers(bad);
        assert_eq!(code(result), "sdk.functions.invalidCall", "{bad:?}");
    }

    // The fixture's cases Core refuses as invalid params agree with these
    // checks, whenever the call names a context.
    let mut refused = 0;
    for case in cases(&fixture(), "hostRequests") {
        let params = &case["params"];
        if case["method"] != "lithe/host/functions/invoke"
            || case["expected"]["code"] != "plugin.host.invalidParams"
            || params.get("context").is_none()
        {
            continue;
        }
        let workspace = WorkspaceContext::from_value(&params["context"]).unwrap();
        let result = functions.invoke(&workspace, params.clone());
        assert_eq!(
            code(result),
            "sdk.functions.invalidCall",
            "{}",
            case["name"]
        );
        refused += 1;
    }
    assert_eq!(refused, 4);
    session.host.assert_quiet();
    session.close();
}

#[test]
fn relay_answers_are_the_hosts_objects_and_a_null_value_stays_inside_value() {
    let backend = Backend::new("test-backend");
    let host = backend.handle().host();
    let mut session = Session::start(backend);
    let listing = {
        let host = host.clone();
        background(move || host.functions().providers("languageImport.settings"))
    };
    let request = session.take("lithe/host/functions/providers");
    let answer = json!({
        "providers": [{ "pluginID": "dev.lithe.fixture.go", "version": 1 }]
    });
    session.reply(&request, answer.clone());
    assert_eq!(done(&listing).unwrap(), answer);

    let call = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"
    });
    let calling = {
        let host = host.clone();
        background(move || host.functions().invoke(&context(), call))
    };
    let request = session.take("lithe/host/functions/invoke");
    session.reply(&request, json!({ "value": null }));
    assert_eq!(done(&calling).unwrap(), json!({ "value": null }));

    // An answer that is not an object is the host's mistake, not a value.
    let broken = {
        let host = host.clone();
        background(move || host.functions().providers("languageImport.settings"))
    };
    let request = session.take("lithe/host/functions/providers");
    session.reply(&request, json!([]));
    assert_eq!(code(done(&broken)), "sdk.host.invalidResult");
    session.close();
}

#[test]
fn cancelling_a_call_cancels_the_relayed_call_it_waits_on() {
    let (sender, outcome) = mpsc::channel();
    let mut backend = Backend::new("test-backend");
    backend
        .function("project.collect", move |params, served| {
            let relay = served.host().functions().invoke(&context(), params);
            let failure = relay.as_ref().err().map(|error| error.code().to_string());
            sender.send(failure).unwrap();
            Ok(relay?)
        })
        .unwrap();
    let mut session = Session::start(backend);
    let call = json!({
        "pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"
    });
    session.host.invoke(41, "project.collect", call);
    let request = session.take("lithe/host/functions/invoke");
    session.host.notify("$/cancelRequest", json!({ "id": 41 }));
    assert_eq!(session.response(41)["error"]["code"], -32800);
    // The relayed request fails with the call's cancellation, and Core is told.
    assert_eq!(
        outcome.recv_timeout(WAIT).unwrap().as_deref(),
        Some("sdk.call.cancelled")
    );
    let cancels: Vec<_> = session
        .sent("$/cancelRequest")
        .into_iter()
        .map(|frame| frame["params"].clone())
        .collect();
    assert_eq!(cancels, [json!({ "id": request["id"] })]);
    session.close();
}
