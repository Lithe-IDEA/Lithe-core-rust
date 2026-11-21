use super::*;
#[cfg(unix)]
use crate::sessions;
use serde_json::json;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::sync::OnceLock;

// The connection rules live in `lithe_remote::client::connections`; the
// probe only reports a refusal as a bad request that names the field.
#[test]
fn malformed_connections_name_the_field_in_the_error() {
    let connection: SshConnection =
        serde_json::from_value(json!({ "host": "dev@box" })).expect("decodes");
    let error = ssh_hop(&connection).expect_err("host with a user is refused");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    let details: serde_json::Value =
        serde_json::from_str(error.details.as_deref().expect("details")).unwrap();
    assert_eq!(details, json!({ "field": "host", "reason": "invalid" }));
}

// The window picks the form from `kind`; a host key question carries the
// public fingerprint so the user can compare it before trusting the key.
#[test]
fn questions_are_relayed_with_their_kind_and_the_host_key() {
    let host_key = challenge_event(&Challenge {
        request_id: "request-1".into(),
        prompt: "The authenticity of host 'build-box (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:fakeFingerprintForTests0123456789abcdefgh.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ".into(),
        secret: false,
        attempt: 1,
    });
    assert_eq!(host_key["type"], "authentication");
    assert_eq!(host_key["kind"], "hostKey");
    assert_eq!(host_key["keyType"], "ED25519");
    assert_eq!(
        host_key["fingerprint"],
        "SHA256:fakeFingerprintForTests0123456789abcdefgh"
    );
    assert_eq!(host_key["secret"], false);

    let password = challenge_event(&Challenge {
        request_id: "request-2".into(),
        prompt: "dev@build-box's password: ".into(),
        secret: true,
        attempt: 2,
    });
    assert_eq!(password["kind"], "password");
    assert_eq!(password["attempt"], 2);
    assert!(password.get("fingerprint").is_none());
}

#[test]
fn launch_stages_become_remote_stage_events() {
    use crate::sessions::stage_event;
    use lithe_remote::client::launch::Stage;
    assert_eq!(
        stage_event(Stage::Authenticated),
        json!({ "type": "remoteStage", "stage": "authenticated" })
    );
    assert_eq!(
        stage_event(Stage::Uploading {
            sent: 1024,
            total: 4096
        }),
        json!({ "type": "remoteStage", "stage": "uploading", "sentBytes": 1024, "totalBytes": 4096 })
    );
    assert_eq!(
        stage_event(Stage::Installed { uploaded: false }),
        json!({ "type": "remoteStage", "stage": "installed", "uploaded": false })
    );
}

#[test]
fn ssh_failures_map_to_stable_statuses() {
    let cases: [(&str, ProbeStatus); 6] = [
        (
            "dev@box: Permission denied (publickey,password).",
            ProbeStatus::AuthenticationFailed,
        ),
        (
            "Received disconnect: Too many authentication failures",
            ProbeStatus::AuthenticationFailed,
        ),
        (
            "Host key verification failed.",
            ProbeStatus::HostKeyRejected,
        ),
        (
            "ssh: Could not resolve hostname nowhere: nodename nor servname provided",
            ProbeStatus::Unreachable,
        ),
        (
            "ssh: connect to host 10.0.0.1 port 22: Operation timed out",
            ProbeStatus::Unreachable,
        ),
        (
            "kex_exchange_identification: read: Connection reset by peer",
            ProbeStatus::Failed,
        ),
    ];
    for (stderr, expected) in cases {
        let result = classify(None, b"", stderr.as_bytes());
        assert_eq!(result.status, expected, "{stderr}");
        assert_eq!(result.host, None);
        assert_eq!(result.diagnostics, stderr);
    }
}

#[test]
fn a_reported_platform_decides_between_connected_and_unsupported() {
    let connected = classify(
        None,
        b"motd\nLITHE-PROBE-BEGIN\nLinux\naarch64\n/home/dev\nLITHE-PROBE-END\n",
        b"",
    );
    assert_eq!(connected.status, ProbeStatus::Connected);
    assert_eq!(
        connected.host,
        Some(ProbedHost {
            system: "Linux".into(),
            machine: "aarch64".into(),
            home_directory: Some("/home/dev".into()),
            server_target: Some("aarch64-unknown-linux-musl"),
            details: Default::default(),
        })
    );
    let unsupported = classify(
        None,
        b"LITHE-PROBE-BEGIN\nFreeBSD\namd64\n/home/dev\nLITHE-PROBE-END\n",
        b"",
    );
    assert_eq!(unsupported.status, ProbeStatus::UnsupportedPlatform);
    assert_eq!(unsupported.host.unwrap().server_target, None);
}

#[test]
fn diagnostics_keep_the_redacted_end_of_long_output() {
    let noise = "debug1: x\n".repeat(1_000);
    let result = classify(
        None,
        format!("{noise}Permission denied for /Users/someone/.ssh/id_ed25519").as_bytes(),
        b"",
    );
    // Standard output without a probe block is not the shell's report.
    assert_eq!(result.status, ProbeStatus::Failed);
    let result = classify(
        None,
        b"",
        format!("{noise}Permission denied for /Users/someone/.ssh/id_ed25519").as_bytes(),
    );
    assert!(result.diagnostics.len() <= DIAGNOSTICS_LIMIT);
    assert!(
        result.diagnostics.ends_with("<HOME>/.ssh/id_ed25519"),
        "{}",
        result.diagnostics
    );
}

/// Stand-in for OpenSSH: ignores its options and runs the remote command
/// with the local shell, or fails like a rejecting server.
#[cfg(unix)]
struct FakeSsh {
    root: PathBuf,
    program: PathBuf,
}

#[cfg(unix)]
impl FakeSsh {
    fn new(label: &str, body: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = temporary_root(label);
        std::fs::create_dir_all(&root).unwrap();
        let program = root.join("ssh");
        std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, program }
    }

    fn path(&self) -> &str {
        self.program.to_str().unwrap()
    }
}

#[cfg(unix)]
impl Drop for FakeSsh {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
fn probe_request() -> ProbeRequest {
    serde_json::from_value(json!({ "connection": { "host": "devbox", "user": "dev" } })).unwrap()
}

#[cfg(unix)]
fn local_uname(flag: &str) -> String {
    let output = Command::new("uname").arg(flag).output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(unix)]
#[test]
fn probing_runs_the_platform_report_through_the_ssh_client() {
    let ssh = FakeSsh::new(
        "remote-probe",
        "for last; do :; done\nexec /bin/sh -c \"$last\"",
    );
    let result = probe_with(probe_request(), Some(ssh.path())).unwrap();
    let host = result.host.expect("the probe block was read");
    assert_eq!(host.system, local_uname("-s"));
    assert_eq!(host.machine, local_uname("-m"));
    assert!(host
        .home_directory
        .as_deref()
        .is_some_and(|home| Path::new(home).is_absolute()));
    assert_eq!(result.status, ProbeStatus::Connected);
    // The details block ran with the real shell and tools.
    assert_eq!(
        host.details.kernel.as_deref(),
        Some(local_uname("-r").as_str())
    );
    assert!(host.details.cpu_count.is_some_and(|count| count > 0));
    assert!(host.details.disk_total_bytes.is_some());
}

#[cfg(unix)]
#[test]
fn a_rejecting_server_is_reported_as_an_authentication_failure() {
    let ssh = FakeSsh::new(
        "remote-probe-denied",
        "echo 'dev@devbox: Permission denied (publickey).' >&2\nexit 255",
    );
    let result = probe_with(probe_request(), Some(ssh.path())).unwrap();
    assert_eq!(result.status, ProbeStatus::AuthenticationFailed);
    assert_eq!(
        result.diagnostics,
        "dev@devbox: Permission denied (publickey)."
    );
}

#[cfg(unix)]
#[test]
fn a_missing_ssh_client_is_a_runtime_error() {
    let missing = temporary_root("remote-probe-missing").join("ssh");
    let error = probe_with(probe_request(), missing.to_str()).unwrap_err();
    assert!(matches!(error.code, ErrorCode::RuntimeMissing), "{error:?}");
}

#[cfg(unix)]
fn connect_request(servers: serde_json::Value) -> sessions::ConnectRequest {
    serde_json::from_value(json!({
        "connection": { "host": "devbox", "user": "dev" },
        "servers": servers,
        "serverVersion": "0.1.0",
    }))
    .unwrap()
}

// A rejected login ends before the remote shell runs; the result names the
// reason instead of failing the request.
#[cfg(unix)]
#[test]
fn connecting_to_a_rejecting_server_reports_the_reason() {
    let ssh = FakeSsh::new(
        "remote-connect-denied",
        "echo 'dev@devbox: Permission denied (password).' >&2\nexit 255",
    );
    let result = sessions::connect_with(connect_request(json!([])), Some(ssh.path())).unwrap();
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["status"], "authenticationFailed");
    assert_eq!(value["session"], serde_json::Value::Null);
    assert_eq!(
        value["diagnostics"],
        "dev@devbox: Permission denied (password)."
    );
}

// The shell runs and reports its platform, but the host ships no server for
// it, so nothing is uploaded and the platform is reported.
#[cfg(unix)]
#[test]
fn connecting_without_a_matching_server_reports_the_platform() {
    let ssh = FakeSsh::new(
        "remote-connect-platform",
        "for last; do :; done\nexec /bin/sh -c \"$last\"",
    );
    let result = sessions::connect_with(connect_request(json!([])), Some(ssh.path())).unwrap();
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["status"], "unsupportedPlatform");
    assert_eq!(value["host"]["system"], local_uname("-s"));
}

/// Variables the fake client started with, as its body wrote them with
/// `env > "$0.environment"`.
#[cfg(unix)]
fn environment_seen_by(ssh: &FakeSsh) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(ssh.program.with_extension("environment"))
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

// ssh sends the remote host every local variable a `SendEnv` pattern names,
// so the probe and the connection start it with only the variables it needs
// and the AskPass helper's (S26).
#[cfg(unix)]
#[test]
fn the_ssh_client_starts_with_only_the_variables_it_needs() {
    // Cargo sets its package variables for every test process; none is kept.
    assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
    let body = "env > \"$0.environment\"\n\
                echo 'dev@devbox: Permission denied (password).' >&2\nexit 255";
    let probing = FakeSsh::new("remote-probe-environment", body);
    probe_with(probe_request(), Some(probing.path())).unwrap();
    let connecting = FakeSsh::new("remote-connect-environment", body);
    sessions::connect_with(connect_request(json!([])), Some(connecting.path())).unwrap();
    for ssh in [&probing, &connecting] {
        let seen = environment_seen_by(ssh);
        // Names only: values may be secrets of the machine running the test.
        let names = seen.keys().collect::<Vec<_>>();
        assert!(!seen.contains_key("CARGO_PKG_NAME"), "{names:?}");
        assert!(!seen.contains_key("LANG"), "{names:?}");
        assert_eq!(
            seen.get("PATH").map(String::as_str),
            std::env::var("PATH").ok().as_deref()
        );
        assert_eq!(
            seen.get("SSH_ASKPASS_REQUIRE").map(String::as_str),
            Some("force"),
            "{names:?}"
        );
    }
}

// The connection log keeps one category per failed status, so a log line
// tells authentication, host key and network failures apart.
#[test]
fn every_failed_status_has_its_own_log_category() {
    use crate::sessions::failure_of;
    let failed = [
        ProbeStatus::UnsupportedPlatform,
        ProbeStatus::AuthenticationFailed,
        ProbeStatus::HostKeyRejected,
        ProbeStatus::Unreachable,
        ProbeStatus::Failed,
    ];
    assert_eq!(failure_of(ProbeStatus::Connected), None);
    let codes: std::collections::BTreeSet<_> = failed
        .iter()
        .map(|status| failure_of(*status).unwrap().1)
        .collect();
    assert_eq!(codes.len(), failed.len());
    assert_eq!(
        failure_of(ProbeStatus::AuthenticationFailed),
        Some(("authentication", "authentication-failed"))
    );
}

#[cfg(unix)]
#[test]
fn unknown_server_targets_and_malformed_hashes_are_rejected() {
    for server in [
        json!({ "target": "riscv64-unknown-linux-musl", "path": "/nowhere", "sha256": "0".repeat(64) }),
        json!({ "target": "x86_64-unknown-linux-musl", "path": "/nowhere", "sha256": "0".repeat(63) }),
        json!({ "target": "x86_64-unknown-linux-musl", "path": "/nowhere", "sha256": format!("{}z", "0".repeat(63)) }),
    ] {
        let error = sessions::connect(connect_request(json!([server]))).unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest), "{error:?}");
    }
    // The expected hash is required: Core never derives it from the file.
    let missing_hash = serde_json::from_value::<sessions::ConnectRequest>(json!({
        "connection": { "host": "devbox" },
        "servers": [{ "target": "x86_64-unknown-linux-musl", "path": "/nowhere" }],
        "serverVersion": "0.1.0",
    }));
    assert!(missing_hash.is_err());
}

// The build for the host's platform is read once the shell reports the
// platform. A file that cannot be read, or whose bytes are not the build the
// request's SHA-256 describes, is refused before anything reaches the host.
#[cfg(unix)]
#[test]
fn a_server_build_that_is_not_the_expected_one_is_never_installed() {
    let ssh = FakeSsh::new(
        "remote-connect-replaced",
        "for last; do :; done\nexec /bin/sh -c \"$last\"",
    );
    let target =
        lithe_remote::client::ServerTarget::from_uname(&local_uname("-s"), &local_uname("-m"))
            .expect("the test machine has a server target")
            .rust_target();
    let replaced = ssh.root.join("replaced-server");
    std::fs::write(&replaced, b"not the build the manifest describes").unwrap();
    let install_root = ssh.root.join("install");
    for (path, problem) in [
        (replaced.clone(), "does not match the expected SHA-256"),
        (ssh.root.join("missing-server"), "could not be read"),
    ] {
        let request: sessions::ConnectRequest = serde_json::from_value(json!({
            "connection": { "host": "devbox", "user": "dev" },
            "servers": [{ "target": target, "path": path, "sha256": "ab".repeat(32) }],
            "serverVersion": "0.1.0",
            "installRoot": install_root,
        }))
        .unwrap();
        let error = sessions::connect_with(request, Some(ssh.path())).unwrap_err();
        assert!(
            matches!(error.code, ErrorCode::RemoteServerInvalid),
            "{error:?}"
        );
        let details = error.details.unwrap_or_default();
        assert!(details.contains(problem), "{details}");
        assert!(!details.contains(path.to_str().unwrap()), "{details}");
    }
    assert!(!install_root.exists());
}

#[cfg(unix)]
#[test]
fn closed_connections_cannot_be_used() {
    let request: sessions::ReadDirRequest =
        serde_json::from_value(json!({ "sessionId": "missing", "path": "/" })).unwrap();
    assert!(matches!(
        sessions::read_dir(request).unwrap_err().code,
        ErrorCode::ConnectionClosed
    ));
    let request: sessions::DisconnectRequest =
        serde_json::from_value(json!({ "sessionId": "missing" })).unwrap();
    assert!(!sessions::disconnect(request));
}

// Application exit closes whatever is open and must be safe with nothing open.
#[cfg(unix)]
#[test]
fn closing_all_connections_with_none_open_succeeds() {
    let connections = std::sync::Mutex::new(std::collections::HashMap::new());
    assert_eq!(sessions::close_registry(&connections), 0);
}

/// A scratch directory no other fixture shares, even across processes.
#[cfg(unix)]
fn temporary_root(label: &str) -> PathBuf {
    static NONCE: OnceLock<u128> = OnceLock::new();
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nonce = NONCE.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos()
    });
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "lithe-workspace-{label}-{}-{nonce}-{sequence}",
        std::process::id()
    ))
}
