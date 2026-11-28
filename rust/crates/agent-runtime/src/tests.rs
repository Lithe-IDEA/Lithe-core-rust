//! Tests of the agent runtime with a scripted backend and agent CLI.
//!
//! A fake [`BackendLink`] stands in for the engine: it answers plugin
//! functions as a synthetic agent's backend would and records every
//! notification Core sends. Every agent CLI is a [`ScriptedServer`], so no
//! process starts. Every wait has its own deadline, and a harness that is
//! dropped ends its sessions and waits, bounded, for them to finish.

use crate::backend::BackendLink;
use crate::{Host, Runtime};
use lithe_diagnostics::core_events::capture;
use lithe_lsp_engine::agent_host::{AgentBackend, AgentCallFailure, AgentHost, AgentHostFailure};
use lithe_lsp_transport::process::{LspProcessLauncher, LspProcessSpec, LspProcessStreams};
use lithe_lsp_transport::scripted::ScriptedServer;
use lithe_plugin_rpc::{
    codes, AGENTS_DID_CANCEL, AGENTS_DID_DECIDE_PERMISSION, AGENTS_DID_EXIT, AGENTS_DID_OUTPUT,
    AGENTS_DID_PROMPT, AGENTS_DID_SET_CONFIG_OPTION, AGENTS_DID_START, AGENTS_REPORT, AGENTS_USAGE,
    AGENTS_WRITE,
};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_request_context::{trace, Source};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

const PLUGIN: &str = "dev.lithe.test.agents";
const AGENT: &str = "sample";
const AGENT_KEY: &str = "dev.lithe.test.agents/sample";
const BACKEND: &str = "backend-1";
const WAIT: Duration = Duration::from_secs(5);
const ALL_GRANTS: &[&str] = &[
    "agents.native",
    "agents.run",
    "agents.sessions.read",
    "backend.launch",
];

/// A directory under the system temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lithe-agent-runtime {prefix} {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // A crashed earlier run with the same process ID may have left it.
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove a stale temporary directory");
        }
        fs::create_dir_all(&path).expect("create a temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("could not remove {}: {error}", self.0.display());
        }
    }
}

/// A synthetic agent plugin: one agent with a native mode, a terminal form,
/// an update command and one session source.
fn manifest() -> Value {
    json!({
        "schemaVersion": 0,
        "id": PLUGIN,
        "displayName": "Agent runtime test plugin",
        "version": "1.0.0",
        "pluginAPIVersion": 0,
        "requires": { "agents": 0, "backend": 0 },
        "requiresTools": ["sample-cli"],
        "vendor": {
            "id": "dev.lithe.test",
            "displayName": "Lithe tests",
            "signatureRequirement": "publisherPackage"
        },
        "permissions": ALL_GRANTS,
        "activationEvents": ["onAgent:sample"],
        "agents": [{
            "id": AGENT,
            "displayName": "Sample Agent",
            "tool": "sample-cli",
            "native": { "function": "agent.launch" },
            "terminal": { "arguments": [] },
            "update": { "arguments": ["update"] },
            "sessionSources": [{
                "id": "projects",
                "roots": ["${env:SAMPLE_HOME}/sessions", "${userHome}/.sample/sessions"],
                "include": ["**/*.jsonl"],
                "format": "jsonLines",
                "scan": "sessions.scan",
                "read": "sessions.read"
            }]
        }],
        "backend": { "launch": { "kind": "packaged", "path": "bin/backend" }, "arguments": ["--stdio"] }
    })
}

fn path_text(path: &Path) -> String {
    path.to_str().expect("test paths are UTF-8").to_string()
}

fn failure(code: &str) -> AgentCallFailure {
    AgentCallFailure {
        code: code.to_string(),
        message: "The test backend failed.".to_string(),
        plugin_error_code: None,
    }
}

/// How the synthetic agent's backend answers its functions.
fn sample_backend(function: &str, params: &Value) -> Result<Value, AgentCallFailure> {
    match function {
        "agent.launch" => Ok(json!({
            "executable": { "kind": "tool", "tool": "sample-cli" },
            "arguments": ["--stream"]
        })),
        "sessions.scan" => Ok(scan(params)),
        "sessions.read" => Ok(read(params)),
        _ => Err(failure("plugin.function.unknown")),
    }
}

/// Parses the synthetic session format: one JSON object per line, with a
/// `type` of `session`, `user`, `call` or `limits`.
fn scan(params: &Value) -> Value {
    let mut sessions = Vec::new();
    let mut calls = Vec::new();
    let mut spans = Vec::new();
    let mut rate_limits = None;
    for line in params["lines"].as_array().expect("scan lines") {
        let offset = line["offset"].as_u64().expect("line offset");
        let length = line["length"].as_u64().expect("line length");
        let range = json!({ "start": offset, "end": offset + length });
        let Ok(record) = serde_json::from_str::<Value>(line["text"].as_str().unwrap_or_default())
        else {
            continue;
        };
        match record["type"].as_str() {
            Some("session") => sessions.push(json!({
                "sessionID": record["id"],
                "cwd": record["cwd"],
                "startedAtMs": record["at"],
                "updatedAtMs": record["at"],
            })),
            Some("user") => {
                sessions.push(json!({
                    "sessionID": record["session"],
                    "updatedAtMs": record["at"],
                    "titleRange": range,
                }));
                spans.push(json!({
                    "sessionID": record["session"],
                    "atMs": record["at"],
                    "range": range,
                    "text": record["text"],
                }));
            }
            Some("call") => calls.push(json!({
                "key": record["key"],
                "sessionID": record["session"],
                "startedAtMs": record["at"],
                "model": record["model"],
                "tokens": { "input": record["input"], "output": record["output"] },
                "firstTokenMs": record["first"],
                "durationMs": record["duration"],
                "range": range,
            })),
            Some("limits") => {
                rate_limits = Some(json!({
                    "atMs": record["at"],
                    "windows": [{ "id": "5h", "usedPercent": record["used"], "windowMinutes": 300 }],
                    "plan": "pro",
                }));
            }
            _ => {}
        }
    }
    let mut answer = json!({ "sessions": sessions, "calls": calls, "spans": spans });
    if let Some(limits) = rate_limits {
        answer["rateLimits"] = limits;
    }
    answer
}

/// Reads items of the synthetic format: the first user text as the title, or
/// every user text as a message.
fn read(params: &Value) -> Value {
    let titles = params["mode"] == "titles";
    let items: Vec<Value> = params["items"]
        .as_array()
        .expect("read items")
        .iter()
        .map(|item| {
            let texts: Vec<String> = item["lines"]
                .as_array()
                .expect("item lines")
                .iter()
                .filter_map(|line| serde_json::from_str::<Value>(line.as_str()?).ok())
                .filter(|record| record["type"] == "user")
                .filter_map(|record| record["text"].as_str().map(str::to_string))
                .collect();
            if titles {
                json!({ "title": texts.first() })
            } else {
                let messages: Vec<Value> = texts
                    .iter()
                    .map(|text| json!({ "role": "user", "text": text }))
                    .collect();
                json!({ "messages": messages })
            }
        })
        .collect();
    json!({ "items": items })
}

/// One CLI the runtime started.
struct Launch {
    executable: PathBuf,
    arguments: Vec<String>,
    working_directory: PathBuf,
    environment: BTreeMap<String, String>,
    server: Arc<ScriptedServer>,
}

/// Starts a new scripted CLI for every launch and keeps what it was given.
#[derive(Default)]
struct RecordingLauncher {
    launches: Mutex<Vec<Launch>>,
}

impl LspProcessLauncher for RecordingLauncher {
    fn launch(&self, spec: LspProcessSpec) -> Result<LspProcessStreams, CoreError> {
        let server = Arc::new(ScriptedServer::new());
        let launch = Launch {
            executable: spec.executable.clone(),
            arguments: spec.arguments.clone(),
            working_directory: spec.working_directory.clone(),
            environment: spec.environment.clone(),
            server: server.clone(),
        };
        let streams = server.launcher().launch(spec)?;
        self.launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(launch);
        Ok(streams)
    }
}

type Answer = Box<dyn Fn(&str, &Value) -> Result<Value, AgentCallFailure> + Send + Sync>;

/// A notification Core sent to a backend.
struct Notice {
    backend: String,
    method: String,
    params: Value,
}

/// The engine as the runtime sees it, with one backend of [`PLUGIN`].
struct FakeLink {
    backend: Mutex<Option<String>>,
    answer: Mutex<Answer>,
    calls: Mutex<Vec<(String, Value)>>,
    notices: Mutex<Vec<Notice>>,
    notices_changed: Condvar,
    /// A notification method the backend refuses, as a backend that stopped.
    refused: Mutex<Option<&'static str>>,
    launcher: Arc<RecordingLauncher>,
}

impl FakeLink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            backend: Mutex::new(Some(BACKEND.to_string())),
            answer: Mutex::new(Box::new(sample_backend)),
            calls: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
            notices_changed: Condvar::new(),
            refused: Mutex::new(None),
            launcher: Arc::new(RecordingLauncher::default()),
        })
    }

    fn set_backend(&self, backend: Option<&str>) {
        *self.backend.lock().unwrap() = backend.map(str::to_string);
    }

    fn set_answer(
        &self,
        answer: impl Fn(&str, &Value) -> Result<Value, AgentCallFailure> + Send + Sync + 'static,
    ) {
        *self.answer.lock().unwrap() = Box::new(answer);
    }

    fn refuse(&self, method: Option<&'static str>) {
        *self.refused.lock().unwrap() = method;
    }

    fn launch_count(&self) -> usize {
        self.launcher.launches.lock().unwrap().len()
    }

    fn server(&self, index: usize) -> Arc<ScriptedServer> {
        self.launcher.launches.lock().unwrap()[index].server.clone()
    }

    fn calls_of(&self, function: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(called, _)| called == function)
            .map(|(_, params)| params.clone())
            .collect()
    }

    fn notices_of(&self, method: &str) -> Vec<Value> {
        self.notices
            .lock()
            .unwrap()
            .iter()
            .filter(|notice| notice.method == method)
            .map(|notice| notice.params.clone())
            .collect()
    }

    /// Waits for a notification of `method` whose params `matches` accepts.
    fn await_notice(&self, method: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        let mut notices = self.notices.lock().unwrap();
        loop {
            if let Some(notice) = notices
                .iter()
                .find(|notice| notice.method == method && matches(&notice.params))
            {
                assert_eq!(notice.backend, BACKEND);
                return notice.params.clone();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                drop(notices);
                panic!("no {method} notification arrived in time");
            }
            notices = self
                .notices_changed
                .wait_timeout(notices, remaining)
                .unwrap()
                .0;
        }
    }

    /// Waits until the `didOutput` notifications of `session` carry `count`
    /// lines, and returns them in order.
    fn await_output(&self, session: &str, count: usize) -> Vec<String> {
        let deadline = Instant::now() + WAIT;
        let mut notices = self.notices.lock().unwrap();
        loop {
            let lines: Vec<String> = notices
                .iter()
                .filter(|notice| {
                    notice.method == AGENTS_DID_OUTPUT && notice.params["session"] == session
                })
                .flat_map(|notice| {
                    notice.params["lines"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|line| line.as_str().map(str::to_string))
                .collect();
            if lines.len() >= count {
                return lines;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                drop(notices);
                panic!("{count} output lines of {session} did not arrive in time");
            }
            notices = self
                .notices_changed
                .wait_timeout(notices, remaining)
                .unwrap()
                .0;
        }
    }
}

impl BackendLink for FakeLink {
    fn session_for(
        &self,
        plugin_id: &str,
        _function_id: &str,
        _root_uri: Option<&str>,
    ) -> Option<String> {
        if plugin_id != PLUGIN {
            return None;
        }
        self.backend.lock().unwrap().clone()
    }

    fn call(
        &self,
        session_id: &str,
        plugin_id: &str,
        function_id: &str,
        params: Value,
        _timeout: Duration,
    ) -> Result<Value, AgentCallFailure> {
        assert_eq!(session_id, BACKEND);
        assert_eq!(plugin_id, PLUGIN);
        self.calls
            .lock()
            .unwrap()
            .push((function_id.to_string(), params.clone()));
        let answer = self.answer.lock().unwrap();
        answer(function_id, &params)
    }

    fn notify(&self, session_id: &str, method: &str, params: Value) -> Result<(), CoreError> {
        if self
            .refused
            .lock()
            .unwrap()
            .is_some_and(|refused| refused == method)
        {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "The test backend refused the notification.",
            ));
        }
        self.notices.lock().unwrap().push(Notice {
            backend: session_id.to_string(),
            method: method.to_string(),
            params,
        });
        self.notices_changed.notify_all();
        Ok(())
    }

    fn launcher(&self) -> Arc<dyn LspProcessLauncher> {
        self.launcher.clone()
    }
}

/// A runtime with the synthetic plugin configured, a tool file, a working
/// directory and a session cache, driven as a user request.
struct Harness {
    dir: TempDir,
    runtime: Arc<Runtime>,
    link: Arc<FakeLink>,
    host: Host,
    tool: PathBuf,
    work: PathBuf,
    /// The last event sequence [`Harness::next_event`] looked at.
    cursor: u64,
    trace: Option<trace::Context>,
}

impl Harness {
    fn new(name: &str, grants: &[&str]) -> Self {
        let dir = TempDir::new(name);
        let tool = dir.path().join("bin").join("sample-cli");
        fs::create_dir_all(tool.parent().unwrap()).unwrap();
        fs::write(&tool, b"").unwrap();
        let work = dir.path().join("work");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(dir.path().join("cache")).unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        let link = FakeLink::new();
        let runtime = Runtime::new(link.clone());
        let host = Host(runtime.clone());
        let previous = trace::replace(trace::Context {
            op: None,
            source: Source::User,
        });
        let harness = Self {
            dir,
            runtime,
            link,
            host,
            tool,
            work,
            cursor: 0,
            trace: Some(previous),
        };
        harness.grant(grants);
        harness.configure_records();
        harness
    }

    fn grant(&self, grants: &[&str]) -> Value {
        self.run(
            "agents.configure",
            json!({ "plugins": [{ "manifest": manifest(), "grantedPermissions": grants }] }),
        )
    }

    fn configure_records(&self) -> Value {
        self.run(
            "agentSessions.configure",
            json!({
                "cacheDirectory": path_text(&self.dir.path().join("cache")),
                "userHome": path_text(&self.dir.path().join("home")),
            }),
        )
    }

    fn run(&self, command: &str, payload: Value) -> Value {
        self.runtime
            .execute(command, payload)
            .unwrap_or_else(|error| panic!("{command} failed: {error:?}"))
    }

    /// The `details` code of a refused command.
    fn refused(&self, command: &str, payload: Value) -> String {
        match self.runtime.execute(command, payload) {
            Ok(answer) => panic!("{command} was not refused: {answer}"),
            Err(error) => error.details.unwrap_or_default(),
        }
    }

    fn start_payload(&self) -> Value {
        json!({
            "pluginID": PLUGIN,
            "agentID": AGENT,
            "cwd": path_text(&self.work),
            "toolPath": path_text(&self.tool),
            "workspaceTrusted": true,
            "environment": { "PATH": "/usr/bin" },
        })
    }

    fn start(&self) -> String {
        let answer = self.run("agents.start", self.start_payload());
        assert_eq!(answer["mode"], "native");
        answer["session"].as_str().expect("session ID").to_string()
    }

    fn answer(&self, method: &str, params: Value) -> Result<Value, AgentHostFailure> {
        self.answer_as(BACKEND, PLUGIN, method, params)
    }

    fn answer_as(
        &self,
        backend: &str,
        plugin: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, AgentHostFailure> {
        self.host.answer(
            AgentBackend {
                session_id: backend,
                plugin_id: plugin,
            },
            method,
            &params,
        )
    }

    fn report(&self, session: &str, events: Value) -> Result<Value, AgentHostFailure> {
        self.answer(
            AGENTS_REPORT,
            json!({ "session": session, "events": events }),
        )
    }

    /// The next event of `kind` for `session` after the ones already looked
    /// at. Events of other kinds are passed over.
    fn next_event(&mut self, session: &str, kind: &str) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "no {kind} event of {session} arrived in time"
            );
            let batch = self.run(
                "agents.waitEvents",
                json!({
                    "after": self.cursor,
                    "timeoutMilliseconds": u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX),
                }),
            );
            for entry in batch["events"].as_array().expect("events") {
                self.cursor = entry["sequence"].as_u64().expect("sequence");
                if entry["session"] == session && entry["event"]["kind"] == kind {
                    return entry["event"].clone();
                }
            }
        }
    }

    fn sessions(&self) -> Vec<Value> {
        self.run("agents.sessions", json!({}))["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Waits until no session runs.
    fn settle(&self) -> bool {
        let deadline = Instant::now() + WAIT;
        let mut after = 0;
        loop {
            if self.runtime.live.list()["sessions"]
                .as_array()
                .is_some_and(Vec::is_empty)
            {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            // A session leaves the list before its `exited` event is queued,
            // so waiting for the next event cannot miss the change.
            let batch = self.runtime.live.queue.wait(after, remaining, 512);
            after = batch["next"].as_u64().unwrap_or(after);
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let servers: Vec<Arc<ScriptedServer>> = self
            .link
            .launcher
            .launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|launch| launch.server.clone())
            .collect();
        for server in servers {
            if server.was_launched() {
                server.exit(Some(0));
            }
        }
        let settled = self.settle();
        trace::replace(self.trace.take().unwrap_or_default());
        if !std::thread::panicking() {
            assert!(settled, "agent sessions still ran after the test");
        }
    }
}

#[test]
fn configure_lists_agents_with_their_grants_and_reports_invalid_manifests() {
    let harness = Harness::new("configure", ALL_GRANTS);
    let mut broken = manifest();
    broken["id"] = json!("dev.lithe.test.broken");
    // Agents need agents.run declared.
    broken["permissions"] = json!(["agents.native", "agents.sessions.read", "backend.launch"]);
    let answer = harness.run(
        "agents.configure",
        json!({ "plugins": [
            {
                "manifest": manifest(),
                // A grant the manifest does not declare is ignored.
                "grantedPermissions": ["agents.sessions.read", "agents.usage.read"],
            },
            { "manifest": broken },
        ]}),
    );
    let agents = answer["agents"].as_array().expect("agents");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["pluginID"], PLUGIN);
    assert_eq!(agents[0]["agentID"], AGENT);
    assert_eq!(agents[0]["mayRun"], false);
    assert_eq!(agents[0]["mayReadSessions"], true);
    assert_eq!(agents[0]["sessionRootVariables"], json!(["SAMPLE_HOME"]));
    let rejected = answer["rejected"].as_array().expect("rejected");
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0]["pluginID"], "dev.lithe.test.broken");
    assert!(rejected[0]["code"]
        .as_str()
        .is_some_and(|code| code.starts_with("plugin.")));
    assert_eq!(
        harness.run("agents.catalog", json!({}))["agents"],
        answer["agents"]
    );

    assert_eq!(
        harness.refused(
            "agents.configure",
            json!({ "plugins": [], "unknown": true })
        ),
        "agents.invalidPayload"
    );
    assert_eq!(
        harness.refused("agents.unknown", json!({})),
        "agents.unknownCommand"
    );
}

#[test]
fn start_refuses_before_anything_runs() {
    let harness = Harness::new("refusals", ALL_GRANTS);
    let refused_with = |change: &dyn Fn(&mut Value)| {
        let mut payload = harness.start_payload();
        change(&mut payload);
        harness.refused("agents.start", payload)
    };

    let previous = trace::replace(trace::Context {
        op: None,
        source: Source::Background,
    });
    assert_eq!(refused_with(&|_| {}), "agents.userActionRequired");
    trace::replace(previous);

    assert_eq!(
        refused_with(&|payload| payload["workspaceTrusted"] = json!(false)),
        "agents.untrustedWorkspace"
    );
    assert_eq!(
        refused_with(&|payload| payload["agentID"] = json!("missing")),
        "agents.unknownAgent"
    );
    assert_eq!(
        refused_with(&|payload| payload["cwd"] = json!("work")),
        "agents.invalidDirectory"
    );
    assert_eq!(
        refused_with(&|payload| payload["toolPath"] = json!(path_text(&harness.work))),
        "agents.toolNotFound"
    );
    assert_eq!(
        refused_with(&|payload| payload["environment"] = json!({ "A=B": "value" })),
        "agents.invalidEnvironment"
    );
    assert_eq!(
        refused_with(&|payload| payload["resume"] = json!("")),
        "agents.invalidResume"
    );

    harness.link.set_backend(None);
    assert_eq!(refused_with(&|_| {}), "agents.backendUnavailable");
    harness.link.set_backend(Some(BACKEND));

    harness.link.set_answer(|_, _| {
        Ok(json!({
            "executable": { "kind": "tool", "tool": "sample-cli" },
            "arguments": [],
            "environment": { "SAMPLE_TOKEN": "value" }
        }))
    });
    assert_eq!(refused_with(&|_| {}), "plugin.agents.launchEnvironment");
    harness.link.set_answer(|_, _| {
        Ok(json!({ "executable": { "kind": "tool", "tool": "other-cli" }, "arguments": [] }))
    });
    assert_eq!(refused_with(&|_| {}), "plugin.agents.undeclaredTool");
    harness
        .link
        .set_answer(|_, _| Ok(json!({ "arguments": "--stream" })));
    assert_eq!(refused_with(&|_| {}), "plugin.agents.invalidLaunch");
    harness
        .link
        .set_answer(|_, _| Err(failure("requestTimeout")));
    assert_eq!(refused_with(&|_| {}), "agents.launchTimeout");
    harness
        .link
        .set_answer(|_, _| Err(failure("plugin.function.failed")));
    assert_eq!(refused_with(&|_| {}), "agents.launchFailed");

    harness.grant(&["agents.sessions.read", "backend.launch"]);
    assert_eq!(refused_with(&|_| {}), "agents.runNotGranted");

    assert_eq!(harness.link.launch_count(), 0);
    assert!(harness.link.notices_of(AGENTS_DID_START).is_empty());
}

#[test]
fn native_session_relays_the_cli_and_checks_reports_against_turns() {
    let mut harness = Harness::new("native", ALL_GRANTS);
    let (session, logs) = capture::run(false, || harness.start());
    assert_eq!(session, "agent-1");
    let started_log = logs
        .iter()
        .find(|log| log.event == "agent.session.started")
        .expect("the start is logged");
    assert_eq!(started_log.fields["session"], json!(session));

    let launches = harness.link.launcher.launches.lock().unwrap();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].executable, harness.tool);
    assert_eq!(launches[0].arguments, ["--stream"]);
    assert_eq!(launches[0].working_directory, harness.work);
    assert_eq!(
        launches[0].environment,
        BTreeMap::from([("PATH".to_string(), "/usr/bin".to_string())])
    );
    drop(launches);
    let server = harness.link.server(0);
    assert_eq!(
        harness.link.calls_of("agent.launch"),
        [json!({ "agentID": AGENT, "cwd": path_text(&harness.work) })]
    );
    assert_eq!(
        harness.link.await_notice(AGENTS_DID_START, |_| true),
        json!({ "agentID": AGENT, "cwd": path_text(&harness.work), "session": session })
    );
    let started = harness.next_event(&session, "started");
    assert_eq!(started["pluginID"], PLUGIN);
    assert_eq!(started["agentID"], AGENT);
    assert_eq!(started["mode"], "native");

    // The CLI's stdout reaches the backend line by line.
    server.send_raw(b"{\"a\":1}\r\n{\"b\":2}\n");
    assert_eq!(
        harness.link.await_output(&session, 2),
        ["{\"a\":1}", "{\"b\":2}"]
    );

    // What the backend writes reaches the CLI's stdin, one line each.
    harness
        .answer(
            AGENTS_WRITE,
            json!({ "session": session, "lines": ["{\"x\":1}", "{\"y\":2}"] }),
        )
        .expect("write");
    assert!(server.await_written_bytes(|bytes| bytes == b"{\"x\":1}\n{\"y\":2}\n"));
    let refusal = harness
        .answer(
            AGENTS_WRITE,
            json!({ "session": session, "lines": ["a\nb"] }),
        )
        .expect_err("a line break is refused");
    assert_eq!(refusal.code, codes::HOST_INVALID_PARAMS);
    for (backend, plugin) in [("backend-2", PLUGIN), (BACKEND, "dev.lithe.test.other")] {
        let refusal = harness
            .answer_as(
                backend,
                plugin,
                AGENTS_WRITE,
                json!({ "session": session, "lines": ["x"] }),
            )
            .expect_err("only the session's backend writes");
        assert_eq!(refusal.code, codes::HOST_NOT_FOUND);
    }

    // A prompt starts a turn.
    let prompt = "Explain the private build script";
    assert_eq!(
        harness.run(
            "agents.prompt",
            json!({ "session": session, "text": prompt })
        ),
        json!({ "turn": 1 })
    );
    assert_eq!(
        harness.link.await_notice(AGENTS_DID_PROMPT, |_| true),
        json!({ "session": session, "turn": 1, "text": prompt })
    );
    assert_eq!(harness.next_event(&session, "turnStarted")["turn"], 1);
    assert_eq!(
        harness.refused(
            "agents.prompt",
            json!({ "session": session, "text": "again" })
        ),
        "agents.turnInProgress"
    );
    assert_eq!(
        harness.refused("agents.prompt", json!({ "session": session, "text": "  " })),
        "agents.invalidPrompt"
    );

    // Reports must fit the turn in progress.
    harness
        .report(
            &session,
            json!([
                { "kind": "session", "agentSessionID": "cli-123", "model": "sample-model" },
                { "kind": "message", "turn": 1, "role": "assistant", "messageID": "m1", "text": "Hi", "delta": true },
                {
                    "kind": "permissionRequested",
                    "turn": 1,
                    "permission": "p1",
                    "title": "Run ls",
                    "options": [
                        { "id": "allow", "label": "Allow", "optionKind": "allowOnce" },
                        { "id": "deny", "label": "Deny", "optionKind": "rejectOnce" }
                    ]
                }
            ]),
        )
        .expect("report");
    assert_eq!(harness.next_event(&session, "message")["text"], "Hi");
    assert_eq!(
        harness.next_event(&session, "permissionRequested")["permission"],
        "p1"
    );
    let sessions = harness.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["pendingPermissions"], json!(["p1"]));
    assert_eq!(sessions[0]["activeTurn"], 1);
    assert_eq!(sessions[0]["agentSessionID"], "cli-123");

    let (refusal, logs) = capture::run(false, || {
        harness
            .report(
                &session,
                json!([{ "kind": "message", "turn": 2, "role": "assistant", "messageID": "m2", "text": "x" }]),
            )
            .expect_err("a later turn is refused")
    });
    assert_eq!(refusal.code, codes::HOST_PRECONDITION_FAILED);
    let rejected = logs
        .iter()
        .find(|log| log.event == "agent.report.rejected")
        .expect("the rejection is logged");
    assert_eq!(
        rejected.fields["code"],
        json!(codes::HOST_PRECONDITION_FAILED)
    );
    let refusal = harness
        .answer(
            AGENTS_REPORT,
            json!({ "session": session, "events": [{ "kind": "notice", "level": "info", "text": "x" }], "extra": 1 }),
        )
        .expect_err("an unknown field is refused");
    assert_eq!(refusal.code, codes::HOST_INVALID_PARAMS);
    let refusal = harness
        .report(&session, json!([{ "kind": "unknown" }]))
        .expect_err("an unknown event is refused");
    assert_eq!(refusal.code, codes::HOST_INVALID_PARAMS);
    let refusal = harness
        .answer_as(
            "backend-2",
            PLUGIN,
            AGENTS_REPORT,
            json!({ "session": session, "events": [{ "kind": "notice", "level": "info", "text": "x" }] }),
        )
        .expect_err("only the session's backend reports");
    assert_eq!(refusal.code, codes::HOST_NOT_FOUND);
    // Nothing of a refused report was applied.
    assert_eq!(harness.sessions()[0]["activeTurn"], 1);

    // The user answers the permission request with one of its options.
    let answer = |option: &str| json!({ "session": session, "permission": "p1", "option": option });
    assert_eq!(
        harness.refused("agents.respondPermission", answer("maybe")),
        "agents.unknownPermissionOption"
    );
    assert_eq!(
        harness.run("agents.respondPermission", answer("allow")),
        json!({ "turn": 1 })
    );
    assert_eq!(
        harness
            .link
            .await_notice(AGENTS_DID_DECIDE_PERMISSION, |_| true),
        json!({ "session": session, "turn": 1, "permission": "p1", "option": "allow" })
    );
    assert_eq!(
        harness.next_event(&session, "permissionDecided")["option"],
        "allow"
    );
    assert_eq!(
        harness.refused("agents.respondPermission", answer("allow")),
        "agents.permissionNotPending"
    );

    // Cancelling asks the backend; the backend ends the turn.
    assert_eq!(
        harness.run("agents.cancel", json!({ "session": session })),
        json!({ "turn": 1 })
    );
    assert_eq!(
        harness.link.await_notice(AGENTS_DID_CANCEL, |_| true),
        json!({ "session": session, "turn": 1 })
    );
    harness
        .report(
            &session,
            json!([
                { "kind": "usage", "turn": 1, "model": "sample-model", "tokens": { "input": 10, "output": 2 } },
                { "kind": "rateLimits", "windows": [{ "id": "5h", "usedPercent": 25.0 }], "plan": "pro" },
                { "kind": "turnEnded", "turn": 1, "reason": "cancelled" }
            ]),
        )
        .expect("the turn ends");
    assert_eq!(
        harness.next_event(&session, "turnEnded")["reason"],
        "cancelled"
    );
    assert_eq!(
        harness.refused("agents.cancel", json!({ "session": session })),
        "agents.noTurnInProgress"
    );
    assert_eq!(
        harness.run(
            "agents.prompt",
            json!({ "session": session, "text": "Next" })
        ),
        json!({ "turn": 2 })
    );

    // The latest rate limits are kept for the usage views.
    let stats = harness.run("agentSessions.stats", json!({ "groupBy": "agent" }));
    let limits = stats["rateLimits"].as_array().expect("rate limits");
    assert_eq!(limits.len(), 1);
    assert_eq!(limits[0]["pluginID"], PLUGIN);
    assert_eq!(limits[0]["windows"][0]["usedPercent"], 25.0);
    assert!(harness
        .dir
        .path()
        .join("cache")
        .join("rate-limits.json")
        .is_file());

    // The CLI exits by itself.
    server.exit(Some(0));
    assert_eq!(
        harness.link.await_notice(AGENTS_DID_EXIT, |_| true),
        json!({ "session": session, "exitCode": 0 })
    );
    let exited = harness.next_event(&session, "exited");
    assert_eq!(exited["reason"], "exited");
    assert_eq!(exited["exitCode"], 0);
    assert!(harness.sessions().is_empty());
    assert_eq!(
        harness.refused(
            "agents.prompt",
            json!({ "session": session, "text": "Late" })
        ),
        "agents.unknownSession"
    );
    let refusal = harness
        .answer(AGENTS_WRITE, json!({ "session": session, "lines": ["x"] }))
        .expect_err("an ended session takes no input");
    assert_eq!(refusal.code, codes::HOST_NOT_FOUND);

    // Lithe's logs carry IDs and codes, never what flowed through the session.
    for log in &logs {
        let text = serde_json::to_string(&log.fields).unwrap();
        assert!(!text.contains("cli-123") && !text.contains("private build"));
    }
}

#[test]
fn a_prompt_the_backend_does_not_take_ends_its_turn() {
    let mut harness = Harness::new("prompt-refused", ALL_GRANTS);
    let session = harness.start();
    harness.link.refuse(Some(AGENTS_DID_PROMPT));
    assert_eq!(
        harness.refused(
            "agents.prompt",
            json!({ "session": session, "text": "Hello" })
        ),
        "agents.backendUnavailable"
    );
    let ended = harness.next_event(&session, "turnEnded");
    assert_eq!(
        ended,
        json!({ "kind": "turnEnded", "turn": 1, "reason": "failed" })
    );
    assert!(harness.sessions()[0].get("activeTurn").is_none());
    harness.link.refuse(None);
    assert_eq!(
        harness.run(
            "agents.prompt",
            json!({ "session": session, "text": "Hello" })
        ),
        json!({ "turn": 2 })
    );
}

#[test]
fn session_settings_are_checked_and_reach_the_backend() {
    let mut harness = Harness::new("settings", ALL_GRANTS);
    let session = harness.start();
    let set =
        |option: &str, value: &str| json!({ "session": session, "option": option, "value": value });
    // Nothing is offered before the backend reports its settings.
    assert_eq!(
        harness.refused("agents.setConfigOption", set("model", "fast")),
        "agents.unknownConfigOption"
    );

    let settings = json!({
        "kind": "configOptions",
        "configOptions": [
            {
                "id": "model",
                "name": "Model",
                "category": "model",
                "currentValue": "fast",
                "options": [
                    { "value": "fast", "name": "Fast" },
                    { "value": "deep", "name": "Deep", "description": "Slower, more careful" }
                ]
            },
            {
                "id": "effort",
                "name": "Reasoning",
                "category": "thoughtLevel",
                "currentValue": "medium",
                "options": [
                    { "value": "low", "name": "Low" },
                    { "value": "medium", "name": "Medium" },
                    { "value": "high", "name": "High" }
                ]
            }
        ]
    });
    harness
        .report(&session, json!([settings.clone()]))
        .expect("settings are not bound to a turn");
    assert_eq!(harness.next_event(&session, "configOptions"), settings);

    assert_eq!(
        harness.refused("agents.setConfigOption", set("mode", "plan")),
        "agents.unknownConfigOption"
    );
    assert_eq!(
        harness.refused("agents.setConfigOption", set("effort", "max")),
        "agents.unknownConfigValue"
    );
    assert_eq!(
        harness.run("agents.setConfigOption", set("effort", "high")),
        json!({})
    );
    assert_eq!(
        harness
            .link
            .await_notice(AGENTS_DID_SET_CONFIG_OPTION, |_| true),
        json!({ "session": session, "option": "effort", "value": "high" })
    );

    // A setting may change while a turn runs; the backend decides when the
    // agent takes it.
    assert_eq!(
        harness.run("agents.prompt", json!({ "session": session, "text": "Hi" })),
        json!({ "turn": 1 })
    );
    assert_eq!(
        harness.run("agents.setConfigOption", set("model", "deep")),
        json!({})
    );

    // Each report replaces the settings; an empty list withdraws them.
    harness
        .report(
            &session,
            json!([{ "kind": "configOptions", "configOptions": [] }]),
        )
        .expect("no settings");
    assert_eq!(
        harness.refused("agents.setConfigOption", set("model", "fast")),
        "agents.unknownConfigOption"
    );

    // Malformed settings are refused and leave the last ones in place.
    let option = |current: &str, values: Value| {
        json!([{
            "kind": "configOptions",
            "configOptions": [{ "id": "model", "name": "Model", "currentValue": current, "options": values }]
        }])
    };
    for bad in [
        option("other", json!([{ "value": "fast", "name": "Fast" }])),
        option("fast", json!([])),
        option(
            "fast",
            json!([{ "value": "fast", "name": "Fast" }, { "value": "fast", "name": "Again" }]),
        ),
        option("fast", json!([{ "value": "fast", "name": "" }])),
        json!([{
            "kind": "configOptions",
            "configOptions": [
                { "id": "model", "name": "Model", "currentValue": "fast", "options": [{ "value": "fast", "name": "Fast" }] },
                { "id": "model", "name": "Model", "currentValue": "fast", "options": [{ "value": "fast", "name": "Fast" }] }
            ]
        }]),
        json!([{
            "kind": "configOptions",
            "configOptions": [{ "id": "model", "name": "Model", "type": "select", "currentValue": "fast", "options": [{ "value": "fast", "name": "Fast" }] }]
        }]),
    ] {
        let refusal = harness
            .report(&session, bad)
            .expect_err("malformed settings");
        assert_eq!(refusal.code, codes::HOST_INVALID_PARAMS);
    }
    assert_eq!(
        harness.refused("agents.setConfigOption", set("model", "fast")),
        "agents.unknownConfigOption"
    );

    // A backend that does not take the setting is reported.
    harness
        .report(
            &session,
            option("fast", json!([{ "value": "fast", "name": "Fast" }])),
        )
        .expect("settings again");
    harness.link.refuse(Some(AGENTS_DID_SET_CONFIG_OPTION));
    assert_eq!(
        harness.refused("agents.setConfigOption", set("model", "fast")),
        "agents.backendUnavailable"
    );
}

#[test]
fn only_a_user_request_prompts_or_answers_permissions() {
    let harness = Harness::new("background", ALL_GRANTS);
    let session = harness.start();
    let previous = trace::replace(trace::Context {
        op: None,
        source: Source::Background,
    });
    let prompt = harness.refused("agents.prompt", json!({ "session": session, "text": "Hi" }));
    let permission = harness.refused(
        "agents.respondPermission",
        json!({ "session": session, "permission": "p1", "option": "allow" }),
    );
    let setting = harness.refused(
        "agents.setConfigOption",
        json!({ "session": session, "option": "model", "value": "fast" }),
    );
    trace::replace(previous);
    assert_eq!(prompt, "agents.userActionRequired");
    assert_eq!(permission, "agents.userActionRequired");
    assert_eq!(setting, "agents.userActionRequired");
    assert!(harness.link.notices_of(AGENTS_DID_PROMPT).is_empty());
}

#[test]
fn an_agent_runs_at_most_four_sessions() {
    let harness = Harness::new("limit", ALL_GRANTS);
    let sessions: Vec<String> = (0..4).map(|_| harness.start()).collect();
    assert_eq!(sessions, ["agent-1", "agent-2", "agent-3", "agent-4"]);
    assert_eq!(
        harness.refused("agents.start", harness.start_payload()),
        "agents.tooManySessions"
    );
    assert_eq!(harness.link.launch_count(), 4);
}

#[test]
fn a_backend_that_ends_takes_its_sessions_with_it() {
    let mut harness = Harness::new("backend-ended", ALL_GRANTS);
    let session = harness.start();
    harness.host.backend_ended("backend-2");
    assert_eq!(harness.sessions().len(), 1);
    harness.host.backend_ended(BACKEND);
    let exited = harness.next_event(&session, "exited");
    assert_eq!(exited["reason"], "backendEnded");
    // The backend is gone, so it is not told.
    assert!(harness.link.notices_of(AGENTS_DID_EXIT).is_empty());
    assert!(harness.sessions().is_empty());
}

#[test]
fn closing_ends_input_and_withdrawing_the_grant_ends_sessions() {
    let mut harness = Harness::new("close", ALL_GRANTS);
    let first = harness.start();
    let second = harness.start();

    assert_eq!(
        harness.run("agents.close", json!({ "session": first })),
        json!({})
    );
    let server = harness.link.server(0);
    assert!(server.input_was_closed());
    server.exit(Some(0));
    let exited = harness.next_event(&first, "exited");
    assert_eq!(exited["reason"], "closed");
    assert_eq!(exited["exitCode"], 0);

    harness.grant(&["agents.sessions.read", "backend.launch"]);
    let exited = harness.next_event(&second, "exited");
    assert_eq!(exited["reason"], "closed");
    assert_eq!(
        harness
            .link
            .await_notice(AGENTS_DID_EXIT, |params| params["session"] == second)["session"],
        json!(second)
    );
    assert!(harness.sessions().is_empty());
    assert_eq!(
        harness.refused("agents.close", json!({ "session": first })),
        "agents.unknownSession"
    );
}

#[test]
fn output_the_backend_cannot_take_ends_the_session() {
    let mut harness = Harness::new("output-refused", ALL_GRANTS);
    let session = harness.start();
    harness.link.refuse(Some(AGENTS_DID_OUTPUT));
    harness.link.server(0).send_raw(b"{\"a\":1}\n");
    let exited = harness.next_event(&session, "exited");
    assert_eq!(exited["reason"], "failed");
    assert_eq!(
        harness.link.await_notice(AGENTS_DID_EXIT, |_| true)["session"],
        json!(session)
    );
}

#[test]
fn a_session_whose_output_thread_cannot_start_ends_as_failed() {
    let mut harness = Harness::new("output-thread-refused", ALL_GRANTS);
    crate::live::REFUSED_THREADS.with(|refused| refused.borrow_mut().push("agent-output"));
    let session = harness.start();
    crate::live::REFUSED_THREADS.with(|refused| refused.borrow_mut().clear());
    harness.next_event(&session, "started");
    let exited = harness.next_event(&session, "exited");
    assert_eq!(exited["reason"], "failed");
    assert!(harness.sessions().is_empty());
    assert_eq!(
        harness.link.await_notice(AGENTS_DID_EXIT, |_| true)["session"],
        json!(session)
    );
}

#[test]
fn a_session_whose_error_reader_cannot_start_ends_as_failed() {
    let mut harness = Harness::new("errors-thread-refused", ALL_GRANTS);
    crate::live::REFUSED_THREADS.with(|refused| refused.borrow_mut().push("agent-errors"));
    let session = harness.start();
    crate::live::REFUSED_THREADS.with(|refused| refused.borrow_mut().clear());
    let exited = harness.next_event(&session, "exited");
    assert_eq!(exited["reason"], "failed");
    assert!(harness.sessions().is_empty());
}

#[test]
fn a_backend_that_stops_while_the_session_starts_ends_the_cli() {
    let harness = Harness::new("start-refused", ALL_GRANTS);
    harness.link.refuse(Some(AGENTS_DID_START));
    assert_eq!(
        harness.refused("agents.start", harness.start_payload()),
        "agents.backendUnavailable"
    );
    assert_eq!(harness.link.launch_count(), 1);
    assert!(harness.link.server(0).input_was_closed());
    assert!(harness.sessions().is_empty());
}

fn write_session_file(harness: &Harness, now: i64) -> PathBuf {
    let directory = harness
        .dir
        .path()
        .join("home")
        .join(".sample")
        .join("sessions")
        .join("project");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("s1.jsonl");
    let lines = [
        json!({ "type": "session", "id": "s1", "cwd": "/work/project", "at": now }),
        json!({ "type": "user", "session": "s1", "text": "hello from the user", "at": now }),
        json!({ "type": "call", "session": "s1", "key": "c1", "model": "sample-model", "input": 100, "output": 300, "first": 500, "duration": 2500, "at": now }),
        json!({ "type": "limits", "at": now, "used": 40.0 }),
    ];
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn records_list_filters_under_directories_counts_and_describes_activity() {
    let harness = Harness::new("records-list", ALL_GRANTS);
    let now = crate::now_ms();
    write_session_file(&harness, now);
    harness.run(
        "agentSessions.sync",
        json!({ "pluginID": PLUGIN, "agentID": AGENT }),
    );

    let answer = harness.run(
        "agentSessions.list",
        json!({ "cwdUnder": ["/work/"], "order": "started", "counts": true, "limit": 1 }),
    );
    assert_eq!(answer["total"], 1, "{answer}");
    assert_eq!(answer["groupsTruncated"], false, "{answer}");
    assert_eq!(
        answer["groups"],
        json!([{ "cwd": "/work/project", "count": 1, "updatedAtMs": now }])
    );
    let session = &answer["sessions"][0];
    assert_eq!(session["messageCount"], 1, "{answer}");
    assert_eq!(session["subagentCount"], 0, "{answer}");
    assert_eq!(session["model"], "sample-model", "{answer}");
    // The last message's range reads back the message itself.
    let read = harness.run(
        "agentSessions.read",
        json!({ "agent": AGENT_KEY, "path": session["path"], "range": session["lastMessageRange"] }),
    );
    assert_eq!(read["messages"][0]["text"], "hello from the user", "{read}");

    // Without `counts` the answer keeps its old shape; a directory that holds
    // none of the sessions lists none and counts none.
    let plain = harness.run("agentSessions.list", json!({}));
    assert!(plain.get("total").is_none(), "{plain}");
    let elsewhere = harness.run(
        "agentSessions.list",
        json!({ "cwdUnder": ["/work/project-two"], "counts": true }),
    );
    assert_eq!(
        (elsewhere["sessions"].clone(), elsewhere["total"].clone()),
        (json!([]), json!(0))
    );

    for payload in [
        json!({ "cwdUnder": [""] }),
        json!({ "cwdUnder": ["/work\nproject"] }),
        json!({ "cwdUnder": ["/w".repeat(2_049)] }),
        json!({ "cwdUnder": vec!["/work"; 65] }),
        json!({ "order": "title" }),
    ] {
        assert_eq!(
            harness.refused("agentSessions.list", payload),
            "agents.invalidPayload"
        );
    }
}

#[test]
fn records_sync_list_search_read_and_sum_through_the_plugin() {
    let harness = Harness::new("records", ALL_GRANTS);
    let now = crate::now_ms();
    let file = write_session_file(&harness, now);

    let report = harness.run(
        "agentSessions.sync",
        json!({ "pluginID": PLUGIN, "agentID": AGENT }),
    );
    assert_eq!(report["filesRead"], 1, "{report}");
    assert_eq!(report["filesFailed"], 0, "{report}");
    let scans = harness.link.calls_of("sessions.scan");
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0]["agentID"], AGENT);
    assert_eq!(scans[0]["sourceID"], "projects");
    assert_eq!(scans[0]["path"], path_text(&file));
    assert_eq!(scans[0]["lines"].as_array().map(Vec::len), Some(4));
    assert_eq!(scans[0]["lines"][0]["offset"], 0);
    let again = harness.run(
        "agentSessions.sync",
        json!({ "pluginID": PLUGIN, "agentID": AGENT }),
    );
    assert_eq!(again["filesUnchanged"], 1, "{again}");

    let sessions = harness.run("agentSessions.list", json!({ "titles": true }))["sessions"].clone();
    assert_eq!(sessions.as_array().map(Vec::len), Some(1), "{sessions}");
    assert_eq!(sessions[0]["sessionID"], "s1");
    assert_eq!(sessions[0]["pluginID"], PLUGIN);
    assert_eq!(sessions[0]["agentID"], AGENT);
    assert_eq!(sessions[0]["cwd"], "/work/project");
    assert_eq!(sessions[0]["title"], "hello from the user");
    let reads = harness.link.calls_of("sessions.read");
    assert_eq!(reads[0]["mode"], "titles");

    let hits = harness.run("agentSessions.search", json!({ "text": "hello" }))["hits"].clone();
    assert_eq!(hits.as_array().map(Vec::len), Some(1), "{hits}");
    assert_eq!(hits[0]["sessionID"], "s1");
    let read = harness.run(
        "agentSessions.read",
        json!({ "agent": AGENT_KEY, "path": hits[0]["path"], "range": hits[0]["range"] }),
    );
    assert_eq!(
        read,
        json!({
            "changed": false,
            "messages": [{ "role": "user", "text": "hello from the user" }],
            "end": hits[0]["range"]["end"],
        })
    );

    // The list gives how much of the file the cache has read and finds one
    // session by its ID, so a viewer can page through a whole record.
    let size = fs::metadata(&file).unwrap().len();
    assert_eq!(sessions[0]["readBytes"], size, "{sessions}");
    let found = harness.run("agentSessions.list", json!({ "sessionID": "s1" }))["sessions"].clone();
    assert_eq!(found.as_array().map(Vec::len), Some(1), "{found}");
    let none = harness.run("agentSessions.list", json!({ "sessionID": "s9" }))["sessions"].clone();
    assert_eq!(none.as_array().map(Vec::len), Some(0), "{none}");
    let whole = harness.run(
        "agentSessions.read",
        json!({ "agent": AGENT_KEY, "path": path_text(&file), "range": { "start": 0, "end": size } }),
    );
    assert_eq!(whole["end"], size, "{whole}");
    assert_eq!(
        whole["messages"].as_array().map(Vec::len),
        Some(1),
        "{whole}"
    );
    // A page that ends inside a line stops at the line before it, and the
    // plugin never sees the cut line.
    let text = fs::read_to_string(&file).unwrap();
    let second = text.find('\n').unwrap() as u64 + 1;
    let third = second + text[second as usize..].find('\n').unwrap() as u64 + 1;
    let page = harness.run(
        "agentSessions.read",
        json!({ "agent": AGENT_KEY, "path": path_text(&file), "range": { "start": 0, "end": third + 5 } }),
    );
    assert_eq!(page["end"], third, "{page}");
    assert_eq!(page["messages"].as_array().map(Vec::len), Some(1), "{page}");
    let last = harness
        .link
        .calls_of("sessions.read")
        .last()
        .cloned()
        .unwrap();
    let sent = last["items"][0]["lines"].as_array().map(Vec::len);
    assert_eq!(sent, Some(2), "{last}");
    let calls = harness.link.calls_of("sessions.read").len();
    let inside = harness.run(
        "agentSessions.read",
        json!({ "agent": AGENT_KEY, "path": path_text(&file), "range": { "start": second + 1, "end": second + 5 } }),
    );
    let expected = json!({ "changed": false, "messages": [], "end": second + 1 });
    assert_eq!(inside, expected);
    assert_eq!(harness.link.calls_of("sessions.read").len(), calls);
    assert_eq!(
        harness.refused(
            "agentSessions.read",
            json!({ "agent": AGENT_KEY, "path": "/etc/hosts", "range": { "start": 0, "end": 1 } }),
        ),
        "agentSessions.outsideSources"
    );

    let stats = harness.run("agentSessions.stats", json!({ "groupBy": "model" }));
    assert_eq!(stats["summaries"][0]["calls"], 1, "{stats}");
    assert_eq!(stats["summaries"][0]["tokens"]["input"], 100, "{stats}");
    // 300 output tokens in the 2 seconds after the first token.
    let speed = json!([{ "model": "sample-model", "calls": 1, "medianTokensPerSecond": 150.0, "lowerBound": false }]);
    assert_eq!(stats["speeds"], speed, "{stats}");
    assert_eq!(
        stats["rateLimits"][0]["windows"][0]["usedPercent"], 40.0,
        "{stats}"
    );
    assert_eq!(stats["rateLimits"][0]["plan"], "pro");

    // The request table lists the same call with what Core derives from it.
    let calls = harness.run("agentSessions.calls", json!({ "limit": 50 }));
    assert_eq!(calls["total"], 1, "{calls}");
    let row = &calls["calls"][0];
    assert_eq!(row["agent"], AGENT_KEY, "{calls}");
    assert_eq!(row["agentID"], "sample", "{calls}");
    assert_eq!(row["call"]["tokens"]["input"], 100, "{calls}");
    assert_eq!(row["speed"]["kind"], "measured", "{calls}");
    assert_eq!(row["speed"]["tokensPerSecond"], 150.0, "{calls}");
    for (filter, total) in [
        (json!({ "limit": 1, "model": "sample-model" }), 1),
        (json!({ "limit": 1, "model": "other-model" }), 0),
        (json!({ "limit": 1, "failed": true }), 0),
        (json!({ "limit": 1, "failed": false }), 1),
        (json!({ "limit": 1, "offset": 1 }), 1),
    ] {
        let page = harness.run("agentSessions.calls", filter.clone());
        assert_eq!(page["total"], total, "{filter} {page}");
    }
    assert_eq!(
        harness.run("agentSessions.calls", json!({ "limit": 1, "offset": 1 }))["calls"],
        json!([])
    );
    for page in [
        json!({ "limit": 0 }),
        json!({ "limit": 501 }),
        json!({ "limit": 1, "offset": 50_001 }),
    ] {
        assert_eq!(
            harness.refused("agentSessions.calls", page),
            "agentSessions.invalidPage"
        );
    }

    // A plugin granted agents.usage.read sees totals, never what the
    // sessions hold.
    let usage = harness
        .answer(AGENTS_USAGE, json!({ "groupBy": "agent" }))
        .expect("usage");
    assert_eq!(usage["summaries"][0]["calls"], 1, "{usage}");
    assert_eq!(usage["speeds"], speed, "{usage}");
    let text = usage.to_string();
    for private in ["\"s1\"", "/work/project", "hello", "s1.jsonl"] {
        assert!(!text.contains(private), "usage carries {private}: {text}");
    }
    let refusal = harness
        .answer(AGENTS_USAGE, json!({ "groupBy": "session" }))
        .expect_err("plugins cannot sum by session");
    assert_eq!(refusal.code, codes::HOST_INVALID_PARAMS);

    assert_eq!(
        harness.run(
            "agentSessions.forget",
            json!({ "pluginID": PLUGIN, "agentID": AGENT })
        ),
        json!({ "files": 1 })
    );
    assert_eq!(
        harness.run("agentSessions.list", json!({}))["sessions"],
        json!([])
    );
}

#[test]
fn rate_limits_survive_a_restart() {
    let dir;
    {
        let harness = Harness::new("rates", ALL_GRANTS);
        write_session_file(&harness, crate::now_ms());
        harness.run(
            "agentSessions.sync",
            json!({ "pluginID": PLUGIN, "agentID": AGENT }),
        );
        dir = harness.dir.path().join("cache");
        let restarted = Runtime::new(FakeLink::new());
        restarted
            .execute(
                "agents.configure",
                json!({ "plugins": [{ "manifest": manifest(), "grantedPermissions": ALL_GRANTS }] }),
            )
            .unwrap();
        restarted
            .execute(
                "agentSessions.configure",
                json!({
                    "cacheDirectory": path_text(&dir),
                    "userHome": path_text(&harness.dir.path().join("home")),
                }),
            )
            .unwrap();
        let stats = restarted
            .execute("agentSessions.stats", json!({ "groupBy": "day" }))
            .unwrap();
        assert_eq!(stats["rateLimits"][0]["windows"][0]["id"], "5h", "{stats}");
    }
    assert!(!dir.exists());
}

#[test]
fn records_stay_closed_without_the_read_grant() {
    let harness = Harness::new(
        "no-read",
        &["agents.native", "agents.run", "backend.launch"],
    );
    write_session_file(&harness, crate::now_ms());
    assert_eq!(
        harness.refused(
            "agentSessions.sync",
            json!({ "pluginID": PLUGIN, "agentID": AGENT })
        ),
        "agentSessions.readNotGranted"
    );
    assert_eq!(
        harness.run("agentSessions.list", json!({}))["sessions"],
        json!([])
    );
    assert_eq!(
        harness.run("agentSessions.search", json!({ "text": "hello" }))["hits"],
        json!([])
    );
    assert_eq!(
        harness.run("agentSessions.stats", json!({ "groupBy": "day" }))["summaries"],
        json!([])
    );
    assert_eq!(
        harness
            .answer(AGENTS_USAGE, json!({ "groupBy": "day" }))
            .expect("usage")["summaries"],
        json!([])
    );
    assert_eq!(
        harness.refused(
            "agentSessions.read",
            json!({ "agent": AGENT_KEY, "path": "/tmp/x", "range": { "start": 0, "end": 1 } }),
        ),
        "agentSessions.readNotGranted"
    );
    assert!(harness.link.calls_of("sessions.scan").is_empty());
}

#[test]
fn wait_events_says_when_events_were_dropped() {
    let harness = Harness::new("missed", ALL_GRANTS);
    for _ in 0..8200 {
        harness
            .runtime
            .live
            .queue
            .push("agent-0", json!({ "kind": "notice" }));
    }
    let batch = harness.run("agents.waitEvents", json!({ "after": 0, "limit": 10 }));
    assert_eq!(batch["missed"], true);
    assert_eq!(batch["events"].as_array().map(Vec::len), Some(10));
    let next = batch["next"].as_u64().unwrap();
    let batch = harness.run("agents.waitEvents", json!({ "after": next, "limit": 10 }));
    assert_eq!(batch["missed"], false);
    let caught_up = harness.run(
        "agents.waitEvents",
        json!({ "after": 8200, "timeoutMilliseconds": 0 }),
    );
    assert_eq!(caught_up["events"], json!([]));
    assert_eq!(caught_up["next"], 8200);
}

#[test]
fn chunk_lines_keep_byte_offsets_and_text_lines_drop_line_breaks() {
    assert_eq!(
        crate::records::chunk_lines_for_test(10, b"a\r\nbb\nccc"),
        [
            json!({ "offset": 10, "length": 3, "text": "a" }),
            json!({ "offset": 13, "length": 3, "text": "bb" }),
            json!({ "offset": 16, "length": 3, "text": "ccc" }),
        ]
    );
    assert_eq!(crate::records::text_lines_for_test(b"a\r\nb\n"), ["a", "b"]);
    assert!(crate::records::text_lines_for_test(b"").is_empty());
}
