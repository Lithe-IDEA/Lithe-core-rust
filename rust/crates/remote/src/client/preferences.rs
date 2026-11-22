//! Remote workspace preferences: the page "Tools › Remote Workspace" of the
//! settings window (design §10, D6).
//!
//! The document holds only the values the user changed; every missing value
//! is automatic, and [`Preferences::ssh`] and [`Preferences::reconnect`] say
//! what automatic resolves to, so the page shows the value that takes effect.
//!
//! - `sshProgram` replaces the `ssh` found on `PATH` for every OpenSSH
//!   process Core starts: probes, connections and `ssh -G`. It must be an
//!   absolute path; an edit also requires an executable file there. Core
//!   never stores a value that starts something other than an executable the
//!   user chose.
//! - `reconnectLimitSeconds` bounds how long a client keeps reconnecting a
//!   lost session automatically. [`reconnect_delays`] turns it into the
//!   pauses between attempts, so every client waits the same way.
//! - `allowReverseForwarding` lets the ports page start reverse forwards: a
//!   port on the host's loopback interface that reaches a port on this
//!   machine's loopback interface. [`Preferences::reverse_forwarding`] is off
//!   unless the user turned it on, because every user of the host can then
//!   reach that local port.
//! - `closePrograms` says what happens to the processes and terminals still
//!   running on the host when a window closes its connection: `ask` every
//!   time, `end` them, or `keep` them running for the next connection.
//!   [`Preferences::closing`] is `ask` unless the user chose otherwise, so
//!   nothing keeps running on a host, or ends, without the user deciding.
//! - The document lives at [`STORAGE_PATH`] below the platform's
//!   user-preference directory, never on a host or in a workspace. It holds
//!   no secret. This module validates and edits it; the platform adapter
//!   reads it and publishes edits atomically.
//!
//! Every refusal names the field and the reason, so a client can explain it
//! in the user's language next to the row it concerns.

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// Document location below the platform's user-preference directory.
pub const STORAGE_PATH: &str = "remote/preferences.json";

/// Largest accepted document, in bytes.
pub const MAX_BYTES: usize = 16 * 1024;

/// Longest accepted SSH program path, in bytes.
pub const MAX_PROGRAM_BYTES: usize = 4096;

/// Reconnect limit when the document sets none.
pub const DEFAULT_RECONNECT_LIMIT_SECONDS: u32 = 60;

/// Largest reconnect limit; 0 turns automatic reconnection off.
pub const MAX_RECONNECT_LIMIT_SECONDS: u32 = 60 * 60;

/// Pause after the first failed attempt; each later pause doubles.
const FIRST_PAUSE_MILLISECONDS: u64 = 1_000;

/// Longest pause between two attempts.
const MAX_PAUSE_MILLISECONDS: u64 = 30_000;

/// Program name OpenSSH installs on `PATH`.
const SSH_NAME: &str = "ssh";

/// Whether reverse forwards are offered when the document sets nothing.
pub const DEFAULT_ALLOW_REVERSE_FORWARDING: bool = false;

/// What happens to programs still running on the host when a window closes
/// its connection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClosePrograms {
    /// Ask every time.
    Ask,
    /// End every process and terminal of the session.
    End,
    /// Leave them running; the next connection to the host finds them.
    Keep,
}

/// The choice when the document sets nothing.
pub const DEFAULT_CLOSE_PROGRAMS: ClosePrograms = ClosePrograms::Ask;

/// Why a document or an edit was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceError {
    /// `document`, `version`, `sshProgram`, `reconnectLimitSeconds` or
    /// `closePrograms`.
    pub field: &'static str,
    /// Why the field was refused.
    pub reason: PreferenceReason,
}

/// The kind of refusal, stable for clients to translate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreferenceReason {
    /// The value has characters or a form the field does not accept.
    Invalid,
    /// The value is longer than the field allows.
    TooLong,
    /// The path is not absolute.
    Relative,
    /// Nothing exists at the path.
    Missing,
    /// The path names a directory or a file that cannot be run.
    NotExecutable,
    /// The number is outside the field's range.
    OutOfRange,
}

impl PreferenceError {
    fn at(field: &'static str, reason: PreferenceReason) -> Self {
        Self { field, reason }
    }
}

impl fmt::Display for PreferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid remote preference at {} ({:?})",
            self.field, self.reason
        )
    }
}

impl std::error::Error for PreferenceError {}

/// The validated document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    version: u32,
    /// Absolute path of the OpenSSH client; `None` uses `ssh` on `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_program: Option<String>,
    /// `None` uses [`DEFAULT_RECONNECT_LIMIT_SECONDS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reconnect_limit_seconds: Option<u32>,
    /// `None` uses [`DEFAULT_ALLOW_REVERSE_FORWARDING`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allow_reverse_forwarding: Option<bool>,
    /// `None` uses [`DEFAULT_CLOSE_PROGRAMS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    close_programs: Option<ClosePrograms>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            ssh_program: None,
            reconnect_limit_seconds: None,
            allow_reverse_forwarding: None,
            close_programs: None,
        }
    }
}

/// Whether a value is the user's or the automatic one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// Nothing is set; Lithe picks the value.
    Automatic,
    /// The document sets the value.
    Setting,
}

/// What is at the SSH program path when the page reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgramStatus {
    /// An executable file.
    Found,
    /// Nothing; for the automatic value, no `ssh` on `PATH`.
    Missing,
    /// A directory or a file that cannot be run.
    NotExecutable,
}

/// The OpenSSH client Core starts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshProgram {
    /// Whether the document sets the value or Lithe picks it.
    pub source: Source,
    /// The setting, or the first `ssh` on `PATH`; `None` when automatic
    /// finds none.
    pub path: Option<String>,
    /// What is at that path when the page reads it.
    pub status: ProgramStatus,
}

impl SshProgram {
    /// The program to start: the path, or `ssh` for the system to look up
    /// when automatic finds none, so the start fails as it always did.
    pub fn command(&self) -> &str {
        self.path.as_deref().unwrap_or(SSH_NAME)
    }
}

/// Whether the ports page offers reverse forwards.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseForwarding {
    /// Whether the document sets the value or Lithe picks it.
    pub source: Source,
    /// The value that takes effect.
    pub allowed: bool,
}

/// What happens to running programs when a window closes its connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Closing {
    /// Whether the document sets the value or Lithe picks it.
    pub source: Source,
    /// The choice that takes effect.
    pub programs: ClosePrograms,
    /// Seconds the host keeps a session whose connection was lost, and
    /// whose programs nobody kept, before ending it. The server sets it,
    /// so no document changes it; clients show it beside the choice.
    pub idle_timeout_seconds: u64,
}

/// How long a lost session is reconnected automatically.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reconnect {
    /// Whether the document sets the value or Lithe picks it.
    pub source: Source,
    /// The limit that takes effect; 0 turns automatic reconnection off.
    pub limit_seconds: u32,
    /// The pause before each attempt, in order; empty when the limit is 0.
    pub delays_milliseconds: Vec<u64>,
}

impl Preferences {
    /// Reads `document`, or the empty document when there is none yet.
    ///
    /// Refuses documents over [`MAX_BYTES`], malformed JSON, unknown fields,
    /// other versions, relative or overlong SSH program paths and reconnect
    /// limits over [`MAX_RECONNECT_LIMIT_SECONDS`]. Whether the program
    /// still exists is reported by [`Self::ssh`], not refused, so a removed
    /// client does not hide the page that changes it.
    pub fn parse(document: Option<&str>) -> Result<Self, PreferenceError> {
        let Some(text) = document else {
            return Ok(Self::default());
        };
        if text.len() > MAX_BYTES {
            return Err(PreferenceError::at("document", PreferenceReason::TooLong));
        }
        let preferences: Self = serde_json::from_str(text)
            .map_err(|_| PreferenceError::at("document", PreferenceReason::Invalid))?;
        if preferences.version != 1 {
            return Err(PreferenceError::at("version", PreferenceReason::Invalid));
        }
        if let Some(program) = &preferences.ssh_program {
            check_program_form(program)?;
        }
        if let Some(limit) = preferences.reconnect_limit_seconds {
            check_limit(limit)?;
        }
        Ok(preferences)
    }

    /// The document text to publish.
    pub fn text(&self) -> Result<String, PreferenceError> {
        let mut text = serde_json::to_string_pretty(self)
            .map_err(|_| PreferenceError::at("document", PreferenceReason::Invalid))?;
        text.push('\n');
        if text.len() > MAX_BYTES {
            return Err(PreferenceError::at("document", PreferenceReason::TooLong));
        }
        Ok(text)
    }

    /// The stored SSH program, `None` when automatic.
    pub fn ssh_program(&self) -> Option<&str> {
        self.ssh_program.as_deref()
    }

    /// The stored reconnect limit, `None` when automatic.
    pub fn reconnect_limit_seconds(&self) -> Option<u32> {
        self.reconnect_limit_seconds
    }

    /// Sets the SSH program, or makes it automatic with `None`. The path is
    /// trimmed and must name an executable file now.
    pub fn set_ssh_program(&mut self, path: Option<&str>) -> Result<(), PreferenceError> {
        let Some(path) = path.map(str::trim) else {
            self.ssh_program = None;
            return Ok(());
        };
        check_program_form(path)?;
        match program_status(Path::new(path)) {
            ProgramStatus::Found => {}
            ProgramStatus::Missing => {
                return Err(PreferenceError::at("sshProgram", PreferenceReason::Missing))
            }
            ProgramStatus::NotExecutable => {
                return Err(PreferenceError::at(
                    "sshProgram",
                    PreferenceReason::NotExecutable,
                ))
            }
        }
        self.ssh_program = Some(path.to_owned());
        Ok(())
    }

    /// The stored reverse-forwarding switch, `None` when automatic.
    pub fn allow_reverse_forwarding(&self) -> Option<bool> {
        self.allow_reverse_forwarding
    }

    /// Turns reverse forwards on or off, or makes the switch automatic with
    /// `None`.
    pub fn set_allow_reverse_forwarding(&mut self, allowed: Option<bool>) {
        self.allow_reverse_forwarding = allowed;
    }

    /// The stored closing choice, `None` when automatic.
    pub fn close_programs(&self) -> Option<ClosePrograms> {
        self.close_programs
    }

    /// Sets what happens to running programs on close, or makes it
    /// automatic with `None`.
    pub fn set_close_programs(&mut self, programs: Option<ClosePrograms>) {
        self.close_programs = programs;
    }

    /// What happens to running programs on close, and whether the user
    /// decided.
    pub fn closing(&self) -> Closing {
        Closing {
            source: match self.close_programs {
                Some(_) => Source::Setting,
                None => Source::Automatic,
            },
            programs: self.close_programs.unwrap_or(DEFAULT_CLOSE_PROGRAMS),
            idle_timeout_seconds: crate::protocol::methods::DEFAULT_IDLE_TIMEOUT_SECONDS,
        }
    }

    /// Whether reverse forwards are offered, and whether the user decided.
    pub fn reverse_forwarding(&self) -> ReverseForwarding {
        ReverseForwarding {
            source: match self.allow_reverse_forwarding {
                Some(_) => Source::Setting,
                None => Source::Automatic,
            },
            allowed: self
                .allow_reverse_forwarding
                .unwrap_or(DEFAULT_ALLOW_REVERSE_FORWARDING),
        }
    }

    /// Sets the reconnect limit, or makes it automatic with `None`.
    pub fn set_reconnect_limit(&mut self, seconds: Option<u32>) -> Result<(), PreferenceError> {
        if let Some(seconds) = seconds {
            check_limit(seconds)?;
        }
        self.reconnect_limit_seconds = seconds;
        Ok(())
    }

    /// The OpenSSH client that takes effect, looking `ssh` up in `path`
    /// (the value of `PATH`) when automatic.
    pub fn ssh(&self, path: Option<&OsStr>) -> SshProgram {
        match &self.ssh_program {
            Some(program) => SshProgram {
                source: Source::Setting,
                path: Some(program.clone()),
                status: program_status(Path::new(program)),
            },
            None => {
                let found = path.and_then(|path| find_on_path(SSH_NAME, path));
                SshProgram {
                    source: Source::Automatic,
                    status: if found.is_some() {
                        ProgramStatus::Found
                    } else {
                        ProgramStatus::Missing
                    },
                    path: found.map(|found| found.to_string_lossy().into_owned()),
                }
            }
        }
    }

    /// The reconnect limit that takes effect and its pauses.
    pub fn reconnect(&self) -> Reconnect {
        let limit_seconds = self
            .reconnect_limit_seconds
            .unwrap_or(DEFAULT_RECONNECT_LIMIT_SECONDS);
        Reconnect {
            source: match self.reconnect_limit_seconds {
                Some(_) => Source::Setting,
                None => Source::Automatic,
            },
            limit_seconds,
            delays_milliseconds: reconnect_delays(limit_seconds),
        }
    }
}

/// The pauses before each reconnect attempt within `limit_seconds`: 1 s,
/// doubling up to 30 s, the last one shortened so they add up to the limit.
/// The default limit gives 1, 2, 4, 8, 16 and 29 seconds.
pub fn reconnect_delays(limit_seconds: u32) -> Vec<u64> {
    let mut remaining = u64::from(limit_seconds) * 1_000;
    let mut pause = FIRST_PAUSE_MILLISECONDS;
    let mut delays = Vec::new();
    while remaining > 0 {
        let next = pause.min(remaining);
        delays.push(next);
        remaining -= next;
        pause = (pause * 2).min(MAX_PAUSE_MILLISECONDS);
    }
    delays
}

fn check_program_form(path: &str) -> Result<(), PreferenceError> {
    let refused = |reason| Err(PreferenceError::at("sshProgram", reason));
    if path.len() > MAX_PROGRAM_BYTES {
        return refused(PreferenceReason::TooLong);
    }
    if path.is_empty() || path.trim() != path || path.chars().any(char::is_control) {
        return refused(PreferenceReason::Invalid);
    }
    if !Path::new(path).is_absolute() {
        return refused(PreferenceReason::Relative);
    }
    Ok(())
}

fn check_limit(seconds: u32) -> Result<(), PreferenceError> {
    if seconds > MAX_RECONNECT_LIMIT_SECONDS {
        return Err(PreferenceError::at(
            "reconnectLimitSeconds",
            PreferenceReason::OutOfRange,
        ));
    }
    Ok(())
}

fn program_status(path: &Path) -> ProgramStatus {
    match path.metadata() {
        Err(_) => ProgramStatus::Missing,
        Ok(metadata) if is_executable(&metadata) => ProgramStatus::Found,
        Ok(_) => ProgramStatus::NotExecutable,
    }
}

/// The first executable `name` in the directories of `path`, as the system
/// would start it.
fn find_on_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(&file))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|metadata| is_executable(&metadata))
        })
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

#[cfg(test)]
mod tests;
