//! `remote.devcontainerConfigs` and `remote.devcontainerUp`: a local
//! folder's Dev Container, built and started by the reference Dev Container
//! CLI, for the connection window.
//!
//! The window lists the folder's `devcontainer.json` files, then asks for
//! `up` with the one the user picked. The CLI builds or reuses the container
//! exactly as the configuration says, mounts included, and runs its
//! lifecycle commands; its log streams to the window as `devcontainerOutput`
//! events, redacted and never logged. The result names the container, the
//! user and the workspace folder inside it, which the window then enters
//! with `remote.connect`'s `container` and opens. Nothing is saved.
//!
//! Building runs the folder's code on this machine (`initializeCommand`,
//! Dockerfiles, feature scripts), so `up` runs only when the host says the
//! folder is trusted (`projectCodeAllowed`), the same rule as Git. Lithe
//! itself adds no mounts and no options to the configuration (S32).

use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::devcontainer::{
    discover, name_command, parse_name, parse_up_result, up_command, UpFailure,
};
use lithe_remote::client::{ContainerEngine, DevcontainerConfig};
use lithe_request_context::events as execution_events;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
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
}

/// `remote.devcontainerConfigs` result.
#[derive(Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Configs {
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
    let folder = local_folder(&request.folder)?;
    Ok(Configs {
        configs: discover(&folder),
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
    let folder = local_folder(&request.folder)?;
    let config = match request.config.as_deref() {
        None => None,
        Some(config) => {
            if !discover(&folder).iter().any(|listed| listed.path == config) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The configuration is not one of the folder's devcontainer.json files",
                ));
            }
            Some(folder.join(config).to_string_lossy().into_owned())
        }
    };
    let mut argv = up_command(request.engine, &folder.to_string_lossy(), config.as_deref());
    if let Some(program) = program {
        argv[0] = program.into();
    }
    let mut log = Log::default();
    let outcome = run_here(&argv, UP_TIMEOUT, |stream, bytes| {
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
            let container = container_name(request.engine, &started.container_id, program)
                .unwrap_or_else(|| started.container_id.clone());
            Ok(Up {
                container: Some(Devcontainer {
                    engine: request.engine,
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
fn container_name(engine: ContainerEngine, id: &str, program: Option<&str>) -> Option<String> {
    let mut argv = name_command(engine, id);
    if let Some(program) = program {
        argv[0] = program.into();
    }
    let outcome = run_here(&argv, NAME_TIMEOUT, |_, _| {}).ok()?;
    if !outcome.status.is_some_and(|status| status.success()) {
        return None;
    }
    parse_name(&outcome.stdout)
}

/// Why [`run_here`] has no outcome.
enum Ran {
    /// Cancelled, out of time or failed while running.
    Stopped(CoreError),
    /// The program does not exist here.
    Missing,
}

/// Runs `argv` on this machine, in Core's own environment, which holds the
/// login shell's `PATH` and the variables, such as `DOCKER_HOST`, that
/// choose the engine.
fn run_here(
    argv: &[String],
    timeout: Duration,
    output: impl FnMut(lithe_git_host::Stream, &[u8]),
) -> Result<lithe_git_host::Outcome, Ran> {
    let (program, arguments) = argv.split_first().expect("the command names a program");
    let mut process = Command::new(program);
    process.args(arguments);
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
        Some(lithe_git_host::Failure::Start(_)) => Err(Ran::Missing),
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
            folder: "app".into(),
        });
        assert!(matches!(
            relative.unwrap_err().code,
            ErrorCode::InvalidRequest
        ));
        let absent = configs(ConfigsRequest {
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
