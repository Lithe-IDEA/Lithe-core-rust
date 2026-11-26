//! `remote.listContainers`: the running containers of one container engine,
//! on this machine or on the host of a connected session, for the connection
//! window.
//!
//! The list comes from the engine CLI's `ps` each time the window asks;
//! nothing is saved. Lithe only lists and enters running containers: it never
//! starts, creates or changes one, so stopped containers are not offered
//! (S32). Connecting to a container is `remote.connect` with `container`.

use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::container::{list_command, parse_containers, MAX_LIST_BYTES};
use lithe_remote::client::{ContainerEngine, ListedContainer};
use lithe_remote::protocol::methods;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Command;
use std::time::{Duration, Instant};

/// How long the engine CLI may take. The first call can start the engine's
/// service or virtual machine, which takes a few seconds.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// `remote.listContainers` request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListRequest {
    /// Engine whose CLI lists the containers.
    pub engine: ContainerEngine,
    /// Connected session whose host runs the engine; this machine when
    /// absent.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// `remote.listContainers` result.
#[derive(Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Containers {
    /// The engine CLI ran. `false` when the host has no such CLI.
    pub available: bool,
    /// Running containers in the engine's order, at most
    /// [`lithe_remote::client::container::MAX_CONTAINERS`].
    pub containers: Vec<ListedContainer>,
    /// What the CLI printed when it failed, such as the message that its
    /// service is not running, redacted; empty otherwise. Shown, never
    /// logged.
    pub diagnostics: String,
}

/// The running containers `request` asks for.
pub fn list(request: ListRequest) -> Result<Containers, CoreError> {
    list_with(request, None)
}

/// `program` replaces the engine CLI on this machine or on the host; tests
/// use a stand-in.
pub(crate) fn list_with(
    request: ListRequest,
    program: Option<&str>,
) -> Result<Containers, CoreError> {
    let mut argv = list_command(request.engine);
    if let Some(program) = program {
        argv[0] = program.into();
    }
    match request.session_id {
        None => list_here(&argv),
        Some(session_id) => list_on_host(&session_id, argv),
    }
}

/// Runs the listing on this machine, in Core's own environment, which holds
/// the login shell's `PATH` and the variables, such as `DOCKER_HOST`, that
/// choose the engine.
fn list_here(argv: &[String]) -> Result<Containers, CoreError> {
    let (program, arguments) = argv.split_first().expect("the listing names the engine");
    let mut process = Command::new(program);
    process.args(arguments);
    let deadline = Instant::now() + LIST_TIMEOUT;
    let mut stop = None;
    let outcome = lithe_git_host::run(
        &mut process,
        None,
        || {
            if let Err(error) = lithe_protocol::cancellation::check() {
                stop = Some(error);
            } else if Instant::now() >= deadline {
                stop = Some(timed_out());
            }
            stop.is_some()
        },
        || {},
        |_, _| {},
    );
    if let Some(error) = stop {
        return Err(error);
    }
    match outcome.failure {
        None => {}
        // No such CLI on this machine.
        Some(lithe_git_host::Failure::Start(_)) => return Ok(Containers::default()),
        Some(failure) => {
            return Err(
                CoreError::new(ErrorCode::ProcessFailed, "The container engine failed")
                    .with_details(format!("{failure:?}")),
            )
        }
    }
    Ok(answer(
        outcome.status.is_some_and(|status| status.success()),
        &outcome.stdout,
        &outcome.stderr,
    ))
}

/// Runs the listing on the host of session `session_id`, in the server's
/// environment there. The connection later enters the container from the
/// same kind of non-login shell that started the server, so the listing finds
/// the CLI, and the engine it talks to, exactly when the connection will.
fn list_on_host(session_id: &str, argv: Vec<String>) -> Result<Containers, CoreError> {
    let params = methods::SpawnParams {
        process_id: 0,
        argv,
        cwd: None,
        environment: methods::EnvironmentSource::Server,
        env: BTreeMap::new(),
        pipe_stdin: false,
        label: None,
    };
    let captured = match crate::host::capture(session_id, params, LIST_TIMEOUT, MAX_LIST_BYTES) {
        Ok(captured) => captured,
        // The host could not start the CLI: it has no such engine.
        Err(error) if is_spawn_failure(&error) => return Ok(Containers::default()),
        Err(error) if matches!(error.code, ErrorCode::TimedOut) => return Err(timed_out()),
        Err(error) => return Err(error),
    };
    // A shell wrapper that found no such command.
    if captured.exit["exitCode"] == 127 {
        return Ok(Containers::default());
    }
    Ok(answer(
        captured.exit["exitCode"] == 0,
        &captured.stdout,
        &captured.stderr,
    ))
}

/// A failed `process.spawn`, which the host reports when the program does
/// not exist or may not run.
fn is_spawn_failure(error: &CoreError) -> bool {
    matches!(error.code, ErrorCode::ProcessFailed)
        && error
            .details
            .as_deref()
            .and_then(|details| serde_json::from_str::<serde_json::Value>(details).ok())
            .is_some_and(|details| details["remoteCode"] == "SPAWN_FAILED")
}

fn timed_out() -> CoreError {
    CoreError::new(
        ErrorCode::TimedOut,
        "The container engine did not list the containers in time",
    )
}

/// The listing of a CLI that ran: its containers when it succeeded, or what
/// it printed about the failure.
fn answer(succeeded: bool, stdout: &[u8], stderr: &[u8]) -> Containers {
    if succeeded {
        return Containers {
            available: true,
            containers: parse_containers(stdout),
            diagnostics: String::new(),
        };
    }
    let mut printed = String::from_utf8_lossy(stdout).into_owned();
    printed.push('\n');
    printed.push_str(&String::from_utf8_lossy(stderr));
    Containers {
        available: true,
        containers: Vec::new(),
        diagnostics: crate::probe::diagnostics(&printed),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A stand-in engine CLI: a shell script in its own directory.
    struct FakeEngine {
        root: PathBuf,
        program: String,
    }

    impl FakeEngine {
        fn new(label: &str, body: &str) -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "lithe-container-list-{label}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let program = root.join("docker");
            std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                root,
                program: program.to_string_lossy().into_owned(),
            }
        }
    }

    impl Drop for FakeEngine {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn here(engine: ContainerEngine) -> ListRequest {
        ListRequest {
            engine,
            session_id: None,
        }
    }

    #[test]
    fn listings_run_ps_on_this_machine_and_read_its_lines() {
        let fake = FakeEngine::new(
            "running",
            r#"printf '%s\n' "$@" > "$0.arguments"
printf 'abc123\tweb,alias\tnginx:1\tUp 2 hours\n'"#,
        );
        let listed = list_with(here(ContainerEngine::Podman), Some(&fake.program)).unwrap();
        assert_eq!(
            listed,
            Containers {
                available: true,
                containers: vec![ListedContainer {
                    id: "abc123".into(),
                    name: "web".into(),
                    image: "nginx:1".into(),
                    status: "Up 2 hours".into(),
                }],
                diagnostics: String::new(),
            }
        );
        let arguments = std::fs::read_to_string(format!("{}.arguments", fake.program)).unwrap();
        assert_eq!(
            arguments,
            "ps\n--no-trunc\n--format\n{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\n"
        );
    }

    #[test]
    fn a_missing_engine_is_unavailable() {
        let fake = FakeEngine::new("missing", "exit 0");
        let missing = fake.root.join("absent").to_string_lossy().into_owned();
        assert_eq!(
            list_with(here(ContainerEngine::Docker), Some(&missing)).unwrap(),
            Containers::default()
        );
    }

    #[test]
    fn requests_name_a_known_engine_and_nothing_else() {
        let decode = |value| serde_json::from_value::<ListRequest>(value);
        let request = decode(serde_json::json!({"engine": "podman", "sessionId": "s"})).unwrap();
        assert_eq!(request.engine, ContainerEngine::Podman);
        assert_eq!(request.session_id.as_deref(), Some("s"));
        assert!(decode(serde_json::json!({"engine": "nerdctl"})).is_err());
        assert!(decode(serde_json::json!({"engine": "docker", "program": "sh"})).is_err());
        assert!(decode(serde_json::json!({})).is_err());
    }
}

#[cfg(test)]
mod result_tests {
    use super::*;

    #[test]
    fn failed_listings_are_available_with_the_message_and_no_containers() {
        // This contract concerns an engine that ran and exited nonzero. A
        // freshly written executable adds an unrelated spawn failure boundary;
        // CLI launch and arguments are covered by the Unix fixture above.
        let listed = answer(
            false,
            b"",
            b"Cannot connect to the Docker daemon. Is the docker daemon running?\n",
        );
        assert!(listed.available);
        assert!(listed.containers.is_empty());
        assert_eq!(
            listed.diagnostics,
            "Cannot connect to the Docker daemon. Is the docker daemon running?"
        );
    }
}
