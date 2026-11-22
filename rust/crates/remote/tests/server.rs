//! Handshake, filesystem and launch behavior of the server binary over the
//! local transport.

mod support;

use lithe_remote::client::launch::{
    launch, launch_command, LaunchError, Launched, Stage, UnsafeReason,
};
use lithe_remote::client::session::ClientError;
use lithe_remote::client::{
    ConnectionKind, ConnectionLog, RemoteSession, ServerArtifact, TransportChain, WriteOptions,
};
use lithe_remote::protocol::frame::{read_frame, skip_preamble, write_frame};
use lithe_remote::protocol::methods::{
    self, encode_bytes, FileKind, FileVersion, OperatingSystem, OrphanProtection, WatchBackend,
};
use lithe_remote::protocol::{ErrorCode, Message};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use support::{connect, connect_command, options, TempDir, SERVER, STEP_TIMEOUT};

fn code<T: std::fmt::Debug>(result: Result<T, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Remote(error)) => error.code,
        other => panic!("expected a remote error, got {other:?}"),
    }
}

#[test]
fn project_plugin_settings_bind_to_the_host_root_and_exact_contents() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let fixture = TempDir::new("project-plugin-settings");
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let source = root.join(".lithe/plugin-settings.json");
    let (session, _events) = connect();
    let request = |operation| {
        session.request_value(
            methods::FS_PROJECT_PLUGIN_SETTINGS,
            json!({"root": root, "operation": operation}),
            STEP_TIMEOUT,
        )
    };
    let read = || request(json!({"kind": "read"})).unwrap();
    let publish = |snapshot: &serde_json::Value, content: Option<&str>| {
        request(json!({"kind": "publish", "baseline": snapshot["baseline"], "content": content}))
    };
    let missing = read();
    assert!(missing["text"].is_null());
    assert!(!root.join(".lithe").exists());
    let original = r#"{"dev.lithe.test.settings":{"flag":true}}"#;
    assert_eq!(publish(&missing, Some(original)).unwrap(), true);
    let original_snapshot = read();
    assert_eq!(original_snapshot["text"], original);
    assert_eq!(original_snapshot["path"], ".lithe/plugin-settings.json");
    assert_eq!(publish(&original_snapshot, Some(original)).unwrap(), false);
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o440)).unwrap();
    assert_eq!(code(publish(&read(), None)), ErrorCode::PermissionDenied);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();

    // Same-size external change defeats a size-based comparison.
    let external = original.replace("true", "null");
    std::fs::write(&source, &external).unwrap();
    assert_eq!(code(publish(&original_snapshot, None)), ErrorCode::Conflict);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), external);
    let changed = read();
    assert_eq!(publish(&changed, Some(original)).unwrap(), true);
    assert_eq!(
        std::fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let before_move = read();
    std::fs::rename(&root, fixture.path().join("previous-project")).unwrap();
    std::fs::create_dir_all(root.join(".lithe")).unwrap();
    std::fs::write(&source, original).unwrap();
    assert_eq!(code(publish(&before_move, None)), ErrorCode::Conflict);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
    let before_link = read();
    std::fs::remove_file(&source).unwrap();
    let outside = fixture.path().join("unrelated");
    std::fs::write(&outside, original).unwrap();
    symlink(&outside, &source).unwrap();
    assert_eq!(
        code(publish(&before_link, None)),
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        code(request(json!({"kind": "read"}))),
        ErrorCode::PermissionDenied
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), original);
    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, original).unwrap();
    assert_eq!(publish(&read(), None).unwrap(), true);
    assert!(read()["text"].is_null());
    assert!(!source.exists());
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn run_configuration_edits_reject_a_stale_batch_before_publishing() {
    let fixture = TempDir::new("run-configuration-edits");
    let root = fixture.path();
    let legacy = ".lithe/run/local.json";
    let saved = ".lithe/run/local.yaml";
    std::fs::create_dir_all(root.join(".lithe/run")).unwrap();
    std::fs::write(root.join(legacy), "edited elsewhere").unwrap();
    let (session, _events) = connect();
    let apply = |changes| {
        session.request_value(
            methods::FS_APPLY_RUN_CONFIG_CHANGES,
            json!({"root": root, "changes": changes}),
            STEP_TIMEOUT,
        )
    };
    let migrate = |base: &[u8]| {
        json!([
            {"path": saved, "baseSha256": null, "content": "migrated"},
            {"path": legacy, "baseSha256": format!("{:x}", Sha256::digest(base)), "content": null}
        ])
    };

    assert_eq!(code(apply(migrate(b"before editing"))), ErrorCode::Conflict);
    assert!(!root.join(saved).exists());
    assert_eq!(
        std::fs::read_to_string(root.join(legacy)).unwrap(),
        "edited elsewhere"
    );

    apply(migrate(b"edited elsewhere")).unwrap();
    assert!(!root.join(legacy).exists());
    assert_eq!(
        std::fs::read_to_string(root.join(saved)).unwrap(),
        "migrated"
    );
    assert_eq!(
        code(apply(json!([{"path": ".lithe/unrelated",
            "baseSha256": null, "content": "must not write"}]))),
        ErrorCode::InvalidRequest
    );
    assert!(!root.join(".lithe/unrelated").exists());
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn scoped_files_read_bounded_prefixes_and_do_not_follow_workspace_links() {
    use std::ffi::CString;
    use std::os::unix::{ffi::OsStrExt, fs::symlink};

    let fixture = TempDir::new("scoped-files");
    let root = fixture.path().join("repo");
    let other = fixture.path().join("outside");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir(&other).unwrap();
    std::fs::write(root.join("src/file"), b"hello").unwrap();
    std::fs::write(other.join("secret"), b"secret").unwrap();
    symlink(&other, root.join("escape")).unwrap();
    symlink(other.join("secret"), root.join("link")).unwrap();
    symlink("src/file", root.join("internal-link")).unwrap();
    let fifo = CString::new(root.join("fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: fifo holds a NUL-terminated path in the test's private fixture.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let root_alias = fixture.path().join("repo-alias");
    symlink(&root, &root_alias).unwrap();

    let (session, _events) = connect();
    let request = |path: &str, access: serde_json::Value| {
        session.request_value(
            methods::FS_SCOPED_FILE,
            json!({"root":root_alias,"path":path,"access":access}),
            STEP_TIMEOUT,
        )
    };
    let list = json!({"kind":"list"});
    let stat = json!({"kind":"stat"});
    let read = json!({"kind":"read","length":5});
    assert_eq!(
        request("", list.clone()).unwrap(),
        json!([{"name":"src","kind":"directory"}])
    );
    assert_eq!(
        request("src", list.clone()).unwrap(),
        json!([{"name":"file","kind":"file"}])
    );
    let metadata = request("src/file", stat.clone()).unwrap();
    assert_eq!(metadata["kind"], "file");
    assert_eq!(metadata["size"], 5);
    assert!(metadata["modifiedMilliseconds"].is_i64());
    assert_eq!(request("link", stat.clone()).unwrap()["kind"], "symlink");
    assert_eq!(
        request("src/file", read.clone()).unwrap(),
        json!({"data":encode_bytes(b"hello"),"isTruncated":false})
    );
    assert_eq!(
        request("src/file", json!({"kind":"read","length":4})).unwrap(),
        json!({"data":encode_bytes(b"hell"),"isTruncated":true})
    );
    for path in [
        "../outside/secret",
        "/src/file",
        "src/../src/file",
        "src\\file",
    ] {
        assert_eq!(code(request(path, read.clone())), ErrorCode::InvalidPath);
    }
    for path in ["escape/secret", "link", "internal-link", "fifo"] {
        assert_eq!(
            code(request(path, read.clone())),
            ErrorCode::PermissionDenied
        );
    }
    assert_eq!(code(request("escape", list)), ErrorCode::PermissionDenied);
    assert_eq!(
        code(request("escape/secret", stat)),
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        code(request("src/file/missing", read)),
        ErrorCode::NotADirectory
    );
    for length in [0, methods::MAX_SCOPED_READ_BYTES + 1] {
        assert_eq!(
            code(request("src/file", json!({"kind":"read","length":length}))),
            ErrorCode::TooLarge
        );
    }
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn scoped_search_matches_disk_text_and_bounds_results() {
    let fixture = TempDir::new("scoped-search");
    let root = fixture.path();
    std::fs::create_dir(root.join("generated")).unwrap();
    std::fs::write(root.join("generated/skip.txt"), "Needle").unwrap();
    std::fs::write(root.join("other.md"), "Needle").unwrap();
    std::fs::write(root.join("first.txt"), "header\r\nNeedle\r\nneedles\r\n").unwrap();
    let encoded: Vec<u8> = std::iter::once(0xfeff)
        .chain("needle\n".encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    std::fs::write(root.join("second.txt"), encoded).unwrap();
    let (session, _events) = connect();
    let request = |query: &str, text, case, words, regex| {
        session.request_value(
            methods::FS_SCOPED_SEARCH,
            json!({
                "root": root,
                "hiddenDirectories": ["generated"],
                "search": {
                    "query": query, "fileMask": "*.txt", "text": text,
                    "caseSensitive": case, "wholeWords": words, "regularExpression": regex
                }
            }),
            STEP_TIMEOUT,
        )
    };
    assert_eq!(
        request("FIRST", false, false, false, false).unwrap()["matches"],
        json!([{"path":"first.txt", "line":null, "preview":null}])
    );
    let result = request("^needle$", true, false, false, true).unwrap();
    assert_eq!(result["isTruncated"], false);
    assert_eq!(
        result["matches"],
        json!([
            {"path":"first.txt", "line":2, "preview":"Needle"},
            {"path":"second.txt", "line":1, "preview":"needle"}
        ])
    );
    assert_eq!(
        request("Needle", true, true, true, false).unwrap()["matches"],
        json!([{"path":"first.txt", "line":2, "preview":"Needle"}])
    );
    assert_eq!(
        code(request("[", true, false, false, true)),
        ErrorCode::InvalidRequest
    );
    std::fs::write(
        root.join("many.txt"),
        "Needle\n".repeat(methods::MAX_SCOPED_SEARCH_RESULTS + 1),
    )
    .unwrap();
    let capped = request("Needle", true, true, true, false).unwrap();
    assert_eq!(capped["isTruncated"], true);
    assert_eq!(
        capped["matches"].as_array().unwrap().len(),
        methods::MAX_SCOPED_SEARCH_RESULTS
    );
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn staged_upload_checks_order_digest_and_atomic_no_replace_publication() {
    let root = TempDir::new("staged-upload");
    let (session, _events) = connect();
    let request = |method: &str, params| session.request_value(method, params, STEP_TIMEOUT);
    let content = vec![b'p'; methods::MAX_UPLOAD_CHUNK_BYTES + 7];
    let digest = format!("{:x}", Sha256::digest(&content));
    let start = |name: &str, digest: &str| {
        request(methods::FS_UPLOAD_START,
        json!({"path": root.join(name), "size": content.len(), "sha256": digest, "mode": 0o755})).unwrap()["uploadId"].as_u64().unwrap()
    };
    let id = start("package", &digest);
    assert_eq!(
        code(request(
            methods::FS_UPLOAD_CHUNK,
            json!({"uploadId": id, "offset": 0,
        "data": encode_bytes(&vec![b'x'; methods::MAX_UPLOAD_CHUNK_BYTES + 1])})
        )),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        code(request(
            methods::FS_UPLOAD_CHUNK,
            json!({"uploadId": id, "offset": 1, "data": encode_bytes(b"x")})
        )),
        ErrorCode::Conflict
    );
    for (index, chunk) in content.chunks(methods::MAX_UPLOAD_CHUNK_BYTES).enumerate() {
        let offset = index * methods::MAX_UPLOAD_CHUNK_BYTES;
        let received = request(
            methods::FS_UPLOAD_CHUNK,
            json!({"uploadId": id, "offset": offset, "data": encode_bytes(chunk)}),
        )
        .unwrap();
        assert_eq!(received["offset"], offset + chunk.len());
        assert!(
            !std::path::Path::new(&root.join("package")).exists(),
            "uncommitted bytes stay private"
        );
    }
    let result = request(methods::FS_UPLOAD_COMMIT, json!({"uploadId": id})).unwrap();
    assert_eq!(result["sha256"], digest);
    assert_eq!(std::fs::read(root.join("package")).unwrap(), content);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(root.join("package"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    let bad = start("bad-digest", &"0".repeat(64));
    for (index, chunk) in content.chunks(methods::MAX_UPLOAD_CHUNK_BYTES).enumerate() {
        request(
            methods::FS_UPLOAD_CHUNK,
            json!({"uploadId": bad, "offset": index * methods::MAX_UPLOAD_CHUNK_BYTES,
            "data": encode_bytes(chunk)}),
        )
        .unwrap();
    }
    assert_eq!(
        code(request(methods::FS_UPLOAD_COMMIT, json!({"uploadId": bad}))),
        ErrorCode::Conflict
    );
    assert!(!std::path::Path::new(&root.join("bad-digest")).exists());
    let collision = start("package", &digest);
    for (index, chunk) in content.chunks(methods::MAX_UPLOAD_CHUNK_BYTES).enumerate() {
        request(methods::FS_UPLOAD_CHUNK, json!({"uploadId": collision, "offset": index * methods::MAX_UPLOAD_CHUNK_BYTES, "data": encode_bytes(chunk)})).unwrap();
    }
    assert_eq!(
        code(request(
            methods::FS_UPLOAD_COMMIT,
            json!({"uploadId": collision})
        )),
        ErrorCode::AlreadyExists
    );
    let aborted = start("aborted", &digest);
    request(methods::FS_UPLOAD_ABORT, json!({"uploadId": aborted})).unwrap();
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        1,
        "all staging inodes are removed"
    );
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn staged_upload_disconnect_drops_unpublished_files_and_connection_ownership() {
    let root = TempDir::new("upload-disconnect");
    let daemon = support::Daemon::new("upload-disconnect");
    let (session, _events) = daemon.connect(None);
    let id = session.request_value(methods::FS_UPLOAD_START,
        json!({"path": root.join("package"), "size": 1, "sha256": "0".repeat(64), "mode": 0o600}), STEP_TIMEOUT).unwrap()["uploadId"].as_u64().unwrap();
    drop(session);
    let (replacement, _) = daemon.connect(None);
    assert_eq!(
        code(replacement.request_value(
            methods::FS_UPLOAD_COMMIT,
            json!({"uploadId": id}),
            STEP_TIMEOUT
        )),
        ErrorCode::NotFound
    );
    replacement.shutdown(STEP_TIMEOUT);
    assert!(daemon.wait_until_stopped(), "upload server did not stop");
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn staged_upload_limit_and_shutdown_release_private_files() {
    let root = TempDir::new("upload-cleanup");
    let daemon = support::Daemon::new("upload-cleanup");
    let (session, _events) = daemon.connect(None);
    for index in 0..4 {
        session.request_value(methods::FS_UPLOAD_START,
            json!({"path": root.join(&format!("file-{index}")), "size": 1, "sha256": "0".repeat(64), "mode": 0o600}), STEP_TIMEOUT).unwrap();
    }
    assert_eq!(code(session.request_value(methods::FS_UPLOAD_START,
        json!({"path": root.join("overflow"), "size": 1, "sha256": "0".repeat(64), "mode": 0o600}), STEP_TIMEOUT)), ErrorCode::Busy);
    session.shutdown(STEP_TIMEOUT);
    assert!(daemon.wait_until_stopped(), "upload server did not stop");
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn tree_mutations_copy_publish_without_replacing_and_preserve_symbolic_links() {
    let root = TempDir::new("tree-mutations");
    let (session, _events) = connect();
    let request = |method: &str, params| session.request_value(method, params, STEP_TIMEOUT);
    request(
        methods::FS_CREATE_DIR,
        json!({"path": root.join("source"), "recursive": false}),
    )
    .unwrap();
    std::fs::write(root.join("source/file with spaces"), "host content").unwrap();
    std::os::unix::fs::symlink("file with spaces", root.join("source/link")).unwrap();
    let copied = request(
        methods::FS_COPY,
        json!({"from": root.join("source"), "to": root.join("copy"), "recursive": true}),
    )
    .unwrap();
    assert_eq!(copied["skippedSpecialFiles"], 0);
    assert_eq!(
        std::fs::read_to_string(root.join("copy/link")).unwrap(),
        "host content"
    );
    assert!(std::fs::symlink_metadata(root.join("copy/link"))
        .unwrap()
        .is_symlink());
    std::fs::write(root.join("copy/file with spaces"), "destination edits").unwrap();
    assert_eq!(
        code(request(
            methods::FS_COPY,
            json!({"from": root.join("source"), "to": root.join("copy"), "recursive": true})
        )),
        ErrorCode::AlreadyExists
    );
    assert_eq!(
        std::fs::read_to_string(root.join("copy/file with spaces")).unwrap(),
        "destination edits"
    );
    request(
        methods::FS_RENAME,
        json!({"from": root.join("copy"), "to": root.join("renamed")}),
    )
    .unwrap();
    request(
        methods::FS_REMOVE,
        json!({"path": root.join("renamed"), "recursive": true}),
    )
    .unwrap();
    assert!(!std::path::Path::new(&root.join("renamed")).exists());
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        1,
        "no staging directory remains"
    );
    session.shutdown(STEP_TIMEOUT);
}

#[cfg(target_os = "linux")]
#[test]
fn trash_moves_content_and_recovery_metadata_to_the_remote_account_data_directory() {
    let root = TempDir::new("trash");
    let (session, _events) = support::connect_with_environment(&[("XDG_DATA_HOME", root.str())]);
    std::fs::write(root.join("recover me.txt"), "keep these bytes").unwrap();
    session
        .request_value(
            methods::FS_TRASH,
            json!({"path": root.join("recover me.txt")}),
            STEP_TIMEOUT,
        )
        .unwrap();
    assert!(!std::path::Path::new(&root.join("recover me.txt")).exists());
    let entries = std::fs::read_dir(root.join("Trash/files"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        std::fs::read_to_string(entries[0].path()).unwrap(),
        "keep these bytes"
    );
    let metadata = std::fs::read_dir(root.join("Trash/info"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let metadata = std::fs::read_to_string(metadata.path()).unwrap();
    assert!(metadata.contains("[Trash Info]"));
    assert!(metadata.contains("recover%20me.txt"));
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn handshake_reports_the_host_as_capabilities() {
    let (session, _events) = connect();
    let hello = session.hello();
    let host = &hello.host;
    if cfg!(target_os = "macos") {
        assert_eq!(host.os, OperatingSystem::Macos);
        assert_eq!(host.watch.backend, WatchBackend::Fsevents);
        assert_eq!(
            host.process.orphan_protection,
            OrphanProtection::ServerShutdown
        );
    } else {
        assert_eq!(host.os, OperatingSystem::Linux);
        assert_eq!(host.watch.backend, WatchBackend::Inotify);
        assert_eq!(
            host.process.orphan_protection,
            OrphanProtection::ParentDeathSignal
        );
    }
    assert_eq!(host.arch, std::env::consts::ARCH);
    assert_eq!(host.path_separator, "/");
    assert!(host
        .home_directory
        .as_deref()
        .is_some_and(|home| home.starts_with('/')));
    let mut sorted = host.features.clone();
    sorted.sort();
    assert_eq!(host.features, sorted);
    // A `--stdio` session ends with its connection, so nobody can reattach
    // and it issues no token.
    assert!(!hello.session.persistent);
    assert!(!hello.session.resumed);
    assert!(hello.session.session_token.is_none());
    assert!(session.ping().is_ok());
    session.shutdown(STEP_TIMEOUT);
}

// The handshake gate and protocol-version check are observed on the raw
// stream, because the client library always says hello first.
#[test]
fn requests_before_hello_and_foreign_protocol_versions_are_rejected() {
    let mut child = Command::new(SERVER)
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    skip_preamble(&mut output, 0).unwrap();
    let mut exchange = |message: Message| -> Message {
        write_frame(&mut input, &serde_json::to_vec(&message).unwrap()).unwrap();
        serde_json::from_slice(&read_frame(&mut output).unwrap().unwrap()).unwrap()
    };

    let early = exchange(Message::Request {
        id: 1,
        method: methods::FS_STAT.into(),
        params: json!({ "path": "/" }),
    });
    assert!(
        matches!(early, Message::Response { error: Some(ref error), .. } if error.code == ErrorCode::InvalidRequest)
    );
    let mismatch = exchange(Message::Request {
        id: 2,
        method: methods::SERVER_HELLO.into(),
        params: json!({ "protocolVersion": 999, "clientName": "test" }),
    });
    assert!(
        matches!(mismatch, Message::Response { error: Some(ref error), .. } if error.code == ErrorCode::ProtocolMismatch)
    );

    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn files_are_written_atomically_and_guarded_against_concurrent_changes() {
    let directory = TempDir::new("files");
    let (session, _events) = connect();
    let file = directory.join("nested/a.txt");
    let create = WriteOptions {
        create_parents: true,
        ..WriteOptions::default()
    };

    let created = session.write_file(&file, b"one", create).unwrap();
    assert_eq!((created.kind, created.size), (FileKind::File, 3));
    // Without a version guard an existing file is never replaced silently.
    assert_eq!(
        code(session.write_file(&file, b"x", WriteOptions::default())),
        ErrorCode::AlreadyExists
    );
    let guarded = WriteOptions {
        expected: Some(FileVersion {
            size: created.size,
            modified_ms: created.modified_ms,
        }),
        ..WriteOptions::default()
    };
    session
        .write_file(&file, b"second", guarded.clone())
        .unwrap();
    // The first version is now stale: another editor's change must not be lost.
    assert_eq!(
        code(session.write_file(&file, b"third", guarded)),
        ErrorCode::Conflict
    );

    let (content, metadata) = session.read_file(&file).unwrap();
    assert_eq!(content, b"second");
    assert_eq!(metadata.size, 6);
    // No temporary files are left next to the target.
    let names: Vec<_> = session
        .read_dir(&directory.join("nested"))
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["a.txt"]);
    session.shutdown(STEP_TIMEOUT);
}

// Two creations of one path both pass the existence check before either
// commits. The commit must refuse to replace, or the first file is lost
// while both requests report success.
#[test]
fn concurrent_creations_of_one_path_keep_exactly_one_file() {
    let directory = TempDir::new("create-race");
    let (session, _events) = connect();
    for round in 0..20 {
        let file = directory.join(&format!("{round}.txt"));
        let results: Vec<_> = std::thread::scope(|scope| {
            let writers: Vec<_> = [b"left".as_slice(), b"right".as_slice()]
                .into_iter()
                .map(|data| {
                    let (session, file) = (&session, &file);
                    scope.spawn(move || {
                        (
                            data,
                            session.write_file(file, data, WriteOptions::default()),
                        )
                    })
                })
                .collect();
            writers
                .into_iter()
                .map(|writer| writer.join().unwrap())
                .collect()
        });
        let winners: Vec<&[u8]> = results
            .iter()
            .filter(|(_, result)| result.is_ok())
            .map(|(data, _)| *data)
            .collect();
        assert_eq!(winners.len(), 1, "round {round}: {results:?}");
        for (_, result) in results.into_iter().filter(|(_, result)| result.is_err()) {
            assert_eq!(code(result), ErrorCode::AlreadyExists);
        }
        assert_eq!(std::fs::read(&file).unwrap(), winners[0]);
    }
    session.shutdown(STEP_TIMEOUT);
}

// A client that disconnects right after sending a write must not leave the
// write's temporary file behind: the server lets running requests finish
// before it exits.
#[test]
fn a_write_in_flight_at_disconnect_completes_without_leftovers() {
    let directory = TempDir::new("drain");
    let file = directory.join("large.bin");
    let data = vec![7_u8; 8 << 20];
    let mut child = Command::new(SERVER)
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    skip_preamble(&mut output, 0).unwrap();
    let mut send = |id, method: &str, params| {
        let message = Message::Request {
            id,
            method: method.into(),
            params,
        };
        write_frame(&mut input, &serde_json::to_vec(&message).unwrap()).unwrap();
    };
    send(
        1,
        methods::SERVER_HELLO,
        json!({ "protocolVersion": lithe_remote::protocol::PROTOCOL_VERSION, "clientName": "test" }),
    );
    send(
        2,
        methods::FS_WRITE_FILE,
        json!({ "path": file, "data": encode_bytes(&data) }),
    );
    drop(input);
    // Responses are drained so the server never blocks on a full pipe.
    while read_frame(&mut output).ok().flatten().is_some() {}
    assert!(child.wait().unwrap().success());

    assert_eq!(std::fs::read(&file).unwrap(), data);
    let names: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["large.bin"]);
}

/// Reader that blocks while its gate is closed, standing in for a server
/// whose input stopped being consumed.
struct GatedReader {
    inner: std::io::PipeReader,
    gate: std::sync::Arc<Gate>,
}

#[derive(Default)]
struct Gate {
    closed: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
}

impl Gate {
    fn set_closed(&self, closed: bool) {
        *self.closed.lock().unwrap() = closed;
        self.changed.notify_all();
    }
}

impl std::io::Read for GatedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut closed = self.gate.closed.lock().unwrap();
        while *closed {
            closed = self.gate.changed.wait(closed).unwrap();
        }
        drop(closed);
        self.inner.read(buffer)
    }
}

/// Opens the gate on drop so the server thread can finish after a failure.
struct OpenOnDrop(std::sync::Arc<Gate>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.set_closed(false);
    }
}

// A request whose frame cannot be written because the peer stopped reading
// must still end at its timeout, and must not hold up later requests.
#[test]
fn a_peer_that_stops_reading_cannot_block_requests_past_their_timeout() {
    let (server_input, client_output) = std::io::pipe().unwrap();
    let (client_input, server_output) = std::io::pipe().unwrap();
    let gate = std::sync::Arc::new(Gate::default());
    let _open = OpenOnDrop(std::sync::Arc::clone(&gate));
    let (served, server_done) = std::sync::mpsc::channel();
    {
        let reader = GatedReader {
            inner: server_input,
            gate: std::sync::Arc::clone(&gate),
        };
        std::thread::spawn(move || {
            let _ = served.send(lithe_remote::server::serve_stdio(reader, server_output));
        });
    }
    let (session, _events) =
        RemoteSession::from_streams(client_input, client_output, &options()).unwrap();
    let session = std::sync::Arc::new(session);
    let directory = TempDir::new("stalled");

    gate.set_closed(true);
    // The requests run on a helper thread so that a regression fails at the
    // test's own deadline instead of hanging the suite.
    let (finished, stalled) = std::sync::mpsc::channel();
    {
        let session = std::sync::Arc::clone(&session);
        // Far larger than a pipe buffer, so writing it blocks.
        let large =
            json!({ "path": directory.join("large"), "data": encode_bytes(&vec![0; 1 << 20]) });
        std::thread::spawn(move || {
            let short = std::time::Duration::from_millis(200);
            let first = session.request_value(methods::FS_WRITE_FILE, large, short);
            let second = session.request_value(methods::FS_STAT, json!({ "path": "/" }), short);
            // Released before reporting, so the test can take the session back.
            drop(session);
            let _ = finished.send((first, second));
        });
    }
    let (first, second) = stalled
        .recv_timeout(STEP_TIMEOUT)
        .expect("requests blocked past their timeout");
    assert!(matches!(first, Err(ClientError::Timeout)), "{first:?}");
    assert!(matches!(second, Err(ClientError::Timeout)), "{second:?}");

    gate.set_closed(false);
    let session = std::sync::Arc::try_unwrap(session)
        .ok()
        .expect("the helper thread released the session");
    session.stat("/").unwrap();
    session.shutdown(STEP_TIMEOUT);
    server_done
        .recv_timeout(STEP_TIMEOUT)
        .expect("server did not stop after shutdown")
        .unwrap();
}

// Files larger than one transfer are read in chunks and reassembled.
#[test]
fn large_files_are_read_in_chunks() {
    let directory = TempDir::new("large");
    let path = directory.path().join("big.bin");
    let content: Vec<u8> = (0..(9 * 1024 * 1024))
        .map(|index| (index % 251) as u8)
        .collect();
    std::fs::write(&path, &content).unwrap();
    let (session, _events) = connect();
    let (read, metadata) = session.read_file(path.to_str().unwrap()).unwrap();
    assert_eq!(metadata.size, content.len() as u64);
    assert!(read == content, "chunked read differs from the file");
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn writing_through_a_symbolic_link_keeps_the_link() {
    let directory = TempDir::new("links");
    std::fs::write(directory.path().join("target.txt"), b"old").unwrap();
    std::os::unix::fs::symlink("target.txt", directory.path().join("link.txt")).unwrap();
    let (session, _events) = connect();

    let overwrite = WriteOptions {
        overwrite: true,
        ..WriteOptions::default()
    };
    session
        .write_file(&directory.join("link.txt"), b"new", overwrite)
        .unwrap();
    let link = session.stat(&directory.join("link.txt")).unwrap();
    assert_eq!(link.kind, FileKind::Symlink);
    assert_eq!(link.symlink_target.as_deref(), Some("target.txt"));
    assert_eq!(link.symlink_target_kind, Some(FileKind::File));
    assert_eq!(
        std::fs::read(directory.path().join("target.txt")).unwrap(),
        b"new"
    );
    assert_eq!(
        session.real_path(&directory.join("link.txt")).unwrap(),
        directory.join("target.txt")
    );
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn directory_operations_report_stable_error_codes() {
    let directory = TempDir::new("dirs");
    let (session, _events) = connect();
    let tree = directory.join("tree");
    session.create_dir(&format!("{tree}/b/c"), true).unwrap();
    std::fs::write(format!("{tree}/a.txt"), b"a").unwrap();

    let names: Vec<_> = session
        .read_dir(&tree)
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["a.txt", "b"]);
    assert_eq!(
        code(session.rename(&format!("{tree}/b"), &format!("{tree}/a.txt"), false)),
        ErrorCode::AlreadyExists
    );
    session
        .rename(&format!("{tree}/b"), &format!("{tree}/d"), false)
        .unwrap();
    assert_eq!(
        code(session.remove(&format!("{tree}/d"), false)),
        ErrorCode::DirectoryNotEmpty
    );
    session.remove(&format!("{tree}/d"), true).unwrap();
    assert_eq!(
        code(session.stat(&format!("{tree}/d"))),
        ErrorCode::NotFound
    );
    assert_eq!(code(session.stat("relative/path")), ErrorCode::InvalidPath);
    assert!(session.volume_info(&tree).unwrap().case_sensitive.is_some());
    session.shutdown(STEP_TIMEOUT);
}

// Pasting a folder in the project tree copies it recursively; links inside
// must stay links rather than duplicating what they point to.
#[test]
fn copies_keep_links_and_permissions_and_refuse_to_nest() {
    use std::os::unix::fs::PermissionsExt;
    let directory = TempDir::new("copy");
    let source = directory.path().join("source");
    std::fs::create_dir_all(source.join("bin")).unwrap();
    std::fs::write(source.join("bin/run.sh"), b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        source.join("bin/run.sh"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    std::os::unix::fs::symlink("bin/run.sh", source.join("start")).unwrap();
    let (session, _events) = connect();

    session
        .copy(
            &directory.join("source"),
            &directory.join("copy"),
            true,
            false,
        )
        .unwrap();
    let script = session.stat(&directory.join("copy/bin/run.sh")).unwrap();
    assert_eq!(script.mode, 0o750);
    let link = session.stat(&directory.join("copy/start")).unwrap();
    assert_eq!(link.kind, FileKind::Symlink);
    assert_eq!(link.symlink_target.as_deref(), Some("bin/run.sh"));

    assert_eq!(
        code(session.copy(
            &directory.join("source"),
            &directory.join("source/inner"),
            true,
            false
        )),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        code(session.copy(
            &directory.join("source"),
            &directory.join("copy"),
            true,
            false
        )),
        ErrorCode::AlreadyExists
    );
    session
        .set_permissions(&directory.join("copy/bin/run.sh"), 0o700)
        .unwrap();
    assert_eq!(
        session
            .stat(&directory.join("copy/bin/run.sh"))
            .unwrap()
            .mode,
        0o700
    );
    session.shutdown(STEP_TIMEOUT);
}

// Regression: overwriting truncates the target before the source is read,
// so copying a file onto itself, through a link or a hard link, erased it.
#[test]
fn copying_a_file_onto_itself_is_refused_and_keeps_the_file() {
    let directory = TempDir::new("self-copy");
    let file = directory.path().join("a.txt");
    std::fs::write(&file, b"precious").unwrap();
    std::os::unix::fs::symlink("a.txt", directory.path().join("link")).unwrap();
    std::fs::hard_link(&file, directory.path().join("hard")).unwrap();
    let (session, _events) = connect();
    for (from, to) in [
        ("a.txt", "a.txt"),
        ("a.txt", "link"),
        ("link", "a.txt"),
        ("a.txt", "hard"),
        ("hard", "a.txt"),
    ] {
        assert_eq!(
            code(session.copy(&directory.join(from), &directory.join(to), false, true)),
            ErrorCode::InvalidRequest,
            "{from} -> {to}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"precious", "{from} -> {to}");
    }
    // Two links to one file may still replace each other.
    std::os::unix::fs::symlink("a.txt", directory.path().join("other")).unwrap();
    session
        .copy(
            &directory.join("link"),
            &directory.join("other"),
            false,
            true,
        )
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"precious");
    session.shutdown(STEP_TIMEOUT);
}

// Regression: copying a tree that contains a FIFO blocked forever on opening
// it and kept a worker slot.
#[test]
fn copies_skip_special_files() {
    let directory = TempDir::new("fifo-copy");
    let source = directory.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data.txt"), b"data").unwrap();
    let fifo = std::ffi::CString::new(source.join("pipe").to_str().unwrap()).unwrap();
    // SAFETY: the path is a NUL-terminated string that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let (session, _events) = connect();

    let result = session
        .copy(
            &directory.join("source"),
            &directory.join("copy"),
            true,
            false,
        )
        .unwrap();
    assert_eq!(result.skipped_special_files, 1);
    assert_eq!(
        std::fs::read(directory.path().join("copy/data.txt")).unwrap(),
        b"data"
    );
    assert!(!directory.path().join("copy/pipe").exists());
    assert_eq!(
        code(session.copy(
            &directory.join("source/pipe"),
            &directory.join("pipe"),
            false,
            false
        )),
        ErrorCode::Unsupported
    );
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn shell_banners_before_the_server_are_ignored() {
    let (session, _events) = connect_command(&[
        "sh".into(),
        "-c".into(),
        "printf 'Welcome to devbox'; exec \"$0\" --stdio".into(),
        SERVER.into(),
    ]);
    session.stat("/").unwrap();
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn a_missing_server_reports_the_transport_output() {
    let result = RemoteSession::connect(
        &TransportChain::local(),
        &[
            "sh".into(),
            "-c".into(),
            "echo 'lithe-remote-server: not found' >&2; exit 127".into(),
        ],
        &options(),
    );
    match result {
        Err(ClientError::Disconnected { diagnostics }) => {
            assert!(diagnostics.contains("not found"), "{diagnostics}")
        }
        Err(other) => panic!("unexpected error {other:?}"),
        Ok(_) => panic!("connected to a missing server"),
    }
}

/// Runs the launch script in a local shell against `root`, exactly as one SSH
/// connection would, and returns the streams the server takes over.
fn run_launch(
    root: &str,
) -> (
    Result<Launched, LaunchError>,
    Child,
    ChildStdout,
    ChildStdin,
) {
    let (result, child, output, input, _) = run_launch_with_stages(root);
    (result, child, output, input)
}

/// [`run_launch`], also returning the stages the launch reported.
fn run_launch_with_stages(
    root: &str,
) -> (
    Result<Launched, LaunchError>,
    Child,
    ChildStdout,
    ChildStdin,
    Vec<Stage>,
) {
    let mut stages = Vec::new();
    let mut child = Command::new("sh")
        .args(&launch_command(Some(root), &["--stdio"])[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let binary = std::fs::read(SERVER).unwrap();
    let result = launch(
        &mut output,
        &mut input,
        |report| {
            let target = report.target()?;
            Some(ServerArtifact::from_bytes(
                target,
                binary,
                env!("CARGO_PKG_VERSION"),
            ))
        },
        &ConnectionLog::begin(ConnectionKind::Ssh),
        |stage| stages.push(stage),
    );
    (result, child, output, input, stages)
}

/// Launches the server, checks that it serves a request and returns whether
/// this launch uploaded it. The stages must come in order, with the upload's
/// progress counting up to the binary's size only when it was sent.
fn start_server(root: &str) -> bool {
    let (launched, mut child, output, input, stages) = run_launch_with_stages(root);
    let launched = launched.unwrap();
    assert_eq!(&stages[..2], [Stage::Authenticated, Stage::PlatformChecked]);
    assert_eq!(
        stages.last(),
        Some(&Stage::Installed {
            uploaded: launched.uploaded
        })
    );
    let progress: Vec<(usize, usize)> = stages[2..stages.len() - 1]
        .iter()
        .map(|stage| match stage {
            Stage::Uploading { sent, total } => (*sent, *total),
            other => panic!("unexpected stage {other:?}"),
        })
        .collect();
    if launched.uploaded {
        let total = std::fs::metadata(SERVER).unwrap().len() as usize;
        assert_eq!(progress.first(), Some(&(0, total)));
        assert_eq!(progress.last(), Some(&(total, total)));
        assert!(progress.windows(2).all(|pair| pair[0].0 < pair[1].0));
    } else {
        assert!(progress.is_empty(), "{progress:?}");
    }
    let (session, _events) = RemoteSession::from_streams(output, input, &options()).unwrap();
    session.stat("/").unwrap();
    session.shutdown(STEP_TIMEOUT);
    assert!(child.wait().unwrap().success());
    launched.uploaded
}

/// The one binary installed under `root`.
fn installed_server(root: &Path) -> PathBuf {
    let versions: Vec<_> = std::fs::read_dir(root.join("bin"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(versions.len(), 1, "{versions:?}");
    versions[0].join("lithe-remote-server")
}

// One connection probes, installs and starts the server, so a host that
// asks for a password asks once; the second launch reuses the install.
#[test]
fn one_connection_installs_once_and_starts_the_server() {
    let install = TempDir::new("launch");
    for expected_upload in [true, false] {
        assert_eq!(start_server(install.str()), expected_upload);
    }
}

// The host hashes the installed file itself, so a file that prints the
// expected identity but holds other bytes is replaced instead of run.
#[test]
fn an_installed_file_with_other_bytes_is_replaced() {
    let install = TempDir::new("launch-replace");
    assert!(start_server(install.str()));
    let installed = installed_server(install.path());
    let expected = format!("{:x}", Sha256::digest(std::fs::read(SERVER).unwrap()));
    std::fs::write(&installed, format!("#!/bin/sh\necho {expected}\n")).unwrap();

    assert!(start_server(install.str()));
    let replaced = format!("{:x}", Sha256::digest(std::fs::read(&installed).unwrap()));
    assert_eq!(replaced, expected);
}

// Anyone who can write to an install directory can replace the server, so
// such a directory is refused before a binary is reused or received there.
#[test]
fn install_directories_that_are_not_private_are_refused() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let install = TempDir::new("launch-unsafe");
    let shared_mode = |path: &str, mode| {
        std::fs::create_dir_all(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let shared = install.join("shared");
    shared_mode(&shared, 0o777);
    let shared_bin = install.join("shared-bin");
    shared_mode(&shared_bin, 0o700);
    shared_mode(&format!("{shared_bin}/bin"), 0o770);
    let linked = install.join("linked");
    std::fs::create_dir(install.join("target")).unwrap();
    symlink(install.join("target"), &linked).unwrap();
    let file = install.join("file");
    std::fs::write(&file, "").unwrap();

    for (root, refused, expected) in [
        (&shared, shared.clone(), UnsafeReason::WritableByOthers),
        (
            &shared_bin,
            format!("{shared_bin}/bin"),
            UnsafeReason::WritableByOthers,
        ),
        (&linked, linked.clone(), UnsafeReason::SymbolicLink),
        (&file, file.clone(), UnsafeReason::NotADirectory),
    ] {
        let (result, mut child, output, input) = run_launch(root);
        match result {
            Err(LaunchError::UnsafeInstall { path, reason }) => {
                assert_eq!((path, reason), (refused, expected))
            }
            other => panic!("{root}: expected a refusal, got {other:?}"),
        }
        drop((output, input));
        assert_eq!(child.wait().unwrap().code(), Some(94), "{root}");
    }
    assert!(!Path::new(&shared).join("bin").exists());
    assert_eq!(
        std::fs::read_dir(format!("{shared_bin}/bin"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        std::fs::read_dir(install.join("target")).unwrap().count(),
        0
    );
}
