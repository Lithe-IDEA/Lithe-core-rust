//! End-to-end scenario shared by the official plugin SDKs (`sdk/node`,
//! `sdk/java`), run against Rust Core through the same JSON commands a host
//! sends. The plugin backend scenario of `sdk/rust` is in `backend.rs`.
//!
//! Each SDK's sample backend is launched exactly as a host launches a build
//! provider: Core validates its manifest and expands the launch plan, the
//! runtime version is checked, and `lsp.startServer` starts it. The scenario
//! then drives the typed build requests, a multi-step plugin function with
//! progress, a plugin failure, cancellation of a running tool, runtime
//! registration, and state recovery across a restart. Both samples answer it
//! identically, which keeps the SDKs' behaviour aligned.
//!
//! `rust/crates/core/Cargo.toml` declares these files as lithe-core
//! integration tests behind the `plugin-sdk-e2e` feature, and
//! `shared/plugins/scripts/sdk-e2e.mjs` builds the samples and runs them;
//! `LITHE_PLUGIN_SDK_ROOT` names the `shared/plugins` directory that holds
//! the samples.

// Each test binary includes this module and uses only the helpers its SDK needs.
#![allow(dead_code)]

pub mod backend;
pub mod keymap;
pub mod native_build;
pub mod vcs;

use lithe_core::execute_json;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

const BUILD_SYSTEM: &str = "sample";
/// Bound for any single wait; a broken backend fails here instead of in CI.
const WAIT: Duration = Duration::from_secs(20);

fn core(command: &str, payload: Value) -> Value {
    let request = json!({ "id": format!("sdk-{command}"), "command": command, "payload": payload });
    let response: Value = serde_json::from_str(&execute_json(&request.to_string()))
        .expect("Core response should be JSON");
    assert_eq!(response["ok"], true, "{command} failed: {response}");
    response["data"].clone()
}

/// The `shared/plugins` directory named by `LITHE_PLUGIN_SDK_ROOT`.
fn repository_root() -> PathBuf {
    let root = std::env::var_os("LITHE_PLUGIN_SDK_ROOT")
        .expect("set LITHE_PLUGIN_SDK_ROOT to the shared/plugins directory");
    PathBuf::from(root)
        .canonicalize()
        .expect("shared/plugins directory")
}

/// Every file of a directory with its contents, to prove the package is unchanged.
fn listing(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("readable directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let contents = fs::read(&path).expect("readable file");
                files.insert(path.strip_prefix(root).unwrap().to_path_buf(), contents);
            }
        }
    }
    files
}

/// Temporary workspace and host state directories, removed even when an
/// assertion fails.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("lithe-plugin-sdk-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture directory");
        // The backend sees canonical paths (macOS /var is /private/var).
        Self {
            root: root.canonicalize().expect("fixture path"),
        }
    }

    fn workspace(&self) -> PathBuf {
        self.root.join("workspace")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One running backend session. Dropping it stops the process and removes
/// the session, so a failed assertion leaves no backend behind.
struct Session {
    id: String,
    plugin_id: String,
    events: Vec<Value>,
}

impl Session {
    fn start(plugin_id: &str, executable: &str, arguments: &[Value], workspace: &Path) -> Self {
        Self::start_provider(
            &format!("buildProvider:{plugin_id}:{BUILD_SYSTEM}"),
            plugin_id,
            executable,
            arguments,
            workspace,
        )
    }

    /// Starts a plugin-owned session under the host's provider ID.
    fn start_provider(
        provider_id: &str,
        plugin_id: &str,
        executable: &str,
        arguments: &[Value],
        workspace: &Path,
    ) -> Self {
        let started = core(
            "lsp.startServer",
            json!({
                "providerId": provider_id,
                "pluginId": plugin_id,
                "executablePath": executable,
                "arguments": arguments,
                // Directory URI with a trailing slash, as the macOS host sends it.
                "rootUri": format!("file://{}/", workspace.display()),
                "workingDirectory": workspace,
                "initializeTimeoutMilliseconds": WAIT.as_millis() as u64,
                "requestTimeoutMilliseconds": WAIT.as_millis() as u64,
                "shutdownTimeoutMilliseconds": 5000,
            }),
        );
        let mut session = Self {
            id: started["sessionId"]
                .as_str()
                .expect("session ID")
                .to_string(),
            plugin_id: plugin_id.to_string(),
            events: Vec::new(),
        };
        session.await_event("the session to become ready", |event| {
            event["type"] == "stateChanged" && event["state"] == "ready"
        });
        session
    }

    /// Waits for the first event, already received or new, that `matches`.
    fn await_event(&mut self, description: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(event) = self.events.iter().find(|event| matches(event)) {
                return event.clone();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {description}; events: {:#?}",
                self.events
            );
            let drained = core(
                "lsp.waitEvents",
                json!({ "sessionId": self.id, "timeoutMilliseconds": remaining.as_millis().min(1000) as u64 }),
            );
            self.events
                .extend(drained["events"].as_array().cloned().unwrap_or_default());
        }
    }

    fn completion(&mut self, operation: &str) -> Value {
        self.await_event(&format!("operation {operation} to end"), |event| {
            // Every terminal outcome, cancellation included, is one requestCompleted.
            event["operationId"] == operation && event["type"] == "requestCompleted"
        })
    }

    fn invoke(&mut self, operation: &str, function: &str, params: Value) {
        core(
            "lsp.functionRequest",
            json!({
                "sessionId": self.id,
                "operationId": operation,
                "function": { "pluginID": self.plugin_id, "functionID": function },
                "params": params,
            }),
        );
    }

    fn call(&mut self, operation: &str, function: &str, params: Value) -> Value {
        self.invoke(operation, function, params);
        self.completion(operation)
    }

    /// The registry after the latest `functionsChanged` event.
    fn functions(&mut self) -> Vec<String> {
        let latest = self
            .events
            .iter()
            .rev()
            .find(|event| event["type"] == "functionsChanged")
            .expect("a functionsChanged event");
        serde_json::from_value(latest["result"]["functions"].clone()).expect("function IDs")
    }

    fn send_build_request(&mut self, operation: &str, plan: &Value) {
        core(
            "lsp.buildRequest",
            json!({
                "sessionId": self.id,
                "operationId": operation,
                "method": plan["provider"]["method"],
                "params": plan["provider"]["params"],
            }),
        );
    }

    fn build_request(&mut self, operation: &str, plan: &Value) -> Value {
        self.send_build_request(operation, plan);
        let completed = self.completion(operation);
        assert!(completed.get("error").is_none(), "{completed}");
        completed["result"]["value"].clone()
    }

    fn stop(&mut self) {
        core("lsp.stopServer", json!({ "sessionId": self.id }));
        self.await_event("the backend to stop", |event| {
            event["type"] == "stateChanged" && event["state"] == "stopped"
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let request = json!({ "id": "sdk-cleanup", "command": "lsp.stopServer", "payload": { "sessionId": self.id } });
        let _ = execute_json(&request.to_string());
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let destroy = json!({ "id": "sdk-cleanup", "command": "lsp.destroyServer", "payload": { "sessionId": self.id } });
            let response: Value =
                serde_json::from_str(&execute_json(&destroy.to_string())).unwrap_or(Value::Null);
            if response["ok"] == true {
                return;
            }
            let wait = json!({ "id": "sdk-cleanup", "command": "lsp.waitEvents", "payload": { "sessionId": self.id, "timeoutMilliseconds": 200 } });
            let _ = execute_json(&wait.to_string());
        }
    }
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

/// One SDK's sample backend, packaged as a host installs it.
pub struct Sample {
    /// `launch.tool` of the sample's provider: `node` or `java`.
    pub tool: &'static str,
    /// The runtime executable the host would resolve for that tool.
    pub runtime: String,
    /// The installed plugin package: `plugin.json` and the launch entry.
    pub package: PathBuf,
    /// Node.js, which runs the sample workspace's build tools for both SDKs.
    pub node: String,
}

/// Node.js from `LITHE_PLUGIN_SDK_NODE`, or `None` with a skip notice.
pub fn node_from_environment(test: &str) -> Option<String> {
    let Some(node) = std::env::var_os("LITHE_PLUGIN_SDK_NODE") else {
        eprintln!("skipped: set LITHE_PLUGIN_SDK_NODE to run {test}");
        return None;
    };
    Some(node.into_string().expect("UTF-8 Node.js path"))
}

pub fn repository_path(relative: &str) -> PathBuf {
    repository_root().join(relative)
}

/// Copies `files` (package-relative destination, source) into a fresh package
/// directory under the system temporary directory, removed on drop.
pub struct TemporaryPackage {
    pub root: PathBuf,
}

impl TemporaryPackage {
    pub fn new(label: &str, files: &[(&str, PathBuf)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lithe-plugin-sdk-package-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for (destination, source) in files {
            let target = root.join(destination);
            fs::create_dir_all(target.parent().expect("package file parent"))
                .expect("package directory");
            fs::copy(source, &target)
                .unwrap_or_else(|error| panic!("copy {}: {error}", source.display()));
        }
        Self {
            root: root.canonicalize().expect("package path"),
        }
    }
}

impl Drop for TemporaryPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The enabled registry a host would hold: the `node` and `java` definitions of
/// `sdk/e2e/runtime-tools.plugin.json`, standing in for the runtime plugins.
fn runtime_registry() -> Value {
    let tools: Value = serde_json::from_slice(
        &fs::read(repository_path("sdk/e2e/runtime-tools.plugin.json")).expect("runtime tools"),
    )
    .expect("runtime tools JSON");
    json!({ "plugins": [{ "manifest": tools }] })
}

/// Starts a plan's `tool` launch the way a host does and returns the argv after
/// `runtime`: Core expands the tool definition's launch template, the host
/// probes the runtime's version, and Core checks it against the plan's minimum.
pub fn host_launch_arguments(plan: &Value, runtime: &str) -> Vec<Value> {
    let launch = &plan["launch"];
    assert_eq!(launch["kind"], "tool", "{plan}");
    let tool = launch["tool"].as_str().expect("launch tool");
    let registry = runtime_registry();
    let expanded = core(
        "toolchains.instantiateLaunch",
        json!({
            "registry": registry,
            "tool": tool,
            "executable": runtime,
            "entry": launch["entry"],
            "runtimeArguments": launch["runtimeArguments"],
            "arguments": plan["arguments"],
        }),
    );
    assert_eq!(expanded["status"], "ok", "{expanded}");
    let expanded = &expanded["launch"];
    let version = Command::new(runtime)
        .args(
            expanded["versionArguments"]
                .as_array()
                .expect("version probe")
                .iter()
                .map(|argument| argument.as_str().unwrap()),
        )
        .output()
        .expect("runtime version probe");
    // Hosts capture stderr with stdout: `java -version` prints to stderr.
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    let mut request = json!({ "registry": registry, "tool": tool, "output": output });
    if let Some(minimum) = launch["minimumVersion"].as_str() {
        request["minimumVersion"] = json!(minimum);
    }
    let check = core("toolchains.checkVersion", request);
    assert_eq!(check["version"]["satisfied"], true, "{check}");
    expanded["arguments"]
        .as_array()
        .expect("launch arguments")
        .clone()
}

/// Runs the shared scenario against `sample`.
pub fn run_sample_scenario(sample: &Sample) {
    let package = &sample.package;
    let node = &sample.node;
    let runtime = &sample.runtime;
    let manifest: Value =
        serde_json::from_slice(&fs::read(package.join("plugin.json")).expect("sample manifest"))
            .expect("sample manifest JSON");
    let plugin_id = manifest["id"].as_str().expect("plugin ID").to_string();
    let package_before = listing(&package);

    // The manifest passes the same validation as an installed plugin.
    let validated = core(
        "plugins.declarative.validate",
        json!({ "manifest": manifest }),
    );
    assert_eq!(validated["status"], "ok", "{validated}");

    let fixture = Fixture::new(sample.tool);
    let workspace = fixture.workspace();
    for project in ["app", "broken", "slow"] {
        fs::create_dir_all(workspace.join(project)).expect("project directory");
    }
    let slow_pid = workspace.join("slow/slow.pid");
    fs::write(
        workspace.join("lithe-sample.json"),
        json!({
            "projects": [
                {
                    "name": "app", "path": "app",
                    "build": [node, "-e", "console.log('built app')"],
                    "dependencies": [{ "name": "left-pad", "version": "1.3.0" }]
                },
                { "name": "broken", "path": "broken", "build": [node, "-e", "process.exit(2)"] },
                {
                    "name": "slow", "path": "slow",
                    "build": [node, "-e", "require('fs').writeFileSync('slow.pid', String(process.pid)); setInterval(() => {}, 1000)"]
                }
            ]
        })
        .to_string(),
    )
    .expect("workspace file");

    // The host expands the launch plan and checks the runtime it found.
    let plan = core(
        "plugins.declarative.languageServerLaunchPlan",
        json!({
            "manifest": manifest,
            "buildSystemID": BUILD_SYSTEM,
            "request": {
                "pluginDirectory": package,
                "stateDirectory": fixture.root.join("state"),
                "workspaceStateDirectory": fixture.root.join("workspace-state"),
                "architecture": if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" },
            }
        }),
    );
    assert_eq!(plan["status"], "ok", "{plan}");
    let plan = &plan["plan"];
    assert_eq!(plan["launch"]["tool"], sample.tool);
    let arguments = host_launch_arguments(plan, runtime);
    for directory in plan["requiredDirectories"].as_array().unwrap() {
        let path = match directory.as_str() {
            Some("state") => fixture.root.join("state"),
            _ => fixture.root.join("workspace-state"),
        };
        fs::create_dir_all(path).expect("host state directory");
    }

    let mut session = Session::start(&plugin_id, runtime, &arguments, &workspace);
    assert_eq!(session.functions(), ["snapshot.read", "workspace.build"]);

    // Typed build requests: Core plans them and decodes the answers.
    let build = |operation: &str, request: Value| {
        core(
            operation,
            json!({ "manifest": manifest, "buildSystemID": BUILD_SYSTEM, "request": request }),
        )
    };
    let model_plan = build("build.modelPlan", json!({ "root": "." }))["plan"].clone();
    assert_eq!(model_plan["provider"]["method"], "lithe/build/model");
    let model_result = session.build_request("model", &model_plan);
    let model = build(
        "build.decodeModel",
        json!({ "workspaceRoot": workspace, "root": ".", "result": model_result }),
    );
    let names: Vec<_> = model["model"]["projects"]
        .as_array()
        .expect("decoded projects")
        .iter()
        .map(|project| project["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["app", "broken", "slow"], "{model}");

    let dependency_plan = build(
        "build.dependencyPlan",
        json!({ "root": ".", "project": { "name": "app", "path": "app" } }),
    )["plan"]
        .clone();
    let dependencies_result = session.build_request("dependencies", &dependency_plan);
    let dependencies = build(
        "build.decodeDependencies",
        json!({ "workspaceRoot": workspace, "root": ".", "project": "app", "result": dependencies_result }),
    );
    assert_eq!(
        dependencies["dependencies"]["dependencies"][0]["name"], "left-pad",
        "{dependencies}"
    );
    assert_log_facts(
        &plugin_id,
        &[
            ("model", "plugins.operation.finished", "INFO"),
            ("dependencies", "plugins.operation.finished", "INFO"),
        ],
    );

    // A multi-step function: prepare, run the tool, record a snapshot.
    let built = session.call(
        "build-app",
        "workspace.build",
        json!({ "projects": ["app"] }),
    );
    assert_eq!(
        built["result"]["value"],
        json!({ "generation": 1, "status": "succeeded", "projects": [{ "name": "app", "exitCode": 0 }] }),
        "{built}"
    );
    let progress: Vec<_> = session
        .events
        .iter()
        .filter(|event| event["type"] == "requestProgress" && event["operationId"] == "build-app")
        .map(|event| event["result"]["message"].clone())
        .collect();
    assert_eq!(progress, ["Preparing", "Building app", "Done"]);
    // The backend registered snapshot.clear once a snapshot existed.
    session.await_event("snapshot.clear to be registered", |event| {
        event["type"] == "functionsChanged"
            && event["result"]["functions"]
                .as_array()
                .is_some_and(|functions| functions.contains(&json!("snapshot.clear")))
    });

    // A plugin failure keeps Lithe's stable code and forwards the plugin's own.
    let failed = session.call(
        "build-broken",
        "workspace.build",
        json!({ "projects": ["broken"] }),
    );
    assert_eq!(
        failed["error"]["code"], "plugin.function.failed",
        "{failed}"
    );
    assert_eq!(failed["error"]["pluginErrorCode"], "sample.buildFailed");

    assert_log_facts(
        &plugin_id,
        &[
            ("build-app", "plugins.operation.finished", "INFO"),
            ("build-broken", "plugins.operation.failed", "ERROR"),
        ],
    );

    // Cancelling a running build ends the tool the backend started.
    session.invoke(
        "build-slow",
        "workspace.build",
        json!({ "projects": ["slow"] }),
    );
    let deadline = Instant::now() + WAIT;
    let pid = loop {
        if let Some(pid) = fs::read_to_string(&slow_pid)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the slow tool never started");
        session.events.extend(
            core(
                "lsp.waitEvents",
                json!({ "sessionId": session.id, "timeoutMilliseconds": 50 }),
            )["events"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
    };
    core(
        "lsp.cancelOperation",
        json!({ "sessionId": session.id, "operationId": "build-slow" }),
    );
    assert_eq!(
        session.completion("build-slow")["error"]["code"],
        "requestCancelled"
    );
    #[cfg(unix)]
    {
        let deadline = Instant::now() + WAIT;
        while is_alive(pid) {
            assert!(
                Instant::now() < deadline,
                "the cancelled tool {pid} is still running"
            );
            // test-stability: allow(rust-real-sleep) reason: the cancelled tool is a separate OS process that offers no notification channel to the test; the deadline above bounds the loop.
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    let _ = pid;

    // State survives a restart: the new process reads the recorded snapshot.
    session.stop();
    drop(session);
    let mut restarted = Session::start(&plugin_id, runtime, &arguments, &workspace);
    restarted.await_event("snapshot.clear to be offered again", |event| {
        event["type"] == "functionsChanged"
            && event["result"]["functions"]
                .as_array()
                .is_some_and(|functions| functions.contains(&json!("snapshot.clear")))
    });
    let snapshot = restarted.call("read", "snapshot.read", json!({}));
    assert_eq!(snapshot["result"]["value"]["generation"], 2, "{snapshot}");
    assert_eq!(snapshot["result"]["value"]["status"], "failed");
    assert_eq!(
        restarted.call("clear", "snapshot.clear", json!({}))["result"]["value"],
        Value::Null
    );
    assert_eq!(
        restarted.call("read-again", "snapshot.read", json!({}))["result"]["value"],
        Value::Null
    );
    restarted.stop();
    drop(restarted);

    // The backend wrote only to the host's state directory, never the package.
    assert!(
        listing(&package) == package_before,
        "the plugin package changed at runtime"
    );
    let state: Vec<_> = fs::read_dir(fixture.root.join("workspace-state"))
        .expect("workspace state directory")
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        state.is_empty(),
        "cleared state left files behind: {state:?}"
    );
}

/// Facts drained from Core's process-global catalog queue but not yet claimed.
///
/// Tests in one binary run in parallel and share that queue, so a drain can
/// return facts recorded for another test's plugin. Every drain happens under
/// this lock and lands here; each test then takes only its own plugin's facts.
static DRAINED_FACTS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Drains Core's queue and returns every unclaimed fact on the plugin's channel.
fn take_plugin_facts(plugin: &str) -> Vec<Value> {
    let channel = format!("plugins/{plugin}/host");
    let mut facts = DRAINED_FACTS.lock().unwrap_or_else(PoisonError::into_inner);
    let drained = core("diagnostics.drainCoreEvents", json!({}));
    facts.extend(drained["events"].as_array().cloned().unwrap_or_default());
    let (mine, others): (Vec<Value>, Vec<Value>) = facts
        .drain(..)
        .partition(|fact| fact["channel"] == channel.as_str());
    *facts = others;
    mine
}

/// Typed SDK notifications reach the Core-owned queue, with host operation correlation.
fn assert_log_facts(plugin: &str, expected: &[(&str, &str, &str)]) {
    let records = take_plugin_facts(plugin);
    for (operation, event, level) in expected {
        let matches: Vec<_> = records
            .iter()
            .filter(|fact| fact["event"] == *event && fact["op"] == *operation)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "one terminal {event} for {operation}: {records:#?}"
        );
        assert_eq!(matches[0]["level"], *level);
        assert!(matches[0]["fields"]["duration_ms"].as_u64().is_some());
        assert!(!matches[0].to_string().contains("built app"));
        assert!(!matches[0].to_string().contains("s3cr3t"));
    }
}
