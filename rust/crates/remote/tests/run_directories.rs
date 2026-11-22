//! Runtime snapshot allocation and cleanup through the real protocol.

mod support;

use lithe_remote::protocol::{methods, ErrorCode};
use serde_json::json;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use support::{TempDir, STEP_TIMEOUT};

#[test]
fn plugin_deployments_reuse_published_files_and_sweep_only_unleased_versions() {
    use std::fs;

    let home = TempDir::new("plugin-deployment-home");
    let environment = [("HOME", home.str())];
    let (publisher, _) = support::connect_with_environment(&environment);
    let (consumer, _) = support::connect_with_environment(&environment);
    let (sweeper, _) = support::connect_with_environment(&environment);
    assert!(publisher
        .hello()
        .host
        .features
        .contains(&methods::Feature::PluginDeployments));
    let plugin = json!({"pluginId": "3rd-party.tools", "version": "1.2.3"});
    let mut key = plugin.clone();
    key["sha256"] = json!("a".repeat(64));
    let lookup = |session: &lithe_remote::client::RemoteSession| {
        session
            .request_value(
                methods::FS_PLUGIN_DEPLOYMENT_LOOKUP,
                key.clone(),
                STEP_TIMEOUT,
            )
            .unwrap()
    };
    let sweep = |installed| {
        sweeper
            .request_value(
                methods::FS_PLUGIN_DEPLOYMENT_SWEEP,
                json!({"installed": installed}),
                STEP_TIMEOUT,
            )
            .unwrap();
    };
    assert!(lookup(&consumer)["path"].is_null());

    let directory = json!({"directoryId": "deployment"});
    let staging = publisher
        .request_value(
            methods::FS_CREATE_RUNTIME_STAGING,
            directory.clone(),
            STEP_TIMEOUT,
        )
        .unwrap();
    let staging = Path::new(staging["path"].as_str().unwrap());
    let package = staging.join("package");
    fs::create_dir(&package).unwrap();
    fs::set_permissions(&package, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(package.join("server"), b"approved bytes").unwrap();
    fs::set_permissions(package.join("server"), fs::Permissions::from_mode(0o500)).unwrap();
    let published = publisher
        .request_value(
            methods::FS_PLUGIN_DEPLOYMENT_COMMIT,
            json!({"key": key, "directoryId": "deployment"}),
            STEP_TIMEOUT,
        )
        .unwrap();
    assert!(!staging.exists());
    publisher
        .request_value(methods::FS_RELEASE_RUN_DIRECTORY, directory, STEP_TIMEOUT)
        .unwrap();

    assert_eq!(lookup(&consumer), published);
    let deployed = Path::new(published["path"].as_str().unwrap()).join("server");
    assert_eq!(fs::read(&deployed).unwrap(), b"approved bytes");
    assert_eq!(fs::metadata(&deployed).unwrap().mode() & 0o777, 0o500);
    publisher.shutdown(STEP_TIMEOUT);
    sweep(json!([]));
    assert!(deployed.exists());

    consumer.shutdown(STEP_TIMEOUT);
    sweep(json!([plugin]));
    assert!(deployed.exists());
    sweep(json!([]));
    assert!(!deployed.exists());
    assert!(lookup(&sweeper)["path"].is_null());
    sweeper.shutdown(STEP_TIMEOUT);
}

#[test]
fn run_directory_release_is_idempotent_and_never_adopts_existing_paths() {
    let home = TempDir::new("run-home");
    let (session, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let request = |method: &str, id: &str| {
        session.request_value(method, json!({"directoryId": id}), STEP_TIMEOUT)
    };
    assert!(session
        .hello()
        .host
        .features
        .contains(&methods::Feature::RunDirectories));
    let allocated = request(methods::FS_CREATE_RUN_DIRECTORY, "owned").unwrap();
    let path = Path::new(allocated["path"].as_str().unwrap());
    assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o777, 0o700);
    std::fs::write(path.join("plugin"), b"approved bytes").unwrap();
    assert!(request(methods::FS_CREATE_RUN_DIRECTORY, "owned").is_err());
    request(methods::FS_RELEASE_RUN_DIRECTORY, "owned").unwrap();
    request(methods::FS_RELEASE_RUN_DIRECTORY, "owned").unwrap();
    assert!(!path.exists());
    std::fs::create_dir(path).unwrap();
    std::fs::write(path.join("user"), b"not owned").unwrap();
    assert!(request(methods::FS_CREATE_RUN_DIRECTORY, "owned").is_err());
    request(methods::FS_RELEASE_RUN_DIRECTORY, "owned").unwrap();
    assert_eq!(std::fs::read(path.join("user")).unwrap(), b"not owned");
    let invalid = request(methods::FS_CREATE_RUN_DIRECTORY, "../escape").unwrap_err();
    assert!(
        matches!(invalid, lithe_remote::client::session::ClientError::Remote(error) if error.code == ErrorCode::InvalidRequest)
    );
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn another_session_cannot_adopt_or_release_an_active_snapshot() {
    let home = TempDir::new("shared-run-home");
    let (owner, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let (other, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let params = json!({"directoryId": "same-identity"});
    let allocated = owner
        .request_value(
            methods::FS_CREATE_RUN_DIRECTORY,
            params.clone(),
            STEP_TIMEOUT,
        )
        .unwrap();
    let path = Path::new(allocated["path"].as_str().unwrap());
    std::fs::write(path.join("plugin"), b"owned bytes").unwrap();
    assert!(other
        .request_value(
            methods::FS_CREATE_RUN_DIRECTORY,
            params.clone(),
            STEP_TIMEOUT
        )
        .is_err());
    other
        .request_value(
            methods::FS_RELEASE_RUN_DIRECTORY,
            params.clone(),
            STEP_TIMEOUT,
        )
        .unwrap();
    other.shutdown(STEP_TIMEOUT);
    assert_eq!(std::fs::read(path.join("plugin")).unwrap(), b"owned bytes");
    owner
        .request_value(methods::FS_RELEASE_RUN_DIRECTORY, params, STEP_TIMEOUT)
        .unwrap();
    assert!(!path.exists());
    owner.shutdown(STEP_TIMEOUT);
}

#[test]
fn failed_release_reports_an_error_and_keeps_ownership_for_retry() {
    let home = TempDir::new("run-retry-home");
    let (session, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let params = json!({"directoryId": "retry"});
    let allocated = session
        .request_value(
            methods::FS_CREATE_RUN_DIRECTORY,
            params.clone(),
            STEP_TIMEOUT,
        )
        .unwrap();
    let path = Path::new(allocated["path"].as_str().unwrap());
    // Replacing the empty owned directory with a file produces a deterministic
    // removal error, even when the suite runs as root on Linux.
    std::fs::remove_dir(path).unwrap();
    std::fs::write(path, b"not a directory").unwrap();
    assert!(session
        .request_value(
            methods::FS_RELEASE_RUN_DIRECTORY,
            params.clone(),
            STEP_TIMEOUT
        )
        .is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"not a directory");
    std::fs::remove_file(path).unwrap();
    std::fs::create_dir(path).unwrap();
    std::fs::write(path.join("plugin"), b"retry cleanup").unwrap();
    session
        .request_value(methods::FS_RELEASE_RUN_DIRECTORY, params, STEP_TIMEOUT)
        .unwrap();
    assert!(!path.exists());
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn allocation_rejects_shared_or_linked_parents_without_changing_them() {
    let home = TempDir::new("run-parent-home");
    let base = home.path().join(".lithe-server");
    std::fs::create_dir(&base).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (session, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let params = json!({"directoryId": "private"});
    let error = session
        .request_value(
            methods::FS_CREATE_RUN_DIRECTORY,
            params.clone(),
            STEP_TIMEOUT,
        )
        .unwrap_err();
    assert!(
        matches!(error, lithe_remote::client::session::ClientError::Remote(error)
        if error.code == ErrorCode::PermissionDenied)
    );
    assert_eq!(std::fs::metadata(&base).unwrap().mode() & 0o777, 0o755);
    assert!(!base.join("plugin-runs").exists());
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = home.path().join("user-data");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, base.join("plugin-runs")).unwrap();
    assert!(session
        .request_value(methods::FS_CREATE_RUN_DIRECTORY, params, STEP_TIMEOUT)
        .is_err());
    assert!(std::fs::symlink_metadata(base.join("plugin-runs"))
        .unwrap()
        .is_symlink());
    assert_eq!(std::fs::read_dir(target).unwrap().count(), 0);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn lost_client_snapshot_is_released_when_the_stdio_session_ends() {
    let home = TempDir::new("run-disconnect-home");
    let (session, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let allocated = session
        .request_value(
            methods::FS_CREATE_RUN_DIRECTORY,
            json!({"directoryId": "disconnected"}),
            STEP_TIMEOUT,
        )
        .unwrap();
    let path = Path::new(allocated["path"].as_str().unwrap());
    std::fs::write(path.join("plugin"), b"approved bytes").unwrap();
    drop(session);
    // Server exit is external to this process; observe the owned path with a
    // monotonic deadline, never wait on a fixed sleep or shared user directory.
    let deadline = std::time::Instant::now() + STEP_TIMEOUT;
    while path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "stdio disconnect leaked its snapshot"
        );
        std::thread::yield_now();
    }
}

#[test]
fn persistent_disconnect_retains_snapshot_until_explicit_shutdown() {
    let home = TempDir::new("persistent-run-home");
    let daemon = support::Daemon::with_environment("persistent-run", &[("HOME", home.str())]);
    let (session, _) = daemon.connect(None);
    let allocated = session
        .request_value(
            methods::FS_CREATE_RUN_DIRECTORY,
            json!({"directoryId": "persistent"}),
            STEP_TIMEOUT,
        )
        .unwrap();
    let path = Path::new(allocated["path"].as_str().unwrap());
    std::fs::write(path.join("plugin"), b"approved bytes").unwrap();
    drop(session);
    let (replacement, _) = daemon.connect(None);
    assert_eq!(
        std::fs::read(path.join("plugin")).unwrap(),
        b"approved bytes"
    );
    // The consuming process is still using this snapshot when the session
    // ends. Teardown must stop it before removing its working directory.
    let process = replacement
        .spawn(lithe_remote::client::SpawnSpec {
            argv: vec!["/bin/cat".into()],
            cwd: Some(path.to_string_lossy().into_owned()),
            pipe_stdin: true,
            ..Default::default()
        })
        .unwrap();
    let pid = process.pid;
    drop(process); // Dropping the subscription leaves the consumer running.
    replacement.shutdown(STEP_TIMEOUT);
    assert!(daemon.wait_until_stopped());
    assert!(
        support::wait_until_gone(pid),
        "snapshot consumer survived shutdown"
    );
    assert!(!path.exists());
}

#[test]
fn abandoned_desktop_snapshot_is_reclaimed_after_persistent_session_expiry() {
    let home = TempDir::new("expired-run-home");
    let daemon = support::Daemon::with_environment("expired-run", &[("HOME", home.str())]);
    let (session, _) = daemon.connect(Some(std::time::Duration::from_secs(1)));
    let allocated = session
        .request_value(
            methods::FS_CREATE_RUN_DIRECTORY,
            json!({"directoryId": "expired"}),
            STEP_TIMEOUT,
        )
        .unwrap();
    let path = Path::new(allocated["path"].as_str().unwrap());
    std::fs::write(path.join("plugin"), b"approved bytes").unwrap();
    drop(session);
    assert!(daemon.wait_until_stopped());
    assert!(!path.exists());
}

const RUNTIME_PROGRAM: &[u8] = b"#!/bin/sh\nprintf managed-runtime\n";

fn managed_runtime_archive(format: &str, link: &str) -> Vec<u8> {
    use std::io::{Cursor, Write};

    if format == "zip" {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        archive.start_file("release/bin/server", options).unwrap();
        archive.write_all(RUNTIME_PROGRAM).unwrap();
        archive
            .add_symlink("release/entry/server", link, options)
            .unwrap();
        return archive.finish().unwrap().into_inner();
    }

    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    let mut file = tar::Header::new_gnu();
    file.set_size(RUNTIME_PROGRAM.len() as u64);
    file.set_mode(0o755);
    archive
        .append_data(&mut file, "release/bin/server", RUNTIME_PROGRAM)
        .unwrap();
    let mut symlink = tar::Header::new_gnu();
    symlink.set_entry_type(tar::EntryType::Symlink);
    symlink.set_mode(0o777);
    symlink.set_size(0);
    archive
        .append_link(&mut symlink, "release/entry/server", link)
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}

fn upload_runtime_fixture(
    session: &lithe_remote::client::RemoteSession,
    directory_id: &str,
    bytes: &[u8],
) -> (std::path::PathBuf, String) {
    use sha2::{Digest, Sha256};

    let request =
        |method: &str, params| session.request_value(method, params, STEP_TIMEOUT).unwrap();
    let digest = format!("{:x}", Sha256::digest(bytes));
    let staging = request(
        methods::FS_CREATE_RUNTIME_STAGING,
        json!({"directoryId": directory_id}),
    );
    let staging = std::path::PathBuf::from(staging["path"].as_str().unwrap());
    let started = request(
        methods::FS_UPLOAD_START,
        json!({
            "path": staging.join("archive"),
            "size": bytes.len(),
            "sha256": digest,
            "mode": 0o400,
        }),
    );
    let id = started["uploadId"].as_u64().unwrap();
    for (index, chunk) in bytes.chunks(methods::MAX_UPLOAD_CHUNK_BYTES).enumerate() {
        request(
            methods::FS_UPLOAD_CHUNK,
            json!({
                "uploadId": id,
                "offset": index * methods::MAX_UPLOAD_CHUNK_BYTES,
                "data": methods::encode_bytes(chunk),
            }),
        );
    }
    request(methods::FS_UPLOAD_COMMIT, json!({"uploadId": id}));
    (staging, digest)
}

fn runtime_layout(format: &str, key: char, archive_sha256: &str) -> serde_json::Value {
    json!({
        "key": {"pluginId": "dev.fixture.runtime", "version": "1.0.0", "sha256": key.to_string().repeat(64)},
        "archiveSha256": archive_sha256, "archive": format,
        "stripPrefix": "release", "home": "", "executable": "entry/server"
    })
}

#[test]
fn managed_runtime_archives_publish_read_only_launch_and_reuse_after_reconnect() {
    use lithe_remote::client::SpawnSpec;
    use std::fs;

    let home = TempDir::new("managed-runtime");
    let daemon = support::Daemon::with_environment("managed-runtime", &[("HOME", home.str())]);
    let (session, _) = daemon.connect(None);
    assert!(session
        .hello()
        .host
        .features
        .contains(&methods::Feature::ManagedRuntimeDeployments));
    let mut installed = Vec::new();
    for (format, key) in [("zip", 'a'), ("tar.gz", 'b')] {
        let archive = managed_runtime_archive(format, "../bin/server");
        let (staging, digest) = upload_runtime_fixture(&session, "runtime-format", &archive);
        let layout = runtime_layout(format, key, &digest);
        let lookup = session
            .request_value(
                methods::FS_MANAGED_RUNTIME_LOOKUP,
                layout.clone(),
                STEP_TIMEOUT,
            )
            .unwrap();
        assert!(lookup.is_null());
        let paths = session
            .request_value(
                methods::FS_MANAGED_RUNTIME_COMMIT,
                json!({
                    "directoryId": "runtime-format", "layout": layout
                }),
                STEP_TIMEOUT,
            )
            .unwrap();
        assert!(!staging.exists());
        let runtime_home = Path::new(paths["home"].as_str().unwrap());
        let executable = Path::new(paths["executable"].as_str().unwrap());
        assert!(executable.starts_with(runtime_home));
        assert_eq!(fs::metadata(runtime_home).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(executable).unwrap().mode() & 0o777, 0o500);
        assert_eq!(
            fs::read_link(runtime_home.join("entry/server")).unwrap(),
            Path::new("../bin/server")
        );
        let process = session
            .spawn(SpawnSpec {
                argv: vec![executable.to_str().unwrap().to_owned()],
                ..SpawnSpec::default()
            })
            .unwrap();
        assert_eq!(
            process.wait(STEP_TIMEOUT).unwrap().stdout,
            b"managed-runtime"
        );
        installed.push((layout, paths));
    }
    drop(session);
    let (replacement, _) = daemon.connect(None);
    let (sweeper, _) = support::connect_with_environment(&[("HOME", home.str())]);
    let sweep = || {
        sweeper
            .request_value(
                methods::FS_PLUGIN_DEPLOYMENT_SWEEP,
                json!({"installed": []}),
                STEP_TIMEOUT,
            )
            .unwrap()
    };
    for (layout, expected) in &installed {
        let found = replacement
            .request_value(
                methods::FS_MANAGED_RUNTIME_LOOKUP,
                layout.clone(),
                STEP_TIMEOUT,
            )
            .unwrap();
        assert_eq!(&found, expected);
    }
    sweep();
    for (_, paths) in &installed {
        assert!(Path::new(paths["home"].as_str().unwrap()).exists());
    }
    replacement.shutdown(STEP_TIMEOUT);
    assert!(daemon.wait_until_stopped());
    sweep();
    for (_, paths) in &installed {
        assert!(!Path::new(paths["home"].as_str().unwrap()).exists());
    }
    sweeper.shutdown(STEP_TIMEOUT);
}

#[test]
fn managed_runtime_refuses_wrong_hash_and_escaping_links_without_publishing() {
    let home = TempDir::new("managed-runtime-rejections");
    let (session, _) = support::connect_with_environment(&[("HOME", home.str())]);
    for (format, key, target) in [
        ("zip", 'c', "../bin/server"),
        ("zip", 'd', "../../escape"),
        ("tar.gz", 'e', "../../escape"),
    ] {
        let archive = managed_runtime_archive(format, target);
        let (staging, digest) = upload_runtime_fixture(&session, "bad-runtime", &archive);
        let wrong = "f".repeat(64);
        let layout = runtime_layout(format, key, if key == 'c' { &wrong } else { &digest });
        let result = session.request_value(
            methods::FS_MANAGED_RUNTIME_COMMIT,
            json!({
                "directoryId": "bad-runtime", "layout": layout
            }),
            STEP_TIMEOUT,
        );
        assert!(
            matches!(&result, Err(lithe_remote::client::session::ClientError::Remote(error))
                if error.code == ErrorCode::InvalidRequest),
            "invalid {format} accepted or not diagnosed: {result:?}"
        );
        assert!(!staging.exists());
        assert!(session
            .request_value(methods::FS_MANAGED_RUNTIME_LOOKUP, layout, STEP_TIMEOUT)
            .unwrap()
            .is_null());
    }
    assert!(!home.path().join("escape").exists());
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn runtime_staging_is_reclaimed_when_cancelled_or_when_persistent_client_disconnects() {
    let home = TempDir::new("runtime-staging");
    let daemon = support::Daemon::with_environment("runtime-staging", &[("HOME", home.str())]);
    let (session, _) = daemon.connect(None);
    let archive = managed_runtime_archive("zip", "../bin/server");
    let (cancelled, digest) = upload_runtime_fixture(&session, "cancelled", &archive);
    session
        .request_value(
            methods::FS_RELEASE_RUN_DIRECTORY,
            json!({"directoryId": "cancelled"}),
            STEP_TIMEOUT,
        )
        .unwrap();
    assert!(!cancelled.exists());
    let layout = runtime_layout("zip", 'f', &digest);
    let commit = |session: &lithe_remote::client::RemoteSession, id| {
        session.request_value(
            methods::FS_MANAGED_RUNTIME_COMMIT,
            json!({"directoryId": id, "layout": layout}),
            STEP_TIMEOUT,
        )
    };
    assert!(commit(&session, "cancelled").is_err());

    let (disconnected, _) = upload_runtime_fixture(&session, "disconnected", &archive);
    drop(session);
    let (replacement, _) = daemon.connect(None);
    assert!(commit(&replacement, "disconnected").is_err());
    assert!(!disconnected.exists());
    assert!(replacement
        .request_value(methods::FS_MANAGED_RUNTIME_LOOKUP, layout, STEP_TIMEOUT)
        .unwrap()
        .is_null());
    replacement.shutdown(STEP_TIMEOUT);
    assert!(daemon.wait_until_stopped());
}

#[test]
fn deployment_sweep_reclaims_abandoned_staging_without_touching_live_directories() {
    use std::fs;

    let home = TempDir::new("deployment-staging-sweep");
    let environment = [("HOME", home.str())];
    let (owner, _) = support::connect_with_environment(&environment);
    let (sweeper, _) = support::connect_with_environment(&environment);
    let create = |method, id| {
        owner
            .request_value(method, json!({"directoryId": id}), STEP_TIMEOUT)
            .unwrap()
    };
    let active = create(methods::FS_CREATE_RUNTIME_STAGING, "active");
    let active = Path::new(active["path"].as_str().unwrap());
    let snapshot = create(methods::FS_CREATE_RUN_DIRECTORY, "snapshot");
    let snapshot = Path::new(snapshot["path"].as_str().unwrap());
    let crashed = support::Daemon::with_environment("crashed-deployment", &environment);
    let (interrupted, _) = crashed.connect(None);
    let abandoned = interrupted
        .request_value(
            methods::FS_CREATE_RUNTIME_STAGING,
            json!({"directoryId": "abandoned"}),
            STEP_TIMEOUT,
        )
        .unwrap();
    let abandoned = Path::new(abandoned["path"].as_str().unwrap());
    fs::write(abandoned.join("unfinished-upload"), b"partial").unwrap();
    let pid = crashed.pid().expect("the test started a live daemon");
    // SAFETY: this is the daemon of this isolated test, verified by its held lock.
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) }, 0);
    assert!(crashed.wait_until_stopped());
    drop(interrupted);
    assert!(abandoned.exists(), "the server exited without cleanup");
    let alias = active.with_file_name("alias");
    std::os::unix::fs::symlink(snapshot, &alias).unwrap();

    sweeper
        .request_value(
            methods::FS_PLUGIN_DEPLOYMENT_SWEEP,
            json!({"installed": []}),
            STEP_TIMEOUT,
        )
        .unwrap();

    assert!(!abandoned.exists());
    assert!(active.is_dir());
    assert!(snapshot.is_dir());
    assert!(fs::symlink_metadata(alias).unwrap().is_symlink());
    owner.shutdown(STEP_TIMEOUT);
    sweeper.shutdown(STEP_TIMEOUT);
}
