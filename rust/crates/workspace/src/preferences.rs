//! `remote.preferences` and `remote.editPreferences`: the values of the
//! settings page "Tools › Remote Workspace" (design §10, D6), and the SSH
//! program every OpenSSH process Core starts uses.
//!
//! The document is machine-local and read and published by the native client
//! like the saved connections; Core validates it, applies one edit at a time
//! and tells what every automatic value resolves to. The rules are in
//! `lithe_remote::client::preferences`.

use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::preferences::{self, PreferenceError, Preferences};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

/// `remote.preferences`: the current document and, for the SSH
/// configuration row, the user's home directory.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    /// Absolute home directory; `sshConfig` is returned only with it.
    #[serde(default)]
    home_directory: Option<String>,
}

/// `remote.editPreferences`: exactly one of `sshProgram` and
/// `reconnectLimit`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    /// Current document text; `null` when the source does not exist.
    document: Option<String>,
    #[serde(default)]
    ssh_program: Option<ProgramEdit>,
    #[serde(default)]
    reconnect_limit: Option<LimitEdit>,
}

/// The SSH program to use; `path: null` makes it automatic.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProgramEdit {
    path: Option<String>,
}

/// The reconnect limit; `seconds: null` makes it automatic.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimitEdit {
    seconds: Option<u32>,
}

/// Where the native client keeps the document.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Storage {
    relative_path: &'static str,
    maximum_bytes: usize,
}

const STORAGE: Storage = Storage {
    relative_path: preferences::STORAGE_PATH,
    maximum_bytes: preferences::MAX_BYTES,
};

/// The stored values, what each one resolves to, and where the document
/// and the user's SSH configuration are.
pub fn read(request: ReadRequest) -> Result<Value, CoreError> {
    let preferences = parse(request.document.as_deref())?;
    let ssh = preferences.ssh(std::env::var_os("PATH").as_deref());
    let mut result = json!({
        "preferences": {
            "sshProgram": preferences.ssh_program(),
            "reconnectLimitSeconds": preferences.reconnect_limit_seconds(),
        },
        "ssh": ssh,
        "reconnect": preferences.reconnect(),
        "storage": STORAGE,
    });
    if let Some(home) = request.home_directory {
        let home = PathBuf::from(home);
        if !home.is_absolute() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "The home directory must be an absolute path",
            )
            .with_details(json!({ "field": "homeDirectory", "reason": "relative" }).to_string()));
        }
        let path = home.join(".ssh").join("config");
        result["sshConfig"] = json!({
            "path": path.to_string_lossy(),
            "exists": path.is_file(),
        });
    }
    Ok(result)
}

/// Applies one edit. An edit that changes nothing returns a `null` document,
/// so the native client creates no source.
pub fn edit(request: EditRequest) -> Result<Value, CoreError> {
    let before = parse(request.document.as_deref())?;
    let mut after = before.clone();
    match (request.ssh_program, request.reconnect_limit) {
        (Some(program), None) => after
            .set_ssh_program(program.path.as_deref())
            .map_err(invalid_request)?,
        (None, Some(limit)) => after
            .set_reconnect_limit(limit.seconds)
            .map_err(invalid_request)?,
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid remote preferences edit",
            )
            .with_details(json!({ "field": "operation", "reason": "invalid" }).to_string()))
        }
    }
    let changed = after != before;
    let document = if changed {
        Some(after.text().map_err(invalid_request)?)
    } else {
        None
    };
    Ok(json!({ "changed": changed, "document": document }))
}

/// The OpenSSH client to start under the preferences in `document`: the
/// setting, else the first `ssh` on `PATH`, else `ssh` for the system to
/// look up. `stand_in`, which tests set, replaces all of them.
pub(crate) fn ssh_program(
    stand_in: Option<&str>,
    document: Option<&str>,
) -> Result<String, CoreError> {
    if let Some(program) = stand_in {
        return Ok(program.to_owned());
    }
    let preferences = parse(document)?;
    let ssh = preferences.ssh(std::env::var_os("PATH").as_deref());
    Ok(ssh.command().to_owned())
}

/// A stored document that cannot be decoded is a parse failure, not a bad
/// request: the caller sent what the source holds.
fn parse(document: Option<&str>) -> Result<Preferences, CoreError> {
    Preferences::parse(document).map_err(|error| {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Invalid remote preferences document",
        )
        .with_details(json!(error).to_string())
    })
}

/// A refused edit, with the field and the reason as details.
fn invalid_request(error: PreferenceError) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote preference")
        .with_details(json!(error).to_string())
}

#[cfg(test)]
mod tests;
