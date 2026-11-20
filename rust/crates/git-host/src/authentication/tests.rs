//! In-process AskPass tests: the helper side runs on a thread against a real
//! channel while the test thread polls the session, with explicit deadlines.

use super::*;
use std::cell::RefCell;
use std::ffi::OsString;
use std::thread::{self, JoinHandle};

const DEADLINE: Duration = Duration::from_secs(10);
const CREDENTIAL: &str = "test-only-private-credential";

thread_local! {
    static RECORDED: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Sink for sessions polled on the test thread.
fn record(event: &str, fields: failure_log::Fields<'_>) {
    let fields = serde_json::to_string(fields).unwrap();
    RECORDED.with(|recorded| recorded.borrow_mut().push((event.to_owned(), fields)));
}

fn recorded() -> Vec<(String, String)> {
    RECORDED.with(RefCell::take)
}

fn rejection(reason: &str) -> (String, String) {
    (
        "vcs.authentication.request_rejected".into(),
        json!([["reason", reason]]).to_string(),
    )
}

fn failure(stage: &str, code: &str) -> (String, String) {
    (
        "vcs.authentication.response_failed".into(),
        json!([
            ["stage", stage],
            ["error", "Git authentication response failed"],
            ["error_code", code],
            ["error_domain", "io"]
        ])
        .to_string(),
    )
}

struct Fixture {
    session: Session,
    address: OsString,
    token: String,
}

fn fixture() -> Fixture {
    let mut session = Session::new().unwrap();
    session.sink = record;
    Fixture {
        address: session.listener.address().to_owned(),
        token: session.token.to_string(),
        session,
    }
}

fn frame(token: &str, prompt: &str) -> Vec<u8> {
    let mut frame = json!({"token": token, "prompt": prompt})
        .to_string()
        .into_bytes();
    frame.push(b'\n');
    frame
}

/// Runs the helper side of one prompt on a thread.
fn spawn_ask(address: &OsStr, token: &str, prompt: &str) -> JoinHandle<io::Result<Vec<u8>>> {
    let (address, token, prompt) = (address.to_owned(), token.to_owned(), prompt.to_owned());
    thread::spawn(move || {
        let mut out = Vec::new();
        ask(&address, &token, &prompt, &mut out).map(|()| out)
    })
}

/// Sends raw bytes as a helper would. Unless `close` is set, it then returns
/// what came back before the session closed the channel.
fn spawn_raw(address: &OsStr, bytes: Vec<u8>, close: bool) -> JoinHandle<io::Result<Vec<u8>>> {
    let address = address.to_owned();
    thread::spawn(move || {
        let mut stream = channel::connect(&address)?;
        stream.write_all(&bytes)?;
        let mut received = Vec::new();
        if !close {
            stream.read_to_end(&mut received)?;
        }
        Ok(received)
    })
}

fn next_challenge(session: &mut Session) -> Challenge {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let mut ready = session.poll().unwrap();
        if let Some(challenge) = ready.pop() {
            assert!(ready.is_empty(), "unexpected second AskPass prompt");
            return challenge;
        }
        assert!(
            Instant::now() < deadline,
            "no AskPass prompt before the deadline"
        );
        thread::yield_now();
    }
}

/// Polls until the helper thread ends; no prompt may appear meanwhile.
fn finish<T>(session: &mut Session, helper: JoinHandle<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    while !helper.is_finished() {
        assert!(
            session.poll().unwrap().is_empty(),
            "unexpected AskPass prompt"
        );
        assert!(
            Instant::now() < deadline,
            "AskPass helper did not finish before the deadline"
        );
        thread::yield_now();
    }
    helper.join().unwrap()
}

/// Polls until the session has recorded something about a helper that
/// already left.
fn settle(session: &mut Session) {
    let deadline = Instant::now() + DEADLINE;
    while RECORDED.with(|recorded| recorded.borrow().is_empty()) {
        assert!(
            session.poll().unwrap().is_empty(),
            "unexpected AskPass prompt"
        );
        assert!(
            Instant::now() < deadline,
            "the session recorded nothing before the deadline"
        );
        thread::yield_now();
    }
}

/// Joins a helper whose channel no longer needs polling.
fn join<T>(helper: JoinHandle<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    while !helper.is_finished() {
        assert!(
            Instant::now() < deadline,
            "AskPass helper did not finish before the deadline"
        );
        thread::yield_now();
    }
    helper.join().unwrap()
}

/// The helper saw the channel close without an answer.
fn assert_no_answer(result: io::Result<Vec<u8>>) {
    match result {
        Ok(received) => assert!(received.is_empty(), "the helper received bytes"),
        Err(error) => assert!(
            matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
            ),
            "unexpected helper error {error:?}"
        ),
    }
}

#[test]
fn configure_points_git_and_ssh_at_this_session() {
    let fixture = fixture();
    let mut command = Command::new("unused-fixture");
    fixture.session.configure(&mut command).unwrap();
    let env: HashMap<OsString, OsString> = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.unwrap().to_owned()))
        .collect();
    let value = |key: &str| env[OsStr::new(key)].clone();
    assert_eq!(value("GIT_ASKPASS"), std::env::current_exe().unwrap());
    assert_eq!(value("SSH_ASKPASS"), value("GIT_ASKPASS"));
    assert_eq!(value("LITHE_GIT_ASKPASS_MODE"), "1");
    assert_eq!(value("SSH_ASKPASS_REQUIRE"), "force");
    assert_eq!(value(ADDRESS_VARIABLE), fixture.address);
    assert_eq!(value(TOKEN_VARIABLE), OsStr::new(&fixture.token));
    assert_eq!(fixture.token.len(), 64);
}

#[test]
fn answer_reaches_the_helper_exactly_once() {
    let mut fixture = fixture();
    let prompt = "Password for 'https://fixture.invalid':";
    let helper = spawn_ask(&fixture.address, &fixture.token, prompt);
    let challenge = next_challenge(&mut fixture.session);
    assert_eq!(challenge.prompt, prompt);
    assert!(challenge.secret);
    assert_eq!(challenge.attempt, 1);
    let id = challenge.request_id;
    assert!(!respond(&id, Some("invalid\nresponse".into())));
    assert!(!respond(&id, Some("invalid\rresponse".into())));
    assert!(!respond(&id, Some("invalid\0response".into())));
    assert!(!respond(&id, Some("x".repeat(MAX_ANSWER + 1))));
    assert!(respond(&id, Some(CREDENTIAL.into())));
    assert!(!respond(&id, Some("replayed".into())));
    let answer = finish(&mut fixture.session, helper).unwrap();
    assert_eq!(answer, format!("{CREDENTIAL}\n").into_bytes());
    assert!(recorded().is_empty());
}

#[test]
fn longest_answer_fits_the_helper_buffer() {
    let mut fixture = fixture();
    let helper = spawn_ask(&fixture.address, &fixture.token, "Password:");
    let challenge = next_challenge(&mut fixture.session);
    let longest = "a".repeat(MAX_ANSWER);
    assert!(respond(&challenge.request_id, Some(longest.clone())));
    let answer = finish(&mut fixture.session, helper).unwrap();
    assert_eq!(answer.len(), MAX_ANSWER + 1);
    assert!(answer.starts_with(longest.as_bytes()) && answer.ends_with(b"\n"));
}

#[test]
fn cancelled_prompt_fails_the_helper() {
    let mut fixture = fixture();
    let helper = spawn_ask(&fixture.address, &fixture.token, "Password:");
    let challenge = next_challenge(&mut fixture.session);
    assert!(respond(&challenge.request_id, None));
    let error = finish(&mut fixture.session, helper).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert!(!registry().contains_key(&challenge.request_id));
    assert!(recorded().is_empty());
}

#[test]
fn wrong_token_is_rejected_without_a_prompt() {
    let mut fixture = fixture();
    let guess = "0".repeat(fixture.token.len());
    let helper = spawn_ask(&fixture.address, &guess, "Password for intruder:");
    assert_no_answer(finish(&mut fixture.session, helper));
    let recorded = recorded();
    assert_eq!(recorded, [rejection("token")]);
    assert!(!recorded[0].1.contains(&guess) && !recorded[0].1.contains("intruder"));
}

#[test]
fn frames_outside_the_protocol_are_rejected() {
    let mut fixture = fixture();
    let mut trailing = frame(&fixture.token, "Password:");
    trailing.extend_from_slice(b"extra");
    // (bytes, whether the helper closes right after sending, reason)
    let cases = [
        (b"not json\n".to_vec(), false, "malformed"),
        (b"{\"token\"".to_vec(), true, "malformed"),
        (trailing, false, "malformed"),
        (vec![b'x'; MAX_FRAME + 1], false, "oversized"),
        (
            frame(&fixture.token, &"p".repeat(MAX_PROMPT + 1)),
            false,
            "oversized",
        ),
    ];
    for (bytes, close, reason) in cases {
        let helper = spawn_raw(&fixture.address, bytes, close);
        assert_no_answer(finish(&mut fixture.session, helper));
        if close {
            // The helper left before the session read its bytes.
            settle(&mut fixture.session);
        }
        assert_eq!(recorded(), [rejection(reason)]);
    }
    assert!(fixture.session.attempts.is_empty());
}

#[test]
fn longest_prompt_is_forwarded() {
    let mut fixture = fixture();
    let prompt = "p".repeat(MAX_PROMPT);
    let helper = spawn_ask(&fixture.address, &fixture.token, &prompt);
    let challenge = next_challenge(&mut fixture.session);
    assert_eq!(challenge.prompt, prompt);
    assert!(respond(&challenge.request_id, None));
    assert_no_answer(finish(&mut fixture.session, helper));
}

#[test]
fn request_larger_than_a_frame_never_leaves_the_helper() {
    let fixture = fixture();
    let mut out = Vec::new();
    let error = ask(
        &fixture.address,
        &fixture.token,
        &"p".repeat(MAX_FRAME),
        &mut out,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(out.is_empty());
}

#[test]
fn attempts_per_prompt_are_bounded() {
    let mut fixture = fixture();
    for attempt in 1..=MAX_ATTEMPTS {
        let helper = spawn_ask(&fixture.address, &fixture.token, "Password:");
        let challenge = next_challenge(&mut fixture.session);
        assert_eq!(challenge.attempt, attempt);
        assert!(respond(&challenge.request_id, None));
        assert_no_answer(finish(&mut fixture.session, helper));
    }
    let helper = spawn_ask(&fixture.address, &fixture.token, "Password:");
    assert_no_answer(finish(&mut fixture.session, helper));
    assert_eq!(recorded(), [rejection("attempt-limit")]);
}

#[test]
fn distinct_prompts_are_bounded() {
    let mut fixture = fixture();
    let mut ask_once = |prompt: &str| {
        let helper = spawn_ask(&fixture.address, &fixture.token, prompt);
        let challenge = next_challenge(&mut fixture.session);
        assert!(respond(&challenge.request_id, None));
        assert_no_answer(finish(&mut fixture.session, helper));
        challenge.attempt
    };
    for index in 0..MAX_PROMPTS {
        assert_eq!(ask_once(&format!("Password {index}:")), 1);
    }
    // A prompt already seen is still forwarded once the limit is reached.
    assert_eq!(ask_once("Password 0:"), 2);
    let helper = spawn_ask(&fixture.address, &fixture.token, "One prompt too many:");
    assert_no_answer(finish(&mut fixture.session, helper));
    assert_eq!(recorded(), [rejection("prompt-limit")]);
}

#[test]
fn helper_that_leaves_ends_its_prompt() {
    let mut fixture = fixture();
    let address = fixture.address.clone();
    let request = frame(&fixture.token, "Password:");
    let (leave, left) = mpsc::channel::<()>();
    let helper = thread::spawn(move || {
        let mut stream = channel::connect(&address)?;
        stream.write_all(&request)?;
        left.recv_timeout(DEADLINE).map_err(io::Error::other)?;
        Ok::<_, io::Error>(())
    });
    let challenge = next_challenge(&mut fixture.session);
    leave.send(()).unwrap();
    join(helper).unwrap();
    let deadline = Instant::now() + DEADLINE;
    while registry().contains_key(&challenge.request_id) {
        assert!(fixture.session.poll().unwrap().is_empty());
        assert!(Instant::now() < deadline, "the prompt outlived its helper");
        thread::yield_now();
    }
    assert!(!respond(&challenge.request_id, Some(CREDENTIAL.into())));
    assert!(fixture.session.pending.is_empty());
    assert!(recorded().is_empty());
}

#[test]
fn bytes_after_the_request_end_its_prompt() {
    let mut fixture = fixture();
    let address = fixture.address.clone();
    let request = frame(&fixture.token, "Password:");
    let (send_more, more) = mpsc::channel::<()>();
    let helper = thread::spawn(move || {
        let mut stream = channel::connect(&address)?;
        stream.write_all(&request)?;
        more.recv_timeout(DEADLINE).map_err(io::Error::other)?;
        stream.write_all(b"second request")?;
        let mut received = Vec::new();
        stream.read_to_end(&mut received)?;
        Ok(received)
    });
    let challenge = next_challenge(&mut fixture.session);
    send_more.send(()).unwrap();
    assert_no_answer(finish(&mut fixture.session, helper));
    assert!(!respond(&challenge.request_id, Some(CREDENTIAL.into())));
    assert_eq!(recorded(), [rejection("malformed")]);
}

#[test]
fn silent_helper_is_dropped_at_the_request_deadline() {
    let mut fixture = fixture();
    let helper = spawn_raw(&fixture.address, Vec::new(), false);
    let deadline = Instant::now() + DEADLINE;
    while fixture.session.pending.is_empty() {
        assert!(fixture.session.poll().unwrap().is_empty());
        assert!(Instant::now() < deadline, "the helper was never accepted");
        thread::yield_now();
    }
    // Expire the deadline instead of waiting for it.
    fixture.session.pending[0].deadline = Instant::now();
    assert!(fixture.session.poll().unwrap().is_empty());
    assert!(fixture.session.pending.is_empty());
    assert_no_answer(join(helper));
    assert_eq!(recorded(), [rejection("timeout")]);
}

#[test]
fn unanswered_prompt_is_recorded_at_its_deadline() {
    let mut fixture = fixture();
    let helper = spawn_ask(&fixture.address, &fixture.token, "Password:");
    let challenge = next_challenge(&mut fixture.session);
    // Expire the prompt timeout instead of waiting for it.
    fixture.session.pending[0].deadline = Instant::now();
    assert!(fixture.session.poll().unwrap().is_empty());
    assert!(fixture.session.pending.is_empty());
    assert_no_answer(join(helper));
    assert!(!respond(&challenge.request_id, Some(CREDENTIAL.into())));
    assert_eq!(recorded(), [failure("wait_answer", "TimedOut")]);
}

#[test]
fn read_failures_are_recorded_unless_the_helper_left() {
    for kind in [
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::NotConnected,
        io::ErrorKind::UnexpectedEof,
    ] {
        read_failed(record, "read_request", &kind.into());
    }
    assert!(recorded().is_empty());
    read_failed(
        record,
        "watch_helper",
        &io::ErrorKind::PermissionDenied.into(),
    );
    assert_eq!(recorded(), [failure("watch_helper", "PermissionDenied")]);
}

#[test]
fn dropping_the_session_fails_waiting_helpers() {
    let mut fixture = fixture();
    let helper = spawn_ask(&fixture.address, &fixture.token, "Password:");
    let challenge = next_challenge(&mut fixture.session);
    drop(fixture.session);
    assert_no_answer(join(helper));
    assert!(!respond(&challenge.request_id, Some(CREDENTIAL.into())));
}

#[cfg(unix)]
#[test]
fn unix_channel_is_private_and_removed_with_the_session() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture();
    let socket = std::path::PathBuf::from(&fixture.address);
    let directory = socket.parent().unwrap().to_path_buf();
    let mode = |path: &std::path::Path| {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode(&directory), 0o700);
    assert_eq!(mode(&socket), 0o600);
    drop(fixture.session);
    assert!(!socket.exists() && !directory.exists());
}

#[test]
fn reply_write_failure_never_logs_the_answer() {
    // A real read-only file rejects writes on every platform.
    let mut file =
        std::fs::File::open(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .unwrap();
    let frame = reply_frame(CREDENTIAL);
    let mut written = 0;
    let error = deliver(&mut file, &frame, &mut written).unwrap_err();
    response_failed(record, "write_response", &error);
    let recorded = recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "vcs.authentication.response_failed");
    let fields: serde_json::Value = serde_json::from_str(&recorded[0].1).unwrap();
    assert_eq!(fields[0], json!(["stage", "write_response"]));
    assert_eq!(
        fields[1],
        json!(["error", "Git authentication response failed"])
    );
    assert_eq!(fields[3], json!(["error_domain", "io"]));
    assert!(!recorded[0].1.contains(CREDENTIAL));
}

#[test]
fn confirmation_unregisters_when_dropped() {
    let confirmation = Confirmation::new().unwrap();
    let id = confirmation.request_id.clone();
    assert!(registry().contains_key(&id));
    drop(confirmation);
    assert!(!registry().contains_key(&id));
}

#[test]
fn prompts_for_names_and_host_keys_are_not_masked() {
    assert!(!is_secret("Username for 'https://fixture.invalid':"));
    assert!(!is_secret(
        "Are you sure you want to continue connecting (yes/no/[fingerprint])?"
    ));
    assert!(is_secret("Enter passphrase for key '/fixture/id':"));
}

#[test]
fn random_hex_and_secret_comparison() {
    let first = random_hex::<16>().unwrap();
    assert_eq!(first.len(), 32);
    assert!(first
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
    assert_ne!(first, random_hex::<16>().unwrap());
    assert!(same_secret(b"abcd", b"abcd"));
    assert!(!same_secret(b"abcd", b"abce"));
    assert!(!same_secret(b"abcd", b"abc"));
    assert!(!same_secret(b"", b"a"));
}
