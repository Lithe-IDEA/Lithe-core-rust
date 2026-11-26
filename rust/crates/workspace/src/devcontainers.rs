//! A local or WSL folder's Dev Container, configured through a basic form
//! and built and started by the reference Dev Container CLI.
//!
//! The window lists the folder's `devcontainer.json` files, then asks for
//! `up` with the one the user picked. The CLI builds or reuses the container
//! exactly as the configuration says, mounts included, and runs its
//! lifecycle commands; its log streams to the window as `devcontainerOutput`
//! events, redacted and never logged. The result names the container, the
//! user and the workspace folder inside it, which the window then enters
//! with `remote.connect`'s `container` and opens. Only the host's explicit
//! Create action publishes the standard document rendered by [`prepare`].
//!
//! Building runs the folder's code on this machine (`initializeCommand`,
//! Dockerfiles, feature scripts), so `up` runs only when the host says the
//! folder is trusted (`projectCodeAllowed`), the same rule as Git. Lithe
//! itself adds no mounts and no options to the configuration (S32).

use crate::container_runtimes::RuntimeTarget;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::devcontainer::{
    discover, name_command, parse_name, parse_up_result, up_command, UpFailure,
};
use lithe_remote::client::{ContainerEngine, DevcontainerConfig};
use lithe_request_context::events as execution_events;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long `up` may take: pulling images and building them can take many
/// minutes on a slow connection.
const UP_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How long the engine may take to name the container.
const NAME_TIMEOUT: Duration = Duration::from_secs(15);

/// Bytes of the CLI's log sent to the window; later lines are dropped and
/// one `truncated` event says so.
const MAX_STREAMED_BYTES: usize = 2 * 1024 * 1024;

/// Longest log line sent, in bytes; a longer one is sent in parts.
const MAX_LINE_BYTES: usize = 4096;

/// `remote.devcontainerConfigs` request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigsRequest {
    /// Absolute folder on this machine.
    pub folder: String,
    /// Environment owning the folder; omitted is this machine.
    #[serde(default)]
    pub runtime: Option<RuntimeTarget>,
}

/// `remote.devcontainerConfigs` result.
#[derive(Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Configs {
    /// Native path the desktop uses for trust and confirmed configuration publication.
    pub native_folder: String,
    /// The folder's configurations in the order the specification searches
    /// them; empty when it has none.
    pub configs: Vec<DevcontainerConfig>,
}

/// `remote.devcontainerUp` request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpRequest {
    /// Absolute folder on this machine.
    pub folder: String,
    /// One of the `path`s `remote.devcontainerConfigs` listed; the CLI's own
    /// search when absent.
    #[serde(default)]
    pub config: Option<String>,
    /// Engine the CLI builds and runs the container with.
    pub engine: ContainerEngine,
    /// Explicit environment selected from discovery; omitted preserves legacy behavior.
    #[serde(default)]
    pub runtime: Option<RuntimeTarget>,
    /// Whether the user trusts the folder, so its code may run here.
    pub project_code_allowed: bool,
}

/// `remote.devcontainerUp` result.
#[derive(Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Up {
    /// The running container; absent when the CLI reported a failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<Devcontainer>,
    /// What the CLI reported about the failure, redacted; empty otherwise.
    /// Shown, never logged.
    pub diagnostics: String,
}

/// The container `up` started, as `remote.connect`'s `container` takes it,
/// and the folder to open in it.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Devcontainer {
    /// Engine that runs the container.
    pub engine: ContainerEngine,
    /// Exact environment used to build and inspect this container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeTarget>,
    /// The container's name, or its id when the engine did not name it.
    pub container: String,
    /// Full container id.
    pub container_id: String,
    /// User the configuration connects as; the image's default when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Absolute folder inside the container that holds the workspace.
    pub workspace_folder: String,
}

/// The configurations of the folder `request` names.
pub fn configs(request: ConfigsRequest) -> Result<Configs, CoreError> {
    let target = request.runtime.unwrap_or(RuntimeTarget {
        engine: ContainerEngine::Docker,
        context: None,
        wsl: None,
    });
    target.validate()?;
    let resolved = resolve_folder(&request.folder, &target)?;
    let native_folder = match target.wsl.as_deref() {
        Some(_) => {
            let argv = vec!["wslpath".into(), "-w".into(), resolved.clone()];
            let outcome = crate::container_runtimes::capture(&target, &argv, NAME_TIMEOUT)?
                .filter(|outcome| outcome.status.is_some_and(|status| status.success()))
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Cannot resolve the native WSL workspace folder",
                    )
                })?;
            String::from_utf8_lossy(&outcome.stdout).trim().to_owned()
        }
        None => resolved.clone(),
    };
    Ok(Configs {
        native_folder,
        configs: configs_for_resolved(&resolved, &target)?,
    })
}

/// Builds or reuses and starts the dev container `request` names.
pub fn up(request: UpRequest) -> Result<Up, CoreError> {
    up_with(request, None)
}

/// `program` replaces both the Dev Container CLI and the engine CLI; tests
/// use one stand-in for both.
pub(crate) fn up_with(request: UpRequest, program: Option<&str>) -> Result<Up, CoreError> {
    if !request.project_code_allowed {
        return Err(CoreError::new(
            ErrorCode::DevcontainerUntrustedFolder,
            "The folder is not trusted, so its dev container was not built or started",
        ));
    }
    let target = request.runtime.clone().unwrap_or(RuntimeTarget {
        engine: request.engine,
        context: None,
        wsl: None,
    });
    target.validate()?;
    if target.engine != request.engine {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Runtime engine mismatch",
        ));
    }
    let resolved_folder = resolve_folder(&request.folder, &target)?;
    let folder = PathBuf::from(&resolved_folder);
    let listed = configs_for(&request.folder, &target)?;
    let config = match request.config.as_deref() {
        None => None,
        Some(config) => {
            if !listed.iter().any(|listed| listed.path == config) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The configuration is not one of the folder's devcontainer.json files",
                ));
            }
            Some(if target.wsl.is_some() {
                format!("{}/{}", resolved_folder.trim_end_matches('/'), config)
            } else {
                folder.join(config).to_string_lossy().into_owned()
            })
        }
    };
    let mut argv = up_command(request.engine, &folder.to_string_lossy(), config.as_deref());
    if let Some(program) = program {
        argv[0] = program.into();
    }
    if target.wsl.is_some() {
        argv[0] = "devcontainer".into();
    }
    let mut log = Log::default();
    let outcome = run_target(&target, &argv, UP_TIMEOUT, |stream, bytes| {
        if matches!(stream, lithe_git_host::Stream::Stderr) {
            log.push(bytes);
        }
    })
    .map_err(|error| match error {
        Ran::Stopped(error) => error,
        Ran::Missing => CoreError::new(
            ErrorCode::RuntimeMissing,
            "The Dev Container CLI (devcontainer) is not installed",
        ),
    })?;
    log.finish();
    match parse_up_result(&outcome.stdout) {
        Ok(started) => {
            let container = container_name(&target, &started.container_id, program)
                .unwrap_or_else(|| started.container_id.clone());
            Ok(Up {
                container: Some(Devcontainer {
                    engine: request.engine,
                    runtime: request.runtime,
                    container,
                    container_id: started.container_id,
                    user: started.remote_user,
                    workspace_folder: started.remote_workspace_folder,
                }),
                diagnostics: String::new(),
            })
        }
        Err(UpFailure::Reported(message)) => {
            let mut printed = String::from_utf8_lossy(&outcome.stderr)
                .trim_end()
                .to_string();
            printed.push('\n');
            printed.push_str(&message);
            Ok(failed(&printed))
        }
        Err(UpFailure::Missing)
            if target.wsl.is_some()
                && outcome.status.and_then(|status| status.code()) == Some(127) =>
        {
            Err(CoreError::new(
                ErrorCode::RuntimeMissing,
                "The Dev Container CLI is not installed in the selected WSL distribution",
            ))
        }
        Err(UpFailure::Missing) if !outcome.status.is_some_and(|status| status.success()) => {
            let mut printed = String::from_utf8_lossy(&outcome.stdout).into_owned();
            printed.push('\n');
            printed.push_str(&String::from_utf8_lossy(&outcome.stderr));
            Ok(failed(&printed))
        }
        Err(UpFailure::Missing | UpFailure::Unusable) => Err(CoreError::new(
            ErrorCode::ParseFailed,
            "The Dev Container CLI did not report a container Lithe can enter",
        )),
    }
}

fn failed(printed: &str) -> Up {
    Up {
        container: None,
        diagnostics: crate::probe::diagnostics(printed),
    }
}

/// The existing absolute folder `folder` names on this machine.
fn local_folder(folder: &str) -> Result<PathBuf, CoreError> {
    let path = Path::new(folder);
    if folder.is_empty() || !path.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The dev container folder must be an absolute path",
        ));
    }
    if !path.is_dir() {
        return Err(CoreError::new(
            ErrorCode::WorkspaceNotFound,
            "The dev container folder does not exist",
        ));
    }
    Ok(path.to_path_buf())
}

/// The name the engine gives container `id`, when it answers in time.
fn container_name(target: &RuntimeTarget, id: &str, program: Option<&str>) -> Option<String> {
    let mut argv = name_command(target.engine, id);
    if let Some(program) = program {
        argv[0] = program.into();
    }
    let outcome = run_target(target, &argv, NAME_TIMEOUT, |_, _| {}).ok()?;
    if !outcome.status.is_some_and(|status| status.success()) {
        return None;
    }
    parse_name(&outcome.stdout)
}

/// Why [`run_target`] has no outcome.
enum Ran {
    /// Cancelled, out of time or failed while running.
    Stopped(CoreError),
    /// The program does not exist here.
    Missing,
}

/// Runs `argv` on this machine, in Core's own environment, which holds the
/// login shell's `PATH` and the variables, such as `DOCKER_HOST`, that
/// choose the engine.
fn run_target(
    target: &RuntimeTarget,
    argv: &[String],
    timeout: Duration,
    output: impl FnMut(lithe_git_host::Stream, &[u8]),
) -> Result<lithe_git_host::Outcome, Ran> {
    let mut process = target.command(argv).map_err(Ran::Stopped)?;
    let deadline = Instant::now() + timeout;
    let mut stop = None;
    let outcome = lithe_git_host::run(
        &mut process,
        None,
        || {
            if let Err(error) = lithe_protocol::cancellation::check() {
                stop = Some(error);
            } else if Instant::now() >= deadline {
                stop = Some(CoreError::new(
                    ErrorCode::TimedOut,
                    "The dev container did not start in time",
                ));
            }
            stop.is_some()
        },
        || {},
        output,
    );
    if let Some(error) = stop {
        return Err(Ran::Stopped(error));
    }
    match outcome.failure {
        None => Ok(outcome),
        Some(lithe_git_host::Failure::Start(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Err(Ran::Missing)
        }
        Some(failure) => Err(Ran::Stopped(
            CoreError::new(ErrorCode::ProcessFailed, "The Dev Container CLI failed")
                .with_details(format!("{failure:?}")),
        )),
    }
}

/// The CLI's log, sent to the window line by line.
#[derive(Default)]
struct Log {
    pending: Vec<u8>,
    sent: usize,
    truncated: bool,
}

impl Log {
    fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            self.send(&line);
        }
        while self.pending.len() > MAX_LINE_BYTES {
            let part: Vec<u8> = self.pending.drain(..MAX_LINE_BYTES).collect();
            self.send(&part);
        }
    }

    fn finish(&mut self) {
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.send(&rest);
        }
    }

    fn send(&mut self, line: &[u8]) {
        if self.truncated {
            return;
        }
        if self.sent + line.len() > MAX_STREAMED_BYTES {
            self.truncated = true;
            execution_events::emit(json!({ "type": "devcontainerOutput", "truncated": true }));
            return;
        }
        self.sent += line.len();
        let text = String::from_utf8_lossy(line);
        let text = text.trim_end_matches(['\r', '\n']);
        execution_events::emit(json!({
            "type": "devcontainerOutput",
            "text": execution_events::redact(text),
        }));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A folder with a configuration and a stand-in CLI beside it: the
    /// script answers `up` with `up_body` and `inspect` with `name_body`.
    struct Fixture {
        root: PathBuf,
        folder: PathBuf,
        program: String,
    }

    impl Fixture {
        fn new(label: &str, up_body: &str, name_body: &str) -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "lithe-devcontainer-up-{label}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            let folder = root.join("app");
            std::fs::create_dir_all(folder.join(".devcontainer/python")).unwrap();
            std::fs::write(
                folder.join(".devcontainer/python/devcontainer.json"),
                "{ \"name\": \"Python\", }",
            )
            .unwrap();
            let program = root.join("devcontainer");
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$0.arguments\"\nif [ \"$1\" = up ]; then\n{up_body}\nelse\n{name_body}\nfi\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                root,
                folder,
                program: program.to_string_lossy().into_owned(),
            }
        }

        fn request(&self, config: Option<&str>, trusted: bool) -> UpRequest {
            UpRequest {
                folder: self.folder.to_string_lossy().into_owned(),
                config: config.map(Into::into),
                engine: ContainerEngine::Podman,
                runtime: None,
                project_code_allowed: trusted,
            }
        }

        fn arguments(&self) -> String {
            std::fs::read_to_string(format!("{}.arguments", self.program)).unwrap_or_default()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const STARTED: &str = r#"printf '[1 ms] Start: Run\n' >&2
printf '{"outcome":"success","containerId":"abc123","remoteUser":"vscode","remoteWorkspaceFolder":"/workspaces/app"}\n'"#;

    #[test]
    fn up_starts_the_picked_configuration_and_names_the_container() {
        let fixture = Fixture::new("started", STARTED, "printf '/eager_turing\\n'");
        let config = ".devcontainer/python/devcontainer.json";
        let up = up_with(fixture.request(Some(config), true), Some(&fixture.program)).unwrap();
        assert_eq!(
            up,
            Up {
                container: Some(Devcontainer {
                    engine: ContainerEngine::Podman,
                    runtime: None,
                    container: "eager_turing".into(),
                    container_id: "abc123".into(),
                    user: Some("vscode".into()),
                    workspace_folder: "/workspaces/app".into(),
                }),
                diagnostics: String::new(),
            }
        );
        let folder = fixture.folder.to_string_lossy();
        assert_eq!(
            fixture.arguments(),
            format!(
                "up\n--workspace-folder\n{folder}\n--config\n{folder}/{config}\n--docker-path\npodman\ninspect\n--format\n{{{{.Name}}}}\nabc123\n"
            )
        );
    }

    #[test]
    fn an_unnamed_container_is_entered_by_its_id() {
        let fixture = Fixture::new("unnamed", STARTED, "exit 1");
        let up = up_with(fixture.request(None, true), Some(&fixture.program)).unwrap();
        assert_eq!(up.container.unwrap().container, "abc123");
    }

    #[test]
    fn untrusted_folders_run_nothing() {
        let fixture = Fixture::new("untrusted", STARTED, "exit 0");
        let error = up_with(fixture.request(None, false), Some(&fixture.program)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::DevcontainerUntrustedFolder));
        assert_eq!(fixture.arguments(), "");
    }

    #[test]
    fn only_listed_configurations_are_accepted() {
        let fixture = Fixture::new("unlisted", STARTED, "exit 0");
        for config in [
            "../outside.json",
            "/etc/devcontainer.json",
            ".devcontainer/x.json",
        ] {
            let error =
                up_with(fixture.request(Some(config), true), Some(&fixture.program)).unwrap_err();
            assert!(matches!(error.code, ErrorCode::InvalidRequest));
        }
        assert_eq!(fixture.arguments(), "");
    }

    #[test]
    fn reported_failures_are_shown_with_the_log_tail() {
        let fixture = Fixture::new(
            "failed",
            r#"printf 'Step 3/7 failed\n' >&2
printf '{"outcome":"error","message":"Command failed: docker build","description":"An error occurred building the image."}\n'
exit 1"#,
            "exit 0",
        );
        let up = up_with(fixture.request(None, true), Some(&fixture.program)).unwrap();
        assert_eq!(up.container, None);
        assert_eq!(
            up.diagnostics,
            "Step 3/7 failed\nCommand failed: docker build\nAn error occurred building the image."
        );
    }

    #[test]
    fn a_missing_cli_is_a_missing_runtime() {
        let fixture = Fixture::new("missing", STARTED, "exit 0");
        let missing = fixture.root.join("absent").to_string_lossy().into_owned();
        let error = up_with(fixture.request(None, true), Some(&missing)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::RuntimeMissing));
    }

    #[test]
    fn a_success_without_a_usable_container_cannot_be_read() {
        let fixture = Fixture::new("unusable", "printf 'done\\n'", "exit 0");
        let error = up_with(fixture.request(None, true), Some(&fixture.program)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::ParseFailed));
    }

    #[test]
    fn configurations_are_listed_for_absolute_existing_folders() {
        let fixture = Fixture::new("configs", STARTED, "exit 0");
        let listed = configs(ConfigsRequest {
            runtime: None,
            folder: fixture.folder.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert_eq!(
            listed.configs,
            vec![DevcontainerConfig {
                path: ".devcontainer/python/devcontainer.json".into(),
                name: Some("Python".into()),
            }]
        );
        let relative = configs(ConfigsRequest {
            runtime: None,
            folder: "app".into(),
        });
        assert!(matches!(
            relative.unwrap_err().code,
            ErrorCode::InvalidRequest
        ));
        let absent = configs(ConfigsRequest {
            runtime: None,
            folder: fixture.root.join("absent").to_string_lossy().into_owned(),
        });
        assert!(matches!(
            absent.unwrap_err().code,
            ErrorCode::WorkspaceNotFound
        ));
    }

    #[test]
    fn requests_name_the_trust_decision_and_nothing_else() {
        let decode = |value| serde_json::from_value::<UpRequest>(value);
        assert!(decode(json!({"folder": "/a", "engine": "docker"})).is_err());
        assert!(decode(
            json!({"folder": "/a", "engine": "docker", "projectCodeAllowed": true, "mount": "/"})
        )
        .is_err());
        let request =
            decode(json!({"folder": "/a", "engine": "docker", "projectCodeAllowed": true}))
                .unwrap();
        assert_eq!(request.config, None);
    }

    /// The `devcontainerOutput` events one run of `log` sends.
    fn log_events(log: impl FnOnce(&mut Log)) -> Vec<serde_json::Value> {
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let records = std::sync::Arc::clone(&observed);
        let sink: execution_events::EventSink = std::sync::Arc::new(move |raw: &str| {
            let event: serde_json::Value = serde_json::from_str(raw).unwrap();
            records.lock().unwrap().push(event);
        });
        execution_events::with_sink(sink, || {
            let mut state = Log::default();
            log(&mut state);
            state.finish();
        });
        let events = observed.lock().unwrap().clone();
        assert!(events
            .iter()
            .all(|event| event["type"] == "devcontainerOutput"));
        events
    }

    #[test]
    fn the_log_is_sent_line_by_line_and_redacted() {
        let events = log_events(|log| {
            // A credential split across two reads is redacted as one line,
            // and a CRLF ending is not part of the text.
            log.push(b"[1 ms] Start: Run: docker build\r\n[2 ms] pull https://ci:hunter2@");
            log.push(b"registry.example/app\n");
            // A line longer than one event is sent in parts; the rest of it
            // is sent when the CLI exits.
            log.push(&vec![b'x'; MAX_LINE_BYTES + 10]);
        });
        let texts: Vec<&str> = events
            .iter()
            .map(|event| event["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts.len(), 4, "{texts:?}");
        assert_eq!(texts[0], "[1 ms] Start: Run: docker build");
        assert!(!texts[1].contains("hunter2"), "{}", texts[1]);
        assert!(texts[1].contains("registry.example/app"), "{}", texts[1]);
        assert_eq!(texts[2].len(), MAX_LINE_BYTES);
        assert_eq!(texts[3].len(), 10);
        assert!(events.iter().all(|event| event.get("truncated").is_none()));
    }

    #[test]
    fn the_log_stops_once_after_its_bound() {
        let line = [vec![b'y'; 4000], vec![b'\n']].concat();
        let sent = MAX_STREAMED_BYTES / line.len();
        let events = log_events(|log| {
            for _ in 0..sent + 20 {
                log.push(&line);
            }
            log.push(b"after the bound\n");
        });
        assert_eq!(events.len(), sent + 1);
        assert!(events[..sent]
            .iter()
            .all(|event| event["text"].as_str().map(str::len) == Some(4000)));
        assert_eq!(events[sent]["truncated"], true);
        assert!(events[sent].get("text").is_none());
    }
}

#[cfg(test)]
#[path = "devcontainers/tests.rs"]
mod form_tests;

/// Basic form fields. The resulting document remains the official CLI's input.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct PrepareRequest {
    /// Optional human-readable configuration name.
    pub name: String,
    /// Image reference, mutually exclusive with dockerfile.
    pub image: String,
    /// Workspace-relative Dockerfile path.
    pub dockerfile: String,
    /// Workspace-relative build context; defaults to the workspace root.
    pub build_context: String,
    /// Container account, empty to preserve the image default.
    pub remote_user: String,
    /// Absolute workspace path in the container, empty to preserve the CLI default.
    pub workspace_folder: String,
    /// Comma or whitespace separated TCP port numbers published with loopback appPort bindings.
    pub ports: String,
    /// Explicit user-authored command executed after creation by the official CLI.
    pub post_create_command: String,
}

/// A validated, standard configuration to publish only after explicit user confirmation.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    /// Workspace-relative write destination; never inside the application installation.
    pub path: String,
    /// Pretty-printed JSON consumed by agents and the reference CLI alike.
    pub content: String,
}

/// Validates form input and renders standard devcontainer.json without performing I/O.
pub fn prepare(request: PrepareRequest) -> Result<Prepared, CoreError> {
    let invalid = |field| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid dev container configuration field",
        )
        .with_details(json!({"field":field}).to_string())
    };
    let image = request.image.trim();
    let dockerfile = request.dockerfile.trim();
    if image.is_empty() == dockerfile.is_empty() {
        return Err(invalid("image"));
    }
    let mut value = serde_json::Map::new();
    if !image.is_empty() {
        if image.len() > 512
            || image.starts_with('-')
            || image.chars().any(char::is_whitespace)
            || image.chars().any(char::is_control)
        {
            return Err(invalid("image"));
        }
        value.insert("image".into(), json!(image));
    } else {
        let relative = |text: &str| {
            !text.is_empty()
                && text.len() <= 1024
                && !text.starts_with('/')
                && !text.contains('\\')
                && !text.contains(':')
                && !text.chars().any(char::is_control)
                && text.split('/').all(|part| part != "..")
        };
        if !relative(dockerfile) {
            return Err(invalid("dockerfile"));
        }
        let context = if request.build_context.trim().is_empty() {
            "."
        } else {
            request.build_context.trim()
        };
        if !relative(context) {
            return Err(invalid("buildContext"));
        }
        // Both paths are relative to .devcontainer/devcontainer.json, not the workspace.
        value.insert(
            "build".into(),
            json!({"dockerfile":format!("../{dockerfile}"),"context":format!("../{context}")}),
        );
    }
    for (key, text, maximum) in [
        ("name", request.name.trim(), 128),
        ("remoteUser", request.remote_user.trim(), 64),
        ("workspaceFolder", request.workspace_folder.trim(), 4096),
        (
            "postCreateCommand",
            request.post_create_command.trim(),
            8192,
        ),
    ] {
        if text.len() > maximum || text.contains('\0') {
            return Err(invalid(key));
        }
        if key == "remoteUser"
            && !text.is_empty()
            && !lithe_remote::client::transport::is_container_user(text)
        {
            return Err(invalid(key));
        }
        if key == "workspaceFolder"
            && !text.is_empty()
            && (!text.starts_with('/') || text.chars().any(char::is_control))
        {
            return Err(invalid(key));
        }
        if !text.is_empty() {
            value.insert(key.into(), json!(text));
        }
    }
    let mut ports = Vec::<u16>::new();
    for port in request
        .ports
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
    {
        let port = port
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| invalid("ports"))?;
        if !ports.contains(&port) {
            ports.push(port);
        }
        if ports.len() > 64 {
            return Err(invalid("ports"));
        }
    }
    if !ports.is_empty() {
        value.insert(
            "appPort".into(),
            json!(ports
                .into_iter()
                .map(|port| format!("127.0.0.1:{port}:{port}"))
                .collect::<Vec<_>>()),
        );
    }
    Ok(Prepared {
        path: ".devcontainer/devcontainer.json".into(),
        content: format!(
            "{}\n",
            serde_json::to_string_pretty(&value).expect("configuration encodes")
        ),
    })
}

/// Resolves a Windows folder in WSL without assuming its automount prefix.
fn resolve_folder(folder: &str, target: &RuntimeTarget) -> Result<String, CoreError> {
    if target.wsl.is_none() {
        return Ok(local_folder(folder)?.to_string_lossy().into_owned());
    }
    if folder.is_empty() || folder.len() > 4096 || folder.chars().any(char::is_control) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid WSL workspace folder",
        ));
    }
    if folder.starts_with('/') {
        return Ok(folder.into());
    }
    if !lithe_platform::paths::is_absolute_path(folder, lithe_platform::Platform::Windows) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The WSL workspace folder must be an absolute Linux or Windows path",
        ));
    }
    let argv = vec!["wslpath".into(), "-a".into(), "-u".into(), folder.into()];
    let result = crate::container_runtimes::capture(target, &argv, NAME_TIMEOUT)?
        .filter(|o| o.status.is_some_and(|s| s.success()))
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Cannot resolve the folder in WSL",
            )
        })?;
    let resolved = String::from_utf8_lossy(&result.stdout).trim().to_string();
    if !resolved.starts_with('/') {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid WSL workspace folder",
        ));
    }
    Ok(resolved)
}

fn configs_for(folder: &str, target: &RuntimeTarget) -> Result<Vec<DevcontainerConfig>, CoreError> {
    target.validate()?;
    let folder = resolve_folder(folder, target)?;
    configs_for_resolved(&folder, target)
}

fn configs_for_resolved(
    folder: &str,
    target: &RuntimeTarget,
) -> Result<Vec<DevcontainerConfig>, CoreError> {
    if target.wsl.is_none() {
        return Ok(discover(Path::new(folder)));
    }
    // Positional arguments keep workspace paths out of shell source. Enumeration is read-only.
    let script = r#"cd -- "$1" || exit 3
for file in .devcontainer/devcontainer.json .devcontainer.json .devcontainer/*/devcontainer.json; do
 [ ! -f "$file" ] || printf '%s\0' "$file"
done"#;
    let argv = vec![
        "sh".into(),
        "-c".into(),
        script.into(),
        "lithe-configs".into(),
        folder.into(),
    ];
    let outcome = crate::container_runtimes::capture(target, &argv, NAME_TIMEOUT)?
        .ok_or_else(|| CoreError::new(ErrorCode::RuntimeMissing, "WSL is unavailable"))?;
    if !outcome.status.is_some_and(|s| s.success()) {
        return Err(CoreError::new(
            ErrorCode::WorkspaceNotFound,
            "The folder does not exist in WSL",
        ));
    }
    let mut found = Vec::new();
    for path in String::from_utf8_lossy(&outcome.stdout)
        .split('\0')
        .filter(|path| !path.is_empty() && !path.chars().any(char::is_control))
        .take(66)
    {
        if !found
            .iter()
            .any(|config: &DevcontainerConfig| config.path == path)
        {
            found.push(DevcontainerConfig {
                path: path.into(),
                name: None,
            });
        }
    }
    Ok(found)
}
