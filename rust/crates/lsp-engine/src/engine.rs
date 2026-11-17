//! The process-wide session table: start attempts, workspace roots, lookup and destruction.

use super::*;

impl LspEngine {
    pub(crate) fn new() -> Self {
        Self::with_launcher(Arc::new(SystemProcessLauncher))
    }

    pub(crate) fn with_launcher(launcher: Arc<dyn LspProcessLauncher>) -> Self {
        let sessions = Arc::new(Mutex::new(BTreeMap::new()));
        Self {
            next_session_id: AtomicU64::new(1),
            next_operation_id: AtomicU64::new(1),
            relay: RelayHub::start(&sessions),
            sessions,
            failed_starts: Mutex::new(BTreeSet::new()),
            launcher,
        }
    }

    pub(crate) fn next_operation_id(&self) -> String {
        format!(
            "lsp-operation-{}",
            self.next_operation_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    pub(crate) fn start_server(
        &self,
        request: StartServerRequest,
    ) -> Result<StartServerResponse, CoreError> {
        let mut registered = None;
        let result = self.start_server_owned(request, |id| registered = Some(id.to_owned()));
        if result.is_err() {
            if let Some(id) = registered {
                if self.lock_sessions()?.contains_key(&id) {
                    self.lock_failed_starts()?.insert(id);
                    // Preserve the original startup error. Incomplete cleanup
                    // stays owned; an explicit startup retry reaps it before
                    // any replacement process can be launched.
                    // log-exempt: Same failure already recorded: this cleanup follows the startup failure returned below and traced by this request's Core failure; residue stays in failed_starts and the next start reports ProcessFailed.
                    let _ = self.cleanup_failed_start();
                }
            }
        }
        result
    }

    pub(crate) fn lock_failed_starts(&self) -> Result<MutexGuard<'_, BTreeSet<String>>, CoreError> {
        self.failed_starts.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Failed startup cleanup lock was poisoned.",
            )
        })
    }

    pub(crate) fn cleanup_failed_start(&self) -> Result<(), CoreError> {
        // Serialize cleanup attempts, but never hold the session registry or
        // protocol-state lock across process disposal. Each retry owns at most
        // one two-second disposal; a backlog must not make one call unbounded.
        let mut failed = self.lock_failed_starts()?;
        if let Some(id) = failed.first().cloned() {
            if self.lock_sessions()?.contains_key(&id) {
                self.destroy(&id)?;
            }
            failed.remove(&id);
        }
        if !failed.is_empty() {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Previous language-server startup cleanup is still pending. Retry startup to continue cleanup.",
            ));
        }
        Ok(())
    }

    pub(crate) fn start_server_owned(
        &self,
        request: StartServerRequest,
        registered: impl FnOnce(&str),
    ) -> Result<StartServerResponse, CoreError> {
        let attempt = startup_attempt(&request);
        let result = self.start_server_attempt(request, &attempt, registered);
        // A session that already reached a terminal result took the attempt;
        // otherwise the process never started. Cancellation is not a result.
        match &result {
            Err(error) if !matches!(error.code, ErrorCode::Cancelled) => {
                plugin_operations::finish(&attempt, Some(AttemptStage::Launch));
            }
            Err(_) => plugin_operations::abandon(&attempt),
            Ok(_) => {}
        }
        result
    }

    fn start_server_attempt(
        &self,
        request: StartServerRequest,
        attempt: &AttemptSlot,
        registered: impl FnOnce(&str),
    ) -> Result<StartServerResponse, CoreError> {
        validate_start_request(&request)?;
        self.cleanup_failed_start()?;
        if lithe_workspace::is_remote()
            && request
                .session_policy
                .as_ref()
                .is_some_and(|policy| policy.remote == RemoteSupport::Unsupported)
        {
            return Err(CoreError::new(
                ErrorCode::NotSupported,
                "The plugin's language server does not support remote workspaces",
            ));
        }
        // Resolving native Git decisions walks the whole workspace, which can take
        // minutes on a large multi-repository tree and used to outlast the host's
        // start deadline before the process was even launched. The start only
        // reads the live context; unresolved decisions are resolved after the
        // launch by `publish_pending_exclusions`, which pushes them to the
        // session through the same path as any later indexing change. Never scan
        // under registry locks.
        let indexing_root = canonical_workspace_path(&request.root_uri);
        let (indexing_context, exclusions_pending) = if let Some(root) = &indexing_root {
            lithe_project::indexing::provider_context_without_scan(root)?
        } else {
            (None, false)
        };
        let check_indexing = || {
            if let Some(context) = &indexing_context {
                return context.check();
            }
            if indexing_root.as_ref().is_some_and(|root| {
                lithe_project::indexing::active_provider_context(root).is_some()
            }) {
                return Err(CoreError::new(
                    ErrorCode::Cancelled,
                    "Indexing policy changed",
                ));
            }
            Ok(())
        };
        let session_id = format!(
            "lsp-session-{}",
            self.next_session_id.fetch_add(1, Ordering::Relaxed)
        );
        let workspace_root = PathBuf::from(&request.working_directory);
        let initialize_timeout = Duration::from_millis(request.initialize_timeout_milliseconds);
        let request_timeout = Duration::from_millis(request.request_timeout_milliseconds);
        let shutdown_timeout = Duration::from_millis(request.shutdown_timeout_milliseconds);
        let workspace_configuration =
            checked_workspace_configuration(request.workspace_configuration)?;
        let initialize = client_initialize_for_plugin(
            ClientInitializeRequest {
                state: LspClientState {
                    offered_toolchain_providers: if request.plugin_id.is_some() {
                        request.toolchain_providers.iter().cloned().collect()
                    } else {
                        Default::default()
                    },
                    offered_keymap_import_providers: if request.plugin_id.is_some() {
                        request.keymap_import_providers.iter().cloned().collect()
                    } else {
                        Default::default()
                    },
                    ..LspClientState::default()
                },
                root_uri: request.root_uri.clone(),
                process_id: (!lithe_workspace::is_remote()).then_some(std::process::id() as i64),
                initialization_options: request.initialization_options,
            },
            request.plugin_id.is_some(),
        )?;
        let process = self.launcher.launch(LspProcessSpec {
            executable: PathBuf::from(&request.executable_path),
            arguments: request.arguments,
            working_directory: workspace_root,
            environment: request.environment,
        })?;

        let request_id = (initialize.state.next_request_id - 1).to_string();
        let now = Instant::now();
        let process_id = process.handle.process_id();
        let indexing_exclusions = request
            .session_policy
            .as_ref()
            .and_then(|policy| policy.indexing_exclusions.clone());
        let plugin_server = request.session_policy.map(PluginServerState::new);
        let session = Arc::new(RuntimeSession {
            startup_attempt: Arc::clone(attempt),
            id: session_id.clone(),
            log_identity: SessionLogIdentity::new(
                request.role,
                &request.provider_id,
                request.plugin_id.as_deref(),
                request.log_subject.as_deref(),
            ),
            provider_id: request.provider_id,
            plugin_id: request.plugin_id,
            workspace_host: lithe_workspace::current(),
            plugin_configuration: Mutex::new(workspace_configuration.map(Arc::new)),
            indexing_exclusions,
            root_uri: request.root_uri,
            indexing_roots: Mutex::default(),
            indexing_revision: std::sync::atomic::AtomicU64::new(0),
            outbound_order: Mutex::new(()),
            state: Mutex::new(SessionState {
                lifecycle: LspLifecycleState::Created,
                client: LspClientState {
                    host_services: request.host_services.into_iter().collect(),
                    host_options: request.host_options,
                    ..initialize.state
                },
                pending: BTreeMap::from([(
                    request_id,
                    PendingRequest {
                        kind: PendingKind::Initialize,
                        operation_id: None,
                        method: "initialize".to_string(),
                        document_uri: None,
                        document_version: None,
                        hover: None,
                        created_at: now,
                        deadline: now + initialize_timeout,
                    },
                )]),
                request_by_operation: BTreeMap::new(),
                function_by_operation: BTreeMap::new(),
                linked_document_paths: LinkedDocumentPaths::default(),
                pending_workspace_file_changes: BTreeMap::new(),
                events: VecDeque::new(),
                next_sequence: 1,
                initialize_deadline: Some(now + initialize_timeout),
                shutdown_deadline: None,
                request_timeout,
                shutdown_timeout,
                terminal_event_emitted: false,
                deadline_cancellations: Vec::new(),
                maintenance_write_deadline: None,
                plugin_server,
                relay_write_deadline: None,
                host_requests: BTreeMap::new(),
                next_host_request_id: 1,
                detailed_diagnostics: false,
                requested_diagnostics: false,
                rejected_plugin_diagnostics: false,
                plugin_diagnostics_window: Instant::now(),
                plugin_diagnostics_count: 0,
                hover_cache: HoverCache::default(),
            }),
            event_signal: Condvar::new(),
            process: process.handle,
            active: AtomicBool::new(true),
            relay: Arc::clone(&self.relay),
        });
        session.transition(LspLifecycleState::Created, None)?;
        session.transition(LspLifecycleState::ProcessStarting, None)?;
        session.transition(LspLifecycleState::Initializing, None)?;
        session.log(
            "info",
            EngineLog::ProcessStarted,
            "Language server process started",
            None,
        );

        // Register before checking the prepared generation. A policy change
        // either retires that generation or sees this session for its update.
        self.lock_sessions()?
            .insert(session_id.clone(), session.clone());
        if let Err(error) = check_indexing() {
            // A retired start is cancelled, not a handshake failure. Abandon
            // the attempt before `fail` or the process exit can record it.
            plugin_operations::abandon(attempt);
            session.fail(
                "indexingChanged",
                "initialize",
                "Indexing policy changed during language-server startup.",
                Some(core_error_detail(&error)),
                None,
            );
            session.active.store(false, Ordering::Release);
            session.kill_process();
            // No native identity has been published yet. Forget the session
            // only after disposal proves that its process tree has exited.
            if session.process.dispose(Duration::from_secs(2)).is_ok() {
                self.lock_sessions()?.remove(&session_id);
            } else {
                registered(&session_id);
            }
            return Err(error);
        }
        // Native owners learn the identity before initialize writes can fail.
        // They keep their package barrier until destroy verifies disposal.
        registered(&session_id);
        session.spawn_readers(process.output, process.errors);
        session.spawn_monitor();
        session.spawn_outbound_maintenance();
        let outbound_order = session.lock_outbound_order()?;
        if let Err(error) = session.send_messages(&outbound_order, initialize.messages) {
            // startServer fails here, so the attempt failed to launch.
            plugin_operations::finish(attempt, Some(AttemptStage::Launch));
            session.fail(
                "transportFailed",
                "initialize",
                "Could not write the initialize request.",
                Some(core_error_detail(&error)),
                None,
            );
            session.kill_process();
            return Err(error);
        }

        if let Err(error) = check_indexing() {
            // The readers already run; abandon before `fail` and the kill so
            // neither the failure nor the observed exit records the cancel.
            plugin_operations::abandon(attempt);
            session.fail(
                "indexingChanged",
                "initialize",
                "Indexing policy changed during language-server startup.",
                Some(core_error_detail(&error)),
                None,
            );
            session.active.store(false, Ordering::Release);
            session.kill_process();
            // The native owner already holds this identity. Keep it until
            // destroy verifies disposal and returns a positive receipt.
            return Err(error);
        }

        if exclusions_pending {
            if let Some(root) = indexing_root.clone() {
                publish_pending_exclusions(root, Arc::clone(&self.sessions), &session);
            }
        }

        Ok(StartServerResponse {
            channel: session.log_identity.channel(),
            session_id,
            state: LspLifecycleState::Initializing,
            process_id,
        })
    }

    pub(crate) fn session(&self, session_id: &str) -> Result<Arc<RuntimeSession>, CoreError> {
        self.lock_sessions()?
            .get(session_id)
            .cloned()
            .ok_or_else(|| unknown_session(session_id))
    }

    pub(crate) fn destroy(&self, session_id: &str) -> Result<(), CoreError> {
        let session = self.session(session_id)?;
        let lifecycle = session.lock_state()?.lifecycle;
        if !matches!(
            lifecycle,
            LspLifecycleState::Stopped | LspLifecycleState::Failed
        ) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "A running language-server session cannot be destroyed.",
            ));
        }
        session.process.dispose(Duration::from_secs(2))?;
        self.lock_sessions()?.remove(session_id);
        Ok(())
    }

    pub(crate) fn lock_sessions(
        &self,
    ) -> Result<MutexGuard<'_, BTreeMap<String, Arc<RuntimeSession>>>, CoreError> {
        self.sessions.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Language-server session registry lock was poisoned.",
            )
        })
    }
}

/// The workspace directory with symbolic links resolved, when it exists.
///
/// Resolving links touches the file system, so it happens here in the engine
/// rather than in the pure normalizer that receives the result.
fn canonical_workspace_path(root_uri: &str) -> Option<PathBuf> {
    let path = url::Url::parse(root_uri).ok()?.to_file_path().ok()?;
    // Core policy identity uses the native canonical path, including Windows'
    // verbatim prefix. A protocol/display path is not a policy registry key.
    lithe_workspace::fs::canonicalize(path).ok()
}

pub(crate) fn canonical_workspace_root(root_uri: &str) -> Option<String> {
    let canonical = canonical_workspace_path(root_uri)?;
    let canonical =
        lithe_platform::paths::slash_separated_path(&canonical, lithe_platform::Platform::HOST);
    // Windows canonical paths carry a verbatim `\\?\` prefix that language servers never report.
    Some(
        canonical
            .strip_prefix("//?/UNC/")
            .map(|rest| format!("//{rest}"))
            .or_else(|| canonical.strip_prefix("//?/").map(str::to_string))
            .unwrap_or(canonical),
    )
}

// Only a declarative language server carries a host attempt. SDK backends report
// their own operations through `lithe/log`, so a stray handoff on another role is
// ignored rather than rejecting the start. The handoff names its plugin, since a
// plugin API 1 server starts with no session plugin; a session another plugin
// owns never takes the attempt, so no fact lands on a plugin that did not run.
fn startup_attempt(request: &StartServerRequest) -> AttemptSlot {
    match &request.startup_operation {
        Some(handoff)
            if request.role == SessionRole::LanguageServer
                && request
                    .plugin_id
                    .as_ref()
                    .is_none_or(|owner| *owner == handoff.plugin_id) =>
        {
            Attempt::slot(&LANGUAGE_SERVER_START, handoff.clone())
        }
        _ => Attempt::none(),
    }
}

/// Resolves the workspace's native Git decisions off the start path and pushes
/// them to every running session of that workspace.
///
/// `start_server_attempt` launches the process without waiting for the full
/// workspace walk. Until this finishes, a server that pulls
/// `workspace/configuration` sees only the exclusions known at start; the
/// update below then re-sends the configuration exactly like a later indexing
/// change, so the server narrows its import scope instead of missing it. During
/// the walk the thread holds no session, only a weak reference to the started
/// one, which is used to record a failure in that session's log channel.
fn publish_pending_exclusions(
    root: PathBuf,
    sessions: Arc<Mutex<BTreeMap<String, Arc<RuntimeSession>>>>,
    started: &Arc<RuntimeSession>,
) {
    let started = Arc::downgrade(started);
    thread::spawn(move || {
        #[cfg(test)]
        deferred_exclusions::wait_while_held(&root);
        // Walk before listing the sessions: the walk can take minutes, and a
        // list taken first would keep sessions stopped meanwhile alive for that
        // long and leave out the ones started meanwhile.
        let result = lithe_project::indexing::prepare_provider_exclusions(&root)
            .and_then(|_| {
                sessions
                    .lock()
                    .map_err(|_| {
                        CoreError::new(ErrorCode::Unknown, "LSP registry lock was poisoned")
                    })
                    .map(|registry| registry.values().cloned().collect::<Vec<_>>())
            })
            .and_then(|running| apply_indexing_to_sessions(&root, running));
        #[cfg(test)]
        deferred_exclusions::finish(&root);
        let Err(error) = result else {
            return;
        };
        // log-exempt: User cancellation: a closed workspace or a replaced indexing policy retires the context, and the replacement publishes its own decisions.
        if matches!(error.code, ErrorCode::Cancelled) {
            return;
        }
        if let Some(session) = started.upgrade() {
            session.log(
                "warn",
                EngineLog::ExclusionsFailed,
                "Indexing exclusions could not be published to the language server",
                Some(core_error_detail(&error)),
            );
        }
    });
}

/// Test seam that fixes the order between a start and its deferred Git walk.
///
/// A test holds a workspace before starting a server, observes the start
/// return while the decisions are still unresolved, then releases the walk and
/// waits for the publisher to finish. Roots no test holds pass straight
/// through, so concurrent tests are unaffected. Every wait is bounded, so a
/// failed assertion cannot leave a publisher thread blocked.
#[cfg(test)]
pub(crate) mod deferred_exclusions {
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
    use std::time::Duration;

    /// Where a held workspace is: its walk waits, the test has released it, or
    /// its publisher has finished.
    #[derive(Clone, Copy, PartialEq)]
    enum Stage {
        Held,
        Released,
        Finished,
    }

    /// Workspaces a test is watching; roots not listed are never held.
    static ROOTS: Mutex<Vec<(PathBuf, Stage)>> = Mutex::new(Vec::new());
    static CHANGED: Condvar = Condvar::new();
    const LIMIT: Duration = Duration::from_secs(5);

    fn roots() -> MutexGuard<'static, Vec<(PathBuf, Stage)>> {
        ROOTS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stage(roots: &[(PathBuf, Stage)], root: &Path) -> Option<Stage> {
        roots
            .iter()
            .find(|(watched, _)| watched == root)
            .map(|(_, stage)| *stage)
    }

    fn advance(root: &Path, from: Stage, to: Stage) {
        let mut roots = roots();
        if let Some(entry) = roots
            .iter_mut()
            .find(|(watched, stage)| watched == root && *stage == from)
        {
            entry.1 = to;
        }
        CHANGED.notify_all();
    }

    /// Holds the walk of the workspace at `root_uri` until the returned guard
    /// releases it or is dropped.
    pub(crate) fn hold(root_uri: &str) -> Held {
        let root = super::canonical_workspace_path(root_uri).expect("the workspace exists");
        let mut roots = roots();
        roots.retain(|(watched, _)| *watched != root);
        roots.push((root.clone(), Stage::Held));
        Held(root)
    }

    pub(crate) struct Held(PathBuf);

    impl Held {
        /// Releases the walk and waits until its publisher has finished;
        /// false when that took longer than the limit.
        pub(crate) fn release_and_wait(&self) -> bool {
            advance(&self.0, Stage::Held, Stage::Released);
            let (_roots, wait) = CHANGED
                .wait_timeout_while(roots(), LIMIT, |roots| {
                    stage(roots, &self.0) != Some(Stage::Finished)
                })
                .unwrap_or_else(PoisonError::into_inner);
            !wait.timed_out()
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            roots().retain(|(watched, _)| *watched != self.0);
            CHANGED.notify_all();
        }
    }

    pub(super) fn wait_while_held(root: &Path) {
        let _released = CHANGED
            .wait_timeout_while(roots(), LIMIT, |roots| {
                stage(roots, root) == Some(Stage::Held)
            })
            .unwrap_or_else(PoisonError::into_inner);
    }

    pub(super) fn finish(root: &Path) {
        advance(root, Stage::Released, Stage::Finished);
    }
}
