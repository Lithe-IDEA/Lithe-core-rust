//! Probing, installing and starting the server over one connection.
//!
//! All steps run inside one remote shell on one stream, so an SSH host that
//! asks for a password asks only once:
//!
//! 1. the shell reports the platform between markers;
//! 2. the client picks the artifact for it and sends one line naming its
//!    version, SHA-256 and size;
//! 3. the shell checks that the install directories are private to the
//!    remote user, then keeps an installed binary whose SHA-256 it computes
//!    itself, or receives exactly `size` bytes, installs them under a
//!    temporary name and verifies them before renaming them into place;
//! 4. the shell `exec`s the server, whose preamble starts the protocol on the
//!    same stream, ready for [`crate::client::RemoteSession::from_streams`].
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
use std::fmt;
use std::io::{self, Read, Write};

const INSTALL_PRESENT: &str = "LITHE-INSTALL-PRESENT";
const INSTALL_NEEDED: &str = "LITHE-INSTALL-NEEDED";
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
///   file whose computed SHA-256 is exactly the expected one.
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
    "fail() { [ -z \"$tmp\" ] || rm -f \"$tmp\"; echo LITHE-INSTALL-FAILED >&2; exit \"$1\"; }; ",
    "guarded; if reusable; then echo LITHE-INSTALL-PRESENT; else ",
    "umask 077; mkdir -p \"$dir\" || { echo LITHE-INSTALL-FAILED; exit 92; }; guarded; ",
    "echo LITHE-INSTALL-NEEDED; tmp=\"$dir/.upload.$$\"; ",
    "head -c \"$size\" > \"$tmp\" && chmod 700 \"$tmp\" || fail 92; got=$(digest \"$tmp\"); ",
    "if [ -n \"$got\" ]; then [ \"$got\" = \"$sha\" ] || fail 93; ",
    "else [ $(wc -c < \"$tmp\") -eq \"$size\" ] && \"$tmp\" --identity 2>/dev/null | grep -q \"$sha\" || fail 93; fi; ",
    "mv -f \"$tmp\" \"$exe\" || fail 92; fi; shift; exec \"$exe\" \"$@\""
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
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "symlink" => Self::SymbolicLink,
            "type" => Self::NotADirectory,
            "owner" => Self::OwnedByOtherUser,
            "writable" => Self::WritableByOthers,
            _ => return None,
        })
    }
}

/// Why [`launch`] could not start the server.
#[derive(Debug)]
pub enum LaunchError {
    /// The stream ended before the shell reported its platform, usually
    /// because the connection or authentication failed.
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
    /// The host could not create the install directory, for example because
    /// the home directory is read-only; SSH's standard error says why.
    InstallFailed,
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
            Self::InstallFailed => {
                formatter.write_str("the remote server install directory could not be created")
            }
            Self::Protocol(message) => write!(formatter, "unexpected remote output: {message}"),
            Self::Io(error) => write!(formatter, "remote stream failed: {error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Drives the script on a started connection. `output` is read one byte at
/// a time so that nothing after the script's lines is consumed; the session
/// reads the rest. Blocking reads end when the caller closes the connection.
pub fn launch(
    output: &mut impl Read,
    input: &mut impl Write,
    artifact_for: impl FnOnce(&ProbeReport) -> Option<ServerArtifact>,
) -> Result<Launched, LaunchError> {
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
    let report = parse_probe(&probe_output).ok_or(LaunchError::NoShell)?;
    let artifact =
        artifact_for(&report).ok_or_else(|| LaunchError::NoArtifact(Box::new(report.clone())))?;
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
    writeln!(
        input,
        "{} {} {} {name}",
        artifact.version,
        artifact.sha256,
        binary.len()
    )
    .and_then(|()| input.flush())
    .map_err(LaunchError::Io)?;

    let answer = read_line(output, NOISE_LIMIT)?.ok_or(LaunchError::NoShell)?;
    let answer = String::from_utf8_lossy(&answer).trim_end().to_owned();
    let uploaded = match answer.as_str() {
        INSTALL_PRESENT => false,
        INSTALL_NEEDED => {
            input
                .write_all(&binary)
                .and_then(|()| input.flush())
                .map_err(LaunchError::Io)?;
            true
        }
        INSTALL_FAILED => return Err(LaunchError::InstallFailed),
        other => {
            return Err(
                unsafe_install(other).unwrap_or_else(|| LaunchError::Protocol(other.to_owned()))
            )
        }
    };
    Ok(Launched { report, uploaded })
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
}
