//! End-to-end native configuration through a real server and owned staging.

mod support;

use lithe_remote::client::{session::ClientError, RemoteSession};
use lithe_remote::protocol::{methods, ErrorCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use support::{TempDir, STEP_TIMEOUT};

fn request(session: &RemoteSession, method: &str, params: Value) -> Result<Value, ClientError> {
    session.request_value(method, params, STEP_TIMEOUT)
}

fn native(session: &RemoteSession, root: &TempDir, operation: Value) -> Result<Value, ClientError> {
    request(
        session,
        methods::FS_NATIVE_SETTINGS,
        json!({"root": root.str(), "operation": operation}),
    )
}

fn error_code(result: Result<Value, ClientError>) -> ErrorCode {
    match result.expect_err("native settings request must fail") {
        ClientError::Remote(error) => error.code,
        other => panic!("unexpected client error: {other}"),
    }
}

fn staging(session: &RemoteSession, id: &str) -> PathBuf {
    let directory = request(
        session,
        methods::FS_CREATE_RUNTIME_STAGING,
        json!({"directoryId": id}),
    )
    .unwrap();
    PathBuf::from(directory["path"].as_str().unwrap())
}

fn stage(
    session: &RemoteSession,
    workspace: &TempDir,
    id: &str,
    baselines: &Value,
) -> Result<Value, ClientError> {
    native(
        session,
        workspace,
        json!({"kind": "stage", "directoryId": id, "baselines": baselines}),
    )
}

fn output(slot: u16, baseline: &Value, contents: &[u8]) -> Value {
    let hash = format!("sha256:{:x}", Sha256::digest(contents));
    json!({"slot": slot, "baseline": baseline, "revision": hash})
}

#[test]
fn native_settings_write_existing_and_missing_sources_only_on_the_host() {
    let workspace = TempDir::new("native-settings-workspace");
    let home = TempDir::new("native-settings-home");
    let existing = workspace.path().join("settings");
    fs::write(&existing, "original private document").unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o640)).unwrap();

    let (session, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let parsed = json!({"kind":"inspect","targets":[
        {"scope":"workspace","path":"settings"},
        {"scope":"home","path":"new-settings"}
    ]});
    let baselines = native(&session, &workspace, parsed.clone()).unwrap();
    assert_eq!(baselines[0]["hostPath"], workspace.join("settings"));
    assert_eq!(
        baselines[0]["revision"],
        output(0, &Value::Null, b"original private document")["revision"]
    );
    assert_eq!(baselines[1]["hostPath"], home.join("new-settings"));
    assert_eq!(baselines[1]["revision"], "missing");
    assert_eq!(baselines[1]["writable"], true);

    let scratch = staging(&session, "native-save");
    let outputs = stage(&session, &workspace, "native-save", &baselines).unwrap();
    for (index, content) in [b"new private document".as_slice(), b"created"]
        .into_iter()
        .enumerate()
    {
        let path = Path::new(outputs[index].as_str().unwrap());
        assert_eq!(path.parent(), Some(scratch.as_path()));
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::write(path, content).unwrap();
    }
    assert_eq!(fs::read(&existing).unwrap(), b"original private document");

    let replacements = [
        output(0, &baselines[0], b"new private document"),
        output(1, &baselines[1], b"created"),
    ];
    let receipt = native(
        &session,
        &workspace,
        json!({"kind":"publish","directoryId":"native-save","replacements":replacements}),
    )
    .unwrap();
    assert_eq!(receipt["committedSlots"], json!([0, 1]));
    assert!(!scratch.exists());
    assert_eq!(fs::read(&existing).unwrap(), b"new private document");
    assert_eq!(
        fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read(home.join("new-settings")).unwrap(), b"created");
    let readback = native(&session, &workspace, parsed).unwrap();
    assert_eq!(readback[0]["revision"], replacements[0]["revision"]);
    assert_eq!(readback[1]["revision"], replacements[1]["revision"]);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn native_settings_reject_link_targets_foreign_scratch_and_stale_baselines() {
    let home = TempDir::new("native-settings-conflicts");
    let workspace = TempDir::new("native-settings-root");
    let file = workspace.path().join("settings");
    fs::write(&file, b"initial").unwrap();
    symlink(&file, workspace.path().join("symbolic")).unwrap();
    fs::hard_link(&file, workspace.path().join("hard")).unwrap();
    let (session, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let (other, _) = support::connect_with_environment(&[("HOME", home.str())]);

    for (scope, path) in [
        ("workspace", "symbolic"),
        ("workspace", "hard"),
        ("home", ".lithe-server/entry"),
    ] {
        assert_eq!(
            error_code(native(
                &session,
                &workspace,
                json!({"kind":"inspect","targets":[{"scope":scope,"path":path}]})
            )),
            ErrorCode::PermissionDenied,
        );
    }
    fs::remove_file(workspace.path().join("hard")).unwrap();
    let inspect = json!({"kind":"inspect","targets":[{"scope":"workspace","path":"settings"}]});
    let baselines = native(&session, &workspace, inspect).unwrap();

    let scratch = staging(&session, "foreign");
    assert_eq!(
        error_code(stage(&other, &workspace, "foreign", &baselines)),
        ErrorCode::NotFound,
    );
    assert!(scratch.exists());
    let outputs = stage(&session, &workspace, "foreign", &baselines).unwrap();
    fs::write(outputs[0].as_str().unwrap(), b"modified").unwrap();
    let forged = output(0, &baselines[0], b"wrong hash");
    assert_eq!(
        error_code(native(
            &session,
            &workspace,
            json!({"kind":"publish","directoryId":"foreign","replacements":[forged]})
        )),
        ErrorCode::InvalidRequest,
    );
    assert!(!scratch.exists());
    assert_eq!(fs::read(&file).unwrap(), b"initial");

    let scratch = staging(&session, "stale");
    let outputs = stage(&session, &workspace, "stale", &baselines).unwrap();
    fs::write(outputs[0].as_str().unwrap(), b"modified").unwrap();
    let external = workspace.path().join("replacement");
    fs::write(&external, b"initial").unwrap();
    fs::rename(external, &file).unwrap();
    assert_eq!(
        error_code(native(
            &session,
            &workspace,
            json!({"kind":"publish","directoryId":"stale","replacements":[output(0, &baselines[0], b"modified")]})
        )),
        ErrorCode::Conflict,
    );
    assert!(!scratch.exists());
    assert_eq!(fs::read(&file).unwrap(), b"initial");
    other.shutdown(STEP_TIMEOUT);
    session.shutdown(STEP_TIMEOUT);
}
