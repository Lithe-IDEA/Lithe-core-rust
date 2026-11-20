//! Connection lifecycle facts (design S34) recorded while a real server is
//! installed, connected, forwarded through and released over the local
//! transport.
//!
//! Every fact must pass the log catalog Core validates with, the facts of one
//! attempt share its `connection` id, and no fact names a path, a host
//! address, a session token or diagnostics the remote printed.

mod support;

use lithe_diagnostics::core_events;
use lithe_remote::client::launch::{launch, launch_command, InstallStep, LaunchError, Launched};
use lithe_remote::client::session::ConnectOptions;
use lithe_remote::client::{
    ConnectionKind, ConnectionLog, PortForward, RemoteSession, ServerArtifact, TransportChain,
};
use lithe_remote::protocol::methods::TunnelTarget;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use support::{options, Daemon, TempDir, SERVER, STEP_TIMEOUT};

const STARTED: &str = "remote.connection.connect.started";
const FINISHED: &str = "remote.connection.connect.finished";
const FAILED: &str = "remote.connection.connect.failed";
const SHELL: &str = "remote.connection.shell_started";
const DEPLOYED: &str = "remote.server.deploy_finished";
const INTERRUPTED: &str = "remote.connection.interrupted";
const CLEANED: &str = "remote.connection.cleaned";
const FORWARD_STARTED: &str = "remote.forward.started";
const FORWARD_REMOVED: &str = "remote.forward.removed";

type Fact = (String, BTreeMap<String, Value>);

/// Every fact this test process recorded. Tests run in parallel, so each
/// reads only the facts of its own attempts, by `connection` id.
static FACTS: Mutex<Vec<Fact>> = Mutex::new(Vec::new());
static RECORDED: Condvar = Condvar::new();

fn sink(event: &str, fields: &[(&'static str, Value)]) {
    let fields = fields
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect();
    FACTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((event.to_owned(), fields));
    RECORDED.notify_all();
}

/// Installs the capturing sink; the first installation in the process wins,
/// so every test may call this before its first connection.
fn install() {
    lithe_remote::failure_log::install_failure_sink(sink);
}

fn belongs_to(fact: &Fact, log: &ConnectionLog) -> bool {
    fact.1.get("connection").and_then(Value::as_str) == Some(log.id())
}

/// Facts of one attempt in recording order, each checked against the catalog.
fn facts_of(log: &ConnectionLog) -> Vec<Fact> {
    let facts: Vec<Fact> = FACTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter(|fact| belongs_to(fact, log))
        .cloned()
        .collect();
    for (event, fields) in &facts {
        if let Err(error) = core_events::prepare(event, fields.clone(), None) {
            panic!("{event} {fields:?} does not match the catalog: {error:?}");
        }
    }
    facts
}

fn names(facts: &[Fact]) -> Vec<&str> {
    facts.iter().map(|(event, _)| event.as_str()).collect()
}

fn only<'a>(facts: &'a [Fact], event: &str) -> &'a BTreeMap<String, Value> {
    let mut matching = facts.iter().filter(|(name, _)| name == event);
    let fact = matching.next().unwrap_or_else(|| panic!("no {event}"));
    assert!(matching.next().is_none(), "two {event} facts");
    &fact.1
}

/// Waits until `log` recorded `event`. The fact is recorded on the session's
/// reader thread, so the test waits on the sink instead of sleeping.
fn wait_for(log: &ConnectionLog, event: &str) -> bool {
    let facts = FACTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_facts, waited) = RECORDED
        .wait_timeout_while(facts, STEP_TIMEOUT, |facts| {
            !facts
                .iter()
                .any(|fact| fact.0 == event && belongs_to(fact, log))
        })
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    !waited.timed_out()
}

/// Asserts that no value names a path or any of `secrets`. The only strings
/// in these facts are controlled tokens, versions and the random id.
fn assert_nothing_identifying(facts: &[Fact], secrets: &[&str]) {
    let mut secrets: Vec<String> = secrets.iter().map(|secret| (*secret).to_owned()).collect();
    secrets.extend(std::env::var("HOME").ok());
    for (event, fields) in facts {
        for (key, value) in fields {
            let Some(text) = value.as_str() else { continue };
            assert!(!text.contains('/'), "{event}.{key} holds a path: {text}");
            for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
                assert!(
                    !text.contains(secret.as_str()),
                    "{event}.{key} leaks {secret}"
                );
            }
        }
    }
    if let Ok(user) = std::env::var("USER") {
        let values = facts.iter().flat_map(|(_, fields)| fields.values());
        assert!(
            values.filter_map(Value::as_str).all(|text| text != user),
            "a fact names the user"
        );
    }
}

/// Runs the launch script in a local shell against `root`, as one SSH
/// connection would. `sha256` replaces the artifact's real digest.
fn run_launch(
    root: &str,
    log: &ConnectionLog,
    sha256: Option<&str>,
) -> (
    Result<Launched, LaunchError>,
    Child,
    ChildStdout,
    ChildStdin,
) {
    let mut child = Command::new("sh")
        .args(&launch_command(Some(root), &["--stdio"])[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let result = launch(
        &mut output,
        &mut input,
        |report| {
            let target = report.target()?;
            let mut artifact =
                ServerArtifact::from_file(target, SERVER.into(), env!("CARGO_PKG_VERSION"))
                    .unwrap();
            if let Some(sha256) = sha256 {
                artifact.sha256 = sha256.into();
            }
            Some(artifact)
        },
        log,
    );
    (result, child, output, input)
}

#[test]
fn an_upload_and_a_reuse_record_every_step_of_the_connection() {
    install();
    let root = TempDir::new("log-install");
    for action in ["uploaded", "reused"] {
        let log = ConnectionLog::begin(ConnectionKind::Ssh);
        let (launched, mut child, output, input) = run_launch(root.str(), &log, None);
        assert_eq!(launched.unwrap().uploaded, action == "uploaded");
        let options = ConnectOptions {
            log: Some(log.clone()),
            ..options()
        };
        let (session, _events) = RemoteSession::from_streams(output, input, &options).unwrap();
        session.stat("/").unwrap();
        drop(session);
        child.wait().unwrap();

        let facts = facts_of(&log);
        assert_eq!(names(&facts), [STARTED, SHELL, DEPLOYED, FINISHED, CLEANED]);
        assert_eq!(only(&facts, STARTED)["transport"], "ssh");
        let deployed = only(&facts, DEPLOYED);
        assert_eq!(deployed["action"], action);
        assert_eq!(deployed["target"], only(&facts, SHELL)["target"]);
        assert_eq!(deployed["server_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(deployed.contains_key("bytes"), action == "uploaded");
        let finished = only(&facts, FINISHED);
        assert_eq!(finished["outcome"], "connected");
        assert_eq!(finished["persistent"], false);
        assert_eq!(finished["resumed"], false);
        assert_eq!(only(&facts, CLEANED)["ended"], "closed");
        assert_nothing_identifying(&facts, &[root.str()]);
    }
}

#[test]
fn a_binary_that_fails_verification_records_the_failed_step() {
    install();
    let root = TempDir::new("log-verify");
    let log = ConnectionLog::begin(ConnectionKind::Ssh);
    let (launched, mut child, output, input) = run_launch(root.str(), &log, Some(&"0".repeat(64)));
    assert!(
        matches!(
            launched,
            Err(LaunchError::InstallFailed(InstallStep::Verify))
        ),
        "{launched:?}"
    );
    drop((output, input));
    child.wait().unwrap();

    let facts = facts_of(&log);
    assert_eq!(names(&facts), [STARTED, SHELL, FAILED]);
    let failed = only(&facts, FAILED);
    assert_eq!(failed["stage"], "deploy");
    assert_eq!(failed["error_code"], "install-verify");
    assert_eq!(failed["error_domain"], "remote");
    assert_nothing_identifying(&facts, &[root.str()]);
}

#[test]
fn a_persistent_session_records_detaching_resuming_and_shutdown() {
    install();
    let daemon = Daemon::new("log-daemon");
    let (session, _events) = daemon.connect(None);
    let first = session.connection_log().clone();
    drop(session);
    let token = serde_json::to_value(daemon.token().unwrap()).unwrap();
    let token = token.as_str().unwrap().to_owned();

    let (session, _events) = daemon.connect(None);
    let second = session.connection_log().clone();
    session.shutdown(STEP_TIMEOUT);
    assert!(daemon.wait_until_stopped(), "{}", daemon.log());

    for (log, resumed, ended) in [(&first, false, "detached"), (&second, true, "shutdown")] {
        let facts = facts_of(log);
        assert_eq!(names(&facts), [STARTED, FINISHED, CLEANED]);
        assert_eq!(only(&facts, STARTED)["transport"], "command");
        let finished = only(&facts, FINISHED);
        assert_eq!(finished["persistent"], true);
        assert_eq!(finished["resumed"], resumed);
        assert_eq!(only(&facts, CLEANED)["ended"], ended);
        assert_nothing_identifying(&facts, &[&token, &daemon.key, daemon.state.str()]);
    }
    assert_ne!(first.id(), second.id());
}

#[test]
fn a_lost_server_is_recorded_as_an_interruption() {
    install();
    let daemon = Daemon::new("log-lost");
    let (session, _events) = daemon.connect(None);
    let log = session.connection_log().clone();
    let pid = daemon.pid().expect("the daemon runs");
    // SAFETY: sending a signal has no memory-safety preconditions.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    assert!(wait_for(&log, INTERRUPTED), "no interruption recorded");
    drop(session);

    let facts = facts_of(&log);
    assert_eq!(names(&facts), [STARTED, FINISHED, INTERRUPTED, CLEANED]);
    assert_eq!(only(&facts, INTERRUPTED)["cause"], "eof");
    assert_eq!(only(&facts, CLEANED)["ended"], "lost");
    assert_nothing_identifying(&facts, &[&daemon.key, daemon.state.str()]);
}

/// A one-connection echo server that uppercases what it reads.
fn spawn_tcp_echo() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
        let mut buffer = [0_u8; 64];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    if stream
                        .write_all(&buffer[..read].to_ascii_uppercase())
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    (port, server)
}

#[test]
fn a_port_forward_records_its_ports_and_connections_but_not_the_host() {
    install();
    let (remote_port, echo) = spawn_tcp_echo();
    let (session, _events) = support::connect();
    let session = Arc::new(session);
    let forward = PortForward::start(
        Arc::clone(&session),
        "127.0.0.1:0".parse().unwrap(),
        TunnelTarget::Tcp {
            host: "127.0.0.1".into(),
            port: remote_port,
        },
    )
    .unwrap();
    let local_port = forward.local_address().port();
    let mut client = TcpStream::connect(forward.local_address()).unwrap();
    client.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
    client.write_all(b"ping").unwrap();
    let mut reply = [0_u8; 4];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"PING");
    drop(client);
    drop(forward);
    echo.join().unwrap();

    let facts = facts_of(session.connection_log());
    let forwarding: Vec<Fact> = facts
        .iter()
        .filter(|(event, _)| event.starts_with("remote.forward."))
        .cloned()
        .collect();
    assert_eq!(names(&forwarding), [FORWARD_STARTED, FORWARD_REMOVED]);
    let started = only(&forwarding, FORWARD_STARTED);
    assert_eq!(started["local_port"], local_port);
    assert_eq!(started["bind_scope"], "loopback");
    assert_eq!(started["target_kind"], "tcp");
    assert_eq!(started["remote_port"], remote_port);
    assert_eq!(started["target_scope"], "loopback");
    let removed = only(&forwarding, FORWARD_REMOVED);
    assert_eq!(removed["local_port"], local_port);
    assert_eq!(removed["connections"], 1);
    assert_nothing_identifying(&facts, &["127.0.0.1"]);
}

#[test]
fn a_failed_handshake_records_its_category_without_remote_output() {
    install();
    let leaked = "secret-diagnostic-from-the-host";
    let log = ConnectionLog::begin(ConnectionKind::Command);
    let options = ConnectOptions {
        log: Some(log.clone()),
        ..options()
    };
    let command = [
        "sh".to_owned(),
        "-c".into(),
        format!("echo {leaked} >&2; exit 3"),
    ];
    let result = RemoteSession::connect(&TransportChain::local(), &command, &options);
    assert!(result.is_err(), "connected to a shell that exited");

    let facts = facts_of(&log);
    assert_eq!(names(&facts), [STARTED, FAILED]);
    let failed = only(&facts, FAILED);
    assert_eq!(failed["stage"], "handshake");
    assert_eq!(failed["error_domain"], "remote");
    assert_nothing_identifying(&facts, &[leaked]);
}

#[test]
fn cancelling_before_the_session_opens_is_not_a_failure() {
    install();
    let log = ConnectionLog::begin(ConnectionKind::Ssh);
    log.connect_cancelled();
    // A later failure of the abandoned attempt is not a second outcome.
    log.connect_failed("connect", "failed", "remote");

    let facts = facts_of(&log);
    assert_eq!(names(&facts), [STARTED, FINISHED]);
    assert_eq!(only(&facts, FINISHED)["outcome"], "cancelled");
}
