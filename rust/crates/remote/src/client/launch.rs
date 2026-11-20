//! Probing, installing and starting the server over one connection.
//!
//! All steps run inside one remote shell on one stream, so an SSH host that
//! asks for a password asks only once:
//!
//! 1. the shell reports the platform between markers;
//! 2. the client picks the artifact for it and sends one line naming its
//!    version, SHA-256 and size;
//! 3. the shell checks that the install directories are private to the
//!    remote user, then either keeps an installed binary whose SHA-256 it
//!    computes itself and answers `LITHE-INSTALL-PRESENT`, or answers
//!    `LITHE-INSTALL-NEEDED`, receives exactly `size` bytes under a temporary
//!    name, verifies them, renames them into place and answers
//!    `LITHE-INSTALL-DONE` with how it verified them; a failed step answers
//!    `LITHE-INSTALL-FAILED` with the step instead;
//! 4. the shell `exec`s the server, whose preamble starts the protocol on the
//!    same stream, ready for [`crate::client::RemoteSession::from_streams`].
//!
//! The steps are recorded in the attempt's [`ConnectionLog`]: the probe
//! answer, the deployment and any failure with its stage.
//!
//! An installed binary is never trusted to report its own hash. When the host
//! has neither `sha256sum` nor `shasum`, every connection uploads the binary
//! again instead. The install root, `bin` and the version directory must not
//! be symbolic links, must belong to the remote user and must not be writable
//! by the group or other users; otherwise the launch is refused with the
//! reason, because anyone who can write there can replace the server.
//!
//! The script crosses SSH hops, so it is a single line without `'`, `\` or
//! `!` (see [`crate::client::transport`]).

use crate::client::bootstrap::{parse_probe, ProbeReport, ServerArtifact, INSTALL_HASH_PREFIX};
use crate::client::connection_log::ConnectionLog;
use std::fmt;
use std::io::{self, Read, Write};
use std::time::Instant;

const INSTALL_PRESENT: &str = "LITHE-INSTALL-PRESENT";
const INSTALL_NEEDED: &str = "LITHE-INSTALL-NEEDED";
const INSTALL_DONE: &str = "LITHE-INSTALL-DONE";
const INSTALL_UNSAFE: &str = "LITHE-INSTALL-UNSAFE";
const INSTALL_FAILED: &str = "LITHE-INSTALL-FAILED";
const PROBE_END: &str = "LITHE-PROBE-END";

/// Output accepted before the probe block ends, for example a login banner.
const NOISE_LIMIT: usize = 64 * 1024;

/// `$1` is the install root; empty means `$HOME/.lithe-server`. The remaining
/// arguments are passed to the server, for example `--stdio`.
///
/// Shell functions keep their own positional parameters, so `set --` inside
/// them splits command output without touching the server arguments:
///
/// - `inspect` reads the mode string and numeric owner from `ls -ldn`;
/// - `shared` tests the group and other write bits of that mode string;
/// - `guard` refuses a directory that is a symbolic link, not a directory,
///   owned by someone else or writable by others, and accepts a missing one;
/// - `digest` prints the SHA-256 computed by the host's own tool, or nothing;
/// - `reusable` accepts an installed binary only when it is a private regular
///   file whose computed SHA-256 is exactly the expected one;
/// - `fail` removes a partial upload and answers the failed step.
///
/// Without a hash tool, an upload is accepted when its size matches and the
/// binary reports the expected identity (`checked=identity`); it is still
/// never reused, because `reusable` requires a computed hash.
const LAUNCH_SCRIPT: &str = concat!(
    crate::client::bootstrap::host_details_script!(),
    "echo LITHE-PROBE-BEGIN; uname -s; uname -m; echo \"$HOME\"; echo LITHE-PROBE-END; ",
    "read version sha size name || exit 90; ",
    "root=\"$1\"; [ -n \"$root\" ] || root=\"$HOME/.lithe-server\"; ",
    "dir=\"$root/bin/$name\"; exe=\"$dir/lithe-remote-server\"; uid=$(id -u); tmp=; ",
    "inspect() { set -- $(ls -ldn \"$1\" 2>/dev/null); kind=\"$1\"; owner=\"$3\"; }; ",
    "shared() { case \"$kind\" in ?????w*|????????w*) return 0;; esac; return 1; }; ",
    "refuse() { echo \"LITHE-INSTALL-UNSAFE $1 $2\"; exit 94; }; ",
    "guard() { [ -e \"$1\" ] || [ -L \"$1\" ] || return 0; [ -L \"$1\" ] && refuse symlink \"$1\"; ",
    "inspect \"$1\"; case \"$kind\" in d*) ;; *) refuse type \"$1\";; esac; ",
    "[ \"$owner\" = \"$uid\" ] || refuse owner \"$1\"; shared && refuse writable \"$1\"; return 0; }; ",
    "guarded() { guard \"$root\"; guard \"$root/bin\"; guard \"$dir\"; }; ",
    "digest() { if command -v sha256sum >/dev/null 2>&1; then set -- $(sha256sum < \"$1\"); ",
    "elif command -v shasum >/dev/null 2>&1; then set -- $(shasum -a 256 < \"$1\"); ",
    "else return 1; fi; echo \"$1\"; }; ",
    "reusable() { [ -L \"$exe\" ] && return 1; [ -f \"$exe\" ] || return 1; inspect \"$exe\"; ",
    "[ \"$owner\" = \"$uid\" ] || return 1; shared && return 1; [ \"$(digest \"$exe\")\" = \"$sha\" ]; }; ",
    "fail() { [ -z \"$tmp\" ] || rm -f \"$tmp\"; echo \"LITHE-INSTALL-FAILED $2\"; exit \"$1\"; }; ",
    "guarded; if reusable; then echo LITHE-INSTALL-PRESENT; else ",
    "umask 077; mkdir -p \"$dir\" || fail 92 mkdir; guarded; ",
    "echo LITHE-INSTALL-NEEDED; tmp=\"$dir/.upload.$$\"; ",
    "head -c \"$size\" > \"$tmp\" && chmod 700 \"$tmp\" || fail 92 write; got=$(digest \"$tmp\"); ",
    "if [ -n \"$got\" ]; then checked=sha256; [ \"$got\" = \"$sha\" ] || fail 93 verify; ",
    "else checked=identity; [ $(wc -c < \"$tmp\") -eq \"$size\" ] && \"$tmp\" --identity 2>/dev/null | grep -q \"$sha\" || fail 93 verify; fi; ",
    "mv -f \"$tmp\" \"$exe\" || fail 92 rename; echo \"LITHE-INSTALL-DONE $checked\"; fi; ",
    "shift; exec \"$exe\" \"$@\""
);

/// Command to run at the end of a chain. `root` overrides the install root,
/// for hosts with a read-only home or for tests; `server_arguments` start
/// the server, for example `["--stdio"]`.
pub fn launch_command(root: Option<&str>, server_arguments: &[&str]) -> Vec<String> {
    let mut command = vec![
        "sh".into(),
        "-c".into(),
        LAUNCH_SCRIPT.into(),
        "lithe-launch".into(),
        root.unwrap_or_default().into(),
    ];
    command.extend(server_arguments.iter().map(|argument| argument.to_string()));
    command
}

/// What happened before the server took over the stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Launched {
    /// Platform the remote shell reported.
    pub report: ProbeReport,
    /// Whether this launch uploaded the binary.
    pub uploaded: bool,
}

/// Why an install directory cannot hold the server safely.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsafeReason {
    /// The path is a symbolic link, which could point anywhere.
    SymbolicLink,
    /// The path is not a directory.
    NotADirectory,
    /// The path belongs to another user.
    OwnedByOtherUser,
    /// The group or other users can write to the path.
    WritableByOthers,
}

impl UnsafeReason {
    const ALL: [Self; 4] = [
        Self::SymbolicLink,
        Self::NotADirectory,
        Self::OwnedByOtherUser,
        Self::WritableByOthers,
    ];

    /// The word the script answers with.
    fn as_str(self) -> &'static str {
        match self {
            Self::SymbolicLink => "symlink",
            Self::NotADirectory => "type",
            Self::OwnedByOtherUser => "owner",
            Self::WritableByOthers => "writable",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == word)
    }
}

/// Install step that failed on the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallStep {
    /// Creating the version directory, for example under a read-only home.
    CreateDirectory,
    /// Writing the received bytes to the temporary file.
    Write,
    /// The received bytes did not match the expected SHA-256, or, on a host
    /// without a hash tool, the expected size and identity.
    Verify,
    /// Moving the verified file into place.
    Rename,
}

impl InstallStep {
    const ALL: [Self; 4] = [
        Self::CreateDirectory,
        Self::Write,
        Self::Verify,
        Self::Rename,
    ];

    /// The word the script answers with.
    fn as_str(self) -> &'static str {
        match self {
            Self::CreateDirectory => "mkdir",
            Self::Write => "write",
            Self::Verify => "verify",
            Self::Rename => "rename",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.as_str() == word)
    }
}

/// How the server was put in place on the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Deployment {
    /// Whether the binary was sent over this connection.
    pub(crate) uploaded: bool,
    /// How the host checked the binary it runs.
    pub(crate) verification: Verification,
}

/// How the host checked the binary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Verification {
    /// The host computed the SHA-256 with its own tool. A reused binary is
    /// always checked this way.
    Sha256,
    /// The host has no hash tool: the upload's size matched and the binary
    /// reported the expected identity.
    Identity,
}

impl Verification {
    /// Log token of the check.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Identity => "identity",
        }
    }
}

/// Why [`launch`] could not start the server.
#[derive(Debug)]
pub enum LaunchError {
    /// The stream ended before the shell answered, usually because the
    /// connection or authentication failed.
    NoShell,
    /// The caller has no artifact for the reported platform.
    NoArtifact(Box<ProbeReport>),
    /// An install directory on the host is not private to the remote user,
    /// so the server is neither reused nor uploaded there.
    UnsafeInstall {
        /// Remote path that failed the check.
        path: String,
        /// Which check failed.
        reason: UnsafeReason,
    },
    /// An install step failed on the host, for example because the home
    /// directory is read-only; SSH's standard error says why.
    InstallFailed(InstallStep),
    /// The shell answered something other than this script's protocol.
    Protocol(String),
    /// The connection's stream failed.
    Io(io::Error),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoShell => formatter.write_str("the remote shell did not start"),
            Self::NoArtifact(report) => write!(
                formatter,
                "no remote server is available for {} {}",
                report.system, report.machine
            ),
            // The path is remote output; `{:?}` escapes control characters.
            Self::UnsafeInstall { path, reason } => {
                let problem = match reason {
                    UnsafeReason::SymbolicLink => "is a symbolic link",
                    UnsafeReason::NotADirectory => "is not a directory",
                    UnsafeReason::OwnedByOtherUser => "belongs to another user",
                    UnsafeReason::WritableByOthers => "is writable by other users",
                };
                write!(
                    formatter,
                    "refusing to install the remote server: {path:?} {problem}"
                )
            }
            Self::InstallFailed(step) => formatter.write_str(match step {
                InstallStep::CreateDirectory => {
                    "the remote server install directory could not be created"
                }
                InstallStep::Write => "the remote server upload could not be written",
                InstallStep::Verify => "the uploaded remote server failed verification",
                InstallStep::Rename => "the verified remote server could not be moved into place",
            }),
            Self::Protocol(message) => write!(formatter, "unexpected remote output: {message}"),
            Self::Io(error) => write!(formatter, "remote stream failed: {error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

impl LaunchError {
    /// Stable category for logs. It never contains remote output or paths.
    pub fn failure_category(&self) -> String {
        match self {
            Self::NoShell => "no-shell".into(),
            Self::NoArtifact(_) => "no-artifact".into(),
            Self::UnsafeInstall { reason, .. } => format!("unsafe-{}", reason.as_str()),
            Self::InstallFailed(step) => format!("install-{}", step.as_str()),
            Self::Protocol(_) => "protocol".into(),
            Self::Io(error) => format!("{:?}", error.kind()),
        }
    }
}

/// Drives the script on a started connection. `output` is read one byte at
/// a time so that nothing after the script's lines is consumed; the session
/// reads the rest. Blocking reads end when the caller closes the connection.
///
/// `log` receives the probe answer, the deployment and failures with their
/// stage. Two failures are left to the caller, which knows more: a shell that
/// never answered the probe, whose cause the SSH process tells, and
/// [`LaunchError::NoArtifact`], which the caller's `artifact_for` decided.
pub fn launch(
    output: &mut impl Read,
    input: &mut impl Write,
    artifact_for: impl FnOnce(&ProbeReport) -> Option<ServerArtifact>,
    log: &ConnectionLog,
) -> Result<Launched, LaunchError> {
    let report = probe(output).inspect_err(|error| log.launch_failed("probe", error))?;
    log.shell_started(report.target());
    let artifact =
        artifact_for(&report).ok_or_else(|| LaunchError::NoArtifact(Box::new(report.clone())))?;
    let (binary, name) =
        read_artifact(&artifact).inspect_err(|error| log.launch_failed("artifact", error))?;
    let started = Instant::now();
    let deployment = deploy(output, input, &artifact, &binary, &name)
        .inspect_err(|error| log.launch_failed("deploy", error))?;
    log.deployed(&artifact, &deployment, binary.len(), started.elapsed());
    Ok(Launched {
        report,
        uploaded: deployment.uploaded,
    })
}

/// Reads the probe block, skipping output before it such as a login banner.
fn probe(output: &mut impl Read) -> Result<ProbeReport, LaunchError> {
    let mut probe_output = Vec::new();
    loop {
        let line = read_line(output, NOISE_LIMIT.saturating_sub(probe_output.len()))?
            .ok_or(LaunchError::NoShell)?;
        probe_output.extend_from_slice(&line);
        probe_output.push(b'\n');
        if String::from_utf8_lossy(&line).trim_end() == PROBE_END {
            break;
        }
    }
    parse_probe(&probe_output).ok_or(LaunchError::NoShell)
}

/// Reads the binary and checks the fields sent to the shell. Returns the
/// bytes and the version directory name.
fn read_artifact(artifact: &ServerArtifact) -> Result<(Vec<u8>, String), LaunchError> {
    let binary = std::fs::read(&artifact.path).map_err(LaunchError::Io)?;
    let name = format!(
        "{}-{}",
        artifact.version,
        &artifact.sha256[..INSTALL_HASH_PREFIX.min(artifact.sha256.len())]
    );
    for field in [&artifact.version, &artifact.sha256, &name] {
        if field.is_empty()
            || !field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return Err(LaunchError::Protocol(format!(
                "invalid artifact field {field:?}"
            )));
        }
    }
    Ok((binary, name))
}

/// Sends the install request, then the binary when the shell needs it, and
/// reads how the shell put the server in place.
fn deploy(
    output: &mut impl Read,
    input: &mut impl Write,
    artifact: &ServerArtifact,
    binary: &[u8],
    name: &str,
) -> Result<Deployment, LaunchError> {
    writeln!(
        input,
        "{} {} {} {name}",
        artifact.version,
        artifact.sha256,
        binary.len()
    )
    .and_then(|()| input.flush())
    .map_err(LaunchError::Io)?;
    match answer(output)?.as_str() {
        INSTALL_PRESENT => Ok(Deployment {
            uploaded: false,
            verification: Verification::Sha256,
        }),
        INSTALL_NEEDED => {
            input
                .write_all(binary)
                .and_then(|()| input.flush())
                .map_err(LaunchError::Io)?;
            let verification = installed(&answer(output)?)?;
            Ok(Deployment {
                uploaded: true,
                verification,
            })
        }
        other => Err(refusal(other)),
    }
}

/// Reads one answer line of the script.
fn answer(output: &mut impl Read) -> Result<String, LaunchError> {
    let line = read_line(output, NOISE_LIMIT)?.ok_or(LaunchError::NoShell)?;
    Ok(String::from_utf8_lossy(&line).trim_end().to_owned())
}

/// Reads `LITHE-INSTALL-DONE <check>` after an upload.
fn installed(answer: &str) -> Result<Verification, LaunchError> {
    match answer
        .strip_prefix(INSTALL_DONE)
        .and_then(|rest| rest.strip_prefix(' '))
    {
        Some("sha256") => Ok(Verification::Sha256),
        Some("identity") => Ok(Verification::Identity),
        _ => Err(refusal(answer)),
    }
}

/// The error for an answer that is not the expected next step.
fn refusal(answer: &str) -> LaunchError {
    unsafe_install(answer)
        .or_else(|| install_failed(answer))
        .unwrap_or_else(|| LaunchError::Protocol(answer.to_owned()))
}

/// Reads `LITHE-INSTALL-UNSAFE <reason> <path>`; the path may contain spaces.
fn unsafe_install(answer: &str) -> Option<LaunchError> {
    let rest = answer.strip_prefix(INSTALL_UNSAFE)?.strip_prefix(' ')?;
    let (reason, path) = rest.split_once(' ')?;
    Some(LaunchError::UnsafeInstall {
        path: path.to_owned(),
        reason: UnsafeReason::parse(reason)?,
    })
}

/// Reads `LITHE-INSTALL-FAILED <step>`.
fn install_failed(answer: &str) -> Option<LaunchError> {
    let step = answer.strip_prefix(INSTALL_FAILED)?.strip_prefix(' ')?;
    InstallStep::parse(step).map(LaunchError::InstallFailed)
}

/// Reads up to a newline. `None` at end of stream before any byte.
fn read_line(output: &mut impl Read, limit: usize) -> Result<Option<Vec<u8>>, LaunchError> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match output.read(&mut byte) {
            Ok(0) if line.is_empty() => return Ok(None),
            Ok(0) => return Ok(Some(line)),
            Ok(_) if byte[0] == b'\n' => return Ok(Some(line)),
            Ok(_) => {
                if line.len() >= limit {
                    return Err(LaunchError::Protocol(
                        "too much output before the server".into(),
                    ));
                }
                line.push(byte[0]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(LaunchError::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The script crosses SSH hops through the transport's quoting, which
    // rejects these characters instead of escaping them.
    #[test]
    fn the_script_can_cross_an_ssh_hop() {
        assert!(!LAUNCH_SCRIPT
            .chars()
            .any(|character| matches!(character, '\'' | '\\' | '!') || character.is_control()));
        let chain = crate::client::TransportChain::new(vec![crate::client::Hop::Ssh(
            crate::client::SshHop::new("box"),
        )]);
        chain.command(&launch_command(None, &["--stdio"])).unwrap();
    }

    #[test]
    fn unsafe_answers_name_the_path_and_the_reason() {
        assert!(matches!(
            unsafe_install("LITHE-INSTALL-UNSAFE writable /home/dev/My Projects/.lithe-server"),
            Some(LaunchError::UnsafeInstall {
                path,
                reason: UnsafeReason::WritableByOthers,
            }) if path == "/home/dev/My Projects/.lithe-server"
        ));
        assert!(unsafe_install("LITHE-INSTALL-UNSAFE other /x").is_none());
        assert!(unsafe_install("LITHE-INSTALL-UNSAFEsymlink /x").is_none());
        assert!(unsafe_install("LITHE-INSTALL-PRESENT").is_none());
    }

    #[test]
    fn upload_answers_name_the_check_or_the_failed_step() {
        assert_eq!(
            installed("LITHE-INSTALL-DONE sha256").unwrap(),
            Verification::Sha256
        );
        assert_eq!(
            installed("LITHE-INSTALL-DONE identity").unwrap(),
            Verification::Identity
        );
        for (answer, step) in [
            ("LITHE-INSTALL-FAILED mkdir", InstallStep::CreateDirectory),
            ("LITHE-INSTALL-FAILED write", InstallStep::Write),
            ("LITHE-INSTALL-FAILED verify", InstallStep::Verify),
            ("LITHE-INSTALL-FAILED rename", InstallStep::Rename),
        ] {
            let error = installed(answer).unwrap_err();
            assert!(
                matches!(error, LaunchError::InstallFailed(found) if found == step),
                "{answer}"
            );
            assert_eq!(
                error.failure_category(),
                format!("install-{}", step.as_str())
            );
        }
        for answer in [
            "LITHE-INSTALL-DONE",
            "LITHE-INSTALL-DONE md5",
            "LITHE-INSTALL-FAILED",
            "LITHE-INSTALL-FAILED other",
        ] {
            assert!(
                matches!(installed(answer), Err(LaunchError::Protocol(_))),
                "{answer}"
            );
        }
    }

    // Categories reach the log, so none carries remote output or a path.
    #[test]
    fn failure_categories_carry_no_remote_text() {
        let unsafe_install = LaunchError::UnsafeInstall {
            path: "/home/dev/.lithe-server".into(),
            reason: UnsafeReason::OwnedByOtherUser,
        };
        assert_eq!(unsafe_install.failure_category(), "unsafe-owner");
        assert_eq!(
            LaunchError::Protocol("banner from dev@box".into()).failure_category(),
            "protocol"
        );
        assert_eq!(
            LaunchError::Io(io::Error::new(io::ErrorKind::BrokenPipe, "/home/dev"))
                .failure_category(),
            "BrokenPipe"
        );
    }
}
