//! Process execution and the login environment.

mod support;

use lithe_remote::client::api::ProcessEvent;
use lithe_remote::client::session::ClientError;
use lithe_remote::client::SpawnSpec;
use lithe_remote::protocol::methods::{
    self, EnvironmentSource, OutputStream, ProcessExitEvent, ProcessOutputEvent,
    ResolvedEnvironmentSource, ResourceLabel,
};
use lithe_remote::protocol::ErrorCode;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::time::Instant;
use support::{
    connect, connect_command, connect_with_environment, wait_until_gone, TempDir, SERVER,
    STEP_TIMEOUT,
};

fn argv(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn process_output_precedes_its_exit_event() {
    let (session, _events) = connect();
    let process = session
        .spawn(SpawnSpec {
            argv: argv(&["sh", "-c", "printf out; printf err >&2; exit 3"]),
            env: BTreeMap::from([("LITHE_TEST".into(), "1".into())]),
            ..SpawnSpec::default()
        })
        .unwrap();
    let output = process.wait(STEP_TIMEOUT).unwrap();
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
    assert_eq!(output.exit.unwrap().exit_code, Some(3));
    drop(process);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn scoped_process_enters_only_a_real_workspace_directory() {
    let (session, _events) = connect();
    let root = TempDir::new("scoped-process");
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("marker"), b"nested").unwrap();
    let outside = TempDir::new("outside-process");
    std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();

    let refused = session.request_value(
        methods::PROCESS_SPAWN_SCOPED,
        json!({
            "processId": session.allocate_id(), "argv": ["true"],
            "scopedCwd": {"root": root.path(), "path": "escape"},
        }),
        STEP_TIMEOUT,
    );
    match refused {
        Err(ClientError::Remote(error)) => assert_eq!(error.code, ErrorCode::PermissionDenied),
        other => panic!("expected a scoped cwd refusal, got {other:?}"),
    }

    let process_id = session.allocate_id();
    let events = session.subscribe("process", process_id);
    session
        .request_value(
            methods::PROCESS_SPAWN_SCOPED,
            json!({
                "processId": process_id, "argv": ["/bin/cat", "marker"],
                "scopedCwd": {"root": root.path(), "path": "nested"},
            }),
            STEP_TIMEOUT,
        )
        .unwrap();
    let mut output = Vec::new();
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        match event.event.as_str() {
            methods::EVENT_PROCESS_OUTPUT => {
                let chunk: ProcessOutputEvent = event.decode().unwrap();
                output.extend(methods::decode_bytes(&chunk.data).unwrap());
            }
            methods::EVENT_PROCESS_EXIT => {
                let exit: ProcessExitEvent = event.decode().unwrap();
                assert_eq!(exit.exit_code, Some(0));
                break;
            }
            unexpected => panic!("unexpected process event {unexpected}"),
        }
    }
    assert_eq!(output, b"nested");
    session.unsubscribe("process", process_id);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn standard_input_is_forwarded_and_closed() {
    let (session, _events) = connect();
    let process = session
        .spawn(SpawnSpec {
            argv: argv(&["cat"]),
            pipe_stdin: true,
            ..SpawnSpec::default()
        })
        .unwrap();
    process.write_stdin(b"echoed").unwrap();
    process.close_stdin().unwrap();
    let output = process.wait(STEP_TIMEOUT).unwrap();
    assert_eq!(output.stdout, b"echoed");
    assert_eq!(output.exit.unwrap().exit_code, Some(0));
    drop(process);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn debug_port_readiness_checks_the_owned_listener_without_connecting() {
    let foreign = TcpListener::bind("127.0.0.1:0").unwrap();
    let (session, _events) = connect();
    let picked = session
        .request_value(methods::PORTS_PICK_LOOPBACK, json!({}), STEP_TIMEOUT)
        .unwrap();
    let picked: u16 = serde_json::from_value(picked).unwrap();
    assert_ne!(picked, 0);
    assert_ne!(picked, foreign.local_addr().unwrap().port());

    let executable = std::env::current_exe().unwrap();
    let process = session
        .spawn(SpawnSpec {
            argv: argv(&[
                executable.to_str().unwrap(),
                "--exact",
                "debug_port_fixture",
                "--nocapture",
            ]),
            env: BTreeMap::from([("LITHE_TEST_DEBUG_PORT_FIXTURE".into(), "1".into())]),
            pipe_stdin: true,
            ..SpawnSpec::default()
        })
        .unwrap();
    let deadline = Instant::now() + STEP_TIMEOUT;
    let mut stdout = Vec::new();
    let port = loop {
        match process
            .next_event(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Some(ProcessEvent::Output(OutputStream::Stdout, bytes)) => stdout.extend(bytes),
            other => panic!("unexpected debug port fixture event: {other:?}"),
        }
        if let Some(port) = stdout
            .split_inclusive(|byte| *byte == b'\n')
            .filter_map(|line| line.strip_suffix(b"\n"))
            .find_map(|line| line.strip_prefix(b"debug-port:"))
        {
            break std::str::from_utf8(port).unwrap().parse::<u16>().unwrap();
        }
    };
    let ready = |port| {
        session.request_value(
            methods::PROCESS_DEBUG_PORT_READY,
            json!({"processId": process.id, "port": port}),
            STEP_TIMEOUT,
        )
    };
    assert_eq!(
        ready(foreign.local_addr().unwrap().port()).unwrap(),
        json!(false)
    );
    assert_eq!(ready(port).unwrap(), json!(true));
    let mut debugger =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), STEP_TIMEOUT).unwrap();
    debugger.write_all(b"attach").unwrap();
    drop(debugger);
    process.close_stdin().unwrap();
    assert_eq!(
        process.wait(STEP_TIMEOUT).unwrap().exit.unwrap().exit_code,
        Some(0)
    );
    assert!(
        matches!(ready(port), Err(ClientError::Remote(error)) if error.code == ErrorCode::UnknownResource)
    );
    drop(process);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn debug_port_fixture() {
    if std::env::var_os("LITHE_TEST_DEBUG_PORT_FIXTURE").is_none() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    println!("\ndebug-port:{}", listener.local_addr().unwrap().port());
    // The parent closes stdin after the first real debugger connected.
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
    let mut handshake = Vec::new();
    std::io::Read::read_to_end(&mut &stream, &mut handshake).unwrap();
    assert_eq!(handshake, b"attach");
}

// Build tools start worker processes; killing only the leader would leave
// them running on the remote host.
#[test]
fn kill_reaches_the_whole_process_group() {
    let (session, _events) = connect();
    let process = session
        .spawn(SpawnSpec {
            argv: argv(&["sh", "-c", "sleep 60 & echo $!; wait"]),
            ..SpawnSpec::default()
        })
        .unwrap();
    let mut line = Vec::new();
    while !line.contains(&b'\n') {
        match process.next_event(STEP_TIMEOUT).unwrap() {
            Some(ProcessEvent::Output(OutputStream::Stdout, bytes)) => line.extend(bytes),
            other => panic!("unexpected event {other:?}"),
        }
    }
    let worker: u32 = String::from_utf8(line).unwrap().trim().parse().unwrap();
    process.kill(false).unwrap();
    let exit = process.wait(STEP_TIMEOUT).unwrap().exit.unwrap();
    assert_eq!(exit.signal, Some(libc::SIGTERM));
    assert!(
        wait_until_gone(worker),
        "worker {worker} survived the group kill"
    );
    match process.kill(false) {
        Err(ClientError::Remote(error)) => assert_eq!(error.code, ErrorCode::UnknownResource),
        other => panic!("expected UNKNOWN_RESOURCE, got {other:?}"),
    }
    drop(process);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn a_live_resource_id_cannot_be_reused() {
    let (session, _events) = connect();
    let process = session
        .spawn(SpawnSpec {
            argv: argv(&["sleep", "60"]),
            ..SpawnSpec::default()
        })
        .unwrap();
    let duplicate = session.request_value(
        methods::PROCESS_SPAWN,
        json!({ "processId": process.id, "argv": ["true"] }),
        STEP_TIMEOUT,
    );
    match duplicate {
        Err(ClientError::Remote(error)) => assert_eq!(error.code, ErrorCode::ResourceIdInUse),
        other => panic!("expected RESOURCE_ID_IN_USE, got {other:?}"),
    }
    let state = session.state().unwrap();
    assert_eq!(state.processes.len(), 1);
    assert_eq!(state.processes[0].argv, ["sleep", "60"]);
    process.kill(true).unwrap();
    process.wait(STEP_TIMEOUT).unwrap();
    drop(process);
    session.shutdown(STEP_TIMEOUT);
}

// Closing a window lists what still runs by the label its client gave and
// the time it has been running, also to a client that did not start it.
#[test]
fn state_reports_labels_and_running_time() {
    let (session, _events) = connect();
    let label = ResourceLabel {
        kind: "run".into(),
        name: "Server".into(),
    };
    let started = Instant::now();
    let labeled = session
        .spawn(SpawnSpec {
            argv: argv(&["sleep", "60"]),
            label: Some(label.clone()),
            ..SpawnSpec::default()
        })
        .unwrap();
    let unlabeled = session
        .spawn(SpawnSpec {
            argv: argv(&["sleep", "60"]),
            ..SpawnSpec::default()
        })
        .unwrap();
    let state = session.state().unwrap();
    // The server started both after `started` and measured before this.
    let elapsed = started.elapsed().as_millis();
    let find = |process_id| {
        state
            .processes
            .iter()
            .find(|running| running.process_id == process_id)
            .unwrap()
    };
    assert_eq!(find(labeled.id).label.as_ref(), Some(&label));
    assert_eq!(find(unlabeled.id).label, None);
    for running in &state.processes {
        assert!(u128::from(running.running_ms) <= elapsed);
    }
    for process in [labeled, unlabeled] {
        process.kill(true).unwrap();
        process.wait(STEP_TIMEOUT).unwrap();
    }
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn a_label_that_breaks_a_rule_starts_nothing() {
    let (session, _events) = connect();
    let directory = TempDir::new("rejected-label");
    let started = directory.join("started");
    let invalid = [
        ResourceLabel {
            kind: "k".repeat(methods::MAX_LABEL_KIND_CHARS + 1),
            name: "Server".into(),
        },
        ResourceLabel {
            kind: "run".into(),
            name: "n".repeat(methods::MAX_LABEL_NAME_CHARS + 1),
        },
        ResourceLabel {
            kind: "run".into(),
            name: "Server\n".into(),
        },
    ];
    for label in invalid {
        let result = session.spawn(SpawnSpec {
            argv: argv(&["touch", &started]),
            label: Some(label),
            ..SpawnSpec::default()
        });
        match result {
            Err(ClientError::Remote(error)) => assert_eq!(error.code, ErrorCode::InvalidRequest),
            Err(other) => panic!("expected INVALID_REQUEST, got {other:?}"),
            Ok(_) => panic!("a process with an invalid label started"),
        }
    }
    assert!(session.state().unwrap().processes.is_empty());
    assert!(
        !std::path::Path::new(&started).exists(),
        "the rejected command ran"
    );
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn disconnecting_a_stdio_session_terminates_its_processes() {
    let (session, events) = connect();
    let pid = session
        .spawn(SpawnSpec {
            argv: argv(&["sleep", "60"]),
            ..SpawnSpec::default()
        })
        .unwrap()
        .pid;
    drop(events);
    // Dropping without shutdown is what a lost network connection looks like.
    drop(session);
    assert!(wait_until_gone(pid), "process {pid} outlived the session");
}

/// Starts a session whose leader exits right away while a member keeps
/// running, as a build tool that leaves a daemon behind does. Returns the
/// handle and the surviving member's pid after the leader's exit event.
fn spawn_session_that_outlives_its_leader(
    session: &lithe_remote::client::RemoteSession,
) -> (lithe_remote::client::api::ProcessHandle<'_>, u32) {
    let process = session
        .spawn(SpawnSpec {
            argv: argv(&["sh", "-c", "sleep 60 >/dev/null 2>&1 & echo $!"]),
            ..SpawnSpec::default()
        })
        .unwrap();
    let output = process.wait(STEP_TIMEOUT).unwrap();
    assert_eq!(output.exit.unwrap().exit_code, Some(0));
    let member = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    (process, member)
}

#[test]
fn kill_reaches_members_that_outlive_the_leader() {
    let (session, _events) = connect();
    let (process, member) = spawn_session_that_outlives_its_leader(&session);
    let state = session.state().unwrap();
    assert!(state
        .processes
        .iter()
        .any(|running| running.process_id == process.id && running.leader_exited));
    process.kill(true).unwrap();
    assert!(wait_until_gone(member), "member {member} survived the kill");
    drop(process);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn disconnecting_terminates_members_that_outlive_the_leader() {
    let (session, events) = connect();
    let (process, member) = spawn_session_that_outlives_its_leader(&session);
    drop(process);
    drop(events);
    drop(session);
    assert!(
        wait_until_gone(member),
        "member {member} outlived the session"
    );
}

// Tools that version managers add to PATH in profile files must be found by
// builds started from the IDE, not only in an interactive terminal. The
// profile and shell are fixed here so the test does not run the developer's
// own shell configuration.
#[test]
fn login_environment_includes_what_profiles_export() {
    let home = TempDir::new("profile");
    std::fs::write(
        home.path().join(".profile"),
        "echo 'profile banner'\nexport LITHE_PROFILE_MARK=from-profile\n",
    )
    .unwrap();
    let (session, _events) =
        connect_with_environment(&[("HOME", home.str()), ("SHELL", "/bin/sh"), ("ENV", "")]);

    let resolved = session.resolve_environment(false).unwrap();
    assert_ne!(
        resolved.source,
        ResolvedEnvironmentSource::Server,
        "{:?}",
        resolved.diagnostics
    );
    assert_eq!(
        resolved
            .environment
            .get("LITHE_PROFILE_MARK")
            .map(String::as_str),
        Some("from-profile")
    );
    assert!(!resolved.environment.contains_key("SHLVL"));

    let process = session
        .spawn(SpawnSpec {
            argv: argv(&["sh", "-c", "printf %s \"$LITHE_PROFILE_MARK\""]),
            environment: EnvironmentSource::Login,
            ..SpawnSpec::default()
        })
        .unwrap();
    assert_eq!(process.wait(STEP_TIMEOUT).unwrap().stdout, b"from-profile");
    drop(process);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn command_candidates_use_host_login_path_in_declaration_order() {
    let home = TempDir::new("command-candidates");
    for directory in ["relative", "first", "second", "override"] {
        std::fs::create_dir(home.path().join(directory)).unwrap();
    }
    let write_executable = |name, script| {
        let path = home.path().join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    };
    // Evaluate the server's environment-printing command without reading any
    // profile files or changing the fixture's inherited PATH.
    write_executable(
        "shell",
        "#!/bin/sh\nfor script do :; done\nexec /bin/sh -c \"$script\"\n",
    );
    write_executable("relative/relative-only", "#!/bin/sh\nprintf relative\n");
    write_executable("first/fallback", "#!/bin/sh\nprintf fallback\n");
    write_executable("second/preferred", "#!/bin/sh\nprintf preferred\n");
    write_executable("override/preferred", "#!/bin/sh\nprintf override\n");
    let shell = home.join("shell");
    let path = format!("relative:{}:{}", home.join("first"), home.join("second"));
    let (session, _events) = connect_command(&[
        "/bin/sh".into(),
        "-c".into(),
        r#"cd "$1" && shift && exec "$@""#.into(),
        "server-fixture".into(),
        home.str().into(),
        "/usr/bin/env".into(),
        format!("HOME={}", home.str()),
        format!("SHELL={shell}"),
        format!("PATH={path}"),
        SERVER.into(),
        "--stdio".into(),
    ]);
    let spawn = |id, commands: &[&str]| {
        session.request_value(
            methods::PROCESS_SPAWN,
            json!({
                "processId": id,
                "argv": [""],
                "commandCandidates": commands,
                "cwd": home.str(),
                "environment": "login",
                "env": { "PATH": home.join("override") },
            }),
            STEP_TIMEOUT,
        )
    };
    let id = session.allocate_id();
    let events = session.subscribe("process", id);
    spawn(id, &["absent", "relative-only", "preferred", "fallback"]).unwrap();
    let deadline = Instant::now() + STEP_TIMEOUT;
    let mut stdout = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("candidate did not exit");
        let event = events
            .recv_timeout(remaining)
            .expect("candidate event did not arrive");
        match event.event.as_str() {
            methods::EVENT_PROCESS_OUTPUT => {
                let output: ProcessOutputEvent = event.decode().unwrap();
                assert_eq!(output.stream, OutputStream::Stdout);
                stdout.extend(methods::decode_bytes(&output.data).unwrap());
            }
            methods::EVENT_PROCESS_EXIT => {
                assert_eq!(
                    event.decode::<ProcessExitEvent>().unwrap().exit_code,
                    Some(0)
                );
                break;
            }
            other => panic!("unexpected process event: {other}"),
        }
    }
    session.unsubscribe("process", id);
    assert_eq!(stdout, b"preferred");

    assert!(matches!(
        spawn(session.allocate_id(), &["relative-only", "absent"]),
        Err(ClientError::Remote(error)) if error.code == ErrorCode::NotFound
    ));
    assert!(matches!(
        spawn(session.allocate_id(), &["second/preferred"]),
        Err(ClientError::Remote(error)) if error.code == ErrorCode::InvalidRequest
    ));
    assert!(session.state().unwrap().processes.is_empty());
    session.shutdown(STEP_TIMEOUT);
}
