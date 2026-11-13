use super::*;
use lithe_lsp_transport::scripted::ScriptedServer;
/// The capabilities every test needs to reach `Ready` with a usable feature
/// surface. Individual tests narrow or extend this.
fn ready_capabilities() -> Value {
    json!({
        "hoverProvider": true,
        "definitionProvider": true,
        "completionProvider": {},
        "renameProvider": true
    })
}

pub(super) fn start_request(server: &ScriptedServer) -> StartServerRequest {
    let _ = server;
    StartServerRequest {
        provider_id: "gopls".to_string(),
        role: SessionRole::LanguageServer,
        log_subject: None,
        plugin_id: None,
        startup_operation: None,
        host_services: Vec::new(),
        host_options: Default::default(),
        toolchain_providers: Vec::new(),
        keymap_import_providers: Vec::new(),
        session_policy: None,
        executable_path: "/usr/bin/scripted-server".to_string(),
        arguments: vec!["--stdio".to_string()],
        environment: BTreeMap::new(),
        root_uri: "file:///workspace".to_string(),
        working_directory: "/workspace".to_string(),
        initialization_options: None,
        workspace_configuration: None,
        initialize_timeout_milliseconds: 10_000,
        request_timeout_milliseconds: 10_000,
        shutdown_timeout_milliseconds: 10_000,
    }
}

#[test]
fn native_owner_keeps_failed_initialize_identity_until_verified_disposal() {
    let server = ScriptedServer::new();
    server.break_input();
    server.block_cleanup(true);
    let engine = LspEngine::with_launcher(server.launcher());
    let registered = Mutex::new(None);
    let result = engine.start_server_owned(start_request(&server), |id| {
        // Registration precedes the very first write, including its failure.
        assert!(server.messages().is_empty());
        *registered.lock().unwrap() = Some(id.to_string());
    });
    assert!(result.is_err());
    let id = registered
        .into_inner()
        .unwrap()
        .expect("native owner received the session");
    assert!(engine.session(&id).is_ok());
    assert!(engine.destroy(&id).is_err());
    assert!(
        engine.session(&id).is_ok(),
        "failed disposal must retain the process owner"
    );
    server.block_cleanup(false);
    engine
        .destroy(&id)
        .expect("verified cleanup allows destruction");
    assert!(engine.session(&id).is_err());
}

#[test]
fn wire_owner_retries_failed_initialize_disposal_before_another_start() {
    let server = ScriptedServer::new();
    server.break_input();
    server.block_cleanup(true);
    let engine = LspEngine::with_launcher(server.launcher());
    assert!(engine.start_server(start_request(&server)).is_err());
    let id = engine
        .lock_failed_starts()
        .unwrap()
        .first()
        .cloned()
        .unwrap();
    let session = engine.session(&id).unwrap();
    let events = session.poll_events().unwrap();
    assert!(events.iter().any(|event| {
        event
            .error
            .as_ref()
            .is_some_and(|error| error.code == "transportFailed")
    }));
    let next_id = engine.next_session_id.load(Ordering::Relaxed);
    let error = engine.start_server(start_request(&server)).unwrap_err();
    assert!(matches!(error.code, ErrorCode::ProcessFailed));
    let error = engine
        .start_server_owned(start_request(&server), |_| {
            panic!("cleanup must precede spawn")
        })
        .unwrap_err();
    assert!(matches!(error.code, ErrorCode::ProcessFailed));
    assert_eq!(engine.next_session_id.load(Ordering::Relaxed), next_id);
    assert_eq!(engine.lock_sessions().unwrap().len(), 1);
    assert!(Arc::ptr_eq(&session, &engine.session(&id).unwrap()));
    server.block_cleanup(false);
    // The original transport remains broken, but retry first reclaims the
    // old owner and also disposes the replacement's failed initialization.
    assert!(engine.start_server(start_request(&server)).is_err());
    assert_eq!(engine.next_session_id.load(Ordering::Relaxed), next_id + 1);
    assert!(engine.lock_failed_starts().unwrap().is_empty());
    assert!(engine.lock_sessions().unwrap().is_empty());
}

#[test]
fn wire_owner_cleans_at_most_one_failed_start_before_rejecting_a_backlog() {
    let server = ScriptedServer::new();
    server.break_input();
    let engine = LspEngine::with_launcher(server.launcher());
    let mut ids = BTreeSet::new();
    for _ in 0..2 {
        let mut id = None;
        assert!(engine
            .start_server_owned(start_request(&server), |registered| {
                id = Some(registered.to_owned());
            })
            .is_err());
        ids.insert(id.unwrap());
    }
    // Simulate two wire starts already in flight before either failure
    // established the cleanup barrier.
    assert_eq!(ids.len(), 2);
    *engine.lock_failed_starts().unwrap() = ids;
    let next_id = engine.next_session_id.load(Ordering::Relaxed);
    let error = engine.start_server(start_request(&server)).unwrap_err();
    assert!(matches!(error.code, ErrorCode::ProcessFailed));
    assert_eq!(engine.next_session_id.load(Ordering::Relaxed), next_id);
    assert_eq!(engine.lock_sessions().unwrap().len(), 1);
    assert_eq!(engine.lock_failed_starts().unwrap().len(), 1);
    engine.cleanup_failed_start().unwrap();
    assert!(engine.lock_sessions().unwrap().is_empty());
    assert!(engine.lock_failed_starts().unwrap().is_empty());
}

#[test]
fn invalid_initial_configuration_is_rejected_before_launch() {
    let server = ScriptedServer::new();
    let engine = LspEngine::with_launcher(server.launcher());
    let mut request = start_request(&server);
    request.workspace_configuration = Some(json!(["not an object"]));
    assert!(engine.start_server(request).is_err());
    assert!(!server.was_launched());
}

/// An engine with a scripted server behind it, plus the started session.
struct Harness {
    engine: LspEngine,
    server: ScriptedServer,
    session_id: String,
    /// Events are drained by every poll, so the harness accumulates them and
    /// tests assert against the whole history.
    events: Vec<LspRuntimeEvent>,
}

struct IndexingPolicyFixture {
    root: PathBuf,
    revision: u64,
}

impl IndexingPolicyFixture {
    fn new(root: PathBuf) -> Self {
        let mut fixture = Self { root, revision: 0 };
        fixture.exclude(&[]);
        fixture
    }

    fn exclude(&mut self, paths: &[&str]) {
        self.revision = lithe_project::indexing::configure(
            serde_json::from_value(json!({
                "root": self.root, "manualExcludedDirectories": paths,
                "includedDirectories": [], "gitIgnoreEnabled": false
            }))
            .unwrap(),
            crate::indexing_changed,
        )
        .unwrap()
        .revision;
    }
}

impl Drop for IndexingPolicyFixture {
    fn drop(&mut self) {
        let _ = lithe_project::indexing::release(lithe_project::indexing::ReleaseIndexingRequest {
            root: self.root.to_string_lossy().into_owned(),
            owner_id: None,
            revision: self.revision,
        });
    }
}

/// A throw-away workspace with a nested directory tree, removed on drop.
struct TemporaryWorkspace {
    root: PathBuf,
}

impl TemporaryWorkspace {
    fn recursive(label: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "lithe-lsp-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("reactor/module-a/nested"))
            .expect("nested fixture should be creatable");
        std::fs::write(
            root.join("reactor/module-a/nested/main.go"),
            "package main\n",
        )
        .expect("fixture file should be writable");
        Self { root }
    }

    fn configure(&self, request: &mut StartServerRequest) {
        request.root_uri = url::Url::from_directory_path(&self.root)
            .expect("fixture root should convert to a URI")
            .to_string();
        request.working_directory = self.root.to_string_lossy().into_owned();
    }
}

impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Harness {
    fn start(configure: impl FnOnce(&mut StartServerRequest)) -> Self {
        let server = ScriptedServer::new();
        let engine = LspEngine::with_launcher(server.launcher());
        let mut request = start_request(&server);
        configure(&mut request);
        let started = engine
            .start_server(request)
            .expect("the server should start");
        Self {
            engine,
            server,
            session_id: started.session_id,
            events: Vec::new(),
        }
    }

    fn ready() -> Self {
        let mut harness = Self::start(|_| {});
        harness.server.complete_initialize(ready_capabilities());
        harness.await_state(LspLifecycleState::Ready);
        harness
    }

    fn session(&self) -> Arc<RuntimeSession> {
        self.engine
            .session(&self.session_id)
            .expect("the session should be registered")
    }

    fn poll(&mut self) -> &[LspRuntimeEvent] {
        let events = self
            .session()
            .poll_events()
            .expect("polling should succeed");
        self.events.extend(events);
        &self.events
    }

    /// Waits until the session reports `lifecycle`, draining events as it
    /// goes so nothing is lost to the poll that observes the transition.
    fn await_state(&mut self, lifecycle: LspLifecycleState) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.poll();
            if self.snapshot().state == lifecycle {
                // The transition can happen between the poll and snapshot;
                // drain once more so its co-published events are retained.
                self.poll();
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        self.poll();
        panic!(
            "session stayed in {:?} instead of reaching {lifecycle:?}",
            self.snapshot().state
        );
    }

    /// Waits until an event matching `matches` has been observed.
    fn await_event(&mut self, matches: impl Fn(&LspRuntimeEvent) -> bool) -> &LspRuntimeEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.poll();
            if self.events.iter().any(&matches) {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        self.events
            .iter()
            .find(|event| matches(event))
            .expect("the expected runtime event was never emitted")
    }

    fn snapshot(&self) -> EngineSnapshot {
        self.session().snapshot().expect("snapshot should succeed")
    }

    fn sync(&self, uri: &str, text: &str) {
        self.session()
            .sync_document(SyncDocumentRequest {
                session_id: self.session_id.clone(),
                uri: uri.to_string(),
                language_id: "go".to_string(),
                text: text.to_string(),
                content_changes: Vec::new(),
            })
            .expect("syncing a document should succeed");
    }

    /// Issues a feature request and returns its opaque operation ID.
    fn request(&self, operation: LspSemanticOperation, uri: &str) -> String {
        let operation_id = self.engine.next_operation_id();
        self.session()
            .request(
                SemanticRequest {
                    query: None,
                    session_id: self.session_id.clone(),
                    operation_id: Some(operation_id.clone()),
                    operation,
                    uri: Some(uri.to_string()),
                    position: Some(LspPosition {
                        line: 0,
                        utf16_column: 0,
                    }),
                    new_name: None,
                    range: None,
                    diagnostics: Vec::new(),
                    completion_context: None,
                    completion_item: None,
                    code_action: None,
                    command: None,
                    formatting_options: None,
                    include_declaration: None,
                },
                operation_id.clone(),
            )
            .expect("a ready session should accept the request");
        operation_id
    }

    /// The recorded JSON-RPC message with the given request ID.
    fn written_request(&self, request_id: &str) -> Value {
        self.server
            .messages()
            .into_iter()
            .find(|message| message["id"] == request_id)
            .expect("the request should have been written")
    }

    /// The notification the server received for `method`, if any.
    fn notification(&self, method: &str) -> Option<Value> {
        self.server
            .messages()
            .into_iter()
            .find(|message| message.get("method").and_then(Value::as_str) == Some(method))
    }
}

/// Criterion 1: a spawned process that never initializes cannot become ready.
#[test]
fn a_server_that_never_answers_initialize_fails_instead_of_becoming_ready() {
    let mut harness = Harness::start(|request| {
        request.initialize_timeout_milliseconds = 30;
    });
    harness.await_state(LspLifecycleState::Failed);

    let states: Vec<_> = harness
        .events
        .iter()
        .filter_map(|event| event.state)
        .collect();
    assert!(
        !states.contains(&LspLifecycleState::Ready),
        "an uninitialized session must never pass through Ready: {states:?}"
    );
    let failure = harness
        .events
        .iter()
        .find_map(|event| event.error.as_ref())
        .expect("the timeout should be reported as a runtime error");
    assert_eq!(failure.code, "initializeTimeout");
    assert_eq!(failure.stage, "initialize");
}

/// Criterion 2: an initialize error cannot become ready.
#[test]
fn an_initialize_error_response_fails_the_session() {
    let mut harness = Harness::start(|_| {});
    let id = harness
        .server
        .await_request("initialize")
        .expect("initialize should be sent");
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32603, "message": "workspace is unsupported" }
    }));
    harness.await_state(LspLifecycleState::Failed);

    assert!(!harness.snapshot().initialized);
    let failure = harness
        .events
        .iter()
        .find_map(|event| event.error.as_ref())
        .expect("the rejection should be reported as a runtime error");
    assert_eq!(failure.code, "initializeFailed");
    assert!(
        failure
            .underlying_message
            .as_deref()
            .is_some_and(|detail| detail.contains("workspace is unsupported")),
        "the server's own message must survive into the error: {:?}",
        failure.underlying_message
    );
}

/// Messages the scripted server received for `method`, in order.
fn notifications(harness: &Harness, method: &str) -> Vec<Value> {
    harness
        .server
        .messages()
        .into_iter()
        .filter(|message| message.get("method").and_then(Value::as_str) == Some(method))
        .collect()
}

#[test]
fn structured_logging_warnings_keep_source_stage_and_code() {
    let harness = Harness::ready();
    let session = harness.session();
    session.log_issue(
        "warn",
        "processCleanup",
        "processCleanupFailed",
        EngineLog::ProcessCleanupFailed,
        "Cleanup failed",
        None,
    );
    let event = session
        .poll_events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "log")
        .unwrap();
    assert_eq!(event.level.as_deref(), Some("warn"));
    let error = event.error.unwrap();
    assert_eq!(error.code, "processCleanupFailed");
    assert_eq!(error.stage, "processCleanup");
    assert!(error.underlying_message.is_none());
    for (stage, code) in [
        ("stderr", "upstreamStderr"),
        ("stderr", "stderrReadFailed"),
        ("shutdown", "shutdownTimeout"),
    ] {
        session.log_issue(
            "warning",
            stage,
            code,
            if code == "upstreamStderr" {
                EngineLog::StderrReceived
            } else if code == "stderrReadFailed" {
                EngineLog::StderrReadFailed
            } else {
                EngineLog::ShutdownTimedOut
            },
            "Updated diagnostic wording",
            None,
        );
        let event = session
            .poll_events()
            .unwrap()
            .into_iter()
            .find(|event| event.kind == "log")
            .unwrap();
        assert_eq!(event.level.as_deref(), Some("warning"));
        let error = event.error.unwrap();
        assert_eq!(error.stage, stage);
        assert_eq!(error.code, code);
    }
    // Ordinary logs must remain unclassified even when their wording matches an issue.
    session.log(
        "warning",
        EngineLog::StderrReceived,
        "Language-server stderr",
        None,
    );
    let event = session
        .poll_events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "log")
        .unwrap();
    assert!(event.error.is_none());
}

#[test]
fn lifecycle_catalog_identity_preserves_v1_event_without_extra_log() {
    for role in [
        SessionRole::LanguageServer,
        SessionRole::Vcs,
        SessionRole::Build,
        SessionRole::Toolchain,
        SessionRole::Backend,
    ] {
        let harness = Harness::start(|request| {
            request.role = role;
            request.provider_id = "c++".into();
            request.log_subject = Some("git".into());
            request.plugin_id = Some("io.example.tools".into());
        });
        let session = harness.session();
        session.active.store(false, Ordering::Release);
        session.kill_process();
        let mut state = session.state.lock().unwrap();
        state.events.clear();
        let sequence = state.next_sequence;
        transition_locked(&session, &mut state, LspLifecycleState::Stopped, None);
        assert!(!state.events.iter().any(|e| e.kind == "log"));
        let event = state.events.front().unwrap();
        let (id, fields) = EngineLog::SessionUpdated.fact(
            &session.log_identity,
            &json!({"state": "stopped"}),
            None,
        );
        assert_eq!(event.event.as_deref(), Some(id.as_str()));
        assert_eq!(event.fields.as_ref(), Some(&fields));
        assert_eq!(
            event.channel.as_deref(),
            Some(session.log_identity.channel().as_str())
        );
        assert_eq!(event.role, Some(role));
        assert_eq!(event.plugin_id.as_deref(), Some("io.example.tools"));
        assert_eq!(
            event.catalog_level,
            Some(lithe_diagnostics::record::LogLevel::Debug)
        );
        let mut legacy = serde_json::to_value(event).unwrap();
        for key in [
            "channel",
            "pluginId",
            "role",
            "event",
            "fields",
            "catalogLevel",
        ] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        assert_eq!(
            legacy,
            json!({"type": "stateChanged", "sequence": sequence,
            "providerId": "c++", "sessionId": session.id, "state": "stopped"})
        );
        state.events.clear();
        let error = runtime_error(
            &session,
            "processCleanupFailed",
            "cleanup",
            None,
            None,
            "Engine process cleanup failed",
            None,
            None,
        );
        let (id, fields) =
            EngineLog::ProcessCleanupFailed.fact(&session.log_identity, &Value::Null, Some(&error));
        push_cleanup_fact(&session, &mut state, id, fields, error);
        let event = state.events.front().unwrap();
        assert_eq!(
            event.channel.as_deref(),
            Some(session.log_identity.channel().as_str())
        );
        assert_eq!(
            event.catalog_level,
            Some(lithe_diagnostics::record::LogLevel::Warn)
        );
        assert_eq!(event.kind, "log");
        assert_eq!(event.level.as_deref(), Some("warning"));
        assert_eq!(
            event.message.as_deref(),
            Some("Engine process cleanup failed")
        );
        assert!(event.detail.is_none() && event.state.is_none());
        assert_eq!(
            event.error.as_ref().map(|error| error.code.as_str()),
            Some("processCleanupFailed")
        );
    }
}

#[test]
fn v1_detail_is_independent_of_catalog_fields() {
    let harness = Harness::start(|_| {});
    let session = harness.session();
    struct Cleanup(Arc<RuntimeSession>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.active.store(false, Ordering::Release);
            self.0.kill_process();
        }
    }
    let _cleanup = Cleanup(session.clone());
    session.poll_events().unwrap();
    let line = "  upstream diagnostic  ";
    session.log_issue(
        "warning",
        "stderr",
        "upstreamStderr",
        EngineLog::StderrReceived.with_fields(json!({"diagnostic": line, "truncated": false})),
        "Language-server stderr",
        Some(line.into()),
    );
    let event = session
        .poll_events()
        .unwrap()
        .into_iter()
        .find(|e| e.event.as_deref() == Some("lsp.stderr.received"))
        .unwrap();
    assert_eq!(event.level.as_deref(), Some("warning"));
    assert_eq!(event.message.as_deref(), Some("Language-server stderr"));
    assert_eq!(event.detail.as_deref(), Some(line));
    assert_eq!(event.fields.as_ref().unwrap()["diagnostic"], line);
    assert_eq!(event.fields.as_ref().unwrap()["truncated"], false);
    // A JSON-looking v1 detail cannot supply or overwrite a catalog fact.
    session.log(
        "info",
        EngineLog::StderrReceived.with_fields(json!({"diagnostic": "kept", "truncated": true})),
        "Language-server stderr",
        Some("{\"diagnostic\":\"overwritten\",\"truncated\":false}".into()),
    );
    let event = session.poll_events().unwrap().into_iter().next().unwrap();
    let fields = event.fields.unwrap();
    assert_eq!(fields["diagnostic"], "kept");
    assert_eq!(fields["truncated"], true);
}

#[test]
fn engine_logging_wire_fixture_preserves_defaults() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/diagnostics/engine-logging-v2.json"
    ))
    .unwrap();
    let request: StartServerRequest = serde_json::from_value(fixture["start"].clone()).unwrap();
    assert_eq!(request.role, SessionRole::Vcs);
    assert_eq!(
        SessionLogIdentity::new(
            request.role,
            &request.provider_id,
            request.plugin_id.as_deref(),
            request.log_subject.as_deref()
        )
        .channel(),
        "vcs/git"
    );
    let mut legacy = fixture["start"].clone();
    legacy.as_object_mut().unwrap().remove("role");
    legacy.as_object_mut().unwrap().remove("logSubject");
    assert_eq!(
        serde_json::from_value::<StartServerRequest>(legacy)
            .unwrap()
            .role,
        SessionRole::LanguageServer
    );
    let request: SetDetailedDiagnosticsRequest =
        serde_json::from_value(fixture["trace"]["payload"].clone()).unwrap();
    assert!(request.enabled);
}

#[test]
fn detailed_diagnostics_waits_for_initialized_and_keeps_the_latest_request() {
    for enabled in [true, false] {
        let harness = Harness::start(|_| {});
        let session = harness.session();
        struct Cleanup(Arc<RuntimeSession>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.active.store(false, Ordering::Release);
                self.0.kill_process();
            }
        }
        let _cleanup = Cleanup(session.clone());
        session.set_detailed_diagnostics(true).unwrap();
        if !enabled {
            session.set_detailed_diagnostics(false).unwrap();
        }
        assert!(!harness
            .server
            .messages()
            .iter()
            .any(|m| m["method"] == "$/setTrace"));
        assert!(!session.lock_state().unwrap().detailed_diagnostics);

        // Drive the real handshake handler synchronously; no reader timing
        // or sleep is needed to establish the notification ordering.
        let initialize = harness.server.await_request("initialize").unwrap();
        session
            .handle_server_message(
                json!({
                    "jsonrpc":"2.0", "id":initialize,
                    "result":{"capabilities":ready_capabilities()}
                })
                .to_string(),
            )
            .unwrap();
        let methods: Vec<_> = harness
            .server
            .messages()
            .into_iter()
            .filter_map(|m| m["method"].as_str().map(str::to_owned))
            .collect();
        if enabled {
            let initialized = methods.iter().position(|m| m == "initialized").unwrap();
            let trace = methods.iter().position(|m| m == "$/setTrace").unwrap();
            assert!(trace > initialized);
        } else {
            assert!(!methods.iter().any(|m| m == "$/setTrace"));
        }
        assert_eq!(session.lock_state().unwrap().detailed_diagnostics, enabled);
        session.set_detailed_diagnostics(false).unwrap();
        assert_eq!(
            harness.server.messages().last().unwrap()["params"],
            json!({"value":"off"})
        );
    }
}

#[test]
fn upstream_catalog_messages_trace_toggle_and_v1_fields_coexist() {
    let harness = Harness::start(|request| {
        request.role = SessionRole::Vcs;
        request.log_subject = Some("git".into());
        request.plugin_id = Some("io.example.git".into());
        request.provider_id = "opaque:provider:git".into();
    });
    let session = harness.session();
    struct Cleanup(Arc<RuntimeSession>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.active.store(false, Ordering::Release);
            self.0.kill_process();
        }
    }
    let _cleanup = Cleanup(session.clone());
    let initialize = harness.server.await_request("initialize").unwrap();
    session
        .handle_server_message(
            json!({
                "jsonrpc":"2.0", "id":initialize, "result":{"capabilities":{}}
            })
            .to_string(),
        )
        .unwrap();
    session.poll_events().unwrap();
    session.log(
        "warning",
        EngineLog::Dropped.with_fields(json!({"count": 7})),
        "Engine stderr lines dropped",
        Some("count=7".into()),
    );
    let dropped = session
        .poll_events()
        .unwrap()
        .into_iter()
        .find(|event| event.event.as_deref() == Some("log.dropped"))
        .unwrap();
    assert_eq!(dropped.channel.as_deref(), Some("vcs/git"));
    assert_eq!(dropped.fields.unwrap()["count"], 7);
    for (method, expected) in [
        ("window/logMessage", "vcs.message.warning.logged"),
        ("window/showMessage", "vcs.message.warning.shown"),
    ] {
        session.handle_server_message(json!({"jsonrpc":"2.0", "method":method, "params":{"type":2,"message":"warning token=private-value"}}).to_string()).unwrap();
        let events = session.poll_events().unwrap();
        let event = events
            .iter()
            .find(|e| e.event.as_deref() == Some(expected))
            .expect("upstream catalog event");
        assert_eq!(event.channel.as_deref(), Some("vcs/git"));
        assert_eq!(event.plugin_id.as_deref(), Some("io.example.git"));
        assert_eq!(event.role, Some(SessionRole::Vcs));
        assert_eq!(
            event.catalog_level,
            Some(lithe_diagnostics::record::LogLevel::Warn)
        );
        assert_eq!(event.message.as_deref(), Some(method));
        assert!(event.level.is_some() && event.detail.is_some());
        let fields = event.fields.as_ref().unwrap();
        assert_eq!(fields["severity"], "warning");
        assert!(!fields["diagnostic"]
            .as_str()
            .unwrap()
            .contains("private-value"));
    }
    let trace = json!({"jsonrpc":"2.0", "method":"$/logTrace", "params":{
        "message":"trace token=fixture-secret", "verbose":json!({
            "textDocument":{"text":"private_source_marker"}
        }).to_string()
    }})
    .to_string();
    session.handle_server_message(trace.clone()).unwrap();
    assert!(!session
        .poll_events()
        .unwrap()
        .iter()
        .any(|e| e.event.as_deref() == Some("vcs.trace.received")));
    session.set_detailed_diagnostics(true).unwrap();
    assert_eq!(
        harness.server.messages().last().unwrap()["params"],
        json!({"value":"verbose"})
    );
    session.handle_server_message(trace.clone()).unwrap();
    let event = session
        .poll_events()
        .unwrap()
        .into_iter()
        .find(|e| e.event.as_deref() == Some("vcs.trace.received"))
        .unwrap();
    assert_eq!(
        event.catalog_level,
        Some(lithe_diagnostics::record::LogLevel::Debug)
    );
    assert_eq!(event.level.as_deref(), Some("info"));
    let fields = event.fields.unwrap();
    assert_eq!(fields["diagnostic"], "trace token=<redacted>");
    let encoded = lithe_diagnostics::record::encode_log_batch(
        serde_json::from_value(json!({"validateCatalog":true,"events":[{
            "ts":"2026-11-03T00:00:00.000Z", "seq":1, "level":"DEBUG",
            "event":"vcs.trace.received", "msg":"", "fields":fields
        }]}))
        .unwrap(),
    );
    let line = encoded.results[0].line.as_ref().unwrap();
    assert!(!line.contains("private_source_marker"));
    assert!(!line.contains("fixture-secret"));
    session.set_detailed_diagnostics(false).unwrap();
    assert_eq!(
        harness.server.messages().last().unwrap()["params"],
        json!({"value":"off"})
    );
    session.handle_server_message(trace).unwrap();
    assert!(!session
        .poll_events()
        .unwrap()
        .iter()
        .any(|e| e.event.as_deref() == Some("vcs.trace.received")));
    assert!(
        serde_json::from_value::<SetDetailedDiagnosticsRequest>(json!({"sessionId":"s"})).is_err()
    );
}

#[test]
fn ranged_edits_extend_text_retained_before_the_server_is_ready() {
    let mut harness = Harness::start(|request| {
        request.provider_id = "gopls".to_string();
    });
    let uri = "file:///workspace/First.java";
    harness.sync(uri, "class First {}");
    let appended = LspDocumentContentChange {
        range: Some(LspRange {
            start: LspPosition {
                line: 0,
                utf16_column: 11,
            },
            end: LspPosition {
                line: 0,
                utf16_column: 11,
            },
        }),
        text: "Renamed".to_string(),
    };
    harness
        .session()
        .sync_document(SyncDocumentRequest {
            session_id: harness.session_id.clone(),
            uri: uri.to_string(),
            language_id: "java".to_string(),
            text: String::new(),
            content_changes: vec![appended.clone()],
        })
        .expect("ranged edits should extend the retained text");

    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);
    assert!(harness.server.await_notification("textDocument/didOpen"));
    let written = String::from_utf8_lossy(&harness.server.written_bytes()).into_owned();
    assert!(written.contains("class FirstRenamed {}"));

    let error = harness
        .session()
        .sync_document(SyncDocumentRequest {
            session_id: harness.session_id.clone(),
            uri: "file:///workspace/Unknown.java".to_string(),
            language_id: "java".to_string(),
            text: String::new(),
            content_changes: vec![appended],
        })
        .expect_err("edits to an unknown document cannot be resolved");
    assert_eq!(error.details.as_deref(), Some("documentNotSynchronized"));
}

/// A path descriptor reaches the server with reserved characters escaped.
#[test]
fn native_path_emits_the_canonical_uri_on_the_lsp_wire() {
    let harness = Harness::ready();
    let mut payload = json!({"sessionId":harness.session_id, "languageId":"go", "text":"package main",
        "uri":{"path":"/workspace/A#B.go","platform":"posix"}});
    lithe_document_uri::resolve_request_paths("lsp.syncDocument", &mut payload).unwrap();
    let request: SyncDocumentRequest = serde_json::from_value(payload).unwrap();
    harness
        .session()
        .sync_document(request)
        .expect("document should synchronize");
    assert!(harness.server.await_notification("textDocument/didOpen"));
    let opened = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "textDocument/didOpen")
        .unwrap();
    assert_eq!(
        opened["params"]["textDocument"]["uri"],
        "file:///workspace/A%23B.go"
    );
}

/// Criterion 3: two syncs emit open version 1 then change version 2.
#[test]
fn consecutive_syncs_open_at_version_one_and_change_to_version_two() {
    let harness = Harness::ready();
    let uri = "file:///workspace/main.go";
    harness.sync(uri, "package main");
    harness.sync(uri, "package main\nfunc main() {}");

    let versions: Vec<_> = harness
        .server
        .messages()
        .into_iter()
        .filter_map(|message| {
            let method = message.get("method")?.as_str()?.to_string();
            let document = message.get("params")?.get("textDocument")?;
            let version = document.get("version")?.as_i64()?;
            Some((method, version))
        })
        .collect();
    assert_eq!(
        versions,
        vec![
            ("textDocument/didOpen".to_string(), 1),
            ("textDocument/didChange".to_string(), 2)
        ]
    );
    assert_eq!(harness.snapshot().open_documents[uri].version, 2);
}

#[test]
fn all_documents_synced_during_initialize_are_opened_when_ready() {
    let mut harness = Harness::start(|_| {});
    let first_uri = "file:///workspace/first.go";
    let second_uri = "file:///workspace/second.go";
    harness.sync(first_uri, "package main\nvar first = 1");
    harness.sync(second_uri, "package main\nvar second = 2");

    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);

    let opened_uris: Vec<_> = harness
        .server
        .messages()
        .into_iter()
        .filter(|message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/didOpen")
        })
        .filter_map(|message| {
            message
                .get("params")?
                .get("textDocument")?
                .get("uri")?
                .as_str()
                .map(ToString::to_string)
        })
        .collect();
    assert_eq!(
        opened_uris,
        vec![first_uri.to_string(), second_uri.to_string()]
    );
    let snapshot = harness.snapshot();
    assert_eq!(snapshot.open_documents[first_uri].version, 1);
    assert_eq!(snapshot.open_documents[second_uri].version, 1);
}

#[test]
fn workspace_file_changes_queue_until_initialize_and_collapse_watcher_bursts() {
    let mut harness = Harness::start(|_| {});
    harness
        .session()
        .workspace_files_changed(vec![
            WorkspaceFileChange {
                uri: "file:///workspace/pom.xml".to_string(),
                kind: WorkspaceFileChangeKind::Changed,
            },
            WorkspaceFileChange {
                uri: "file:///workspace/src/Main.java".to_string(),
                kind: WorkspaceFileChangeKind::Created,
            },
            WorkspaceFileChange {
                uri: "file:///workspace/src/Main.java".to_string(),
                kind: WorkspaceFileChangeKind::Changed,
            },
        ])
        .expect("watcher events should queue before initialize");
    assert!(!harness
        .server
        .messages()
        .iter()
        .any(|message| { message["method"] == "workspace/didChangeWatchedFiles" }));

    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);
    let notification = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "workspace/didChangeWatchedFiles")
        .expect("queued watcher events should flush after initialize");
    assert_eq!(
        notification["params"]["changes"],
        json!([
            { "uri": "file:///workspace/pom.xml", "type": 2 },
            { "uri": "file:///workspace/src/Main.java", "type": 2 }
        ])
    );
}

/// Criterion 4: a crash fails pending operations with `serverExited`.
#[test]
fn a_crash_fails_every_pending_operation_once_with_server_exited() {
    let mut harness = Harness::ready();
    let uri = "file:///workspace/main.go";
    harness.sync(uri, "package main");
    let hover = harness.request(LspSemanticOperation::Hover, uri);
    let definition = harness.request(LspSemanticOperation::Definition, uri);

    harness.server.exit(Some(134));
    harness.await_state(LspLifecycleState::Failed);

    let failures: Vec<_> = harness
        .events
        .iter()
        .filter(|event| event.kind == "requestCompleted")
        .collect();
    assert_eq!(
        failures.len(),
        2,
        "each pending operation must fail exactly once"
    );
    for event in failures {
        let error = event.error.as_ref().expect("a crash cannot yield a result");
        assert_eq!(error.code, "serverExited");
        assert_eq!(error.process_exit_code, Some(134));
    }
    let completed: Vec<_> = harness
        .events
        .iter()
        .filter_map(|event| event.operation_id.clone())
        .collect();
    assert!(completed.contains(&hover) && completed.contains(&definition));
    assert!(harness.snapshot().pending_operation_ids.is_empty());
}

/// Criterion 5: a request deadline removes the pending request.
/// Criterion 6: a late response after that timeout is ignored.
#[test]
fn a_timed_out_request_is_removed_cancelled_and_deaf_to_its_late_response() {
    let mut harness = Harness::start(|request| {
        request.request_timeout_milliseconds = 30;
    });
    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);
    let uri = "file:///workspace/main.go";
    harness.sync(uri, "package main");
    let operation = harness.request(LspSemanticOperation::Hover, uri);
    let request_id = harness
        .server
        .await_request("textDocument/hover")
        .expect("the hover request should reach the server");

    let timeout = harness
        .await_event(|event| {
            event
                .error
                .as_ref()
                .is_some_and(|error| error.code == "requestTimeout")
        })
        .clone();
    assert_eq!(timeout.operation_id.as_deref(), Some(operation.as_str()));
    assert!(harness.snapshot().pending_operation_ids.is_empty());
    // The completion event is queued before the cancellation is written, so
    // the wire assertion has to wait for the write rather than assume it.
    assert!(
        harness.server.await_notification("$/cancelRequest"),
        "a timed-out request must be cancelled on the wire"
    );

    // The server answers anyway. Nothing may reach the application: the
    // operation has already been completed with its timeout error.
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": { "contents": "too late" }
    }));
    thread::sleep(Duration::from_millis(50));
    harness.poll();
    let completions = harness
        .events
        .iter()
        .filter(|event| event.operation_id.as_deref() == Some(operation.as_str()))
        .count();
    assert_eq!(
        completions, 1,
        "a late response must not complete the operation a second time"
    );
}

/// Criterion 7: responses from an old session cannot affect a restarted one.
#[test]
fn a_restarted_session_ignores_the_previous_session_s_responses() {
    // The first session is taken to Ready and then torn down. Its hover
    // request id is recorded first, because every session numbers its
    // requests from one: an id alone cannot tell two sessions apart, so only
    // per-session pending state can reject a foreign response.
    let mut first = Harness::ready();
    let uri = "file:///workspace/main.go";
    first.sync(uri, "package main");
    first.request(LspSemanticOperation::Hover, uri);
    let stale_id = first
        .server
        .await_request("textDocument/hover")
        .expect("the hover request should reach the server");
    first.server.exit(Some(0));
    first.await_state(LspLifecycleState::Failed);
    first.engine.destroy(&first.session_id).unwrap();

    let mut restarted = Harness::ready();
    restarted.sync(uri, "package main");
    let operation = restarted.request(LspSemanticOperation::Hover, uri);
    assert_eq!(
        restarted
            .server
            .await_request("textDocument/hover")
            .as_deref(),
        Some(stale_id.as_str()),
        "the restarted session must reuse the id, or this proves nothing"
    );

    // Delivered to the new session, the id does match a pending request, so
    // isolation cannot rest on ids. It rests on the process: the old
    // session's reader thread is gone with its process, so its responses
    // have no path into the new session at all.
    let before = restarted.snapshot();
    first.server.send(json!({
        "jsonrpc": "2.0",
        "id": stale_id,
        "result": { "contents": "from the dead session" }
    }));
    thread::sleep(Duration::from_millis(50));
    restarted.poll();
    assert_eq!(
        restarted.snapshot().pending_operation_ids,
        before.pending_operation_ids,
        "the old session's response must not complete the new session's request"
    );
    assert!(
        !restarted
            .events
            .iter()
            .any(|event| event.kind == "requestCompleted"),
        "no operation may complete from a foreign session's traffic"
    );

    // The new session's own response still lands, so the isolation above is
    // not merely a dead session.
    restarted.server.send(json!({
        "jsonrpc": "2.0",
        "id": stale_id,
        "result": { "contents": "from the live session" }
    }));
    let completion = restarted
        .await_event(|event| event.kind == "requestCompleted")
        .clone();
    assert_eq!(completion.operation_id.as_deref(), Some(operation.as_str()));
    assert_eq!(
        completion
            .result
            .as_ref()
            .and_then(|result| result.get("hover")?.get("contents"))
            .and_then(Value::as_str),
        Some("from the live session")
    );
}

/// A write that fails mid-session is a transport failure, not a silent drop.
#[test]
fn a_broken_stdin_fails_the_session_and_stops_writing() {
    let mut harness = Harness::ready();
    let written = harness.server.written_bytes().len();
    harness.server.break_input();
    let error = harness
        .session()
        .sync_document(SyncDocumentRequest {
            session_id: harness.session_id.clone(),
            uri: "file:///workspace/main.go".to_string(),
            language_id: "go".to_string(),
            text: "package main".to_string(),
            content_changes: Vec::new(),
        })
        .expect_err("a broken pipe must surface to the caller");
    assert!(matches!(error.code, ErrorCode::ProcessFailed));

    harness.await_state(LspLifecycleState::Failed);
    let failure = harness
        .events
        .iter()
        .find_map(|event| event.error.as_ref())
        .expect("the failure should be reported as a runtime error");
    assert_eq!(failure.code, "transportFailed");
    assert_eq!(
        harness.server.written_bytes().len(),
        written,
        "nothing may be written after the pipe breaks"
    );
}

/// The stopped session's own late response is dropped rather than acted on,
/// which is the same rule seen from the other side of a restart.
#[test]
fn a_response_to_an_already_failed_session_is_logged_and_dropped() {
    let mut harness = Harness::ready();
    let uri = "file:///workspace/main.go";
    harness.sync(uri, "package main");
    harness.request(LspSemanticOperation::Hover, uri);
    let request_id = harness
        .server
        .await_request("textDocument/hover")
        .expect("the hover request should reach the server");

    harness.server.exit(Some(1));
    harness.await_state(LspLifecycleState::Failed);
    let completions = harness
        .events
        .iter()
        .filter(|event| event.kind == "requestCompleted")
        .count();
    assert_eq!(completions, 1, "the crash already completed the operation");

    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": { "contents": "unreachable" }
    }));
    thread::sleep(Duration::from_millis(50));
    harness.poll();
    assert_eq!(
        harness
            .events
            .iter()
            .filter(|event| event.kind == "requestCompleted")
            .count(),
        completions,
        "a response after the terminal state must not complete anything"
    );
    assert_eq!(harness.snapshot().state, LspLifecycleState::Failed);
}

#[test]
fn indexing_policy_withdrawal_serializes_with_a_checked_document_commit() {
    assert_policy_withdrawal_serializes_with_checked_commit(false);
}

#[test]
fn indexing_policy_withdrawal_serializes_with_checked_diagnostics() {
    assert_policy_withdrawal_serializes_with_checked_commit(true);
}

fn assert_policy_withdrawal_serializes_with_checked_commit(diagnostics: bool) {
    let workspace = TemporaryWorkspace::recursive("indexing-document-commit");
    let root = workspace.root.canonicalize().unwrap();
    let mut policy = IndexingPolicyFixture::new(root.clone());
    let mut harness = Harness::start(|request| workspace.configure(request));
    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);
    let uri = url::Url::from_file_path(root.join("reactor/hidden.go"))
        .unwrap()
        .to_string();
    let session = harness.session();
    let (checked_tx, checked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (sync_done_tx, sync_done_rx) = std::sync::mpsc::channel();
    let syncing = {
        let session = session.clone();
        let uri = uri.clone();
        thread::spawn(move || {
            let checked = || {
                checked_tx.send(()).unwrap();
                release_rx
                    .recv_timeout(Duration::from_secs(3))
                    .expect("document commit was not released");
            };
            let result = if diagnostics {
                session.handle_server_message_after_policy_check(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{
                    "uri":uri,"diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"message":"old diagnostic"}]
                }}).to_string(), checked)
            } else {
                session
                    .sync_document_after_policy_check(
                        SyncDocumentRequest {
                            session_id: session.id.clone(),
                            uri,
                            language_id: "java".into(),
                            text: "class Hidden {}".into(),
                            content_changes: Vec::new(),
                        },
                        checked,
                    )
                    .map(|_| ())
            };
            let _ = sync_done_tx.send(());
            result
        })
    };
    checked_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("document eligibility was not reached");
    assert!(
        matches!(
            session.indexing_roots.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ),
        "eligibility and document publication must retain the policy serialization guard"
    );
    policy.exclude(&["reactor"]);
    let (update_done_tx, update_done_rx) = std::sync::mpsc::channel();
    let updating = {
        let session = session.clone();
        let root = root.clone();
        thread::spawn(move || {
            let result = apply_indexing_to_sessions(&root, vec![session]);
            let _ = update_done_tx.send(());
            result
        })
    };
    release_tx.send(()).unwrap();
    sync_done_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("document commit did not finish");
    update_done_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("policy update did not finish");
    syncing.join().unwrap().unwrap();
    updating.join().unwrap().unwrap();
    assert!(!harness.snapshot().open_documents.contains_key(&uri));
    assert!(!session
        .lock_state()
        .unwrap()
        .client
        .diagnostics
        .contains_key(&uri));
    if !diagnostics {
        assert!(notifications(&harness, "textDocument/didClose")
            .iter()
            .any(|message| message["params"]["textDocument"]["uri"] == uri));
    }
}

#[test]
fn indexing_policy_withdraws_queued_file_changes_before_service_readiness() {
    let workspace = TemporaryWorkspace::recursive("indexing-pending-file-changes");
    let root = workspace.root.canonicalize().unwrap();
    let mut policy = IndexingPolicyFixture::new(root.clone());
    let mut harness = Harness::start(|request| workspace.configure(request));
    let hidden = url::Url::from_file_path(root.join("reactor/hidden.go"))
        .unwrap()
        .to_string();
    let visible = url::Url::from_file_path(root.join("visible.go"))
        .unwrap()
        .to_string();
    harness
        .session()
        .workspace_files_changed(vec![
            WorkspaceFileChange {
                uri: hidden.clone(),
                kind: WorkspaceFileChangeKind::Created,
            },
            WorkspaceFileChange {
                uri: visible.clone(),
                kind: WorkspaceFileChangeKind::Created,
            },
        ])
        .unwrap();
    assert_eq!(
        harness
            .session()
            .lock_state()
            .unwrap()
            .pending_workspace_file_changes
            .len(),
        2
    );
    policy.exclude(&["reactor"]);
    // The policy is installed, but this independent session has not received
    // its withdrawal callback yet. Readiness must recheck queued eligibility.
    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);
    let changes = notifications(&harness, "workspace/didChangeWatchedFiles");
    assert!(changes.iter().any(|message| message["params"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["uri"] == visible)));
    assert!(!changes.iter().any(|message| message["params"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["uri"] == hidden)));
}

#[test]
fn indexing_startup_cancels_when_policy_changes_during_process_launch() {
    const CANCELLED_PLUGIN: &str = "io.example.indexing-cancel-launch";
    struct RetireOnLaunch {
        inner: Arc<dyn LspProcessLauncher>,
        root: PathBuf,
        revision: Arc<AtomicU64>,
        process: Arc<Mutex<Option<Arc<dyn LspProcessHandle>>>>,
    }
    impl LspProcessLauncher for RetireOnLaunch {
        fn launch(
            &self,
            spec: LspProcessSpec,
        ) -> Result<lithe_lsp_transport::process::LspProcessStreams, CoreError> {
            let process = self.inner.launch(spec)?;
            *self.process.lock().unwrap() = Some(process.handle.clone());
            let status = lithe_project::indexing::configure(serde_json::from_value(json!({
                "root": self.root, "manualExcludedDirectories": ["reactor"], "includedDirectories": [], "gitIgnoreEnabled": false
            })).unwrap(), crate::indexing_changed)?;
            self.revision.store(status.revision, Ordering::Release);
            Ok(process)
        }
    }
    struct Cleanup {
        root: PathBuf,
        revision: Arc<AtomicU64>,
        process: Arc<Mutex<Option<Arc<dyn LspProcessHandle>>>>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(process) = self.process.lock().unwrap().as_ref() {
                process.terminate();
            }
            let _ =
                lithe_project::indexing::release(lithe_project::indexing::ReleaseIndexingRequest {
                    root: self.root.to_string_lossy().into_owned(),
                    owner_id: None,
                    revision: self.revision.load(Ordering::Acquire),
                });
        }
    }
    for (initially_configured, cleanup_blocked, native_owner) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
        (false, true, true),
        (true, true, true),
    ] {
        let workspace = TemporaryWorkspace::recursive(&format!(
            "indexing-startup-retired-{initially_configured}"
        ));
        let root = workspace.root.canonicalize().unwrap();
        let installed = if initially_configured {
            lithe_project::indexing::configure(serde_json::from_value(json!({
        "root":root,"manualExcludedDirectories":[],"includedDirectories":[],"gitIgnoreEnabled":false
    })).unwrap(), crate::indexing_changed).unwrap().revision
        } else {
            0
        };
        let cleanup = Cleanup {
            root: root.clone(),
            revision: Arc::new(AtomicU64::new(installed)),
            process: Arc::new(Mutex::new(None)),
        };
        let server = ScriptedServer::new();
        server.block_cleanup(cleanup_blocked);
        let engine = LspEngine::with_launcher(Arc::new(RetireOnLaunch {
            inner: server.launcher(),
            root: root.clone(),
            revision: cleanup.revision.clone(),
            process: cleanup.process.clone(),
        }));
        let mut request = start_request(&server);
        request.provider_id = "python".into();
        request.root_uri = url::Url::from_directory_path(&root).unwrap().to_string();
        request.working_directory = root.to_string_lossy().into_owned();
        request.startup_operation = Some(AttemptHandoff {
            plugin_id: CANCELLED_PLUGIN.to_string(),
            operation_id: "indexing-cancel-before-registration".to_string(),
            elapsed_ms: 5,
        });
        let mut registered = None;
        let result = if native_owner {
            engine.start_server_owned(request, |id| registered = Some(id.to_owned()))
        } else {
            engine.start_server(request)
        };
        assert!(matches!(result.unwrap_err().code, ErrorCode::Cancelled));
        if cleanup_blocked {
            let id = if native_owner {
                registered.expect("incomplete disposal must reach the native owner")
            } else {
                engine
                    .lock_failed_starts()
                    .unwrap()
                    .first()
                    .cloned()
                    .expect("incomplete wire disposal retains its internal owner")
            };
            assert!(engine.destroy(&id).is_err());
            assert!(engine.session(&id).is_ok());
            server.block_cleanup(false);
            if native_owner {
                engine
                    .destroy(&id)
                    .expect("native owner can retry verified disposal");
            } else {
                engine
                    .cleanup_failed_start()
                    .expect("wire retry confirms disposal");
                assert!(engine.lock_failed_starts().unwrap().is_empty());
            }
        } else {
            assert!(
                registered.is_none(),
                "disposed startup needs no native barrier"
            );
        }
        assert!(engine.lock_sessions().unwrap().is_empty());
        assert!(
            server.input_was_closed(),
            "the cancelled startup owns process termination"
        );
        assert!(
            plugin_operations::test_capture::take(CANCELLED_PLUGIN).is_empty(),
            "a cancelled start records no plugin operation result"
        );
    }
}

#[test]
fn native_owner_retains_indexing_cancellation_after_registration() {
    const CANCELLED_PLUGIN: &str = "io.example.indexing-cancel-registered";
    let workspace = TemporaryWorkspace::recursive("indexing-owned-startup");
    let root = workspace.root.canonicalize().unwrap();
    let mut policy = IndexingPolicyFixture::new(root.clone());
    let server = ScriptedServer::new();
    server.block_cleanup(true);
    let engine = LspEngine::with_launcher(server.launcher());
    let mut request = start_request(&server);
    request.provider_id = "python".into();
    request.root_uri = url::Url::from_directory_path(&root).unwrap().to_string();
    request.working_directory = root.to_string_lossy().into_owned();
    request.startup_operation = Some(AttemptHandoff {
        plugin_id: CANCELLED_PLUGIN.to_string(),
        operation_id: "indexing-cancel-after-registration".to_string(),
        elapsed_ms: 5,
    });
    let mut registered = None;
    let result = engine.start_server_owned(request, |id| {
        registered = Some(id.to_owned());
        // Revoke between the initial policy check and initialize write.
        policy.exclude(&["reactor"]);
    });
    assert!(matches!(result.unwrap_err().code, ErrorCode::Cancelled));
    let id = registered.expect("native owner received its session");
    assert!(engine.destroy(&id).is_err());
    assert!(engine.session(&id).is_ok());
    server.block_cleanup(false);
    engine
        .destroy(&id)
        .expect("native owner confirms process disposal");
    assert!(engine.session(&id).is_err());
    assert!(
        plugin_operations::test_capture::take(CANCELLED_PLUGIN).is_empty(),
        "a cancelled start records no plugin operation result"
    );
}

#[test]
fn indexing_released_root_allows_a_standalone_provider_without_old_exclusions() {
    let workspace = TemporaryWorkspace::recursive("indexing-released-standalone");
    let root = workspace.root.canonicalize().unwrap();
    let root_uri = url::Url::from_directory_path(&root).unwrap().to_string();
    let status = lithe_project::indexing::configure(serde_json::from_value(json!({
        "root": root, "manualExcludedDirectories": ["reactor"], "includedDirectories": [], "gitIgnoreEnabled": false
    })).unwrap(), crate::indexing_changed).unwrap();
    let old = lithe_project::indexing::context(&root).unwrap();
    lithe_project::indexing::release(lithe_project::indexing::ReleaseIndexingRequest {
        root: root.to_string_lossy().into_owned(),
        revision: status.revision,
        owner_id: None,
    })
    .unwrap();
    assert!(
        old.check().is_err(),
        "late inventory readers must remain cancelled"
    );
    let harness = Harness::start(|request| {
        workspace.configure(request);
        request.root_uri = root_uri.clone();
        request.initialization_options =
            Some(json!({"settings":{"java":{"import":{"exclusions":["**/vendor/**"]}}}}));
    });
    let initialize = notifications(&harness, "initialize").pop().unwrap();
    assert_eq!(
        initialize["params"]["initializationOptions"]["settings"]["java"]["import"]["exclusions"],
        json!(["**/vendor/**"])
    );
    let file_uri = url::Url::from_file_path(root.join("reactor/visible.go"))
        .unwrap()
        .to_string();
    assert!(!lithe_project::indexing::uri_is_excluded(&root_uri, &file_uri).unwrap());
}

#[test]
fn indexing_cold_start_resolves_git_before_provider_initialization() {
    let workspace = TemporaryWorkspace::recursive("indexing-cold-start");
    let root = workspace.root.canonicalize().unwrap();
    std::fs::create_dir_all(root.join("ignored")).unwrap();
    std::fs::write(root.join("ignored/hidden.txt"), "hidden").unwrap();
    std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    let git = lithe_git::execute_git_readonly(
        &root.to_string_lossy(),
        &["init".into(), "--quiet".into()],
        None,
    )
    .unwrap();
    assert_eq!(git.exit_code, 0, "{}", git.stderr);
    let status = lithe_project::indexing::configure(serde_json::from_value(json!({
        "root": root, "manualExcludedDirectories": [], "includedDirectories": [], "gitIgnoreEnabled": true
    })).unwrap(), crate::indexing_changed).unwrap();
    let harness = Harness::start(|request| {
        workspace.configure(request);
        request.root_uri = url::Url::from_directory_path(&root).unwrap().to_string();
    });
    assert!(notifications(&harness, "initialize").pop().is_some());
    for (provider, pointer) in [
        ("go", "/gopls/directoryFilters"),
        ("rust", "/rust-analyzer/files/exclude"),
    ] {
        let value =
            crate::scan_settings::configuration(provider, &harness.session().root_uri, None)
                .unwrap();
        assert!(value
            .pointer(pointer)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item
                == if provider == "go" {
                    "-ignored"
                } else {
                    "ignored"
                }));
    }
    lithe_project::indexing::release(lithe_project::indexing::ReleaseIndexingRequest {
        root: root.to_string_lossy().into_owned(),
        revision: status.revision,
        owner_id: None,
    })
    .unwrap();
}

#[test]
fn indexing_change_withdraws_open_working_copies_and_rejects_sync() {
    let workspace = TemporaryWorkspace::recursive("indexing-working-copy");
    let root = workspace.root.canonicalize().unwrap();
    let root_uri = url::Url::from_directory_path(&root).unwrap().to_string();
    let mut harness = Harness::start(|request| {
        request.root_uri = root_uri.clone();
        request.working_directory = root.to_string_lossy().into_owned();
    });
    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);
    let uri = url::Url::from_file_path(root.join("reactor/main.go"))
        .unwrap()
        .to_string();
    harness.sync(&uri, "package main");
    let status = lithe_project::indexing::configure(
        serde_json::from_value(json!({
            "root":root,"manualExcludedDirectories":["reactor"],
            "includedDirectories":[],"gitIgnoreEnabled":false
        }))
        .unwrap(),
        crate::indexing_changed,
    )
    .unwrap();
    apply_indexing_to_sessions(&root, vec![harness.session()]).unwrap();
    assert!(!harness
        .session()
        .lock_state()
        .unwrap()
        .client
        .open_documents
        .contains_key(&uri));
    assert!(lithe_project::indexing::uri_is_excluded(&root_uri, &uri).unwrap());
    let session = harness.session();
    assert!(session
        .sync_document(SyncDocumentRequest {
            session_id: harness.session_id.clone(),
            uri: uri.clone(),
            language_id: "go".into(),
            text: "package main".into(),
            content_changes: Vec::new(),
        })
        .is_err());
    session.handle_server_message(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics", "params":{
        "uri":uri,"diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"severity":1,"message":"late"}]
    }}).to_string()).unwrap();
    assert!(!session
        .lock_state()
        .unwrap()
        .client
        .diagnostics
        .contains_key(&uri));

    assert_eq!(
        harness.session().indexing_revision.load(Ordering::Acquire),
        status.revision
    );
    lithe_project::indexing::release(lithe_project::indexing::ReleaseIndexingRequest {
        owner_id: None,
        root: root.to_string_lossy().into_owned(),
        revision: status.revision,
    })
    .unwrap();
}

/// Criterion 8: diagnostics for a stale document version are ignored.
/// Criterion 9: closing a document clears document and diagnostic state.
#[test]
fn diagnostics_follow_the_current_document_version_and_clear_on_close() {
    let mut harness = Harness::ready();
    let uri = "file:///workspace/main.go";
    harness.sync(uri, "package main");
    harness.sync(uri, "package main\nfunc main() {}");

    harness.server.send(json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "version": 2,
            "diagnostics": [{
                "range": {
                    "start": { "line": 1, "character": 0 },
                    "end": { "line": 1, "character": 4 }
                },
                "severity": 1,
                "message": "current"
            }]
        }
    }));
    harness.await_event(|event| {
        event.kind == "diagnostics"
            && event
                .diagnostics
                .as_ref()
                .is_some_and(|list| list.len() == 1)
    });
    assert_eq!(harness.snapshot().diagnostic_versions[uri], 2);

    // Version 1 is behind the open document, so it cannot replace version 2.
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "version": 1, "diagnostics": [] }
    }));
    thread::sleep(Duration::from_millis(50));
    harness.poll();
    assert_eq!(
        harness.snapshot().diagnostic_versions[uri],
        2,
        "a stale version must not clear current diagnostics"
    );

    harness.session().close_document(uri).unwrap();
    let snapshot = harness.snapshot();
    assert!(!snapshot.open_documents.contains_key(uri));
    assert!(!snapshot.diagnostic_versions.contains_key(uri));
    assert!(
        harness.notification("textDocument/didClose").is_some(),
        "the server must be told the document closed"
    );
    // Clearing is published so the editor drops its markers, rather than
    // leaving them until the next unrelated publish.
    let cleared = harness
        .poll()
        .iter()
        .rev()
        .find(|event| event.kind == "diagnostics" && event.uri.as_deref() == Some(uri));
    assert_eq!(
        cleared
            .and_then(|event| event.diagnostics.as_ref())
            .map(Vec::len),
        Some(0)
    );
}

/// A declarative plugin start handed off by the host ends in exactly one Core
/// result: `initialize` success, an exit before it, or nothing for a user stop.
/// Later exits belong to the session, not the start. A plugin API 1 server
/// starts host-owned, so the handoff alone names the plugin; a session owned
/// by a different plugin records nothing.
#[test]
fn declarative_plugin_start_records_one_host_operation_per_attempt() {
    const PLUGIN: &str = "io.example.declarative-start";
    const OTHER: &str = "io.example.declarative-start-other";
    let handoff = |operation_id: &'static str, owner: Option<&'static str>| {
        move |request: &mut StartServerRequest| {
            request.plugin_id = owner.map(str::to_string);
            request.startup_operation = Some(AttemptHandoff {
                plugin_id: PLUGIN.to_string(),
                operation_id: operation_id.to_string(),
                elapsed_ms: 5,
            });
        }
    };

    let mut ready = Harness::start(handoff("declarative-ready", Some(PLUGIN)));
    ready.server.complete_initialize(ready_capabilities());
    ready.await_state(LspLifecycleState::Ready);
    ready.server.exit(Some(1));
    ready.await_state(LspLifecycleState::Failed);

    let mut exited = Harness::start(handoff("declarative-exited", None));
    exited.server.exit(Some(2));
    exited.await_state(LspLifecycleState::Failed);

    let mut stopped = Harness::start(handoff("declarative-stopped", Some(PLUGIN)));
    stopped.session().stop().unwrap();
    stopped.await_state(LspLifecycleState::Stopped);

    let mut foreign = Harness::start(handoff("declarative-foreign", Some(OTHER)));
    foreign.server.exit(Some(3));
    foreign.await_state(LspLifecycleState::Failed);
    assert!(plugin_operations::test_capture::take(OTHER).is_empty());

    let facts = plugin_operations::test_capture::take(PLUGIN);
    assert!(facts
        .iter()
        .all(|fact| std::ptr::eq(fact.contribution, &LANGUAGE_SERVER_START)));
    let results: Vec<_> = facts
        .into_iter()
        .map(|fact| (fact.operation_id, fact.failure))
        .collect();
    assert_eq!(
        results,
        [
            (Some("declarative-ready".to_string()), None),
            (
                Some("declarative-exited".to_string()),
                Some(AttemptStage::Exit)
            ),
        ]
    );
}

/// startServer failing after spawn is a `launch` failure, and an initialize
/// timeout decided under the state lock is the only result even when the
/// response arrives afterwards.
#[test]
fn declarative_plugin_start_attributes_write_failure_and_timeout() {
    const PLUGIN: &str = "io.example.declarative-start-failures";
    let handoff = |operation_id: &str, request: &mut StartServerRequest| {
        request.plugin_id = Some(PLUGIN.to_string());
        request.startup_operation = Some(AttemptHandoff {
            plugin_id: PLUGIN.to_string(),
            operation_id: operation_id.to_string(),
            elapsed_ms: 5,
        });
    };

    let broken = ScriptedServer::new();
    broken.break_input();
    let engine = LspEngine::with_launcher(broken.launcher());
    let mut request = start_request(&broken);
    handoff("declarative-write-failed", &mut request);
    assert!(engine.start_server(request).is_err());

    let mut timed_out = Harness::start(|request| {
        handoff("declarative-timed-out", request);
        request.initialize_timeout_milliseconds = 60_000;
    });
    timed_out
        .session()
        .expire_deadlines_at(Instant::now() + Duration::from_secs(61));
    timed_out.await_state(LspLifecycleState::Failed);
    timed_out.server.complete_initialize(ready_capabilities());

    let results: Vec<_> = plugin_operations::test_capture::take(PLUGIN)
        .into_iter()
        .map(|fact| (fact.operation_id, fact.failure))
        .collect();
    assert_eq!(
        results,
        [
            (
                Some("declarative-write-failed".to_string()),
                Some(AttemptStage::Launch)
            ),
            (
                Some("declarative-timed-out".to_string()),
                Some(AttemptStage::Timeout)
            ),
        ]
    );
}

/// Criterion 10: shutdown sends exit after the response, and a shutdown that
/// is never answered force-terminates.
#[test]
fn shutdown_sends_exit_after_the_response_and_force_terminates_on_timeout() {
    let mut harness = Harness::ready();
    harness.session().stop().unwrap();
    let shutdown_id = harness
        .server
        .await_request("shutdown")
        .expect("stop should request shutdown");
    assert!(
        harness.notification("exit").is_none(),
        "exit must not precede the shutdown response"
    );

    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": shutdown_id,
        "result": null
    }));
    assert!(
        harness.server.await_notification("exit"),
        "exit must follow the shutdown response"
    );
    harness.server.exit(Some(0));
    harness.await_state(LspLifecycleState::Stopped);

    let mut silent = Harness::start(|request| {
        request.shutdown_timeout_milliseconds = 30;
    });
    silent.server.complete_initialize(ready_capabilities());
    silent.await_state(LspLifecycleState::Ready);
    silent.session().stop().unwrap();
    silent
        .server
        .await_request("shutdown")
        .expect("stop should request shutdown");
    // No response ever arrives, so the deadline must kill the process
    // instead of leaving the session stuck in Stopping.
    silent.await_state(LspLifecycleState::Stopped);
    silent.await_event(|event| {
        event.kind == "log"
            && event
                .error
                .as_ref()
                .is_some_and(|error| error.stage == "shutdown" && error.code == "shutdownTimeout")
    });
}

/// Criterion 11: a malformed `Content-Length` is a transport failure.
#[test]
fn a_malformed_content_length_header_fails_the_session_in_transport() {
    let mut harness = Harness::ready();
    harness.server.send_raw(b"Content-Length: banana\r\n\r\n{}");
    harness.await_state(LspLifecycleState::Failed);

    let failure = harness
        .events
        .iter()
        .find_map(|event| event.error.as_ref())
        .expect("bad framing should be reported as a runtime error");
    assert_eq!(failure.code, "transportFailed");
    assert_eq!(failure.stage, "transport");
}

/// Criterion 12: a partial frame is retained until it completes.
/// Criterion 13: consecutive frames are handled in order.
#[test]
fn partial_frames_are_buffered_and_consecutive_frames_arrive_in_order() {
    let mut harness = Harness::ready();
    let uri = "file:///workspace/main.go";
    harness.sync(uri, "package main");

    let split = |uri: &str, message: &str| {
        let body = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "version": 1,
                "diagnostics": [{
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 1 }
                    },
                    "severity": 1,
                    "message": message
                }]
            }
        })
        .to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    };

    let first = split(uri, "first");
    let boundary = first.len() - 12;
    harness.server.send_raw(&first.as_bytes()[..boundary]);
    thread::sleep(Duration::from_millis(40));
    harness.poll();
    assert!(
        !harness
            .events
            .iter()
            .any(|event| event.kind == "diagnostics"),
        "an incomplete frame must not be delivered"
    );

    // The tail of the first frame and a whole second frame arrive together,
    // which is exactly how a stream coalesces writes.
    harness
        .server
        .send_raw(format!("{}{}", &first[boundary..], split(uri, "second")).as_bytes());
    harness.await_event(|event| {
        event
            .diagnostics
            .as_ref()
            .and_then(|list| list.first())
            .is_some_and(|diagnostic| diagnostic.message == "second")
    });
    let delivered: Vec<_> = harness
        .events
        .iter()
        .filter(|event| event.kind == "diagnostics")
        .filter_map(|event| event.diagnostics.as_ref())
        .filter_map(|list| list.first())
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    assert_eq!(delivered, vec!["first".to_string(), "second".to_string()]);
    assert_eq!(harness.snapshot().state, LspLifecycleState::Ready);
}

/// Criterion 14: dynamic registration and unregistration change availability.
#[test]
fn features_event_reports_completion_trigger_characters() {
    let mut harness = Harness::start(|_| {});
    harness.server.complete_initialize(json!({
        "completionProvider": {
            "triggerCharacters": [".", ":", ">"],
            "allCommitCharacters": [";"]
        }
    }));
    let event = harness
        .await_event(|event| event.kind == "featuresChanged" && event.completion_options.is_some())
        .clone();
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(
        json["completionOptions"],
        json!({"triggerCharacters": [".", ":", ">"], "allCommitCharacters": [";"]})
    );
}

#[test]
fn dynamic_capability_registration_and_unregistration_change_availability() {
    let mut harness = Harness::start(|_| {});
    // Formatting is absent from the static capabilities, so it can only
    // become available through dynamic registration.
    harness
        .server
        .complete_initialize(json!({ "hoverProvider": true }));
    harness.await_state(LspLifecycleState::Ready);
    let uri = "file:///workspace/main.go";
    harness.sync(uri, "package main");
    assert!(harness
        .session()
        .request(
            SemanticRequest {
                query: None,
                session_id: harness.session_id.clone(),
                operation_id: Some("op-formatting".to_string()),
                operation: LspSemanticOperation::Formatting,
                uri: Some(uri.to_string()),
                position: None,
                new_name: None,
                range: None,
                diagnostics: Vec::new(),
                completion_context: None,
                completion_item: None,
                code_action: None,
                command: None,
                formatting_options: None,
                include_declaration: None,
            },
            "op-formatting".to_string(),
        )
        .is_err());

    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "registration-1",
        "method": "client/registerCapability",
        "params": {
            "registrations": [{
                "id": "formatting-1",
                "method": "textDocument/formatting",
                "registerOptions": {}
            }]
        }
    }));
    let registered = harness
        .await_event(|event| {
            event
                .capabilities
                .as_ref()
                .is_some_and(|names| names.iter().any(|name| name == "formatting"))
        })
        .clone();
    assert_eq!(registered.kind, "featuresChanged");

    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "registration-2",
        "method": "client/unregisterCapability",
        "params": {
            "unregisterations": [{
                "id": "formatting-1",
                "method": "textDocument/formatting"
            }]
        }
    }));
    // The initialize handshake also emitted a formatting-free feature set, so
    // the withdrawal is only identifiable by coming after the registration.
    let registered_at = registered.sequence;
    harness.await_event(|event| {
        event.kind == "featuresChanged"
            && event.sequence > registered_at
            && event
                .capabilities
                .as_ref()
                .is_some_and(|names| !names.iter().any(|name| name == "formatting"))
    });
    assert!(
        harness
            .session()
            .request(
                SemanticRequest {
                    query: None,
                    session_id: harness.session_id.clone(),
                    operation_id: Some("op-formatting-2".to_string()),
                    operation: LspSemanticOperation::Formatting,
                    uri: Some(uri.to_string()),
                    position: None,
                    new_name: None,
                    range: None,
                    diagnostics: Vec::new(),
                    completion_context: None,
                    completion_item: None,
                    code_action: None,
                    command: None,
                    formatting_options: None,
                    include_declaration: None,
                },
                "op-formatting-2".to_string(),
            )
            .is_err(),
        "an unregistered capability must stop being offered"
    );
}

/// Criterion 15: replacing the workspace stops the old root and clears it.
#[test]
fn replacing_the_workspace_stops_the_old_root_and_clears_its_state() {
    let old_server = ScriptedServer::new();
    let engine = LspEngine::with_launcher(old_server.launcher());
    let old = engine.start_server(start_request(&old_server)).unwrap();
    old_server.complete_initialize(ready_capabilities());
    let old_session = engine.session(&old.session_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && old_session.snapshot().unwrap().state != LspLifecycleState::Ready
    {
        thread::sleep(Duration::from_millis(2));
    }
    let uri = "file:///workspace/main.go";
    old_session
        .sync_document(SyncDocumentRequest {
            session_id: old.session_id.clone(),
            uri: uri.to_string(),
            language_id: "go".to_string(),
            text: "package main".to_string(),
            content_changes: Vec::new(),
        })
        .unwrap();
    old_server.send(json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "version": 1,
            "diagnostics": [{
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 1 }
                },
                "severity": 1,
                "message": "old root"
            }]
        }
    }));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && !old_session
            .snapshot()
            .unwrap()
            .diagnostic_versions
            .contains_key(uri)
    {
        thread::sleep(Duration::from_millis(2));
    }

    old_session.stop().unwrap();
    let shutdown = old_server.await_request("shutdown").unwrap();
    old_server.send(json!({ "jsonrpc": "2.0", "id": shutdown, "result": null }));
    old_server.exit(Some(0));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && old_session.snapshot().unwrap().state != LspLifecycleState::Stopped
    {
        thread::sleep(Duration::from_millis(2));
    }

    let stopped = old_session.snapshot().unwrap();
    assert_eq!(stopped.root_uri, "file:///workspace");
    assert_eq!(stopped.state, LspLifecycleState::Stopped);
    assert!(stopped.open_documents.is_empty());
    assert!(stopped.diagnostic_versions.is_empty());
    assert!(stopped.pending_operation_ids.is_empty());
    assert!(
        old_server.input_was_closed(),
        "the old root's stdin must be released"
    );

    // Only a stopped session may be destroyed, and a destroyed one is
    // unreachable, so the replacement cannot inherit any of its state.
    engine.destroy(&old.session_id).unwrap();
    assert!(engine.session(&old.session_id).is_err());
    assert!(old_session
        .sync_document(SyncDocumentRequest {
            session_id: old.session_id.clone(),
            uri: uri.to_string(),
            language_id: "go".to_string(),
            text: "package main".to_string(),
            content_changes: Vec::new(),
        })
        .is_err());
}

#[test]
fn semantic_operations_are_protocol_methods_but_never_expose_request_ids() {
    assert_eq!(
        semantic_method(LspSemanticOperation::Definition),
        "textDocument/definition"
    );
}

#[test]
fn workspace_execute_command_does_not_require_an_open_document() {
    let mut harness = Harness::start(|_| {});
    harness.server.complete_initialize(json!({
        "executeCommandProvider": { "commands": ["source.fix"] }
    }));
    harness.await_state(LspLifecycleState::Ready);
    let operation_id = harness.engine.next_operation_id();

    harness
        .session()
        .request(
            SemanticRequest {
                query: None,
                session_id: harness.session_id.clone(),
                operation_id: Some(operation_id.clone()),
                operation: LspSemanticOperation::ExecuteCommand,
                uri: None,
                position: None,
                new_name: None,
                range: None,
                diagnostics: Vec::new(),
                completion_context: None,
                completion_item: None,
                code_action: None,
                command: Some(json!({
                    "title": "Apply fix",
                    "command": "source.fix",
                    "arguments": []
                })),
                formatting_options: None,
                include_declaration: None,
            },
            operation_id,
        )
        .expect("workspace commands should not be gated on an open document");

    let request = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "workspace/executeCommand")
        .expect("the command should reach the language server");
    assert_eq!(request["params"]["command"], "source.fix");
    assert!(request["params"].get("textDocument").is_none());
}

#[test]
fn document_saves_honor_server_options_and_never_replace_newer_editor_text() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/lsp/document-save-v1.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let option = &case["saveOption"];
        let mut harness = Harness::start(|_| {});
        let mut capabilities = ready_capabilities();
        capabilities["textDocumentSync"] = json!({"change": 1, "save": option});
        harness.server.complete_initialize(capabilities);
        harness.await_state(LspLifecycleState::Ready);
        let uri = case["uri"].as_str().unwrap();
        let saved_text = case["savedText"].as_str().unwrap();
        harness.sync(uri, saved_text);
        let saved = || SaveDocumentRequest {
            session_id: harness.session_id.clone(),
            uri: uri.to_string(),
            text: saved_text.to_string(),
        };
        let supported = case["sent"].as_bool().unwrap();
        assert_eq!(harness.session().save_document(saved()).unwrap(), supported);
        if supported {
            let notification = harness
                .server
                .await_notification_at("textDocument/didSave", 0)
                .unwrap();
            assert_eq!(notification["params"]["textDocument"]["uri"], uri);
            assert_eq!(
                notification["params"].get("text"),
                case["includeText"]
                    .as_bool()
                    .unwrap()
                    .then_some(&json!(saved_text))
            );
        }
        harness.sync(uri, "newer unsaved edit");
        assert!(!harness.session().save_document(saved()).unwrap());
        let session = harness.session();
        let state = session.lock_state().unwrap();
        assert_eq!(
            state.client.open_documents[uri].text.as_ref(),
            "newer unsaved edit"
        );
        assert_eq!(state.client.open_documents[uri].version, 2);
    }
}

/// Removes a scratch directory when the owning test ends or unwinds.
#[cfg(unix)]
struct ScratchDirectory(PathBuf);

#[cfg(unix)]
impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Regression: a server publishes diagnostics under the resolved path of a
// file the host opened through a symbolic link; matching the URIs textually
// dropped every error in such a workspace.
#[cfg(unix)]
#[test]
fn diagnostics_published_under_the_resolved_path_reach_the_linked_document() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let scratch = ScratchDirectory(std::env::temp_dir().join(format!(
        "lithe-linked-diagnostics-{}-{stamp}",
        std::process::id()
    )));
    let workspace = scratch.0.join("resolved");
    std::fs::create_dir_all(&workspace).expect("workspace should be creatable");
    std::fs::write(workspace.join("main.go"), "package main").expect("source file");
    let link = scratch.0.join("link");
    std::os::unix::fs::symlink(&workspace, &link).expect("link should be creatable");
    let opened_uri = url::Url::from_file_path(link.join("main.go"))
        .expect("linked file converts to a URI")
        .to_string();
    let published_uri = url::Url::from_file_path(
        workspace
            .canonicalize()
            .expect("workspace canonicalizes")
            .join("main.go"),
    )
    .expect("resolved file converts to a URI")
    .to_string();
    assert_ne!(opened_uri, published_uri);

    let mut harness = Harness::ready();
    harness.sync(&opened_uri, "package main");
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": published_uri,
            "diagnostics": [{
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 7 }
                },
                "severity": 1,
                "message": "reported at the resolved path"
            }]
        }
    }));
    let event = harness
        .await_event(|event| event.kind == "diagnostics")
        .clone();
    assert_eq!(event.uri.as_deref(), Some(opened_uri.as_str()));
    assert_eq!(event.diagnostics.map(|list| list.len()), Some(1));
}

#[cfg(unix)]
fn open_document(uri: &str) -> (String, LspClientDocument) {
    (
        uri.to_string(),
        LspClientDocument {
            uri: uri.to_string(),
            language_id: "go".to_string(),
            version: 1,
            text: "".into(),
        },
    )
}

// Servers publish for many files nobody has open. A session whose documents
// were not opened through a link must remember that and leave those
// messages untouched; closing a document must forget what was learned.
#[cfg(unix)]
#[test]
fn diagnostics_for_unlinked_documents_are_left_alone_and_forgotten_on_close() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let scratch = ScratchDirectory(std::env::temp_dir().join(format!(
        "lithe-unlinked-diagnostics-{}-{stamp}",
        std::process::id()
    )));
    std::fs::create_dir_all(&scratch.0).expect("scratch should be creatable");
    let real = scratch.0.canonicalize().expect("scratch canonicalizes");
    std::fs::write(real.join("main.go"), "package main").expect("source file");
    std::fs::write(real.join("other.go"), "package main").expect("source file");
    let uri_of = |name: &str| {
        url::Url::from_file_path(real.join(name))
            .expect("file converts to a URI")
            .to_string()
    };
    let message = |uri: &str| {
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": [] }
        })
    };

    let documents: BTreeMap<_, _> = [open_document(&uri_of("main.go"))].into();
    let mut linked = LinkedDocumentPaths::default();
    assert_eq!(
        diagnostics_addressed_to_open_document(
            &documents,
            &mut linked,
            &message(&uri_of("other.go"))
        ),
        None
    );
    assert!(linked.resolved.is_empty());
    assert!(linked.examined.contains(&uri_of("main.go")));

    diagnostics_addressed_to_open_document(
        &BTreeMap::new(),
        &mut linked,
        &message(&uri_of("other.go")),
    );
    assert!(linked.examined.is_empty());
}

#[test]
fn wait_events_returns_queued_events_without_waiting_the_timeout() {
    let harness = Harness::start(|_| {});
    let started = Instant::now();
    let events = harness
        .session()
        .wait_events(Duration::from_secs(2))
        .expect("waiting should succeed");
    assert!(
        !events.is_empty(),
        "starting a session should enqueue at least one lifecycle event"
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "queued events must not wait out the timeout"
    );
}

#[test]
fn wait_events_times_out_with_an_empty_queue() {
    let mut harness = Harness::ready();
    harness.poll();
    let started = Instant::now();
    let events = harness
        .session()
        .wait_events(Duration::from_millis(40))
        .expect("waiting should succeed");
    assert!(events.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(30));
}

#[test]
fn wait_events_wakes_when_the_session_enqueues_a_lifecycle_event() {
    let mut harness = Harness::ready();
    harness.poll();
    let session = harness.session();
    let waiter = thread::spawn({
        let session = Arc::clone(&session);
        move || {
            session
                .wait_events(Duration::from_secs(2))
                .expect("waiting should succeed")
        }
    });
    thread::sleep(Duration::from_millis(30));
    session.stop().expect("the session should stop");
    let events = waiter.join().expect("the waiter thread should finish");
    assert!(
        events.iter().any(|event| event.kind == "stateChanged"),
        "stop should wake waiters with a lifecycle event, got {events:?}"
    );
}

#[test]
fn wait_events_errors_after_terminal_state_events_are_drained() {
    let mut harness = Harness::ready();
    harness.poll();
    harness.session().stop().unwrap();
    let shutdown_id = harness
        .server
        .await_request("shutdown")
        .expect("stop should request shutdown");
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": shutdown_id,
        "result": null
    }));
    assert!(
        harness.server.await_notification("exit"),
        "exit must follow the shutdown response"
    );
    harness.server.exit(Some(0));
    harness.await_state(LspLifecycleState::Stopped);

    // Drain any remaining lifecycle events from the stop transition.
    loop {
        let events = match harness.session().wait_events(Duration::from_millis(20)) {
            Ok(events) => events,
            Err(error) => {
                assert!(matches!(error.code, ErrorCode::ProcessFailed));
                assert_eq!(error.details.as_deref(), Some("sessionStopped"));
                return;
            }
        };
        if events.is_empty() {
            break;
        }
    }

    let error = harness
        .session()
        .wait_events(Duration::from_millis(50))
        .expect_err("drained terminal sessions must not return empty Ok");
    assert!(matches!(error.code, ErrorCode::ProcessFailed));
    assert_eq!(error.details.as_deref(), Some("sessionStopped"));
}

#[test]
fn provider_debug_preparation_retains_progress() {
    let mut harness = plugin_backend(json!({"litheBuild":{"version":0}}));
    let operation = build_request(&harness, "lithe/build/prepareDebug", json!({}), None).unwrap();
    let request = harness
        .server
        .await_request("lithe/build/prepareDebug")
        .unwrap();
    let wire_id = request.clone();
    assert_eq!(
        harness.written_request(&request)["params"]["operationID"],
        wire_id
    );
    harness.server.send(json!({"jsonrpc":"2.0","method":"lithe/build/progress","params":{"operationID":wire_id,"output":"built"}}));
    let progress = harness.await_event(|event| {
        event.kind == "requestProgress" && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(progress.result.as_ref().unwrap()["output"], "built");
    harness.server.send(json!({"jsonrpc":"2.0","id":request,"result":{"program":"app/bin/main","arguments":[],"environment":{},"workingDirectory":"app"}}));
    let done = harness.await_event(|event| {
        event.kind == "requestCompleted"
            && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(
        done.result.as_ref().unwrap()["value"]["program"],
        "app/bin/main"
    );
}

#[test]
fn inline_test_reports_reach_the_provider() {
    let mut harness = plugin_backend(json!({"litheBuild":{"version":0}}));
    let operation = build_request(
        &harness,
        "lithe/build/test",
        json!({"testResults":true}),
        None,
    )
    .unwrap();
    let id = harness.server.await_request("lithe/build/test").unwrap();
    let value = json!({"exitCode":1,"output":"failure","testResults":{"format":"litheTestResults","version":0,"tests":[]}});
    harness
        .server
        .send(json!({"jsonrpc":"2.0","id":id,"result":value}));
    let event = harness.await_event(|event| {
        event.kind == "requestCompleted"
            && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(event.result.as_ref().unwrap()["value"], value);
}

#[test]
fn provider_native_configuration_returns_only_the_projection() {
    let mut harness = plugin_backend(json!({"litheBuild":{"version":0}}));
    let operation = build_request(&harness, "lithe/build/configuration", json!({}), None).unwrap();
    let id = harness
        .server
        .await_request("lithe/build/configuration")
        .unwrap();
    let result = json!({"format":"litheBuildConfiguration","version":0,"sources":[]});
    harness
        .server
        .send(json!({"jsonrpc":"2.0","id":id,"result":result}));
    let event = harness.await_event(|event| {
        event.kind == "requestCompleted"
            && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(event.result.as_ref().unwrap()["value"], result);
}

/// Launch options reach the provider unchanged, and only run preparation accepts them.
#[test]
fn provider_run_options_reach_the_wire_for_run_preparation_only() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../shared/plugins/contract/v1/fixtures/build-providers/build-run-options.json"
    ))
    .unwrap();
    let params = json!({"launchOptions":fixture["launchOptions"]});
    let mut current = plugin_backend(json!({"litheBuild":{"version":0}}));
    let error = build_request(&current, "lithe/build/model", params.clone(), None).unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    let operation = build_request(&current, "lithe/build/prepareRun", params, None).unwrap();
    let id = current
        .server
        .await_request("lithe/build/prepareRun")
        .unwrap();
    let message = current
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "lithe/build/prepareRun")
        .unwrap();
    assert_eq!(message["params"]["launchOptions"], fixture["launchOptions"]);
    current
        .server
        .send(json!({"jsonrpc":"2.0","id":id,"result":fixture["result"]}));
    let event = current.await_event(|event| {
        event.kind == "requestCompleted"
            && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(event.result.as_ref().unwrap()["value"], fixture["result"]);
}

/// Port ownership reaches the provider and comes back in the raw plan.
#[test]
fn provider_connection_debug_options_reach_the_wire() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../shared/plugins/contract/v1/fixtures/build-providers/build-connection-debug-options.json"
    ))
    .unwrap();
    let params = json!({"launchOptions":fixture["launchOptions"]});
    let mut current = plugin_backend(json!({"litheBuild":{"version":0}}));
    let operation = build_request(&current, "lithe/build/prepareRun", params, None).unwrap();
    let id = current
        .server
        .await_request("lithe/build/prepareRun")
        .unwrap();
    current
        .server
        .send(json!({"jsonrpc":"2.0","id":id,"result":fixture["result"]}));
    let event = current.await_event(|event| {
        event.kind == "requestCompleted"
            && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(event.result.as_ref().unwrap()["value"], fixture["result"]);
}

#[test]
fn provider_run_preparation_returns_the_raw_plan() {
    let mut harness = plugin_backend(json!({"litheBuild":{"version":0}}));
    let operation = build_request(&harness, "lithe/build/prepareRun", json!({}), None).unwrap();
    let id = harness
        .server
        .await_request("lithe/build/prepareRun")
        .unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../shared/plugins/contract/v1/fixtures/build-providers/build-run-preparation.json"
    ))
    .unwrap();
    harness
        .server
        .send(json!({"jsonrpc":"2.0","id":id,"result":fixture["result"]}));
    let event = harness.await_event(|event| {
        event.kind == "requestCompleted"
            && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(event.result.as_ref().unwrap()["value"], fixture["result"]);
}

#[test]
fn provider_test_discovery_uses_the_existing_request_lifecycle() {
    let mut harness = plugin_backend(json!({"litheBuild":{"version":0}}));
    let operation = build_request(
        &harness,
        "lithe/build/discoverTests",
        json!({"root":"."}),
        None,
    )
    .unwrap();
    let id = harness
        .server
        .await_request("lithe/build/discoverTests")
        .unwrap();
    let result = json!({"format":"litheTestDiscovery","version":0,"tests":[]});
    harness
        .server
        .send(json!({"jsonrpc":"2.0","id":id,"result":result}));
    let event = harness.await_event(|event| {
        event.kind == "requestCompleted"
            && event.operation_id.as_deref() == Some(operation.as_str())
    });
    assert_eq!(event.result.as_ref().unwrap()["value"], result);
    assert!(harness.snapshot().pending_operation_ids.is_empty());
}

/// Ordered messages use a completed round trip as a barrier: old progress
/// and replies are drained before checking that cancellation stayed final.
#[test]
fn provider_execution_progress_cancel_and_late_results_are_isolated() {
    for method in [
        "lithe/build/execute",
        "lithe/build/test",
        "lithe/build/prepareRun",
    ] {
        let mut harness = Harness::start(|_| {});
        harness
            .server
            .complete_initialize(json!({"experimental":{"litheBuild":{"version":0}}}));
        harness.await_state(LspLifecycleState::Ready);
        let operation =
            build_request(&harness, method, json!({"operationID":"spoof"}), None).unwrap();
        let id = harness.server.await_request(method).unwrap();
        assert_eq!(harness.written_request(&id)["params"]["operationID"], id);
        harness
            .server
            .send(json!({"jsonrpc":"2.0","method":"lithe/build/progress",
        "params":{"operationID":id,"output":"first","percentage":20}}));
        let progress = harness
            .await_event(|event| event.kind == "requestProgress")
            .clone();
        assert_eq!(progress.operation_id.as_deref(), Some(operation.as_str()));
        assert_eq!(progress.result.unwrap()["output"], "first");
        harness.session().cancel_operation(&operation).unwrap();
        let ended = harness
            .await_event(|event| {
                event.kind == "requestCompleted"
                    && event.operation_id.as_deref() == Some(operation.as_str())
            })
            .clone();
        assert_eq!(ended.error.unwrap().code, "requestCancelled");
        harness
            .server
            .send(json!({"jsonrpc":"2.0","method":"lithe/build/progress",
        "params":{"operationID":id,"output":"late"}}));
        harness
            .server
            .send(json!({"jsonrpc":"2.0","id":id,"result":{"exitCode":0,"output":"late"}}));
        let barrier = build_request(&harness, "lithe/build/model", json!({}), None).unwrap();
        let barrier_id = harness.server.await_request("lithe/build/model").unwrap();
        harness
            .server
            .send(json!({"jsonrpc":"2.0","id":barrier_id,"result":{}}));
        harness.await_event(|event| {
            event.kind == "requestCompleted"
                && event.operation_id.as_deref() == Some(barrier.as_str())
        });
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| event.kind == "requestProgress")
                .count(),
            1
        );
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| event.kind == "requestCompleted"
                    && event.operation_id.as_deref() == Some(operation.as_str()))
                .count(),
            1
        );
    }
}

/// Host operation IDs may be reused; only the current wire request ID
/// may contribute progress to that new call.
#[test]
fn provider_execution_reused_host_ids_reject_old_wire_progress() {
    for method in [
        "lithe/build/execute",
        "lithe/build/test",
        "lithe/build/prepareRun",
    ] {
        let mut harness = plugin_backend(json!({"litheBuild":{"version":0}}));
        let operation = "reused-build".to_string();
        let send = |harness: &Harness| {
            harness
                .session()
                .build_request(
                    BuildProviderRequest {
                        session_id: harness.session_id.clone(),
                        operation_id: Some(operation.clone()),
                        method: method.to_string(),
                        params: json!({}),
                        timeout_milliseconds: None,
                    },
                    operation.clone(),
                )
                .unwrap()
        };
        send(&harness);
        let old = harness.server.await_request(method).unwrap();
        harness.session().cancel_operation(&operation).unwrap();
        harness.await_event(|event| {
            event.kind == "requestCompleted"
                && event.operation_id.as_deref() == Some(operation.as_str())
        });
        send(&harness);
        let current = harness.server.await_request_at(method, 1).unwrap();
        assert_ne!(old, current);
        for (id, output) in [(old, "old"), (current, "current")] {
            harness
                .server
                .send(json!({"jsonrpc":"2.0","method":"lithe/build/progress",
            "params":{"operationID":id,"output":output}}));
        }
        harness.await_event(|event| {
            event.kind == "requestProgress"
                && event
                    .result
                    .as_ref()
                    .is_some_and(|result| result["output"] == "current")
        });
        assert!(harness
            .events
            .iter()
            .filter(|event| event.kind == "requestProgress")
            .all(|event| event.result.as_ref().unwrap()["output"] != "old"));
        harness.session().cancel_operation(&operation).unwrap();
    }
}

#[test]
fn provider_execution_validates_results_and_expires_without_sleeping() {
    for method in [
        "lithe/build/execute",
        "lithe/build/test",
        "lithe/build/prepareRun",
    ] {
        let mut harness = Harness::start(|_| {});
        harness
            .server
            .complete_initialize(json!({"experimental":{"litheBuild":{"version":0}}}));
        harness.await_state(LspLifecycleState::Ready);
        for (index, (reply, code)) in [
            (json!({"exitCode":7,"output":"failed build"}), None),
            (
                json!({"exitCode":"bad"}),
                Some("plugin.build.invalidResult"),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let operation = build_request(&harness, method, json!({}), None).unwrap();
            let id = harness.server.await_request_at(method, index).unwrap();
            harness
                .server
                .send(json!({"jsonrpc":"2.0","id":id,"result":reply}));
            let ended = harness
                .await_event(|event| {
                    event.kind == "requestCompleted"
                        && event.operation_id.as_deref() == Some(operation.as_str())
                })
                .clone();
            // Preparation returns raw launch data; the build decoder validates it.
            let code = if method == "lithe/build/prepareRun" {
                None
            } else {
                code
            };
            assert_eq!(ended.error.as_ref().map(|error| error.code.as_str()), code);
            if code.is_none() {
                assert_eq!(ended.result.unwrap()["value"], reply);
            }
        }
        let operation = build_request(&harness, method, json!({}), Some(1000)).unwrap();
        harness.server.await_request_at(method, 2).unwrap();
        harness
            .session()
            .expire_deadlines_at(Instant::now() + Duration::from_secs(2));
        assert_eq!(
            harness
                .await_event(|event| event.kind == "requestCompleted"
                    && event.operation_id.as_deref() == Some(operation.as_str()))
                .error
                .as_ref()
                .unwrap()
                .code,
            "requestTimeout"
        );
        assert!(harness.snapshot().pending_operation_ids.is_empty());
    }
}

/// A process that answered the build-provider protocol with version 0.
fn negotiated_build_provider() -> Harness {
    let mut harness = Harness::start(|_| {});
    harness.server.complete_initialize(json!({
        "experimental": { "litheBuild": { "version": 0 } }
    }));
    harness.await_state(LspLifecycleState::Ready);
    harness
}

fn build_request(
    harness: &Harness,
    method: &str,
    params: Value,
    timeout_milliseconds: Option<u64>,
) -> Result<String, CoreError> {
    let operation_id = harness.engine.next_operation_id();
    harness
        .session()
        .build_request(
            BuildProviderRequest {
                session_id: harness.session_id.clone(),
                operation_id: Some(operation_id.clone()),
                method: method.to_string(),
                params,
                timeout_milliseconds,
            },
            operation_id.clone(),
        )
        .map(|()| operation_id)
}

/// Every session offers the protocol, but one whose process did not
/// choose a version must never receive a build request: a shared
/// language server keeps serving its language without it.
#[test]
fn build_requests_need_a_negotiated_protocol_version() {
    let harness = Harness::ready();
    let initialize = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "initialize")
        .expect("initialize should have been written");
    assert_eq!(
        initialize["params"]["capabilities"]["experimental"]["litheBuild"],
        json!({ "versions": [0] })
    );

    let error = build_request(&harness, "lithe/build/model", json!({}), None)
        .expect_err("an unnegotiated session must refuse build requests");
    assert!(matches!(error.code, ErrorCode::NotSupported));
    assert!(harness
        .server
        .messages()
        .iter()
        .all(|message| message["method"] != "lithe/build/model"));
}

/// A reply naming a version the host did not offer negotiates nothing,
/// for the build protocol and the plugin-function protocol alike.
#[test]
fn a_reply_naming_an_unoffered_version_negotiates_nothing() {
    let harness = plugin_backend(json!({
        "litheBuild": { "version": 1 },
        "litheFunctions": { "version": 1, "functions": [{ "id": "snapshot.refresh" }] }
    }));
    let error = build_request(&harness, "lithe/build/model", json!({}), None)
        .expect_err("version 1 was not offered");
    assert!(matches!(error.code, ErrorCode::NotSupported));
    let error = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None)
        .expect_err("version 1 was not offered");
    assert!(matches!(error.code, ErrorCode::NotSupported));
    assert!(harness.server.messages().iter().all(|message| {
        message["method"] != "lithe/build/model" && message["method"] != "lithe/function/invoke"
    }));
}

/// A model request carries the declared shared-input snapshot to the provider verbatim.
#[test]
fn model_inputs_reach_the_provider_verbatim() {
    let mut harness = negotiated_build_provider();
    let params = json!({"modelInputs":{"version":0,"revision":7,"inputs":[]}});
    let operation = build_request(&harness, "lithe/build/model", params.clone(), None).unwrap();
    let id = harness
        .server
        .await_request("lithe/build/model")
        .expect("the provider receives its snapshot");
    assert_eq!(harness.written_request(&id)["params"], params);
    harness
        .server
        .send(json!({"jsonrpc":"2.0","id":id,"result":{"projects":[]}}));
    harness.await_event(|event| {
        event.operation_id.as_deref() == Some(&operation) && event.kind == "requestCompleted"
    });
}

#[test]
fn a_negotiated_build_request_forwards_the_plan_and_returns_the_reply() {
    let mut harness = negotiated_build_provider();
    let params = json!({
        "buildSystemID": "make",
        "root": "",
        "options": { "offline": true },
        "optionArguments": ["--offline"],
        "environment": {}
    });
    let operation =
        build_request(&harness, "lithe/build/model", params.clone(), None).expect("accepted");
    let request_id = harness
        .server
        .await_request("lithe/build/model")
        .expect("the request should reach the provider");
    assert_eq!(harness.written_request(&request_id)["params"], params);

    // A reply with a `locations` field proves no navigation normalizer
    // rewrites the document Core decodes.
    let reply = json!({ "format": "litheBuildModel", "version": 0, "locations": [] });
    harness
        .server
        .send(json!({ "jsonrpc": "2.0", "id": request_id, "result": reply }));
    let completed = harness
        .await_event(|event| {
            event.kind == "requestCompleted"
                && event.operation_id.as_deref() == Some(operation.as_str())
        })
        .clone();
    assert!(completed.error.is_none());
    assert_eq!(completed.result, Some(json!({ "value": reply })));
    assert!(harness.snapshot().pending_operation_ids.is_empty());
}

/// Only the negotiated methods pass, so the build path cannot become a
/// generic JSON-RPC tunnel into a language server.
#[test]
fn build_requests_accept_only_the_protocol_methods() {
    let harness = negotiated_build_provider();
    for method in ["workspace/executeCommand", "shutdown"] {
        let error = build_request(&harness, method, json!({}), None)
            .expect_err("only protocol methods are build requests");
        assert!(matches!(error.code, ErrorCode::InvalidRequest), "{method}");
    }
    assert!(harness
        .server
        .messages()
        .iter()
        .all(|message| message["method"] != "workspace/executeCommand"));
}

/// A build request's own deadline replaces the session's request timeout,
/// here shortened, and the timed-out request is cancelled on the wire.
#[test]
fn a_build_request_times_out_on_its_own_deadline() {
    let mut harness = negotiated_build_provider();
    let operation =
        build_request(&harness, "lithe/build/dependencies", json!({}), Some(30)).expect("accepted");
    harness
        .server
        .await_request("lithe/build/dependencies")
        .expect("the request should reach the provider");
    let timeout = harness
        .await_event(|event| event.operation_id.as_deref() == Some(operation.as_str()))
        .clone();
    assert_eq!(
        timeout.error.as_ref().map(|error| error.code.as_str()),
        Some("requestTimeout")
    );
    assert!(harness.server.await_notification("$/cancelRequest"));
}

#[test]
fn keymap_import_negotiation_is_available_only_to_plugin_owned_sessions() {
    for plugin_owned in [false, true] {
        let mut harness = Harness::start(|request| {
            request.keymap_import_providers = vec!["fixture".to_string()];
            if plugin_owned {
                request.plugin_id = Some("dev.lithe.fixture.import".to_string());
            }
        });
        harness.server.complete_initialize(json!({"experimental": {
            "litheKeymapImport": {"version": 0, "providers": ["fixture"]}
        }}));
        harness.await_state(LspLifecycleState::Ready);
        let initialize = harness
            .server
            .messages()
            .into_iter()
            .find(|message| message["method"] == "initialize")
            .unwrap();
        let offer = initialize["params"]["capabilities"]["experimental"].get("litheKeymapImport");
        assert_eq!(offer.is_some(), plugin_owned);
        if plugin_owned {
            assert_eq!(
                offer,
                Some(&json!({"versions": [0], "providers": ["fixture"]}))
            );
        }
        let event = harness
            .events
            .iter()
            .find(|event| event.kind == "keymapImportProviders");
        assert_eq!(event.is_some(), plugin_owned);
        if let Some(event) = event {
            assert_eq!(event.result, Some(json!({"providers": ["fixture"]})));
        }
    }
}

#[test]
fn keymap_import_provider_cannot_select_an_undeclared_identity() {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some("dev.lithe.fixture.import".to_string());
        request.keymap_import_providers = vec!["fixture".to_string()];
    });
    harness.server.complete_initialize(json!({"experimental": {
        "litheKeymapImport": {"version": 0, "providers": ["fixture", "other"]}
    }}));
    harness.await_state(LspLifecycleState::Ready);
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "keymapImportProviders"));
}

#[test]
fn keymap_import_request_only_reaches_its_negotiated_provider() {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some("dev.lithe.fixture.import".to_string());
        request.keymap_import_providers = vec!["fixture".to_string()];
    });
    harness.server.complete_initialize(json!({"experimental": {
        "litheKeymapImport": {"version": 0, "providers": ["fixture"]}
    }}));
    harness.await_state(LspLifecycleState::Ready);
    let request = |provider: &str| KeymapImportProviderRequest {
        session_id: harness.session_id.clone(),
        operation_id: None,
        provider_id: provider.to_string(),
        snapshot_id: "snapshot-1".to_string(),
        timeout_milliseconds: Some(1_000),
    };
    assert!(harness
        .session()
        .keymap_import_request(request("other"), "refused".to_string())
        .is_err());
    assert!(harness
        .server
        .messages()
        .iter()
        .all(|message| message["method"] != "lithe/keymap/import"));
    harness
        .session()
        .keymap_import_request(request("fixture"), "import-1".to_string())
        .unwrap();
    let id = harness.server.await_request("lithe/keymap/import").unwrap();
    let wire_id: u64 = id.parse().unwrap();
    let sent = harness.written_request(&id);
    assert_eq!(
        sent["params"],
        json!({"providerID": "fixture", "snapshotID": "snapshot-1"})
    );
    let response = json!({"jsonrpc": "2.0", "id": wire_id, "result": {"bindings": []}});
    harness.server.send(response.clone());
    let event = harness.await_event(|event| {
        event.kind == "requestCompleted" && event.operation_id.as_deref() == Some("import-1")
    });
    assert_eq!(
        event.result,
        Some(json!({"value": {"id": wire_id, "response": response}}))
    );
}

#[test]
fn keymap_import_request_rejects_paths_and_can_be_cancelled() {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some("dev.lithe.fixture.import".to_string());
        request.keymap_import_providers = vec!["fixture".to_string()];
    });
    harness.server.complete_initialize(json!({"experimental": {
        "litheKeymapImport": {"version": 0, "providers": ["fixture"]}
    }}));
    harness.await_state(LspLifecycleState::Ready);
    let mut request = KeymapImportProviderRequest {
        session_id: harness.session_id.clone(),
        operation_id: None,
        provider_id: "fixture".to_string(),
        snapshot_id: "../private".to_string(),
        timeout_milliseconds: None,
    };
    assert!(harness
        .session()
        .keymap_import_request(request.clone(), "invalid".to_string())
        .is_err());
    assert!(harness
        .server
        .messages()
        .iter()
        .all(|message| message["method"] != "lithe/keymap/import"));
    request.snapshot_id = "snapshot-1".to_string();
    harness
        .session()
        .keymap_import_request(request, "import-2".to_string())
        .unwrap();
    harness.server.await_request("lithe/keymap/import").unwrap();
    harness.session().cancel_operation("import-2").unwrap();
    assert!(harness.server.await_notification("$/cancelRequest"));
    let event = harness.await_event(|event| {
        event.kind == "requestCompleted" && event.operation_id.as_deref() == Some("import-2")
    });
    assert_eq!(
        event.error.as_ref().map(|error| error.code.as_str()),
        Some("requestCancelled")
    );
    assert!(event.result.is_none());
}

#[test]
fn keymap_import_request_rejects_oversized_provider_replies() {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some("dev.lithe.fixture.import".to_string());
        request.keymap_import_providers = vec!["fixture".to_string()];
    });
    harness.server.complete_initialize(json!({"experimental": {
        "litheKeymapImport": {"version": 0, "providers": ["fixture"]}
    }}));
    harness.await_state(LspLifecycleState::Ready);
    let request = KeymapImportProviderRequest {
        session_id: harness.session_id.clone(),
        operation_id: None,
        provider_id: "fixture".to_string(),
        snapshot_id: "snapshot-1".to_string(),
        timeout_milliseconds: Some(5_000),
    };
    harness
        .session()
        .keymap_import_request(request, "import-3".to_string())
        .unwrap();
    let id = harness.server.await_request("lithe/keymap/import").unwrap();
    harness
        .server
        .send(json!({"jsonrpc": "2.0", "id": id.parse::<u64>().unwrap(),
        "result": {"data": "x".repeat(lithe_lsp_client::keymap_provider::MAX_REPLY_BYTES)}}));
    let event = harness.await_event(|event| {
        event.kind == "requestCompleted" && event.operation_id.as_deref() == Some("import-3")
    });
    assert_eq!(
        event.error.as_ref().map(|error| error.code.as_str()),
        Some("plugin.keymapImport.replyTooLarge")
    );
    assert!(event.result.is_none());
}

// Toolchain providers against a scripted backend. Only a plugin-owned
// session launched with declared providers is offered `litheToolchains`,
// and only a provider its process selected receives `toolchains/query`.

fn toolchain_backend(answer: Value) -> Harness {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some("dev.lithe.fixture.toolchain-provider".to_string());
        request.toolchain_providers = vec!["mise".to_string()];
    });
    harness
        .server
        .complete_initialize(json!({ "experimental": { "litheToolchains": answer } }));
    harness.await_state(LspLifecycleState::Ready);
    harness
}

fn toolchain_query(harness: &Harness, provider: &str) -> Result<String, CoreError> {
    let operation_id = harness.engine.next_operation_id();
    harness
        .session()
        .toolchain_request(
            ToolchainProviderRequest {
                session_id: harness.session_id.clone(),
                operation_id: Some(operation_id.clone()),
                params: json!({ "operationID": "query-1", "generation": 1, "providerID": provider }),
                timeout_milliseconds: None,
            },
            operation_id.clone(),
        )
        .map(|()| operation_id)
}

fn no_toolchain_query_written(harness: &Harness) -> bool {
    harness
        .server
        .messages()
        .iter()
        .all(|message| message["method"] != "toolchains/query")
}

#[test]
fn host_sessions_are_not_offered_the_toolchain_protocol() {
    let harness = Harness::start(|request| {
        request.toolchain_providers = vec!["mise".to_string()];
    });
    harness.server.complete_initialize(json!({}));
    let initialize = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "initialize")
        .expect("initialize should have been written");
    assert!(initialize["params"]["capabilities"]["experimental"]
        .get("litheToolchains")
        .is_none());
}

#[test]
fn a_query_reaches_only_a_selected_provider_and_returns_the_raw_response() {
    let mut harness = toolchain_backend(json!({ "version": 0, "providers": ["mise"] }));
    let initialize = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "initialize")
        .expect("initialize should have been written");
    assert_eq!(
        initialize["params"]["capabilities"]["experimental"]["litheToolchains"],
        json!({ "versions": [0], "providers": ["mise"] })
    );
    let event = harness
        .await_event(|event| event.kind == "toolchainProviders")
        .clone();
    assert_eq!(event.result, Some(json!({ "providers": ["mise"] })));

    let refused = toolchain_query(&harness, "asdf").expect_err("not selected");
    assert_eq!(
        refused.details.as_deref(),
        Some("plugin.toolchain.providerUnavailable")
    );
    assert!(no_toolchain_query_written(&harness));

    let operation = toolchain_query(&harness, "mise").expect("written");
    let id = harness
        .server
        .await_request("toolchains/query")
        .expect("the query is written");
    let wire_id: u64 = id.parse().expect("a numeric wire ID");
    // Even a provider error stays a raw reply: decodeReply owns its mapping.
    let response = json!({
        "jsonrpc": "2.0", "id": wire_id,
        "error": { "code": -32073, "message": "untrusted", "data": { "code": "x" } }
    });
    harness.server.send(response.clone());
    let event = harness
        .await_event(|event| {
            event.kind == "requestCompleted"
                && event.operation_id.as_deref() == Some(operation.as_str())
        })
        .clone();
    assert!(event.error.is_none());
    assert_eq!(
        event.result,
        Some(json!({ "value": { "id": wire_id, "response": response } }))
    );
}

#[test]
fn an_undeclared_toolchain_provider_enables_nothing() {
    let mut harness = toolchain_backend(json!({ "version": 0, "providers": ["mise", "asdf"] }));
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "toolchainProviders"));
    let error = toolchain_query(&harness, "mise").expect_err("refused");
    assert!(matches!(error.code, ErrorCode::NotSupported));
    assert!(no_toolchain_query_written(&harness));
}

// VCS providers against a scripted backend. Only a plugin-owned session
// is offered `litheVcs`, and only one whose process answered with a VCS
// ID and every required capability receives requests, each planned by
// Core from an operation name.

const VCS_PLUGIN: &str = "dev.lithe.fixture.vcs-provider";

fn vcs_capabilities() -> Value {
    json!({ "status": 0, "baseContent": 0, "checkin": 0, "rollback": 0, "update": 0 })
}

fn vcs_backend(answer: Value) -> Harness {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some(VCS_PLUGIN.to_string());
    });
    harness
        .server
        .complete_initialize(json!({ "experimental": { "litheVcs": answer } }));
    harness.await_state(LspLifecycleState::Ready);
    harness
}

fn negotiated_vcs_provider() -> Harness {
    vcs_backend(json!({ "version": 0, "vcs": "svn", "capabilities": vcs_capabilities() }))
}

/// A native absolute path on every platform the tests run on.
fn working_copy() -> String {
    std::env::temp_dir()
        .join("lithe-vcs-working-copy")
        .to_string_lossy()
        .into_owned()
}

fn vcs_request(
    harness: &Harness,
    vcs: &str,
    operation: &str,
    arguments: Value,
) -> Result<String, CoreError> {
    let operation_id = harness.engine.next_operation_id();
    harness
        .session()
        .vcs_request(
            VcsProviderRequest {
                session_id: harness.session_id.clone(),
                operation_id: Some(operation_id.clone()),
                vcs: vcs.to_string(),
                operation: operation.to_string(),
                root: working_copy(),
                tool: None,
                arguments,
                timeout_milliseconds: None,
            },
            operation_id.clone(),
        )
        .map(|()| operation_id)
}

/// Feature names the session recorded from the provider's answer.
fn vcs_features(harness: &Harness) -> Vec<String> {
    let session = harness.session();
    let state = session.lock_state().expect("state");
    state.client.server_capabilities.clone()
}

fn no_vcs_request_written(harness: &Harness) -> bool {
    harness.server.messages().iter().all(|message| {
        !message["method"]
            .as_str()
            .is_some_and(|method| method.starts_with("lithe/vcs/"))
    })
}

#[test]
fn host_sessions_are_not_offered_the_vcs_protocol() {
    let harness = Harness::ready();
    let initialize = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "initialize")
        .expect("initialize should have been written");
    assert!(initialize["params"]["capabilities"]["experimental"]
        .get("litheVcs")
        .is_none());
    let error = vcs_request(&harness, "svn", "status", json!({}))
        .expect_err("a host session is never a provider");
    assert!(matches!(error.code, ErrorCode::NotSupported));
    assert_eq!(error.details.as_deref(), Some("vcs.provider.unavailable"));
    assert!(no_vcs_request_written(&harness));
}

/// The negotiated VCS and capabilities reach the host as one event, so
/// it can check the VCS against the manifest and pick the actions to show.
#[test]
fn a_negotiated_provider_is_announced_with_its_capabilities() {
    let mut harness = negotiated_vcs_provider();
    let event = harness
        .await_event(|event| event.kind == "vcsProvider")
        .clone();
    assert_eq!(
        event.result,
        Some(json!({ "vcs": "svn", "capabilities": vcs_capabilities() }))
    );
    assert!(vcs_features(&harness)
        .iter()
        .any(|feature| feature == "litheVcs"));
}

/// The host has no side for credentials or console routing: an
/// authentication request is refused so the provider fails with
/// `vcs.authentication.failed`, and an execution report is passed on as a
/// session log entry.
#[test]
fn provider_authentication_is_refused_and_execution_reports_pass_through() {
    let mut harness = negotiated_vcs_provider();
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "auth-1",
        "method": "lithe/vcs/authenticate",
        "params": { "realm": "<https://svn.example.invalid:443> Example" }
    }));
    assert_eq!(await_reply(&harness, "auth-1")["error"]["code"], -32601);
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "method": "lithe/vcs/execution",
        "params": { "kind": "started", "arguments": ["status", "--xml"] }
    }));
    let event = harness
        .await_event(|event| {
            event.kind == "log" && event.message.as_deref() == Some("lithe/vcs/execution")
        })
        .clone();
    assert!(event
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("--xml")));
}

/// An answer without a required capability makes the process a plain
/// plugin session: no announcement, no feature, and no request.
#[test]
fn an_incomplete_vcs_answer_is_not_a_provider() {
    let mut harness = vcs_backend(json!({
        "version": 0, "vcs": "svn", "capabilities": { "status": 0, "baseContent": 0 }
    }));
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "vcsProvider"));
    assert!(vcs_features(&harness)
        .iter()
        .all(|feature| feature != "litheVcs"));
    let error = vcs_request(&harness, "svn", "status", json!({})).expect_err("refused");
    assert_eq!(error.details.as_deref(), Some("vcs.provider.unavailable"));
    assert!(no_vcs_request_written(&harness));
}

#[test]
fn a_vcs_request_is_planned_from_its_operation_and_returns_the_reply() {
    let mut harness = negotiated_vcs_provider();
    let operation = vcs_request(
        &harness,
        "svn",
        "checkin",
        json!({ "paths": ["src/b.txt", "src/a.txt"], "message": "Fix" }),
    )
    .expect("accepted");
    let request_id = harness
        .server
        .await_request("lithe/vcs/checkin")
        .expect("the request should reach the provider");
    assert_eq!(
        harness.written_request(&request_id)["params"],
        json!({
            "paths": ["src/a.txt", "src/b.txt"],
            "message": "Fix",
            "root": { "path": working_copy() },
            "tool": null
        })
    );
    let reply = json!({ "revision": { "id": "12", "display": "r12", "kind": "number" } });
    harness
        .server
        .send(json!({ "jsonrpc": "2.0", "id": request_id, "result": reply }));
    let completed = harness
        .await_event(|event| {
            event.kind == "requestCompleted"
                && event.operation_id.as_deref() == Some(operation.as_str())
        })
        .clone();
    assert!(completed.error.is_none(), "{:?}", completed.error);
    assert_eq!(completed.result, Some(json!({ "value": reply })));
    assert!(harness.snapshot().pending_operation_ids.is_empty());
}

/// The host names an operation, never a method, so the session cannot
/// be used as a tunnel; another VCS, an unknown operation, a capability
/// the provider lacks, or a path leaving the root never reach the wire.
#[test]
fn vcs_requests_are_refused_before_they_are_written() {
    let harness = negotiated_vcs_provider();
    let error = vcs_request(&harness, "hg", "status", json!({})).expect_err("wrong VCS");
    assert_eq!(error.details.as_deref(), Some("vcs.provider.unavailable"));
    let error = vcs_request(&harness, "svn", "shutdown", json!({})).expect_err("no such operation");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    let error = vcs_request(&harness, "svn", "annotate", json!({ "path": "a.txt" }))
        .expect_err("not negotiated");
    assert!(matches!(error.code, ErrorCode::NotSupported));
    assert_eq!(error.details.as_deref(), Some("vcs.capability.unsupported"));
    let error = vcs_request(&harness, "svn", "rollback", json!({ "paths": ["../x"] }))
        .expect_err("outside the root");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    assert!(no_vcs_request_written(&harness));
}

/// A provider may name one of the contract's failure codes; any other
/// code, including one of Lithe's own, becomes the generic failure.
#[test]
fn vcs_failures_keep_only_contract_codes() {
    let mut harness = negotiated_vcs_provider();
    for (index, (data, expected)) in [
        (json!({ "code": "vcs.outOfDate" }), "vcs.outOfDate"),
        (
            json!({ "code": "plugin.vcs.invalidOutput" }),
            "vcs.provider.failed",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let operation = vcs_request(&harness, "svn", "update", json!({})).expect("sent");
        let request_id = harness
            .server
            .await_request_at("lithe/vcs/update", index)
            .expect("the request should reach the provider");
        harness.server.send(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": { "code": -32000, "message": "Working copy is out of date.", "data": data }
        }));
        let error = harness
            .await_event(|event| event.operation_id.as_deref() == Some(operation.as_str()))
            .error
            .clone()
            .expect("an error reply fails the request");
        assert_eq!(error.code, expected);
        assert_eq!(error.message, "Working copy is out of date.");
    }
}

// Plugin functions against a scripted backend. The scripted server plays
// the plugin process, so every outcome (result, failure, cancellation,
// deadline, late reply, revocation, and compatibility with a backend that
// only speaks the build protocol) is driven at a chosen moment.

const FUNCTION_PLUGIN: &str = "dev.lithe.fixture.build";

/// A session owned by `FUNCTION_PLUGIN` whose backend answered
/// `initialize` with `experimental`.
fn plugin_backend(experimental: Value) -> Harness {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some(FUNCTION_PLUGIN.to_string());
    });
    harness
        .server
        .complete_initialize(json!({ "experimental": experimental }));
    harness.await_state(LspLifecycleState::Ready);
    harness
}

fn function_backend() -> Harness {
    plugin_backend(json!({
        "litheBuild": { "version": 0 },
        "litheFunctions": { "version": 0, "functions": [{ "id": "snapshot.refresh" }] }
    }))
}

fn invoke(
    harness: &Harness,
    plugin_id: &str,
    function_id: &str,
    timeout_milliseconds: Option<u64>,
) -> Result<String, CoreError> {
    let operation_id = harness.engine.next_operation_id();
    invoke_with_operation(
        harness,
        plugin_id,
        function_id,
        timeout_milliseconds,
        operation_id,
    )
}

/// Starts a call using a host-selected ID so regressions can exercise ID reuse.
fn invoke_with_operation(
    harness: &Harness,
    plugin_id: &str,
    function_id: &str,
    timeout_milliseconds: Option<u64>,
    operation_id: String,
) -> Result<String, CoreError> {
    harness
        .session()
        .function_request(
            PluginFunctionRequest {
                session_id: harness.session_id.clone(),
                operation_id: Some(operation_id.clone()),
                function: lithe_plugin_rpc::FunctionReference {
                    plugin_id: plugin_id.to_string(),
                    function_id: function_id.to_string(),
                },
                params: json!({ "project": "app" }),
                timeout_milliseconds,
                context: None,
            },
            operation_id.clone(),
            None,
        )
        .map(|()| operation_id)
}

/// The engine's reply to a request the scripted backend sent, waited for
/// with a bounded deadline because the reply is written by the reader thread.
fn await_reply(harness: &Harness, id: &str) -> Value {
    harness
        .server
        .await_response(id)
        .unwrap_or_else(|| panic!("the engine never answered backend request {id}"))
}

fn plugin_configured() -> Harness {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some("dev.lithe.go".to_string());
        request.workspace_configuration = Some(json!({
            "gopls": { "ui": { "staticcheck": true } },
            "go.dotted": 7
        }));
    });
    harness.server.complete_initialize(ready_capabilities());
    harness.await_state(LspLifecycleState::Ready);
    harness
}

#[test]
fn plugin_configuration_answers_sections_and_announces_itself() {
    let harness = plugin_configured();
    assert!(harness
        .server
        .await_notification("workspace/didChangeConfiguration"));
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "cfg-1",
        "method": "workspace/configuration",
        "params": { "items": [
            { "section": "gopls" },
            { "section": "gopls.ui.staticcheck" },
            { "section": "go.dotted" },
            { "section": "missing.path" }
        ] }
    }));
    let reply = await_reply(&harness, "cfg-1");
    assert_eq!(
        reply["result"],
        json!([{ "ui": { "staticcheck": true } }, true, 7, null])
    );
}

#[test]
fn plugin_configuration_updates_reach_a_running_server() {
    let harness = plugin_configured();
    let response = harness
        .session()
        .update_plugin_configuration(json!({ "gopls": { "ui": { "staticcheck": false } } }))
        .expect("the update should be accepted");
    assert!(response.sent);
    let change = harness
        .server
        .await_notification_at("workspace/didChangeConfiguration", 1)
        .expect("the configuration change was never sent");
    assert_eq!(
        change["params"]["settings"]["gopls"]["ui"]["staticcheck"],
        json!(false)
    );
}

#[test]
fn host_servers_reject_plugin_configuration() {
    let harness = Harness::ready();
    assert!(harness
        .session()
        .update_plugin_configuration(json!({}))
        .is_err());
    assert!(checked_workspace_configuration(Some(json!([1]))).is_err());
}

fn completions(harness: &mut Harness, operation: &str) -> Vec<LspRuntimeEvent> {
    harness.poll();
    harness
        .events
        .iter()
        .filter(|event| {
            event.kind == "requestCompleted" && event.operation_id.as_deref() == Some(operation)
        })
        .cloned()
        .collect()
}

#[test]
fn a_plugin_function_is_routed_to_its_plugin_and_returns_its_result() {
    let mut harness = function_backend();
    let initialize = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "initialize")
        .expect("initialize should have been written");
    assert_eq!(
        initialize["params"]["capabilities"]["experimental"],
        json!({
            "litheBuild": { "versions": [0] },
            "litheFunctions": { "versions": [0] },
            "litheLogging": { "versions": [0] },
            "litheVcs": { "versions": [0] }
        })
    );

    // Another plugin's reference never reaches this process.
    let error = invoke(
        &harness,
        "dev.lithe.fixture.other-build",
        "snapshot.refresh",
        None,
    )
    .expect_err("a foreign plugin is refused");
    assert!(matches!(error.code, ErrorCode::PermissionDenied));
    assert_eq!(
        error.details.as_deref(),
        Some("plugin.function.wrongPlugin")
    );
    let error = invoke(&harness, FUNCTION_PLUGIN, "snapshot.clear", None)
        .expect_err("an unregistered function is refused");
    assert_eq!(error.details.as_deref(), Some("plugin.function.unknown"));
    assert!(harness
        .server
        .messages()
        .iter()
        .all(|message| message["method"] != "lithe/function/invoke"));

    let operation = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None).expect("sent");
    let request_id = harness
        .server
        .await_request("lithe/function/invoke")
        .expect("the call should reach the backend");
    let wire_operation = harness.written_request(&request_id)["params"]["operationID"].clone();
    assert!(wire_operation.as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(
        harness.written_request(&request_id)["params"],
        json!({
            "functionID": "snapshot.refresh",
            "operationID": wire_operation,
            "params": { "project": "app" }
        })
    );
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": { "snapshot": 3, "locations": [] }
    }));
    let completed = harness
        .await_event(|event| {
            event.kind == "requestCompleted"
                && event.operation_id.as_deref() == Some(operation.as_str())
        })
        .clone();
    assert!(completed.error.is_none());
    assert_eq!(
        completed.result,
        Some(json!({ "value": { "snapshot": 3, "locations": [] } }))
    );
    assert!(harness.snapshot().pending_operation_ids.is_empty());
}

#[test]
fn a_failing_plugin_function_reports_the_stable_and_the_plugin_code() {
    let mut harness = function_backend();
    let operation = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None).expect("sent");
    let request_id = harness
        .server
        .await_request("lithe/function/invoke")
        .expect("the call should reach the backend");
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "error": { "code": -32000, "message": "Snapshot is locked.", "data": { "code": "build.snapshotLocked" } }
    }));
    let error = harness
        .await_event(|event| event.operation_id.as_deref() == Some(operation.as_str()))
        .error
        .clone()
        .expect("an error reply fails the call");
    assert_eq!(error.code, "plugin.function.failed");
    assert_eq!(error.message, "Snapshot is locked.");
    assert_eq!(
        error.plugin_error_code.as_deref(),
        Some("build.snapshotLocked")
    );
}

/// Cancellation reaches the backend, ends the call once, and neither a
/// late reply nor a late progress report revives it.
#[test]
fn a_cancelled_plugin_function_ignores_its_late_reply_and_progress() {
    let mut harness = function_backend();
    let operation = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None).expect("sent");
    let request_id = harness
        .server
        .await_request("lithe/function/invoke")
        .expect("the call should reach the backend");
    let wire_operation = harness.written_request(&request_id)["params"]["operationID"].clone();
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "method": "lithe/function/progress",
        "params": { "operationID": wire_operation, "message": "Reading snapshot", "percentage": 40 }
    }));
    let progress = harness
        .await_event(|event| event.kind == "requestProgress")
        .clone();
    assert_eq!(progress.operation_id.as_deref(), Some(operation.as_str()));
    assert_eq!(
        progress.result,
        Some(json!({ "operationID": operation, "message": "Reading snapshot", "percentage": 40 }))
    );

    harness
        .session()
        .cancel_operation(&operation)
        .expect("a running call can be cancelled");
    assert!(harness.server.await_notification("$/cancelRequest"));
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "method": "lithe/function/progress",
        "params": { "operationID": wire_operation, "percentage": 90 }
    }));
    harness
        .server
        .send(json!({ "jsonrpc": "2.0", "id": request_id, "result": "too late" }));
    // A request the backend sends after the late messages is answered only
    // once the reader has processed them, so its reply orders the check.
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "fence",
        "method": "workspace/configuration",
        "params": { "items": [] }
    }));
    await_reply(&harness, "fence");

    let ended = completions(&mut harness, &operation);
    assert_eq!(
        ended.len(),
        1,
        "a late reply must not complete the call again"
    );
    assert_eq!(
        ended[0].error.as_ref().map(|error| error.code.as_str()),
        Some("requestCancelled")
    );
    assert_eq!(
        harness
            .events
            .iter()
            .filter(|event| event.kind == "requestProgress")
            .count(),
        1,
        "progress after cancellation must be dropped"
    );
}

#[test]
fn a_plugin_function_times_out_on_its_own_deadline() {
    let mut harness = function_backend();
    let operation =
        invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", Some(60_000)).expect("sent");
    harness
        .server
        .await_request("lithe/function/invoke")
        .expect("the call should reach the backend");
    harness
        .session()
        .expire_deadlines_at(Instant::now() + Duration::from_secs(30));
    assert!(completions(&mut harness, &operation).is_empty());
    assert_eq!(
        harness.snapshot().pending_operation_ids,
        vec![operation.clone()]
    );
    harness
        .session()
        .expire_deadlines_at(Instant::now() + Duration::from_secs(61));
    let timeout = harness
        .await_event(|event| event.operation_id.as_deref() == Some(operation.as_str()))
        .clone();
    assert_eq!(
        timeout.error.as_ref().map(|error| error.code.as_str()),
        Some("requestTimeout")
    );
    assert!(harness.server.await_notification("$/cancelRequest"));
}

/// Registration after `initialize` is all or nothing, and withdrawing a
/// function ends its running calls instead of letting them finish later.
#[test]
fn unregistering_a_running_function_revokes_the_call() {
    let mut harness = function_backend();
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "register-bad",
        "method": "lithe/function/register",
        "params": { "functions": [{ "id": "snapshot.clear" }, { "id": "snapshot.refresh" }] }
    }));
    let refused = await_reply(&harness, "register-bad");
    assert_eq!(
        refused["error"]["data"]["code"],
        "plugin.function.invalidRegistration"
    );

    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "register",
        "method": "lithe/function/register",
        "params": { "functions": [{ "id": "snapshot.clear" }] }
    }));
    assert_eq!(await_reply(&harness, "register")["result"], Value::Null);
    let changed = harness
        .await_event(|event| {
            event.kind == "functionsChanged"
                && event.result.as_ref().is_some_and(|result| {
                    result["functions"] == json!(["snapshot.clear", "snapshot.refresh"])
                })
        })
        .clone();
    assert_eq!(
        changed.result,
        Some(json!({ "functions": ["snapshot.clear", "snapshot.refresh"], "removed": [] }))
    );

    let operation = invoke(&harness, FUNCTION_PLUGIN, "snapshot.clear", None).expect("sent");
    let request_id = harness
        .server
        .await_request("lithe/function/invoke")
        .expect("the call should reach the backend");
    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "unregister",
        "method": "lithe/function/unregister",
        "params": { "ids": ["snapshot.clear"] }
    }));
    assert_eq!(await_reply(&harness, "unregister")["result"], Value::Null);
    let revoked = harness
        .await_event(|event| event.operation_id.as_deref() == Some(operation.as_str()))
        .clone();
    assert_eq!(
        revoked.error.as_ref().map(|error| error.code.as_str()),
        Some("plugin.function.revoked")
    );
    // The revocation is published before the cancellation is written, so
    // the write is awaited rather than expected to be there already.
    let cancel = harness
        .server
        .await_notification_at("$/cancelRequest", 0)
        .expect("the revoked call should be cancelled on the wire");
    assert_eq!(cancel["params"]["id"], request_id);
    let error = invoke(&harness, FUNCTION_PLUGIN, "snapshot.clear", None)
        .expect_err("a withdrawn function is unknown");
    assert_eq!(error.details.as_deref(), Some("plugin.function.unknown"));
    assert!(harness.snapshot().pending_operation_ids.is_empty());
}

/// A backend that speaks only the build protocol keeps its build requests;
/// it receives no function calls and its function messages are not
/// mistaken for registrations.
#[test]
fn a_build_only_backend_keeps_working_without_plugin_functions() {
    let mut harness = plugin_backend(json!({ "litheBuild": { "version": 0 } }));
    let error = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None)
        .expect_err("functions need the negotiated protocol");
    assert!(matches!(error.code, ErrorCode::NotSupported));
    assert_eq!(
        error.details.as_deref(),
        Some("plugin.function.unavailable")
    );

    harness.server.send(json!({
        "jsonrpc": "2.0",
        "id": "register",
        "method": "lithe/function/register",
        "params": { "functions": [{ "id": "snapshot.refresh" }] }
    }));
    assert_eq!(await_reply(&harness, "register")["error"]["code"], -32601);

    let operation = build_request(&harness, "lithe/build/model", json!({}), None)
        .expect("the build protocol is unaffected");
    let request_id = harness
        .server
        .await_request("lithe/build/model")
        .expect("the build request should reach the backend");
    let reply = json!({ "format": "litheBuildModel", "version": 0 });
    harness
        .server
        .send(json!({ "jsonrpc": "2.0", "id": request_id, "result": reply }));
    let completed = harness
        .await_event(|event| event.operation_id.as_deref() == Some(operation.as_str()))
        .clone();
    assert_eq!(completed.result, Some(json!({ "value": reply })));
}

/// Host-owned sessions neither offer nor accept the plugin-function protocol.
#[test]
fn a_host_owned_session_serves_no_plugin_functions() {
    let mut harness = Harness::start(|_| {});
    harness.server.complete_initialize(json!({
        "experimental": { "litheFunctions": { "version": 0, "functions": [{ "id": "snapshot.refresh" }] } }
    }));
    harness.await_state(LspLifecycleState::Ready);
    let initialize = harness
        .server
        .messages()
        .into_iter()
        .find(|message| message["method"] == "initialize")
        .expect("initialize written");
    assert!(initialize["params"]["capabilities"]["experimental"]
        .get("litheFunctions")
        .is_none());
    assert!(!harness
        .session()
        .lock_state()
        .expect("session state")
        .client
        .server_capabilities
        .iter()
        .any(|feature| feature == "litheFunctions"));
    for (id, method, params) in [
        (
            "host-register",
            "lithe/function/register",
            json!({ "functions": [{ "id": "snapshot.clear" }] }),
        ),
        (
            "host-unregister",
            "lithe/function/unregister",
            json!({ "ids": ["snapshot.refresh"] }),
        ),
    ] {
        harness
            .server
            .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        assert_eq!(await_reply(&harness, id)["error"]["code"], -32601);
    }
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "functionsChanged"));
    let error = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None)
        .expect_err("a session without a plugin is refused");
    assert_eq!(
        error.details.as_deref(),
        Some("plugin.function.unavailable")
    );
}

/// Ends the first invocation at a controlled boundary before reusing its host ID.
fn finish_function_for_reuse(harness: &mut Harness, operation: &str, request: &str, ending: &str) {
    match ending {
        "cancel" => harness
            .session()
            .cancel_operation(operation)
            .expect("cancelled"),
        "timeout" => harness
            .session()
            .expire_deadlines_at(Instant::now() + Duration::from_secs(61)),
        "complete" => harness
            .server
            .send(json!({ "jsonrpc": "2.0", "id": request, "result": "first" })),
        "error" => harness.server.send(json!({ "jsonrpc": "2.0", "id": request, "error": { "code": -32000, "message": "first failed" } })),
        "revoke" => {
            harness.server.send(json!({
                "jsonrpc": "2.0", "id": "withdraw-first", "method": "lithe/function/unregister",
                "params": { "ids": ["snapshot.refresh"] }
            }));
            assert_eq!(
                await_reply(harness, "withdraw-first")["result"],
                Value::Null
            );
        }
        _ => panic!("unknown termination scenario"),
    }
    harness.await_event(|event| {
        event.kind == "requestCompleted" && event.operation_id.as_deref() == Some(operation)
    });
    if matches!(ending, "cancel" | "timeout" | "revoke") {
        assert!(harness.server.await_notification("$/cancelRequest"));
    }
    if ending == "revoke" {
        harness.server.send(json!({
            "jsonrpc": "2.0", "id": "restore-first", "method": "lithe/function/register",
            "params": { "functions": [{ "id": "snapshot.refresh" }] }
        }));
        assert_eq!(await_reply(harness, "restore-first")["result"], Value::Null);
    }
}

#[test]
fn plugin_function_initialization_publishes_the_full_registry() {
    for functions in [
        json!([{ "id": "snapshot.refresh" }, { "id": "snapshot.clear" }]),
        json!([]),
    ] {
        let mut harness =
            plugin_backend(json!({ "litheFunctions": { "version": 0, "functions": functions } }));
        let initialized = harness
            .await_event(|event| event.kind == "functionsChanged")
            .clone();
        let mut expected: Vec<_> = functions
            .as_array()
            .expect("descriptors")
            .iter()
            .map(|item| item["id"].as_str().expect("function identifier"))
            .collect();
        expected.sort_unstable();
        assert_eq!(
            initialized.result,
            Some(json!({ "functions": expected, "removed": [] }))
        );
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| event.kind == "functionsChanged")
                .count(),
            1
        );
    }
}

/// An ended call's response and progress cannot become the next call's result,
/// even when the host chooses the same operation ID after every terminal outcome.
#[test]
fn reused_plugin_function_operation_ids_ignore_old_wire_progress() {
    for ending in ["cancel", "timeout", "complete", "error", "revoke"] {
        let mut harness = function_backend();
        harness.server.send(json!({
            "jsonrpc": "2.0", "id": "register-second", "method": "lithe/function/register",
            "params": { "functions": [{ "id": "snapshot.clear" }] }
        }));
        assert_eq!(
            await_reply(&harness, "register-second")["result"],
            Value::Null
        );
        let operation = "reusable-function-operation";
        invoke_with_operation(
            &harness,
            FUNCTION_PLUGIN,
            "snapshot.refresh",
            Some(60_000),
            operation.to_string(),
        )
        .expect("first call");
        let first = harness
            .server
            .await_request("lithe/function/invoke")
            .expect("first wire request");
        let old_wire = harness.written_request(&first)["params"]["operationID"].clone();
        finish_function_for_reuse(&mut harness, operation, &first, ending);
        invoke_with_operation(
            &harness,
            FUNCTION_PLUGIN,
            "snapshot.clear",
            None,
            operation.to_string(),
        )
        .expect("reused host ID");
        let second = harness
            .server
            .await_request_at("lithe/function/invoke", 1)
            .expect("second wire request");
        let current_wire = harness.written_request(&second)["params"]["operationID"].clone();
        assert_ne!(
            old_wire, current_wire,
            "wire identity must change after {ending}"
        );
        harness.server.send(json!({
            "jsonrpc": "2.0", "id": "withdraw-old-function", "method": "lithe/function/unregister",
            "params": { "ids": ["snapshot.refresh"] }
        }));
        assert_eq!(
            await_reply(&harness, "withdraw-old-function")["result"],
            Value::Null
        );
        assert_eq!(
            harness.snapshot().pending_operation_ids,
            vec![operation.to_string()]
        );
        assert!(harness
            .server
            .messages()
            .iter()
            .all(|message| message["method"] != "$/cancelRequest"
                || message["params"]["id"] != second));
        harness
            .server
            .send(json!({ "jsonrpc": "2.0", "id": first, "result": "late" }));
        harness.server.send(json!({ "jsonrpc": "2.0", "method": "lithe/function/progress", "params": { "operationID": old_wire, "message": "stale" } }));
        harness.server.send(json!({ "jsonrpc": "2.0", "method": "lithe/function/progress", "params": { "operationID": current_wire, "message": "current", "percentage": 50 } }));
        let progress = harness
            .await_event(|event| event.kind == "requestProgress")
            .clone();
        assert_eq!(progress.operation_id.as_deref(), Some(operation));
        assert_eq!(
            progress.result,
            Some(json!({ "operationID": operation, "message": "current", "percentage": 50 })),
            "{ending}"
        );
        harness
            .server
            .send(json!({ "jsonrpc": "2.0", "id": second, "result": "second" }));
        harness.await_event(|event| {
            event.kind == "requestCompleted" && event.result == Some(json!({ "value": "second" }))
        });
        assert_eq!(completions(&mut harness, operation).len(), 2, "{ending}");
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| event.kind == "requestProgress")
                .count(),
            1,
            "{ending}"
        );
        assert!(harness.snapshot().pending_operation_ids.is_empty());
    }
}

/// Revocation after a function has ended must never cancel an unrelated
/// build that reused the same host operation ID.
#[test]
fn reused_plugin_function_operation_ids_preserve_build_requests() {
    for ending in ["cancel", "timeout", "complete", "error", "revoke"] {
        let mut harness = function_backend();
        let operation = "reusable-build-operation";
        invoke_with_operation(
            &harness,
            FUNCTION_PLUGIN,
            "snapshot.refresh",
            Some(60_000),
            operation.to_string(),
        )
        .expect("first call");
        let first = harness
            .server
            .await_request("lithe/function/invoke")
            .expect("function request");
        let old_wire = harness.written_request(&first)["params"]["operationID"].clone();
        finish_function_for_reuse(&mut harness, operation, &first, ending);
        harness
            .session()
            .build_request(
                BuildProviderRequest {
                    session_id: harness.session_id.clone(),
                    operation_id: Some(operation.to_string()),
                    method: "lithe/build/model".to_string(),
                    params: json!({}),
                    timeout_milliseconds: None,
                },
                operation.to_string(),
            )
            .expect("build may reuse host operation ID");
        let build = harness
            .server
            .await_request("lithe/build/model")
            .expect("build request");
        harness
            .server
            .send(json!({ "jsonrpc": "2.0", "id": first, "result": "late" }));
        harness.server.send(json!({ "jsonrpc": "2.0", "method": "lithe/function/progress", "params": { "operationID": old_wire, "message": "stale" } }));
        harness.server.send(json!({
            "jsonrpc": "2.0", "id": "withdraw-during-build", "method": "lithe/function/unregister",
            "params": { "ids": ["snapshot.refresh"] }
        }));
        assert_eq!(
            await_reply(&harness, "withdraw-during-build")["result"],
            Value::Null
        );
        harness.poll();
        assert!(harness
            .events
            .iter()
            .any(|event| event.kind == "functionsChanged"
                && event.result
                    == Some(json!({ "functions": [], "removed": ["snapshot.refresh"] }))));
        assert_eq!(
            harness.snapshot().pending_operation_ids,
            vec![operation.to_string()],
            "{ending}"
        );
        assert!(harness
            .server
            .messages()
            .iter()
            .all(|message| message["method"] != "$/cancelRequest"
                || message["params"]["id"] != build));
        harness
            .server
            .send(json!({ "jsonrpc": "2.0", "id": build, "result": { "version": 1 } }));
        harness.await_event(|event| {
            event.kind == "requestCompleted" && event.method.as_deref() == Some("lithe/build/model")
        });
        let ended = completions(&mut harness, operation);
        assert_eq!(ended.len(), 2, "{ending}");
        assert!(ended[1].error.is_none(), "{ending}: {:?}", ended[1]);
        assert!(
            harness
                .events
                .iter()
                .all(|event| event.kind != "requestProgress"),
            "{ending}"
        );
    }
}

#[test]
fn stopping_a_plugin_function_withdraws_registry_and_ignores_late_messages() {
    let mut harness = function_backend();
    let operation = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None).expect("call");
    let request = harness
        .server
        .await_request("lithe/function/invoke")
        .expect("wire request");
    let wire = harness.written_request(&request)["params"]["operationID"].clone();
    harness.session().stop().expect("shutdown starts");
    let shutdown = harness
        .server
        .await_request("shutdown")
        .expect("shutdown request");
    harness.poll();
    let registry_changes = harness
        .events
        .iter()
        .filter(|event| event.kind == "functionsChanged")
        .count();
    assert_eq!(
        registry_changes, 2,
        "initialize publishes, then stop withdraws"
    );
    harness.server.send(json!({ "jsonrpc": "2.0", "method": "lithe/function/progress", "params": { "operationID": wire, "message": "late" } }));
    harness
        .server
        .send(json!({ "jsonrpc": "2.0", "id": request, "result": "late" }));
    for (id, method, params) in [
        (
            "late-register",
            "lithe/function/register",
            json!({ "functions": [{ "id": "snapshot.late" }] }),
        ),
        (
            "late-unregister",
            "lithe/function/unregister",
            json!({ "ids": ["snapshot.refresh"] }),
        ),
    ] {
        harness
            .server
            .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        // The reply fences every earlier notification before asserting absence.
        assert_eq!(await_reply(&harness, id)["error"]["code"], -32601);
    }
    harness.poll();
    assert!(harness
        .session()
        .lock_state()
        .expect("session state")
        .client
        .plugin_functions
        .is_empty());
    assert_eq!(
        harness
            .events
            .iter()
            .filter(|event| event.kind == "functionsChanged")
            .count(),
        registry_changes
    );
    harness
        .server
        .send(json!({ "jsonrpc": "2.0", "id": shutdown, "result": null }));
    assert!(harness.server.await_notification("exit"));
    harness.server.exit(Some(0));
    harness.await_state(LspLifecycleState::Stopped);
    assert_eq!(completions(&mut harness, &operation).len(), 1);
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "requestProgress"));
    assert!(harness
        .events
        .iter()
        .any(|event| event.kind == "functionsChanged"
            && event.result == Some(json!({ "functions": [], "removed": ["snapshot.refresh"] }))));
}

#[test]
fn a_plugin_function_process_failure_withdraws_registry_and_completes_calls() {
    let mut harness = function_backend();
    let operation = invoke(&harness, FUNCTION_PLUGIN, "snapshot.refresh", None).expect("call");
    harness
        .server
        .await_request("lithe/function/invoke")
        .expect("wire request");
    harness.server.exit(Some(42));
    harness.await_state(LspLifecycleState::Failed);
    let ended = completions(&mut harness, &operation);
    assert_eq!(ended.len(), 1);
    assert!(ended[0].error.is_some());
    assert!(harness.snapshot().pending_operation_ids.is_empty());
    assert!(harness
        .events
        .iter()
        .any(|event| event.kind == "functionsChanged"
            && event.result == Some(json!({ "functions": [], "removed": ["snapshot.refresh"] }))));
}

/// Domain dispatch could route back into the backend that asked, whose
/// handler would wait on itself; a host method the host does not offer is
/// unavailable.
#[test]
fn a_backend_cannot_ask_the_host_to_dispatch_back_into_itself() {
    let harness = function_backend();
    for (id, method, code) in [
        (
            "invoke",
            "lithe/function/invoke",
            "plugin.function.recursiveCall",
        ),
        (
            "model",
            "lithe/build/model",
            "plugin.function.recursiveCall",
        ),
        (
            "process",
            "lithe/host/process.run",
            "plugin.host.unavailable",
        ),
    ] {
        harness.server.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {}
        }));
        let reply = await_reply(&harness, id);
        assert_eq!(reply["error"]["data"]["code"], code, "{method}");
    }
}

fn host_service_backend(host_services: &[&str]) -> Harness {
    let host_services: Vec<String> = host_services.iter().map(ToString::to_string).collect();
    let mut harness = Harness::start(move |request| {
        request.plugin_id = Some(FUNCTION_PLUGIN.to_string());
        request.host_services = host_services.clone();
    });
    harness.server.complete_initialize(json!({
        "experimental": { "litheFunctions": { "version": 0 } }
    }));
    harness.await_state(LspLifecycleState::Ready);
    harness
}

fn host_response(
    harness: &Harness,
    host_request_id: &str,
    result: Option<Value>,
    error: Option<&str>,
) -> Result<(), CoreError> {
    // Decoded from the wire shape `lsp.hostResponse` documents, so the
    // field spelling is checked with the round trip.
    let mut wire = json!({
        "sessionId": harness.session_id,
        "hostRequestID": host_request_id,
    });
    if let Some(result) = result {
        wire["result"] = result;
    }
    if let Some(message) = error {
        wire["error"] = match message.split_once('|') {
            Some((code, message)) => json!({ "message": message, "code": code }),
            None => json!({ "message": message }),
        };
    }
    let response: PluginHostResponse = serde_json::from_value(wire).expect("documented wire shape");
    harness.session().host_response(response)
}

/// A granted credential request reaches the host as `hostRequest`. The
/// backend receives exactly the host's answer under its own ID, and a
/// second answer to the same request is refused instead of reaching a
/// later request.
#[test]
fn a_granted_host_service_request_round_trips_through_the_host() {
    let mut harness = host_service_backend(&["credentials"]);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "read",
        "method": "lithe/host/credentials/get",
        "params": { "key": "connection.prod" }
    }));
    let event = harness
        .await_event(|event| event.kind == "hostRequest")
        .clone();
    assert_eq!(event.method.as_deref(), Some("lithe/host/credentials/get"));
    let request = event.result.expect("the request is described");
    assert_eq!(request["service"], "credentials");
    assert_eq!(request["params"], json!({ "key": "connection.prod" }));
    let read_id = request["hostRequestID"].as_str().unwrap().to_string();
    host_response(
        &harness,
        &read_id,
        Some(json!({ "secret": "s3cret" })),
        None,
    )
    .expect("the host answers once");
    assert_eq!(
        await_reply(&harness, "read")["result"],
        json!({ "secret": "s3cret" })
    );
    assert!(host_response(&harness, &read_id, None, None).is_err());

    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "write",
        "method": "lithe/host/credentials/set",
        "params": { "key": "connection.prod", "secret": "s3cret" }
    }));
    let event = harness
        .await_event(|event| {
            event.kind == "hostRequest"
                && event.method.as_deref() == Some("lithe/host/credentials/set")
        })
        .clone();
    let write_id = event.result.unwrap()["hostRequestID"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(write_id, read_id);
    host_response(&harness, &write_id, None, Some("The keychain is locked."))
        .expect("the host reports its failure");
    let reply = await_reply(&harness, "write");
    assert_eq!(reply["error"]["data"]["code"], "plugin.host.failed");
    assert_eq!(reply["error"]["message"], "The keychain is locked.");
}

/// Host services are offered only for the services the plugin's
/// permissions grant; a refusal never reaches the host.
#[test]
fn host_services_need_a_grant() {
    let mut harness = host_service_backend(&[]);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "read",
        "method": "lithe/host/credentials/get",
        "params": { "key": "connection.prod" }
    }));
    let reply = await_reply(&harness, "read");
    assert_eq!(
        reply["error"]["data"]["code"],
        "plugin.host.permissionDenied"
    );
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "hostRequest"));
}

/// A backend that negotiated no plugin-function protocol is offered no
/// host service and receives no host notification, whatever the plugin's
/// permissions grant.
#[test]
fn a_backend_without_the_function_protocol_gets_no_host_services() {
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some(FUNCTION_PLUGIN.to_string());
        request.host_services = vec!["credentials".to_string(), "workspace".to_string()];
    });
    harness.server.complete_initialize(json!({
        "experimental": { "litheBuild": { "version": 0 } }
    }));
    harness.await_state(LspLifecycleState::Ready);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "read",
        "method": "lithe/host/credentials/get",
        "params": { "key": "connection.prod" }
    }));
    let reply = await_reply(&harness, "read");
    assert_eq!(reply["error"]["data"]["code"], "plugin.host.unavailable");
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "hostRequest"));
    let request: PluginHostNotification = serde_json::from_value(json!({
        "sessionId": harness.session_id,
        "method": "lithe/host/workspace/didChangeContexts",
        "params": { "revision": 2 },
    }))
    .expect("documented wire shape");
    assert!(!harness
        .session()
        .host_notify(request)
        .expect("a known notification"));
}

/// A host that stops answering cannot make a backend grow the session's
/// state: requests beyond the bound fail at once.
#[test]
fn unanswered_host_requests_are_bounded() {
    let harness = host_service_backend(&["credentials"]);
    let limit = lithe_plugin_rpc::MAX_PENDING_HOST_REQUESTS;
    for index in 0..=limit {
        harness.server.send(json!({
            "jsonrpc": "2.0", "id": format!("read-{index}"),
            "method": "lithe/host/credentials/get",
            "params": { "key": "connection.prod" }
        }));
    }
    let reply = await_reply(&harness, &format!("read-{limit}"));
    assert_eq!(reply["error"]["code"], -32014);
    assert_eq!(reply["error"]["data"]["code"], "plugin.host.limitExceeded");
    assert_eq!(
        harness.session().lock_state().unwrap().host_requests.len(),
        limit
    );
}

/// Core answers capability discovery itself, listing only the session's
/// grants; the host never sees the request.
#[test]
fn capability_discovery_is_answered_by_core() {
    let mut harness = host_service_backend(&["workspace"]);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "caps",
        "method": "lithe/host/getCapabilities"
    }));
    let reply = await_reply(&harness, "caps");
    assert_eq!(reply["result"]["protocolVersion"], 0);
    assert_eq!(reply["result"]["backendAuthority"], "local");
    assert!(reply["result"]["services"]["workspace"].is_object());
    assert!(reply["result"]["services"]["credentials"].is_null());
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "hostRequest"));
}

/// Editor options the host declared at start are reported with the
/// methods they belong to, and only when those methods are granted.
#[test]
fn capability_discovery_reports_granted_editor_options() {
    use lithe_plugin_editor::actions::{EditorHostOptions, REGISTER_PROVIDERS};
    let mut harness = Harness::start(|request| {
        request.plugin_id = Some(FUNCTION_PLUGIN.to_string());
        request.host_services = vec![REGISTER_PROVIDERS.to_string()];
        request.host_options.editor = Some(EditorHostOptions {
            action_icons: vec!["run".into(), "test-failed".into()],
            action_source_bytes: 1024 * 1024,
            provider_types: vec!["hover".into(), "codeLens".into()],
        });
    });
    harness.server.complete_initialize(json!({
        "experimental": { "litheFunctions": { "version": 0 } }
    }));
    harness.await_state(LspLifecycleState::Ready);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "caps",
        "method": "lithe/host/getCapabilities"
    }));
    let reply = await_reply(&harness, "caps");
    let editor = &reply["result"]["services"]["editor"];
    assert_eq!(editor["methods"], json!([REGISTER_PROVIDERS]));
    assert_eq!(
        editor["options"],
        json!({ "providerTypes": ["hover", "codeLens"] })
    );
    assert!(reply["result"]["limits"]["editorActionSourceBytes"].is_null());
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "hostRequest"));
}

/// Host options belong to a plugin backend and must be ones Core can
/// enforce.
#[test]
fn start_contract_rejects_unenforceable_host_options() {
    use lithe_plugin_editor::actions::EditorHostOptions;
    let server = ScriptedServer::new();
    let mut request = start_request(&server);
    let options = EditorHostOptions {
        action_icons: vec!["run".into()],
        action_source_bytes: 1024 * 1024,
        provider_types: Vec::new(),
    };
    request.host_options.editor = Some(options.clone());
    request.plugin_id = None;
    assert!(validate_start_request(&request).is_err());
    request.plugin_id = Some(FUNCTION_PLUGIN.to_string());
    assert!(validate_start_request(&request).is_ok());
    for invalid in [
        EditorHostOptions {
            action_source_bytes: 2 * 1024 * 1024,
            ..options.clone()
        },
        EditorHostOptions {
            action_icons: vec!["Run Fast".into()],
            ..options.clone()
        },
        EditorHostOptions {
            provider_types: vec!["diagnostics".into()],
            ..options.clone()
        },
    ] {
        request.host_options.editor = Some(invalid);
        assert!(validate_start_request(&request).is_err());
    }
}

/// A field a method does not define is answered as an unsupported option
/// naming it, so a backend can retry without it; the host never sees the
/// request.
#[test]
fn an_unknown_option_is_named_to_the_backend() {
    let mut harness = host_service_backend(&["workspace"]);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "folders",
        "method": "lithe/host/workspace/getFolders",
        "params": {
            "context": { "contextID": "w-1", "generation": 1 },
            "includeHidden": true
        }
    }));
    let reply = await_reply(&harness, "folders");
    assert_eq!(reply["error"]["code"], -32018);
    assert_eq!(
        reply["error"]["data"]["code"],
        "plugin.host.unsupportedOption"
    );
    assert_eq!(
        reply["error"]["data"]["details"],
        json!({ "option": "includeHidden" })
    );
    harness.poll();
    assert!(harness
        .events
        .iter()
        .all(|event| event.kind != "hostRequest"));
}

/// A host failure's code reaches the backend when it is one a host may
/// report; an unknown code is `plugin.host.failed`.
#[test]
fn typed_host_failures_reach_the_backend() {
    let mut harness = host_service_backend(&["workspace"]);
    for (id, code, expected, rpc_code) in [
        (
            "a",
            "plugin.host.contextExpired",
            "plugin.host.contextExpired",
            -32011,
        ),
        ("b", "plugin.function.unknown", "plugin.host.failed", -32603),
    ] {
        harness.server.send(json!({
            "jsonrpc": "2.0", "id": id,
            "method": "lithe/host/workspace/getFolders",
            "params": { "context": { "contextID": "w1", "generation": 1 } }
        }));
        let event = harness
            .await_event(|event| event.kind == "hostRequest")
            .clone();
        let host_request_id = event.result.unwrap()["hostRequestID"]
            .as_str()
            .unwrap()
            .to_string();
        harness.events.clear();
        host_response(
            &harness,
            &host_request_id,
            None,
            Some(&format!("{code}|The workspace closed.")),
        )
        .expect("the host reports its failure");
        let reply = await_reply(&harness, id);
        assert_eq!(reply["error"]["data"]["code"], expected, "{code}");
        assert_eq!(reply["error"]["code"], rpc_code, "{code}");
    }
}

/// A host failure's details reach the backend under
/// `error.data.details`, next to the stable code.
#[test]
fn host_failure_details_reach_the_backend() {
    let mut harness = host_service_backend(&["workspace"]);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "folders",
        "method": "lithe/host/workspace/getFolders",
        "params": { "context": { "contextID": "w1", "generation": 1 } }
    }));
    let host_request_id = harness
        .await_event(|event| event.kind == "hostRequest")
        .clone()
        .result
        .unwrap()["hostRequestID"]
        .as_str()
        .unwrap()
        .to_string();
    let response: PluginHostResponse = serde_json::from_value(json!({
        "sessionId": harness.session_id,
        "hostRequestID": host_request_id,
        "error": {
            "message": "The context changed.",
            "code": "plugin.host.contextExpired",
            "details": { "generation": 2 }
        }
    }))
    .expect("documented wire shape");
    harness
        .session()
        .host_response(response)
        .expect("the host reports its failure");
    let reply = await_reply(&harness, "folders");
    assert_eq!(reply["error"]["data"]["code"], "plugin.host.contextExpired");
    assert_eq!(
        reply["error"]["data"]["details"],
        json!({ "generation": 2 })
    );
}

/// `lsp.hostNotify` writes an admitted notification to the backend and
/// refuses one the session was not granted.
#[test]
fn host_notifications_reach_the_backend() {
    let harness = host_service_backend(&["workspace"]);
    let notify = |method: &str, params: Value| {
        let request: PluginHostNotification = serde_json::from_value(json!({
            "sessionId": harness.session_id,
            "method": method,
            "params": params,
        }))
        .expect("documented wire shape");
        harness.session().host_notify(request)
    };
    assert!(notify(
        "lithe/host/workspace/didChangeContexts",
        json!({ "revision": 2 })
    )
    .expect("an admitted notification"));
    assert!(harness
        .server
        .await_notification("lithe/host/workspace/didChangeContexts"));
    assert!(notify("lithe/build/run", json!({})).is_err());
}

/// A backend that cancels its own host request is answered at once, the
/// host learns which request to stop, and its late answer is refused.
#[test]
fn a_backend_can_cancel_its_host_request() {
    let mut harness = host_service_backend(&["workspace"]);
    harness.server.send(json!({
        "jsonrpc": "2.0", "id": "list",
        "method": "lithe/host/workspace/listContexts"
    }));
    let host_request_id = harness
        .await_event(|event| event.kind == "hostRequest")
        .clone()
        .result
        .unwrap()["hostRequestID"]
        .as_str()
        .unwrap()
        .to_string();
    harness.server.send(json!({
        "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": "list" }
    }));
    let cancelled = harness
        .await_event(|event| event.kind == "hostRequestCancelled")
        .clone();
    assert_eq!(
        cancelled.result.unwrap()["hostRequestID"],
        json!(host_request_id)
    );
    let reply = await_reply(&harness, "list");
    assert_eq!(reply["error"]["data"]["code"], "plugin.host.cancelled");
    assert!(host_response(&harness, &host_request_id, Some(json!({})), None).is_err());
}

/// A call's workspace context reaches the backend as `params.context`,
/// and a malformed reference never leaves Core.
#[test]
fn invocations_carry_the_workspace_context() {
    let harness = plugin_backend(json!({
        "litheFunctions": { "version": 0, "functions": [{ "id": "snapshot.refresh" }] }
    }));
    let call = |context: Value| {
        let operation_id = harness.engine.next_operation_id();
        harness.session().function_request(
            PluginFunctionRequest {
                session_id: harness.session_id.clone(),
                operation_id: Some(operation_id.clone()),
                function: lithe_plugin_rpc::FunctionReference {
                    plugin_id: FUNCTION_PLUGIN.to_string(),
                    function_id: "snapshot.refresh".to_string(),
                },
                params: json!({}),
                timeout_milliseconds: None,
                context: Some(context),
            },
            operation_id,
            None,
        )
    };
    let error = call(json!({ "contextID": "w/1", "generation": 1 }))
        .expect_err("a malformed context is refused");
    assert_eq!(
        error.details.as_deref(),
        Some("plugin.function.invalidParams")
    );
    call(json!({ "contextID": "w1", "generation": 2 })).expect("sent");
    let request_id = harness
        .server
        .await_request("lithe/function/invoke")
        .expect("the call should reach the backend");
    assert_eq!(
        harness.written_request(&request_id)["params"]["context"],
        json!({ "contextID": "w1", "generation": 2 })
    );
}

/// Requests and notifications of a plugin backend to its own language
/// server (plan API-25). The stub servers are Metals-like (a gate and a
/// command that runs one at a time) and rust-analyzer-like (neither), so
/// no case depends on one language.
mod plugin_owned_server {
    use super::*;

    const PLUGIN: &str = "dev.lithe.metals";
    const SERVER: &str = "metals";
    const STATUS: &str = "metals/status";

    /// The policy of the Metals-like server. `gated` adds the gate that
    /// holds requests until the server reports it is ready.
    fn metals_policy(gated: bool) -> Value {
        let mut policy = json!({
            "serverID": SERVER,
            "requestMethods": [
                "textDocument/definition",
                "workspace/executeCommand",
                "metals/slow",
                "metals/didFocus"
            ],
            "commands": ["build-import", "build-connect"],
            "serializedCommands": ["build-import"],
            "forwardNotifications": [STATUS],
        });
        if gated {
            policy["gates"] = json!([{
                "id": "imported",
                "until": "notification",
                "method": STATUS,
                "match": { "text": "ready" }
            }]);
        }
        policy
    }

    /// A rust-analyzer-like server: no gate, no serialized command.
    fn rust_analyzer_policy() -> Value {
        json!({
            "serverID": SERVER,
            "requestMethods": ["experimental/runnables"],
        })
    }

    fn start(policy: Value) -> Harness {
        let mut harness = Harness::start(move |request| {
            request.plugin_id = Some(PLUGIN.to_string());
            request.session_policy =
                Some(serde_json::from_value(policy).expect("the policy should decode"));
        });
        harness.server.complete_initialize(ready_capabilities());
        harness.await_state(LspLifecycleState::Ready);
        harness
    }

    /// The `lsp.pluginServerRequest` payload the host sends.
    fn wire(harness: &Harness, operation_id: &str, method: &str, params: Value) -> Value {
        json!({
            "sessionId": harness.session_id,
            "operationId": operation_id,
            "pluginId": PLUGIN,
            "serverID": SERVER,
            "method": method,
            "params": params,
        })
    }

    fn send(harness: &Harness, wire: Value) -> Result<(), CoreError> {
        let operation_id = wire["operationId"].as_str().unwrap().to_string();
        let request: PluginServerRequest =
            serde_json::from_value(wire).expect("documented wire shape");
        harness
            .session()
            .plugin_server_request(request, operation_id)
    }

    fn call(
        harness: &Harness,
        operation_id: &str,
        method: &str,
        params: Value,
    ) -> Result<(), CoreError> {
        send(harness, wire(harness, operation_id, method, params))
    }

    fn run_command(harness: &Harness, operation_id: &str, command: &str) -> Result<(), CoreError> {
        call(
            harness,
            operation_id,
            "workspace/executeCommand",
            json!({ "command": command, "arguments": [] }),
        )
    }

    fn notify(harness: &Harness, method: &str, params: Value) -> Result<bool, CoreError> {
        let request: PluginServerNotify = serde_json::from_value(json!({
            "sessionId": harness.session_id,
            "pluginId": PLUGIN,
            "serverID": SERVER,
            "method": method,
            "params": params,
        }))
        .expect("documented wire shape");
        harness.session().plugin_server_notify(request)
    }

    /// The server reports `text` through its status notification.
    fn report_status(harness: &Harness, text: &str) {
        harness.server.send(json!({
            "jsonrpc": "2.0",
            "method": STATUS,
            "params": { "text": text }
        }));
    }

    /// Waits until Core has applied the status notification of `text`. The
    /// notification is forwarded to the backend, so the forwarded event
    /// proves the engine already processed it.
    fn await_status_seen(harness: &mut Harness, text: &str) {
        harness.await_event(|event| {
            event.kind == "pluginServerNotification"
                && event
                    .result
                    .as_ref()
                    .is_some_and(|result| result["params"]["text"] == text)
        });
    }

    fn reply(harness: &Harness, request_id: &str, result: Value) {
        harness.server.send(json!({
            "jsonrpc": "2.0", "id": request_id, "result": result
        }));
    }

    fn completed(harness: &mut Harness, operation_id: &str) -> LspRuntimeEvent {
        harness
            .await_event(|event| {
                event.kind == "requestCompleted"
                    && event.operation_id.as_deref() == Some(operation_id)
            })
            .clone()
    }

    fn error_code(event: &LspRuntimeEvent) -> &str {
        &event.error.as_ref().expect("the request failed").code
    }

    fn written(harness: &Harness, method: &str) -> usize {
        harness
            .server
            .messages()
            .iter()
            .filter(|message| message["method"] == method)
            .count()
    }

    fn refusal_code(error: CoreError) -> String {
        error.details.expect("a refusal names its plugin.host code")
    }

    /// The server's result reaches the backend untouched, even for a
    /// method whose result Lithe normalizes for its own editor.
    #[test]
    fn a_declared_request_returns_the_servers_own_result() {
        let mut harness = start(metals_policy(false));
        call(
            &harness,
            "def-1",
            "textDocument/definition",
            json!({ "textDocument": { "uri": "file:///workspace/A.scala" } }),
        )
        .expect("a declared method is sent");
        let request_id = harness
            .server
            .await_request("textDocument/definition")
            .expect("the request should reach the server");
        let location = json!({
            "uri": "jar:file:///cache/lib.jar!/Lib.scala",
            "range": {
                "start": { "line": 1, "character": 2 },
                "end": { "line": 1, "character": 5 }
            }
        });
        reply(&harness, &request_id, json!([location]));

        let event = completed(&mut harness, "def-1");
        assert_eq!(event.result, Some(json!({ "value": [location] })));
        assert!(event.error.is_none());
    }

    #[test]
    fn a_server_without_gates_or_commands_sends_at_once() {
        let mut harness = start(rust_analyzer_policy());
        call(&harness, "run-1", "experimental/runnables", json!({}))
            .expect("a declared method is sent");
        let request_id = harness
            .server
            .await_request("experimental/runnables")
            .expect("nothing holds the request");
        reply(&harness, &request_id, json!([]));
        assert_eq!(
            completed(&mut harness, "run-1").result,
            Some(json!({ "value": [] }))
        );
    }

    #[test]
    fn a_server_error_keeps_its_code_and_message_as_details() {
        let mut harness = start(metals_policy(false));
        call(&harness, "slow-1", "metals/slow", json!({})).unwrap();
        let request_id = harness.server.await_request("metals/slow").unwrap();
        harness.server.send(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": { "code": -32001, "message": "Build server is not connected." }
        }));

        let event = completed(&mut harness, "slow-1");
        assert_eq!(error_code(&event), "plugin.host.languageServerError");
        assert_eq!(
            event.error.unwrap().details,
            Some(json!({ "code": -32001, "message": "Build server is not connected." }))
        );
    }

    #[test]
    fn a_method_the_plugin_did_not_declare_never_reaches_the_server() {
        let harness = start(metals_policy(false));
        let error = call(&harness, "bad-1", "textDocument/rename", json!({}))
            .expect_err("an undeclared method is refused");
        assert_eq!(refusal_code(error), "plugin.host.methodNotDeclared");

        let error = run_command(&harness, "bad-2", "wipe-workspace")
            .expect_err("an undeclared command is refused");
        assert_eq!(refusal_code(error), "plugin.host.methodNotDeclared");
        assert_eq!(written(&harness, "textDocument/rename"), 0);
        assert_eq!(written(&harness, "workspace/executeCommand"), 0);
    }

    /// The host resolves the session, so a call that names another plugin
    /// or server, or a session without a policy, is the host's mistake and
    /// must not reach any server.
    #[test]
    fn a_session_of_another_contribution_is_refused() {
        let harness = start(metals_policy(false));
        for (plugin_id, server_id) in [("dev.lithe.other", SERVER), (PLUGIN, "other-server")] {
            let mut foreign = wire(&harness, "foreign-1", "metals/slow", json!({}));
            foreign["pluginId"] = json!(plugin_id);
            foreign["serverID"] = json!(server_id);
            let error = send(&harness, foreign).expect_err("a foreign contribution is refused");
            assert_eq!(refusal_code(error), "plugin.host.foreignContribution");
        }
        assert_eq!(written(&harness, "metals/slow"), 0);

        let plain = Harness::ready();
        let error = call(&plain, "plain-1", "metals/slow", json!({}))
            .expect_err("a session without a policy is no plugin server");
        assert_eq!(refusal_code(error), "plugin.host.foreignContribution");
    }

    #[test]
    fn requests_wait_for_the_gate_and_are_sent_in_order() {
        let mut harness = start(metals_policy(true));
        call(&harness, "first", "metals/slow", json!({ "n": 1 })).unwrap();
        call(
            &harness,
            "second",
            "textDocument/definition",
            json!({ "n": 2 }),
        )
        .unwrap();
        assert_eq!(written(&harness, "metals/slow"), 0);

        // A status that does not match the gate leaves it closed.
        report_status(&harness, "importing");
        await_status_seen(&mut harness, "importing");
        assert_eq!(written(&harness, "metals/slow"), 0);
        assert_eq!(written(&harness, "textDocument/definition"), 0);

        report_status(&harness, "ready");
        let first = harness.server.await_request("metals/slow").unwrap();
        let second = harness
            .server
            .await_request("textDocument/definition")
            .unwrap();
        let messages = harness.server.messages();
        let position = |id: &str| messages.iter().position(|message| message["id"] == id);
        assert!(
            position(&first) < position(&second),
            "arrival order is kept"
        );

        reply(&harness, &first, json!("one"));
        assert_eq!(
            completed(&mut harness, "first").result,
            Some(json!({ "value": "one" }))
        );
    }

    #[test]
    fn a_held_request_can_be_cancelled_without_touching_the_server() {
        let mut harness = start(metals_policy(true));
        call(&harness, "held-1", "metals/slow", json!({})).unwrap();
        harness
            .session()
            .cancel_operation("held-1")
            .expect("a held request can be cancelled");

        let event = completed(&mut harness, "held-1");
        assert_eq!(error_code(&event), "plugin.host.cancelled");

        // Opening the gate afterwards sends nothing: the request is gone.
        report_status(&harness, "ready");
        await_status_seen(&mut harness, "ready");
        assert_eq!(written(&harness, "metals/slow"), 0);
        assert_eq!(written(&harness, "$/cancelRequest"), 0);
    }

    #[test]
    fn a_held_request_times_out_while_it_waits() {
        let mut harness = start(metals_policy(true));
        let mut held = wire(&harness, "held-1", "metals/slow", json!({}));
        held["timeoutMilliseconds"] = json!(1);
        send(&harness, held).unwrap();

        let event = completed(&mut harness, "held-1");
        assert_eq!(error_code(&event), "plugin.host.timeout");
        assert_eq!(written(&harness, "metals/slow"), 0);
    }

    #[test]
    fn a_sent_request_times_out_and_is_cancelled_on_the_server() {
        let mut harness = start(metals_policy(false));
        let mut slow = wire(&harness, "slow-1", "metals/slow", json!({}));
        slow["timeoutMilliseconds"] = json!(1);
        send(&harness, slow).unwrap();
        let request_id = harness.server.await_request("metals/slow").unwrap();

        let event = completed(&mut harness, "slow-1");
        assert_eq!(error_code(&event), "plugin.host.timeout");
        let cancel = harness
            .server
            .await_notification_at("$/cancelRequest", 0)
            .expect("the server is told to stop");
        assert_eq!(cancel["params"]["id"], json!(request_id));
    }

    /// A second request for a serialized command does not start a second
    /// run; both finish with the answer of the one that runs.
    #[test]
    fn a_serialized_command_joins_the_one_that_runs() {
        let mut harness = start(metals_policy(false));
        run_command(&harness, "import-1", "build-import").unwrap();
        let request_id = harness
            .server
            .await_request("workspace/executeCommand")
            .unwrap();
        run_command(&harness, "import-2", "build-import").unwrap();
        // A command that is not serialized is independent of the first.
        run_command(&harness, "connect-1", "build-connect").unwrap();
        harness
            .server
            .await_request_at("workspace/executeCommand", 1)
            .expect("an independent command is sent");
        assert_eq!(written(&harness, "workspace/executeCommand"), 2);

        reply(&harness, &request_id, json!({ "imported": 3 }));
        for operation_id in ["import-1", "import-2"] {
            let event = completed(&mut harness, operation_id);
            assert_eq!(event.result, Some(json!({ "value": { "imported": 3 } })));
        }
        assert!(!harness
            .events
            .iter()
            .any(|event| event.operation_id.as_deref() == Some("connect-1")));

        // The command can run again once the first run is over.
        run_command(&harness, "import-3", "build-import").unwrap();
        harness
            .server
            .await_request_at("workspace/executeCommand", 2)
            .expect("a finished command can run again");
    }

    #[test]
    fn cancelling_a_joiner_leaves_the_running_command_alone() {
        let mut harness = start(metals_policy(false));
        run_command(&harness, "import-1", "build-import").unwrap();
        let request_id = harness
            .server
            .await_request("workspace/executeCommand")
            .unwrap();
        run_command(&harness, "import-2", "build-import").unwrap();
        harness.session().cancel_operation("import-2").unwrap();
        assert_eq!(
            error_code(&completed(&mut harness, "import-2")),
            "plugin.host.cancelled"
        );
        assert_eq!(written(&harness, "$/cancelRequest"), 0);

        reply(&harness, &request_id, json!(null));
        assert_eq!(
            completed(&mut harness, "import-1").result,
            Some(json!({ "value": null }))
        );
    }

    #[test]
    fn only_declared_server_notifications_are_forwarded() {
        let mut harness = start(metals_policy(false));
        harness.server.send(json!({
            "jsonrpc": "2.0",
            "method": "metals/unlisted",
            "params": { "text": "private" }
        }));
        report_status(&harness, "indexing");
        await_status_seen(&mut harness, "indexing");

        let forwarded: Vec<_> = harness
            .events
            .iter()
            .filter(|event| event.kind == "pluginServerNotification")
            .collect();
        assert_eq!(forwarded.len(), 1);
        assert_eq!(
            forwarded[0].result,
            Some(json!({
                "serverID": SERVER,
                "method": STATUS,
                "params": { "text": "indexing" }
            }))
        );
    }

    #[test]
    fn a_declared_notification_is_sent_and_a_command_is_not_a_notification() {
        let harness = start(metals_policy(false));
        let sent = notify(
            &harness,
            "metals/didFocus",
            json!({ "uri": "file:///A.scala" }),
        );
        assert!(sent.expect("a declared notification is sent"));
        let message = harness
            .server
            .await_notification_at("metals/didFocus", 0)
            .expect("the server receives it");
        assert_eq!(message["params"], json!({ "uri": "file:///A.scala" }));

        let error = notify(&harness, "workspace/executeCommand", json!({}))
            .expect_err("commands run as requests only");
        assert_eq!(refusal_code(error), "plugin.host.methodNotDeclared");
        let error = notify(&harness, "metals/unlisted", json!({}))
            .expect_err("an undeclared notification is refused");
        assert_eq!(refusal_code(error), "plugin.host.methodNotDeclared");
    }

    /// New settings replace the stored configuration, so they also answer
    /// the server's own `workspace/configuration` requests.
    #[test]
    fn changed_configuration_goes_through_the_plugin_configuration() {
        let harness = start(metals_policy(false));
        let settings = json!({ "metals": { "scalafmt": true } });
        let sent = notify(
            &harness,
            "workspace/didChangeConfiguration",
            json!({ "settings": settings }),
        );
        assert!(sent.expect("the change is accepted"));
        let change = harness
            .server
            .await_notification_at("workspace/didChangeConfiguration", 0)
            .expect("the server receives the change");
        assert_eq!(change["params"]["settings"], settings);

        let error = notify(
            &harness,
            "workspace/didChangeConfiguration",
            json!({ "settings": 1 }),
        )
        .expect_err("settings must be an object");
        assert_eq!(refusal_code(error), "plugin.host.invalidParams");
    }

    #[test]
    fn a_stopped_server_fails_what_is_in_flight_and_refuses_new_calls() {
        let mut harness = start(metals_policy(false));
        call(&harness, "slow-1", "metals/slow", json!({})).unwrap();
        call(&harness, "focus-1", "metals/didFocus", json!({})).unwrap();
        harness.server.await_request("metals/slow").unwrap();
        harness.server.await_request("metals/didFocus").unwrap();

        harness.server.exit(Some(1));
        harness.await_state(LspLifecycleState::Failed);
        for operation_id in ["slow-1", "focus-1"] {
            let event = completed(&mut harness, operation_id);
            assert_eq!(error_code(&event), "plugin.host.languageServerUnavailable");
        }

        let error = call(&harness, "late-1", "metals/slow", json!({}))
            .expect_err("a stopped server takes no requests");
        assert_eq!(refusal_code(error), "plugin.host.languageServerUnavailable");
    }

    #[test]
    fn a_gated_request_still_waiting_fails_when_the_server_stops() {
        let mut harness = start(metals_policy(true));
        call(&harness, "held-1", "metals/slow", json!({})).unwrap();

        harness.server.exit(Some(1));
        harness.await_state(LspLifecycleState::Failed);
        let event = completed(&mut harness, "held-1");
        assert_eq!(error_code(&event), "plugin.host.languageServerUnavailable");
    }

    #[test]
    fn an_invalid_policy_is_refused_at_start() {
        let refused = |plugin_id: Option<&str>, policy: Value| {
            let server = ScriptedServer::new();
            let engine = LspEngine::with_launcher(server.launcher());
            let mut request = start_request(&server);
            request.plugin_id = plugin_id.map(ToString::to_string);
            request.session_policy = Some(serde_json::from_value(policy).unwrap());
            match engine.start_server(request) {
                Err(error) => matches!(error.code, ErrorCode::InvalidRequest),
                Ok(_) => false,
            }
        };
        // The server ID is no protocol name.
        assert!(refused(Some(PLUGIN), json!({ "serverID": "" })));
        // A serialized command must also be a declared command.
        assert!(refused(
            Some(PLUGIN),
            json!({ "serverID": SERVER, "serializedCommands": ["build-import"] })
        ));
        // A host-owned server has no backend, so it cannot have a policy.
        assert!(refused(None, metals_policy(false)));
    }
}
