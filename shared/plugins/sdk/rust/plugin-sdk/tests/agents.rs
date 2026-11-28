//! Native agent sessions and the `agents` host service (plan API-45), driven
//! by a scripted host and `fixtures/agents/native-session.json`.

mod common;

use common::{Host, WAIT};
use lithe_plugin_sdk::{
    AgentSession, AgentSessionHandle, AgentSessionStart, Backend, FunctionError,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../contract/v1/fixtures/agents/native-session.json");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("the fixture is JSON")
}

/// Opens once agent-2 exited. agent-1's first output waits for it, so
/// sessions sharing one worker never get past it.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn open(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }

    fn wait(&self) {
        let (open, ready) = &*self.0;
        let (open, timeout) = ready
            .wait_timeout_while(open.lock().unwrap(), WAIT, |open| !*open)
            .unwrap();
        assert!(*open && !timeout.timed_out(), "agent-2 to exit in time");
    }
}

/// Records every call as `[session, call, args...]`, as the fixture lists
/// them; fails sessions in `/work/broken` and prompts that say `fail`.
struct Recorder {
    session: String,
    calls: mpsc::Sender<Value>,
    other_exited: Gate,
}

impl Recorder {
    fn record(&self, call: Value) {
        let mut entry = vec![json!(self.session)];
        entry.extend(call.as_array().unwrap().iter().cloned());
        self.calls.send(Value::Array(entry)).unwrap();
    }
}

impl AgentSession for Recorder {
    fn output(&mut self, lines: Vec<String>) -> Result<(), FunctionError> {
        if self.session == "agent-1" && lines.iter().any(|line| line == "one") {
            self.other_exited.wait();
        }
        self.record(json!(["output", lines]));
        Ok(())
    }

    fn prompt(&mut self, turn: u64, text: String) -> Result<(), FunctionError> {
        self.record(json!(["prompt", turn, text]));
        if text == "fail" {
            return Err(FunctionError::internal("prompt_failed"));
        }
        Ok(())
    }

    fn cancel(&mut self, turn: u64) -> Result<(), FunctionError> {
        self.record(json!(["cancel", turn]));
        Ok(())
    }

    fn decide_permission(
        &mut self,
        turn: u64,
        permission: String,
        option: String,
    ) -> Result<(), FunctionError> {
        self.record(json!(["decidePermission", turn, permission, option]));
        Ok(())
    }

    fn set_config_option(&mut self, option: String, value: String) -> Result<(), FunctionError> {
        self.record(json!(["setConfigOption", option, value]));
        Ok(())
    }

    fn exited(&mut self, exit_code: Option<i64>) {
        self.record(json!(["exited", exit_code]));
        if self.session == "agent-2" {
            self.other_exited.open();
        }
    }
}

fn recording_backend(calls: mpsc::Sender<Value>) -> Backend {
    let mut backend = Backend::new("agents-test");
    let other_exited = Gate::default();
    backend.on_agent_sessions(
        move |start: AgentSessionStart, session: AgentSessionHandle| {
            assert_eq!(session.id(), start.session);
            if start.cwd == "/work/broken" {
                return Err(FunctionError::internal("start_failed"));
            }
            let recorder = Recorder {
                session: start.session.clone(),
                calls: calls.clone(),
                other_exited: other_exited.clone(),
            };
            recorder.record(json!([
                "start",
                {
                    "session": start.session,
                    "agentID": start.agent_id,
                    "cwd": start.cwd,
                    "resume": start.resume,
                }
            ]));
            Ok(Box::new(recorder))
        },
    );
    backend
}

fn started(backend: Backend) -> Host {
    let mut host = Host::start(backend);
    host.initialize(json!([0]));
    host
}

fn send_all(host: &mut Host, notifications: &Value) {
    for notification in notifications.as_array().unwrap() {
        host.notify(
            notification["method"].as_str().unwrap(),
            notification["params"].clone(),
        );
    }
}

/// The calls of every session, in the order each session received them.
/// Every session's last call is its exit, so nothing follows the count.
fn calls_by_session(calls: &mpsc::Receiver<Value>, count: usize) -> BTreeMap<String, Vec<Value>> {
    let mut sessions: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for _ in 0..count {
        let call = calls.recv_timeout(WAIT).expect("a session call in time");
        let mut call = call.as_array().unwrap().clone();
        let session = call.remove(0).as_str().unwrap().to_string();
        sessions
            .entry(session)
            .or_default()
            .push(Value::Array(call));
    }
    sessions
}

fn expected_calls(calls: &Value) -> BTreeMap<String, Vec<Value>> {
    calls
        .as_object()
        .unwrap()
        .iter()
        .map(|(session, calls)| (session.clone(), calls.as_array().unwrap().clone()))
        .collect()
}

fn count(calls: &BTreeMap<String, Vec<Value>>) -> usize {
    calls.values().map(Vec::len).sum()
}

/// Every `operationFailed` code among `frames`.
fn failure_codes(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter(|frame| {
            frame["method"] == "lithe/log" && frame["params"]["kind"] == "operationFailed"
        })
        .map(|frame| frame["params"]["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn each_session_gets_its_notifications_in_order_from_start_to_exit() {
    let fixture = fixture();
    let expected = expected_calls(&fixture["sessions"]["calls"]);
    let (sender, calls) = mpsc::channel();
    let mut host = started(recording_backend(sender));
    send_all(&mut host, &fixture["sessions"]["notifications"]);
    assert_eq!(calls_by_session(&calls, count(&expected)), expected);
    let (code, frames) = host.finish();
    assert_eq!(code, 1);
    // agent-9 never started, so its output reaches no handler.
    assert_eq!(failure_codes(&frames), ["agent_session_unknown"]);
}

#[test]
fn a_session_that_fails_to_start_or_a_failed_prompt_ends_the_turn_as_failed() {
    let fixture = fixture();
    let failures = &fixture["failures"];
    let expected = expected_calls(&failures["calls"]);
    let (sender, calls) = mpsc::channel();
    let mut host = started(recording_backend(sender));
    send_all(&mut host, &failures["notifications"]);

    let mut reports = BTreeSet::new();
    let wanted = failures["reports"].as_array().unwrap().len();
    while reports.len() < wanted {
        let frame = host.next();
        if frame["method"] == "lithe/host/agents/report" {
            reports.insert(frame["params"].to_string());
            host.send(json!({ "jsonrpc": "2.0", "id": frame["id"], "result": {} }));
        }
    }
    let expected_reports: BTreeSet<String> = failures["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::to_string)
        .collect();
    assert_eq!(reports, expected_reports);
    assert_eq!(calls_by_session(&calls, count(&expected)), expected);
    assert_eq!(host.close(), 1);
}

#[test]
fn agent_host_methods_send_the_fixture_requests() {
    let fixture = fixture();
    let backend = Backend::new("agents-test");
    let handle = backend.handle();
    let mut host = started(backend);
    for case in fixture["methods"].as_array().unwrap() {
        let agents = handle.host().agents();
        let case_clone = case.clone();
        let (done, result) = mpsc::channel();
        thread::spawn(move || {
            let session = case_clone["session"].as_str().unwrap_or_default();
            let outcome = match case_clone["call"].as_str().unwrap() {
                "write" => {
                    let lines: Vec<String> =
                        serde_json::from_value(case_clone["lines"].clone()).unwrap();
                    agents.write(session, &lines).map(|()| json!({}))
                }
                "report" => agents
                    .report(session, case_clone["events"].as_array().unwrap())
                    .map(|()| json!({})),
                _ => agents.usage(case_clone["params"].clone()),
            };
            done.send(outcome.map_err(|error| error.code().to_string()))
                .unwrap();
        });
        let request = host.next();
        assert_eq!(request["method"], case["request"]["method"]);
        assert_eq!(request["params"], case["request"]["params"]);
        let answer = if case["call"] == "usage" {
            json!({ "sinceMs": 1, "untilMs": 2, "summaries": [], "rateLimits": [] })
        } else {
            json!({})
        };
        host.send(json!({ "jsonrpc": "2.0", "id": request["id"], "result": answer }));
        let outcome = result.recv_timeout(WAIT).expect("the request to finish");
        assert_eq!(outcome, Ok(answer));
    }
    assert_eq!(host.close(), 1);
}

#[test]
fn agent_notifications_are_routed_only_through_agent_sessions() {
    let mut backend = Backend::new("agents-test");
    for method in [
        "lithe/host/agents/didStart",
        "lithe/host/agents/didOutput",
        "lithe/host/agents/didExit",
    ] {
        let refused = backend.on_host_notification(method, |_| Ok(()));
        assert_eq!(refused.err().unwrap().code(), "sdk.method.reserved");
    }
    // Without a registration a started session is only logged.
    let mut host = started(backend);
    host.notify(
        "lithe/host/agents/didStart",
        json!({ "session": "agent-1", "agentID": "example", "cwd": "/work/orders" }),
    );
    host.notify(
        "lithe/host/agents/didOutput",
        json!({ "session": "agent-1", "lines": ["one"] }),
    );
    // The reader logs both before it reads the end of the input.
    let (code, frames) = host.finish();
    assert_eq!(code, 1);
    assert_eq!(failure_codes(&frames), ["agent_unhandled"]);
}
