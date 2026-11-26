//! Discovers Docker contexts and WSL engines without changing the user's active context.
//! Targets are carried unchanged through build, inspection and connection; no daemon is started.

use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::{
    transport::{is_wsl_distribution, wsl_program},
    ContainerEngine,
};
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::process::Command;
use std::time::{Duration, Instant};

/// Maximum contexts and WSL distributions probed in one discovery.
const MAX_TARGETS: usize = 24;

/// A CLI target. WSL paths and commands are resolved in the named distribution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTarget {
    /// Docker or Podman executable syntax.
    pub engine: ContainerEngine,
    /// Explicit Docker context; omitted preserves the upstream environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// WSL distribution owning the engine and Dev Container CLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsl: Option<String>,
}

impl RuntimeTarget {
    /// Rejects option-like names and unsupported context/engine combinations before I/O.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.context.as_deref().is_some_and(|name| {
            !lithe_remote::client::container::is_context_name(name)
                || self.engine != ContainerEngine::Docker
        }) || self
            .wsl
            .as_deref()
            .is_some_and(|name| !is_wsl_distribution(name))
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid container runtime target",
            ));
        }
        Ok(())
    }

    /// Wraps a command in WSL and applies context only to this child process.
    pub(crate) fn command(&self, argv: &[String]) -> Result<Command, CoreError> {
        self.validate()?;
        let mut words = argv.to_vec();
        if let Some(context) = &self.context {
            words.splice(
                0..0,
                [
                    "env".into(),
                    "-u".into(),
                    "DOCKER_HOST".into(),
                    "-u".into(),
                    "DOCKER_TLS_VERIFY".into(),
                    "-u".into(),
                    "DOCKER_CERT_PATH".into(),
                    format!("DOCKER_CONTEXT={context}"),
                ],
            );
        }
        let mut process = if let Some(distribution) = &self.wsl {
            let mut process = Command::new(wsl_program());
            process
                .args(["--distribution", distribution, "--exec"])
                .args(words);
            process.env("WSL_UTF8", "1");
            process
        } else {
            let mut process = Command::new(&argv[0]);
            process.args(&argv[1..]);
            if let Some(context) = &self.context {
                process
                    .env_remove("DOCKER_HOST")
                    .env_remove("DOCKER_TLS_VERIFY")
                    .env_remove("DOCKER_CERT_PATH")
                    .env("DOCKER_CONTEXT", context);
            }
            process
        };
        process.stdin(std::process::Stdio::null());
        Ok(process)
    }
}

/// A discovered target, using upstream names rather than an application-name heuristic.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRuntime {
    /// Stable opaque picker key, also used for namespacing container rows.
    pub id: String,
    /// Exact engine/context/distribution used by subsequent operations.
    #[serde(flatten)]
    pub target: RuntimeTarget,
    /// Context description, such as OrbStack or Docker Desktop, from the engine CLI.
    pub label: String,
    /// Whether upstream selected this context at discovery time.
    pub current: bool,
    /// Whether the daemon answered a bounded read-only `info` request.
    pub available: bool,
    /// Upstream failure, redacted for display and never logged as free text.
    pub diagnostics: String,
}

/// Discovery result; an empty list means no engine CLI was found.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtimes {
    /// Local contexts first, followed by each WSL distribution's contexts.
    pub runtimes: Vec<ContainerRuntime>,
    /// Preferred usable native context, falling back to the first usable WSL engine.
    pub selected_runtime: Option<String>,
    /// Discovery failures not associated with a listed runtime, redacted for display.
    pub diagnostics: Vec<String>,
}

/// Reads installed CLI contexts and, on Windows, engines in user WSL distributions.
pub fn discover() -> Result<Runtimes, CoreError> {
    let mut result = Runtimes::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    discover_on(None, &mut result, deadline)?;
    let distributions = match crate::wsl::list() {
        Ok(distributions) => distributions,
        Err(error) if matches!(error.code, ErrorCode::Cancelled) => return Err(error),
        Err(error) => {
            result.diagnostics.push(error.message);
            crate::wsl::Distributions::default()
        }
    };
    if !distributions.diagnostics.is_empty() {
        result.diagnostics.push(distributions.diagnostics);
    }
    let distributions_truncated = distributions.distributions.len() > 8;
    for distribution in distributions.distributions.into_iter().take(8) {
        discover_on(Some(distribution), &mut result, deadline)?;
    }
    if distributions_truncated || result.runtimes.len() >= MAX_TARGETS || Instant::now() >= deadline
    {
        result.diagnostics.push(
            "Container runtime discovery reached its limit; some environments were not probed"
                .into(),
        );
    }
    result.selected_runtime = result
        .runtimes
        .iter()
        .find(|r| r.available && r.current && r.target.wsl.is_none())
        .or_else(|| result.runtimes.iter().find(|r| r.available))
        .or_else(|| result.runtimes.first())
        .map(|r| r.id.clone());
    Ok(result)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContextRow {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    current: bool,
    #[serde(default)]
    docker_endpoint: String,
}

fn discover_on(
    distribution: Option<String>,
    result: &mut Runtimes,
    deadline: Instant,
) -> Result<(), CoreError> {
    for engine in [ContainerEngine::Docker, ContainerEngine::Podman] {
        if result.runtimes.len() >= MAX_TARGETS || Instant::now() >= deadline {
            break;
        }
        let base = RuntimeTarget {
            engine,
            context: None,
            wsl: distribution.clone(),
        };
        let argv = match engine {
            ContainerEngine::Docker => vec![
                "docker".into(),
                "context".into(),
                "ls".into(),
                "--format".into(),
                "{{json .}}".into(),
            ],
            ContainerEngine::Podman => vec!["podman".into(), "--version".into()],
        };
        let outcome = match capture(
            &base,
            &argv,
            Duration::from_secs(5).min(deadline.saturating_duration_since(Instant::now())),
        ) {
            Ok(Some(outcome)) => outcome,
            Ok(None) => continue,
            Err(error) if matches!(error.code, ErrorCode::Cancelled) => return Err(error),
            Err(error) => {
                result.diagnostics.push(error.message);
                continue;
            }
        };
        if !outcome.status.is_some_and(|s| s.success()) {
            // WSL's env/exec reports a missing optional CLI with 127.
            if outcome
                .status
                .and_then(|s| s.code())
                .is_some_and(|code| code == 127)
            {
                continue;
            }
            result
                .diagnostics
                .push(crate::probe::diagnostics(&String::from_utf8_lossy(
                    &outcome.stderr,
                )));
            continue;
        }
        let mut targets = Vec::new();
        if engine == ContainerEngine::Docker {
            for line in String::from_utf8_lossy(&outcome.stdout)
                .lines()
                .take(MAX_TARGETS)
            {
                let row: ContextRow = match serde_json::from_str(line) {
                    Ok(row) => row,
                    Err(_) => {
                        result
                            .diagnostics
                            .push("Invalid Docker context listing".into());
                        continue;
                    }
                };
                // Dev Container's bind mount requires a daemon on this computer or in this WSL.
                if !row.docker_endpoint.starts_with("unix://")
                    && !row.docker_endpoint.starts_with("npipe://")
                {
                    continue;
                }
                let label = if row.description.is_empty() {
                    row.name.clone()
                } else {
                    row.description
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(128)
                        .collect()
                };
                targets.push((
                    RuntimeTarget {
                        context: Some(row.name),
                        ..base.clone()
                    },
                    label,
                    row.current,
                ));
            }
        } else {
            targets.push((base, "Podman".into(), true));
        }
        for (target, label, current) in targets {
            if result.runtimes.len() >= MAX_TARGETS || Instant::now() >= deadline {
                break;
            }
            if target.validate().is_err() {
                result
                    .diagnostics
                    .push("Invalid container runtime target".into());
                continue;
            }
            let info = vec![
                engine.name().into(),
                "info".into(),
                "--format".into(),
                "{{.ServerVersion}}".into(),
            ];
            let status = capture(
                &target,
                &info,
                Duration::from_secs(3).min(deadline.saturating_duration_since(Instant::now())),
            );
            let (available, diagnostics) = match status {
                Ok(Some(outcome)) => (
                    outcome.status.is_some_and(|s| s.success()),
                    crate::probe::diagnostics(&String::from_utf8_lossy(&outcome.stderr)),
                ),
                Ok(None) => (false, String::new()),
                Err(error) if matches!(error.code, ErrorCode::Cancelled) => return Err(error),
                Err(error) => (false, error.message),
            };
            let id = serde_json::to_string(&target).expect("runtime target encodes");
            result.runtimes.push(ContainerRuntime {
                id,
                target,
                label,
                current,
                available,
                diagnostics,
            });
        }
    }
    Ok(())
}

/// Runs one read-only probe with cancellation, a local deadline and process-tree cleanup.
pub(crate) fn capture(
    target: &RuntimeTarget,
    argv: &[String],
    timeout: Duration,
) -> Result<Option<lithe_git_host::Outcome>, CoreError> {
    let mut command = target.command(argv)?;
    let deadline = Instant::now() + timeout;
    let mut stopped = None;
    let outcome = lithe_git_host::run(
        &mut command,
        None,
        || {
            if let Err(error) = lithe_protocol::cancellation::check() {
                stopped = Some(error);
            } else if Instant::now() >= deadline {
                stopped = Some(CoreError::new(
                    ErrorCode::TimedOut,
                    "Container runtime detection timed out",
                ));
            }
            stopped.is_some()
        },
        || {},
        |_, _| {},
    );
    if let Some(error) = stopped {
        return Err(error);
    }
    match &outcome.failure {
        None => Ok(Some(outcome)),
        // log-exempt: Expected absence: an optional container engine is not installed.
        Some(lithe_git_host::Failure::Start(error)) if error.kind() == ErrorKind::NotFound => {
            Ok(None)
        }
        Some(failure) => Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Container runtime probe failed",
        )
        .with_details(format!("{failure:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_environment_is_child_local_and_wsl_arguments_are_not_shell_source() {
        let target = RuntimeTarget {
            engine: ContainerEngine::Docker,
            context: Some("orbstack".into()),
            wsl: None,
        };
        let command = target.command(&["docker".into(), "info".into()]).unwrap();
        assert_eq!(command.get_program(), "docker");
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "DOCKER_CONTEXT"
                && value == Some(std::ffi::OsStr::new("orbstack"))));
        let wsl = RuntimeTarget {
            wsl: Some("Ubuntu-24.04".into()),
            ..target
        };
        let command = wsl
            .command(&["devcontainer".into(), "up".into(), "/a path/$x".into()])
            .unwrap();
        let arguments: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &arguments[..3],
            &["--distribution", "Ubuntu-24.04", "--exec"]
        );
        assert_eq!(arguments.last().unwrap(), "/a path/$x");
        assert!(arguments.iter().any(|a| a == "DOCKER_CONTEXT=orbstack"));
    }
}
