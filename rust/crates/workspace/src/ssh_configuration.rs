//! `remote.effectiveSettings` and `remote.sshConfigHosts`: what OpenSSH
//! makes of a saved connection, and the hosts the user's SSH configuration
//! names, for the overview and the import page (design §3.1).
//!
//! Both run the local `ssh -G`, which prints the configuration a connection
//! would use and exits without connecting. It runs with the transport's
//! restricted environment and a deadline, because `Match exec` lines of the
//! user's configuration still run commands. Configuration files are only
//! read below the home directory the request names. The rules are in
//! `lithe_remote::client::ssh_config`.

use crate::connections::parse;
use crate::probe::{diagnostics, ssh_hop};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::connections::{Connections, SshConnection};
use lithe_remote::client::ssh_config::{self, HostName, Output, MAX_ALIASES};
use lithe_remote::client::{Hop, SshHop, TransportChain};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Upper bound for one `ssh -G`.
const COMMAND_DEADLINE: Duration = Duration::from_secs(10);

/// Upper bound for reading every alias of an import.
const IMPORT_DEADLINE: Duration = Duration::from_secs(30);

/// `remote.effectiveSettings`: the connection to explain.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectiveRequest {
    connection: SshConnection,
    /// Remote preferences document, whose SSH program runs `ssh -G`;
    /// `null` or absent is every value automatic.
    #[serde(default)]
    preferences: Option<String>,
}

/// `remote.sshConfigHosts`: where the configuration is, and the saved
/// connections to compare with.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostsRequest {
    /// The user's home directory; `.ssh/config` below it is read.
    home_directory: String,
    /// Current connections document; `null` when the source does not exist.
    document: Option<String>,
    /// Remote preferences document, whose SSH program runs `ssh -G`;
    /// `null` or absent is every value automatic.
    #[serde(default)]
    preferences: Option<String>,
}

/// The values the connection uses and where each comes from.
pub fn effective_settings(request: EffectiveRequest) -> Result<Value, CoreError> {
    effective_with(request, None)
}

/// `ssh_program` replaces the preferences' SSH program; tests use a
/// stand-in.
fn effective_with(
    request: EffectiveRequest,
    ssh_program: Option<&str>,
) -> Result<Value, CoreError> {
    let program = crate::preferences::ssh_program(ssh_program, request.preferences.as_deref())?;
    let connection = request.connection.normalized();
    let hop = with_program(ssh_hop(&connection)?, &program);
    let deadline = Instant::now() + COMMAND_DEADLINE;
    let configured = print_configuration(&hop, true, deadline)?;
    let baseline = print_configuration(&hop, false, deadline)?;
    let settings = ssh_config::effective_settings(&connection, &configured, &baseline)
        .map_err(|error| unreadable_output(error.key))?;
    Ok(json!(settings))
}

/// The `Host` aliases of the user's configuration, in the order OpenSSH
/// reads them, with what each resolves to and whether a saved connection
/// already reaches it.
///
/// Every alias is listed once, regardless of case, up to
/// [`MAX_ALIASES`]. `status` is `importable`, `exists` (with the saved
/// entry), `pattern` for names with wildcards, `invalid` for names Lithe
/// refuses as a host, or `unreadable` when `ssh -G` failed for it.
/// `usedAsJumpBy` names the aliases that jump through it.
pub fn ssh_config_hosts(request: HostsRequest) -> Result<Value, CoreError> {
    hosts_with(request, None)
}

fn hosts_with(request: HostsRequest, ssh_program: Option<&str>) -> Result<Value, CoreError> {
    let home = PathBuf::from(&request.home_directory);
    if !home.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The home directory must be an absolute path",
        ));
    }
    let saved = parse(request.document.as_deref())?;
    let program = crate::preferences::ssh_program(ssh_program, request.preferences.as_deref())?;
    let files = ssh_config::configuration_files(&home);
    let (names, truncated) = distinct_names(&files);

    let deadline = Instant::now() + IMPORT_DEADLINE;
    let mut hosts: Vec<Value> = Vec::with_capacity(names.len());
    let mut jumps: Vec<(String, String)> = Vec::new();
    for name in names {
        let alias = match name {
            HostName::Pattern(pattern) => {
                hosts.push(json!({ "alias": pattern, "status": "pattern" }));
                continue;
            }
            HostName::Alias(alias) => alias,
        };
        let connection = SshConnection::to_host(alias.clone());
        let Ok(hop) = ssh_hop(&connection) else {
            hosts.push(json!({ "alias": alias, "status": "invalid" }));
            continue;
        };
        let hop = with_program(hop, &program);
        let resolved =
            print_configuration(&hop, true, deadline.min(Instant::now() + COMMAND_DEADLINE))
                .and_then(|output| {
                    ssh_config::configured_host(&output)
                        .map_err(|error| unreadable_output(error.key))
                });
        let configured = match resolved {
            Ok(configured) => configured,
            // Without a client, or once the user cancelled, no alias can be read.
            Err(error)
                if matches!(error.code, ErrorCode::RuntimeMissing | ErrorCode::Cancelled) =>
            {
                return Err(error)
            }
            Err(_) => {
                hosts.push(json!({ "alias": alias, "status": "unreadable" }));
                continue;
            }
        };
        if let Some(proxy_jump) = &configured.proxy_jump {
            for jump in ssh_config::jump_hosts(proxy_jump) {
                jumps.push((jump.to_lowercase(), alias.clone()));
            }
        }
        let mut host = json!({ "alias": alias, "status": "importable" });
        if let Some(entry) = existing(&saved, &alias, &configured) {
            host["status"] = json!("exists");
            host["existingId"] = json!(entry.id());
            host["existingName"] = json!(entry.name());
        }
        host["resolved"] = json!(configured);
        hosts.push(host);
    }
    for host in &mut hosts {
        let alias = host["alias"].as_str().unwrap_or_default().to_lowercase();
        let users: Vec<&str> = jumps
            .iter()
            .filter(|(jump, _)| *jump == alias)
            .map(|(_, user)| user.as_str())
            .collect();
        host["usedAsJumpBy"] = json!(users);
    }
    let files: Vec<String> = files
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect();
    Ok(json!({
        "files": files,
        "hosts": hosts,
        "truncated": truncated,
    }))
}

/// Names of every file's `Host` lines, each once regardless of case, at most
/// [`MAX_ALIASES`]; `true` when some were left out.
fn distinct_names(files: &[(PathBuf, String)]) -> (Vec<HostName>, bool) {
    let mut seen: Vec<String> = Vec::new();
    let mut names = Vec::new();
    for (_, text) in files {
        for name in ssh_config::host_aliases(text).0 {
            let word = match &name {
                HostName::Alias(word) | HostName::Pattern(word) => word.to_lowercase(),
            };
            if seen.contains(&word) {
                continue;
            }
            if names.len() == MAX_ALIASES {
                return (names, true);
            }
            seen.push(word);
            names.push(name);
        }
    }
    (names, false)
}

/// The saved entry that reaches `alias`, either through the alias itself or
/// through the address it resolves to.
fn existing<'a>(
    saved: &'a Connections,
    alias: &str,
    configured: &ssh_config::ConfiguredHost,
) -> Option<&'a lithe_remote::client::connections::SavedConnection> {
    saved.existing(&SshConnection::to_host(alias)).or_else(|| {
        let resolved: SshConnection = serde_json::from_value(json!({
            "host": configured.host_name,
            "user": configured.user,
            "port": configured.port,
        }))
        .ok()?;
        saved.existing(&resolved)
    })
}

fn with_program(mut hop: SshHop, program: &str) -> SshHop {
    hop.program = program.into();
    hop
}

/// Runs `ssh -G` for `hop`, with or without the configuration files, and
/// reads its output.
fn print_configuration(
    hop: &SshHop,
    user_configuration: bool,
    deadline: Instant,
) -> Result<Output, CoreError> {
    let argv = hop
        .configuration_command(user_configuration)
        .map_err(|error| {
            CoreError::new(ErrorCode::InvalidRequest, "Invalid SSH connection")
                .with_details(error.to_string())
        })?;
    let (program, arguments) = argv
        .split_first()
        .expect("a configuration command always names the client");
    let mut process = Command::new(program);
    process.args(arguments);
    TransportChain::new(vec![Hop::Ssh(hop.clone())]).restrict_environment(&mut process);
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
                    "OpenSSH did not print its configuration in time",
                ));
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
    if !outcome.status.is_some_and(|status| status.success()) {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "OpenSSH could not read its configuration",
        )
        .with_details(diagnostics(&String::from_utf8_lossy(&outcome.stderr))));
    }
    Ok(Output::parse(&String::from_utf8_lossy(&outcome.stdout)))
}

fn unreadable_output(key: &str) -> CoreError {
    CoreError::new(ErrorCode::ParseFailed, "Unexpected ssh -G output")
        .with_details(format!("missing or malformed `{key}`"))
}

#[cfg(test)]
mod tests;
