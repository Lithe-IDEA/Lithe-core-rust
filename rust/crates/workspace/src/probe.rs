//! Remote development connections: checking that a saved SSH connection
//! reaches a usable host before any workspace is opened on it.
//!
//! The command builds the transport chain with `lithe_remote::client`, runs
//! its platform probe through the system OpenSSH client, and relays every
//! question OpenSSH asks (passwords, key passphrases, one-time codes, unknown
//! host keys) to the host UI through the shared AskPass session. Core never
//! stores an answer; hosts keep saved secrets in their own secure storage.
//!
//! Note: .agents/notes/implemented/feature/2026-10-24-remote-ssh-connections.md

use lithe_git_host::authentication::Challenge;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::bootstrap::{parse_probe, probe_command};
use lithe_remote::client::connections::SshConnection;
use lithe_remote::client::{Hop, SshHop, TransportChain};
use lithe_request_context::events as execution_events;
use serde::{Deserialize, Serialize};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

/// Upper bound for one probe, including the time the user needs to answer
/// prompts. Each prompt is additionally bounded by the AskPass session.
const PROBE_DEADLINE: Duration = Duration::from_secs(5 * 60);

/// Bytes of OpenSSH's standard error kept as diagnostics.
const DIAGNOSTICS_LIMIT: usize = 4 * 1024;

/// `remote.probe`: the saved connection to check.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeRequest {
    connection: SshConnection,
}

/// Stable outcome of a probe. Failures to connect are results, not Core
/// errors, so the host can show a specific explanation for each.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeStatus {
    /// A shell ran on the host and a server build exists for it.
    Connected,
    /// The host answered, but no server build exists for its platform.
    UnsupportedPlatform,
    /// The server rejected every offered credential.
    AuthenticationFailed,
    /// The host key is unknown and was declined, or it changed.
    HostKeyRejected,
    /// The name did not resolve, or the host could not be reached in time.
    Unreachable,
    /// The connection failed for another reason; see `diagnostics`.
    Failed,
}

/// Platform facts the host's shell reported.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbedHost {
    /// `uname -s`, for example `Linux` or `Darwin`.
    pub(crate) system: String,
    /// `uname -m`, for example `x86_64` or `arm64`.
    pub(crate) machine: String,
    pub(crate) home_directory: Option<String>,
    /// Rust target of the server build for this host, or `None`.
    pub(crate) server_target: Option<&'static str>,
    /// Facts for display, such as the distribution and memory.
    pub(crate) details: lithe_remote::client::bootstrap::HostDetails,
}

/// `remote.probe` result: the outcome, and the host when its shell ran.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub(crate) status: ProbeStatus,
    /// Present when the host's shell ran the probe.
    pub(crate) host: Option<ProbedHost>,
    /// Redacted tail of OpenSSH's standard error.
    pub(crate) diagnostics: String,
}

/// Probes the host named by the request's connection.
pub fn probe(request: ProbeRequest) -> Result<ProbeResult, CoreError> {
    probe_with(request, None)
}

/// `ssh_program` replaces the `ssh` found on `PATH`; tests use a stand-in.
fn probe_with(request: ProbeRequest, ssh_program: Option<&str>) -> Result<ProbeResult, CoreError> {
    let mut hop = ssh_hop(&request.connection)?;
    if let Some(program) = ssh_program {
        hop.program = program.into();
    }
    let chain = TransportChain::new(vec![Hop::Ssh(hop)]);
    let argv = chain.command(&probe_command()).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid SSH connection")
            .with_details(error.to_string())
    })?;
    let (program, arguments) = argv
        .split_first()
        .expect("an SSH hop always produces a command");
    let mut process = Command::new(program);
    process.args(arguments);
    // Before the AskPass variables, which must reach ssh.
    chain.restrict_environment(&mut process);

    let mut authentication = lithe_git_host::authentication::Session::new().map_err(|error| {
        CoreError::new(
            ErrorCode::ProcessStartFailed,
            "Could not prepare SSH authentication",
        )
        .with_details(error.to_string())
    })?;
    authentication.configure(&mut process).map_err(|error| {
        CoreError::new(
            ErrorCode::ProcessStartFailed,
            "Could not configure SSH authentication",
        )
        .with_details(error.to_string())
    })?;

    let deadline = Instant::now() + PROBE_DEADLINE;
    let mut stop = None;
    let output = lithe_git_host::run(
        &mut process,
        None,
        || {
            if let Err(error) = lithe_protocol::cancellation::check() {
                stop = Some(error);
            } else if Instant::now() >= deadline {
                stop = Some(CoreError::new(
                    ErrorCode::TimedOut,
                    "The SSH connection did not finish in time",
                ));
            } else {
                match authentication.poll() {
                    Ok(challenges) => challenges.into_iter().for_each(relay_challenge),
                    Err(error) => {
                        stop = Some(
                            CoreError::new(
                                ErrorCode::ProcessFailed,
                                "SSH authentication transport failed",
                            )
                            .with_details(error.to_string()),
                        )
                    }
                }
            }
            stop.is_some()
        },
        || {},
        |_, _| {},
    );
    if let Some(error) = stop {
        return Err(error);
    }
    match output.failure {
        None => {}
        Some(lithe_git_host::Failure::Start(error)) => {
            return Err(
                CoreError::new(ErrorCode::RuntimeMissing, "Could not start the SSH client")
                    .with_details(error.to_string()),
            )
        }
        Some(failure) => {
            return Err(
                CoreError::new(ErrorCode::ProcessFailed, "The SSH client failed")
                    .with_details(format!("{failure:?}")),
            )
        }
    }
    Ok(classify(output.status, &output.stdout, &output.stderr))
}

/// Answers a prompt that a running probe relayed.
pub fn respond(request_id: &str, answer: Option<String>) -> bool {
    lithe_git_host::authentication::respond(request_id, answer)
}

/// Relays one OpenSSH question as an `authentication` event. `kind` tells
/// the window which form to show; a host key question also carries the
/// key's algorithm and fingerprint, which are public.
pub(crate) fn relay_challenge(challenge: Challenge) {
    execution_events::emit(challenge_event(&challenge));
}

/// The `authentication` event of `challenge`.
pub(crate) fn challenge_event(challenge: &Challenge) -> serde_json::Value {
    let prompt = lithe_remote::client::prompt::classify(&challenge.prompt);
    let mut event = serde_json::json!({
        "type": "authentication",
        "requestId": challenge.request_id,
        "prompt": execution_events::redact(&challenge.prompt),
        "secret": challenge.secret,
        "attempt": challenge.attempt,
        "kind": prompt.kind,
    });
    if let Some(key_type) = prompt.key_type {
        event["keyType"] = key_type.into();
    }
    if let Some(fingerprint) = prompt.fingerprint {
        event["fingerprint"] = fingerprint.into();
    }
    event
}

/// The SSH hop of `connection`, refused with the field and the reason when
/// one is malformed. The rules are in `lithe_remote::client::connections`.
pub(crate) fn ssh_hop(connection: &SshConnection) -> Result<SshHop, CoreError> {
    connection
        .clone()
        .normalized()
        .hop()
        .map_err(crate::connections::invalid_request)
}

/// Interprets the finished probe. OpenSSH reports connection failures only
/// as text, so the categories match its stable English messages.
pub(crate) fn classify(exit: Option<ExitStatus>, stdout: &[u8], stderr: &[u8]) -> ProbeResult {
    let stderr_text = String::from_utf8_lossy(stderr);
    let diagnostics = diagnostics(&stderr_text);
    if let Some(report) = parse_probe(stdout) {
        let server_target = report.target().map(|target| target.rust_target());
        return ProbeResult {
            status: if server_target.is_some() {
                ProbeStatus::Connected
            } else {
                ProbeStatus::UnsupportedPlatform
            },
            host: Some(ProbedHost {
                system: report.system,
                machine: report.machine,
                home_directory: report.home,
                server_target,
                details: report.details,
            }),
            diagnostics,
        };
    }
    let lower = stderr_text.to_ascii_lowercase();
    let mentions = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    let status = if mentions(&[
        "host key verification failed",
        "remote host identification has changed",
    ]) {
        ProbeStatus::HostKeyRejected
    } else if mentions(&[
        "permission denied",
        "too many authentication failures",
        "no supported authentication methods",
    ]) {
        ProbeStatus::AuthenticationFailed
    } else if mentions(&[
        "could not resolve hostname",
        "connection refused",
        "connection timed out",
        "operation timed out",
        "no route to host",
        "network is unreachable",
    ]) {
        ProbeStatus::Unreachable
    } else {
        ProbeStatus::Failed
    };
    let diagnostics = if diagnostics.is_empty() {
        // Some failures, such as a remote shell that exits early, print
        // nothing; the exit status is then the only clue.
        match exit.and_then(|exit| exit.code()) {
            Some(code) => format!("ssh exited with status {code}"),
            None => "ssh was terminated by a signal".into(),
        }
    } else {
        diagnostics
    };
    ProbeResult {
        status,
        host: None,
        diagnostics,
    }
}

/// Keeps the end of the output, where OpenSSH prints the reason it gave up,
/// and removes credentials and home directory paths.
pub(crate) fn diagnostics(stderr: &str) -> String {
    let trimmed = stderr.trim();
    let mut start = trimmed.len().saturating_sub(DIAGNOSTICS_LIMIT);
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    execution_events::redact(&trimmed[start..])
}

#[cfg(test)]
mod tests;
