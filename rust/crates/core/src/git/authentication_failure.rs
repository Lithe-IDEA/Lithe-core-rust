//! Locale-independent recognition of Git remote authentication failures.
//!
//! Every Git process Core starts runs with `LC_ALL=C` and
//! `GIT_TERMINAL_PROMPT=0` (see `lithe_git_host::configuration::configure` and
//! the remote spawn path), so Git, its HTTP transport and OpenSSH print fixed
//! English diagnostics whatever the user's system language is. Matching those
//! fixed markers is therefore stable, and hosts receive a code instead of
//! matching stderr themselves.

use super::GitCommandResponse;
use crate::protocol::{CoreError, ErrorCode};

/// Fixed C-locale fragments that mean the remote refused the credentials or
/// none could be obtained. Compared case-sensitively against Git stderr.
const MARKERS: &[&str] = &[
    // HTTP(S): wrong or revoked credentials.
    "Authentication failed",
    "Invalid username or password",
    "HTTP Basic: Access denied",
    "The requested URL returned error: 401",
    "The requested URL returned error: 403",
    // HTTP(S): no credential available and prompting is disabled.
    "could not read Username",
    "could not read Password",
    "terminal prompts disabled",
    // SSH: key or agent rejected.
    "Permission denied (publickey",
    "Permission denied (password",
    "Permission denied (keyboard-interactive",
    // Hosting service refused the authenticated account.
    "Permission to ",
];

/// Returns whether `text` carries a C-locale Git authentication failure marker.
pub(super) fn is_authentication_failure(text: &str) -> bool {
    text.lines().any(|line| {
        MARKERS.iter().any(|marker| match *marker {
            // "Permission to <repo> denied to <user>." is only meaningful as a whole.
            "Permission to " => line.contains(marker) && line.contains(" denied to "),
            _ => line.contains(marker),
        })
    })
}

/// Upgrades a failed `git.write` response to `git_authentication_failed`.
///
/// Only failures that carry no more specific category are changed: a response
/// with a non-zero exit and no `operationError` gains one whose message is the
/// first stderr line, and a generic `process_failed` operation error keeps its
/// message and details under the new code. Successful responses are untouched.
pub(super) fn classify(response: &mut GitCommandResponse) {
    let failed = response.exit_code != 0 || response.operation_error.is_some();
    if !failed {
        return;
    }
    match &mut response.operation_error {
        Some(error) => {
            if matches!(error.code, ErrorCode::ProcessFailed)
                && (is_authentication_failure(&error.message)
                    || error
                        .details
                        .as_deref()
                        .is_some_and(is_authentication_failure)
                    || response
                        .invocations
                        .iter()
                        .any(|invocation| is_authentication_failure(&invocation.stderr)))
            {
                error.code = ErrorCode::GitAuthenticationFailed;
            }
        }
        None => {
            if is_authentication_failure(&response.stderr) {
                let message = response
                    .stderr
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("Git authentication failed")
                    .to_string();
                response.operation_error = Some(
                    CoreError::new(ErrorCode::GitAuthenticationFailed, message)
                        .with_details(response.output.clone()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(stderr: &str) -> GitCommandResponse {
        super::super::GitProcessOutput {
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code: 128,
        }
        .into_command_response(&["push".to_string()])
    }

    #[test]
    fn recognises_http_and_ssh_failure_samples() {
        for sample in [
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://example.com/a.git/'\n",
            "fatal: could not read Username for 'https://example.com': terminal prompts disabled\n",
            "fatal: unable to access 'https://example.com/a.git/': The requested URL returned error: 403\n",
            "remote: Permission to a/b.git denied to someone.\nfatal: unable to access 'https://github.com/a/b.git/': The requested URL returned error: 403\n",
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.\n",
            "user@host: Permission denied (publickey,password).\r\nfatal: Could not read from remote repository.\r\n",
        ] {
            assert!(is_authentication_failure(sample), "{sample}");
        }
    }

    #[test]
    fn shared_fixture_samples_classify_as_documented() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/git/authentication-failure-v1.json"
        ))
        .unwrap();
        for (key, expected) in [("failures", true), ("unrelated", false)] {
            for sample in fixture[key].as_array().unwrap() {
                let mut response = failed(sample["stderr"].as_str().unwrap());
                classify(&mut response);
                let code = response
                    .operation_error
                    .map(|error| serde_json::to_value(error).unwrap()["code"].clone());
                assert_eq!(
                    code == Some(fixture["errorCode"].clone()),
                    expected,
                    "{}",
                    sample["name"]
                );
            }
        }
    }

    #[test]
    fn ignores_unrelated_failures() {
        for sample in [
            "fatal: unable to access 'https://example.com/': Could not resolve host: example.com\n",
            "fatal: repository 'https://example.com/a.git/' not found\n",
            "ssh: connect to host example.com port 22: Connection timed out\nfatal: Could not read from remote repository.\n",
            "! [rejected] main -> main (non-fast-forward)\n",
            "error: Permission to write denied\n",
        ] {
            assert!(!is_authentication_failure(sample), "{sample}");
        }
    }

    #[test]
    fn failed_exit_gains_the_stable_code() {
        let mut response =
            failed("fatal: Authentication failed for 'https://example.com/a.git/'\n");
        classify(&mut response);
        let error = response.operation_error.expect("operation error");
        assert!(matches!(error.code, ErrorCode::GitAuthenticationFailed));
        assert_eq!(
            error.message,
            "fatal: Authentication failed for 'https://example.com/a.git/'"
        );
        assert!(error.details.unwrap().contains("Authentication failed"));
        let json =
            serde_json::to_value(CoreError::new(ErrorCode::GitAuthenticationFailed, "x")).unwrap();
        assert_eq!(json["code"], "git_authentication_failed");
    }

    #[test]
    fn generic_process_failure_is_upgraded_and_specific_ones_are_kept() {
        let mut response = failed("git@h: Permission denied (publickey).\n");
        response.operation_error = Some(
            CoreError::new(ErrorCode::ProcessFailed, "Some Fetch operations failed")
                .with_details("origin: git@h: Permission denied (publickey)."),
        );
        classify(&mut response);
        let error = response.operation_error.unwrap();
        assert!(matches!(error.code, ErrorCode::GitAuthenticationFailed));
        assert_eq!(error.message, "Some Fetch operations failed");

        let mut response = failed("fatal: Authentication failed\n");
        response.operation_error = Some(CoreError::new(ErrorCode::InvalidRequest, "stale"));
        classify(&mut response);
        assert!(matches!(
            response.operation_error.unwrap().code,
            ErrorCode::InvalidRequest
        ));
    }

    #[test]
    fn success_and_other_failures_are_untouched() {
        let mut ok = failed("");
        ok.exit_code = 0;
        classify(&mut ok);
        assert!(ok.operation_error.is_none());
        let mut other = failed("fatal: not a git repository\n");
        classify(&mut other);
        assert!(other.operation_error.is_none());
    }
}
