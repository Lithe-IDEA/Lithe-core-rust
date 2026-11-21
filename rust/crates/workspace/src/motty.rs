//! `remote.mottyHosts`: the SSH hosts saved in Motty, and what an import
//! into the saved connections makes of each (design §3.1).
//!
//! Core opens Motty's configuration database read-only and never reads its
//! vault, so passwords and keys stay where they are. A database that is
//! missing or in another layout is a result, not an error, so the import
//! page can say why it has nothing to offer. The rules are in
//! `lithe_remote::client::motty`.

use crate::connections::parse;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_remote::client::motty;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// The database, relative to the platform's user configuration directory.
const DATABASE: [&str; 2] = ["motty", "config.db"];

/// `remote.mottyHosts`: where Motty keeps its data, and the saved
/// connections to compare with.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostsRequest {
    /// The platform's user configuration directory (`~/Library/Application
    /// Support` on macOS, `%APPDATA%` on Windows, `~/.config` on Linux).
    config_directory: String,
    /// Current connections document; `null` when the source does not exist.
    document: Option<String>,
}

/// The hosts of Motty's database, or `unavailable` with the reason.
pub fn hosts(request: HostsRequest) -> Result<Value, CoreError> {
    let directory = PathBuf::from(&request.config_directory);
    if !directory.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The configuration directory must be an absolute path",
        ));
    }
    let saved = parse(request.document.as_deref())?;
    let database = DATABASE
        .iter()
        .fold(directory, |path, component| path.join(component));
    let path = database.display().to_string();
    Ok(match motty::read(&database) {
        Ok(catalog) => {
            let mut report = json!(motty::candidates(&catalog, &saved));
            report["database"] = json!(path);
            report
        }
        Err(reason) => json!({ "database": path, "unavailable": reason }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_database_is_a_result_that_says_so() {
        let directory = std::env::temp_dir().join(format!(
            "lithe-workspace-motty-missing-{}",
            std::process::id()
        ));
        let result = hosts(HostsRequest {
            config_directory: directory.display().to_string(),
            document: None,
        })
        .unwrap();
        assert_eq!(result["unavailable"], "missing");
        assert!(result["database"]
            .as_str()
            .unwrap()
            .ends_with(&format!("motty{}config.db", std::path::MAIN_SEPARATOR)));
    }

    #[test]
    fn a_relative_directory_is_refused() {
        let error = hosts(HostsRequest {
            config_directory: "motty".into(),
            document: None,
        })
        .unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
    }
}
