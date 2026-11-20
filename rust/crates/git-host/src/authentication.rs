//! AskPass transport between Git or SSH and the host that shows prompts.
//!
//! Git and SSH run this executable in helper mode ([`helper_main`]) when they
//! need a credential. The helper sends the prompt to the [`Session`] that
//! started the command over a channel that only the same account can open: a
//! socket in an owner-only directory on Unix, or a named pipe whose security
//! descriptor admits only the current user on Windows. Each request also
//! carries the session token from the child's environment, so another process
//! of the same account that finds the channel still cannot inject prompts.
//!
//! The token, request frames, and answers live in buffers that are sized up
//! front, so they never reallocate, and are zeroed when dropped. Copies made
//! outside this module are not covered: the child's environment, the host
//! caller's JSON values, and operating system buffers.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as channel;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as channel;
#[cfg(test)]
mod tests;

use crate::failure_log::{self, FailureSink};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{mpsc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Largest request frame, newline excluded.
const MAX_FRAME: usize = 16 * 1024;
/// Longest prompt the session forwards to the host.
const MAX_PROMPT: usize = 8192;
/// Longest answer the host may send back.
const MAX_ANSWER: usize = 8192;
/// How long a prompt waits for the user to answer.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a connected helper has to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long delivering an answer, and then seeing the helper close, may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// How often [`Confirmation::wait`] checks for cancellation.
const CANCEL_CHECK: Duration = Duration::from_millis(10);
/// Connections the session serves at once; later ones wait to be accepted.
const MAX_PENDING: usize = 4;
/// Connections one [`Session::poll`] accepts, so a flood of rejected
/// connections cannot hold the caller's loop.
const MAX_ACCEPTS: usize = 16;
/// Distinct prompts one session forwards.
const MAX_PROMPTS: usize = 16;
/// Times one session forwards the same prompt.
const MAX_ATTEMPTS: usize = 3;
/// The channel address the helper connects to.
const ADDRESS_VARIABLE: &str = "LITHE_GIT_ASKPASS_ADDRESS";
/// The session token the helper must present.
const TOKEN_VARIABLE: &str = "LITHE_GIT_ASKPASS_TOKEN";

/// The host's answer; `None` cancels the prompt.
type Answer = Option<Zeroizing<String>>;
type Registry = HashMap<String, mpsc::SyncSender<Answer>>;
static ANSWERS: OnceLock<Mutex<Registry>> = OnceLock::new();

/// Prompts and decisions that wait for [`respond`]. The map has no invariant
/// that a panicking holder could break, so a poisoned lock is used as is.
fn registry() -> MutexGuard<'static, Registry> {
    ANSWERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Lowercase hex of `N` random bytes from the operating system.
fn random_hex<const N: usize>() -> io::Result<String> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = Zeroizing::new([0u8; N]);
    rand::rngs::OsRng
        .try_fill_bytes(bytes.as_mut())
        .map_err(io::Error::other)?;
    let mut text = String::with_capacity(N * 2);
    for byte in bytes.iter() {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Ok(text)
}

/// What one nonblocking accept on the channel produced.
enum Accept<S> {
    /// A connection from the session's own account.
    Peer(S),
    /// A connection that was closed, and why.
    // The Windows pipe's descriptor keeps other accounts out before a
    // connection exists, so only Unix rejects at accept time.
    #[cfg_attr(windows, allow(dead_code))]
    Rejected(&'static str),
    /// Nothing is waiting.
    Empty,
}

/// A credential prompt from Git or SSH that the host must show to the user.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    /// Identifies the prompt when the host replies through [`respond`].
    pub request_id: String,
    /// The prompt text exactly as Git or SSH asked it.
    pub prompt: String,
    /// Whether the answer is a secret and the input must be masked.
    pub secret: bool,
    /// How many times this prompt has been asked in the session, starting at 1.
    pub attempt: usize,
}

/// One request frame. Fields borrow from the zeroed input buffer when they
/// contain no escapes, which a hex token never does.
#[derive(Deserialize, Serialize)]
struct WireRequest<'a> {
    #[serde(borrow)]
    token: Cow<'a, str>,
    #[serde(borrow)]
    prompt: Cow<'a, str>,
}

/// One invocation owns its channel, outstanding prompts, and reply
/// registrations. Dropping it closes the channel and fails every prompt that
/// has not been answered yet.
pub struct Session {
    listener: channel::Listener,
    token: Zeroizing<String>,
    pending: Vec<Pending>,
    attempts: HashMap<String, usize>,
    helper: PathBuf,
    sink: FailureSink,
}

impl Session {
    /// Opens a private channel with a fresh token for one Git invocation.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            listener: channel::Listener::bind()?,
            token: Zeroizing::new(random_hex::<32>()?),
            pending: Vec::new(),
            attempts: HashMap::new(),
            helper: std::env::current_exe()?,
            sink: failure_log::emit,
        })
    }

    /// Points the command's Git and SSH askpass at this executable and session.
    pub fn configure(&self, command: &mut Command) -> io::Result<()> {
        // AskPass is an executable path, including spaces. The child
        // environment selects the early helper mode without a shell.
        command
            .env("GIT_ASKPASS", &self.helper)
            .env("SSH_ASKPASS", &self.helper)
            .env("LITHE_GIT_ASKPASS_MODE", "1")
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "lithe-askpass")
            .env(ADDRESS_VARIABLE, self.listener.address())
            .env(TOKEN_VARIABLE, self.token.as_str());
        Ok(())
    }

    /// Accepts and advances helper connections without blocking, and returns
    /// the prompts that became ready. The caller's process loop drives it, so
    /// no detached worker outlives the command.
    pub fn poll(&mut self) -> io::Result<Vec<Challenge>> {
        let now = Instant::now();
        for _ in 0..MAX_ACCEPTS {
            if self.pending.len() >= MAX_PENDING {
                break;
            }
            match self.listener.accept() {
                Ok(Accept::Peer(stream)) => self.pending.push(Pending::new(stream, now)),
                Ok(Accept::Rejected(reason)) => rejected(self.sink, reason),
                Ok(Accept::Empty) => break,
                Err(error) if is_retry(&error) => break,
                // log-exempt: Expected absence: the client left before it was accepted, so nothing is left to serve.
                Err(error) if is_gone(&error) => {}
                Err(error) => return Err(error),
            }
        }
        let mut context = Context {
            token: self.token.as_bytes(),
            attempts: &mut self.attempts,
            sink: self.sink,
            challenges: Vec::new(),
        };
        self.pending
            .retain_mut(|pending| pending.advance(&mut context, now));
        Ok(context.challenges)
    }
}

/// Session state that every connection consults.
struct Context<'a> {
    token: &'a [u8],
    attempts: &'a mut HashMap<String, usize>,
    sink: FailureSink,
    challenges: Vec<Challenge>,
}

impl Context<'_> {
    /// Applies the token and the session's limits. Returns the prompt and its
    /// attempt number, or the rejection reason.
    fn admit(&mut self, request: WireRequest<'_>) -> Result<(String, usize), &'static str> {
        if !same_secret(request.token.as_bytes(), self.token) {
            return Err("token");
        }
        if request.prompt.len() > MAX_PROMPT {
            return Err("oversized");
        }
        let prompt = request.prompt.into_owned();
        if !self.attempts.contains_key(&prompt) && self.attempts.len() >= MAX_PROMPTS {
            return Err("prompt-limit");
        }
        let count = self.attempts.entry(prompt.clone()).or_insert(0);
        if *count >= MAX_ATTEMPTS {
            return Err("attempt-limit");
        }
        *count += 1;
        Ok((prompt, *count))
    }
}

/// One accepted helper connection.
struct Pending {
    stream: channel::Stream,
    /// The request frame; released as soon as it is parsed.
    input: Zeroizing<Vec<u8>>,
    filled: usize,
    stage: Stage,
    deadline: Instant,
}

enum Stage {
    /// Reading the helper's request frame.
    Request,
    /// Waiting for the host to answer through [`respond`].
    Answer {
        _registration: Registration,
        receiver: mpsc::Receiver<Answer>,
    },
    /// Writing the answer back to the helper.
    Reply {
        frame: Zeroizing<Vec<u8>>,
        written: usize,
    },
    /// The answer is written; waiting for the helper to close its end, so that
    /// closing ours can never discard bytes it has not read yet.
    Drain,
}

impl Pending {
    fn new(stream: channel::Stream, now: Instant) -> Self {
        Self {
            stream,
            input: Zeroizing::new(vec![0; MAX_FRAME + 1]),
            filled: 0,
            stage: Stage::Request,
            deadline: now + REQUEST_TIMEOUT,
        }
    }

    /// Moves the connection forward without blocking. Returns whether it
    /// stays open.
    fn advance(&mut self, context: &mut Context<'_>, now: Instant) -> bool {
        if now >= self.deadline {
            let stage = match self.stage {
                Stage::Request => {
                    rejected(context.sink, "timeout");
                    return false;
                }
                // The user did not answer before the prompt timeout.
                Stage::Answer { .. } => "wait_answer",
                Stage::Reply { .. } => "write_response",
                // The helper stayed connected after reading its answer.
                Stage::Drain => "drain_helper",
            };
            response_failed(context.sink, stage, &io::ErrorKind::TimedOut.into());
            return false;
        }
        match &mut self.stage {
            Stage::Request => self.read_request(context, now),
            Stage::Answer { receiver, .. } => match receiver.try_recv() {
                Ok(Some(answer)) => {
                    self.stage = Stage::Reply {
                        frame: reply_frame(&answer),
                        written: 0,
                    };
                    self.deadline = now + WRITE_TIMEOUT;
                    self.advance(context, now)
                }
                // log-exempt: User cancellation: the host cancelled the prompt; the helper sees the channel close and fails.
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => false,
                Err(mpsc::TryRecvError::Empty) => self.helper_waiting(context.sink),
            },
            Stage::Reply { frame, written } => match deliver(&mut self.stream, frame, written) {
                Ok(false) => true,
                Ok(true) => {
                    self.stage = Stage::Drain;
                    self.deadline = now + WRITE_TIMEOUT;
                    true
                }
                Err(error) => {
                    response_failed(context.sink, "write_response", &error);
                    false
                }
            },
            Stage::Drain => self.drain(context.sink),
        }
    }

    /// Reads what has arrived of the request frame, and registers the prompt
    /// once the whole frame is there.
    fn read_request(&mut self, context: &mut Context<'_>, now: Instant) -> bool {
        let start = self.filled;
        let count = match self.stream.read(&mut self.input[start..]) {
            Ok(0) => {
                if start > 0 {
                    rejected(context.sink, "malformed");
                }
                return false;
            }
            Ok(count) => count,
            Err(error) if is_retry(&error) => return true,
            Err(error) => {
                read_failed(context.sink, "read_request", &error);
                return false;
            }
        };
        self.filled += count;
        let newline = self.input[start..self.filled]
            .iter()
            .position(|&byte| byte == b'\n');
        let Some(end) = newline.map(|offset| start + offset) else {
            if self.filled == self.input.len() {
                rejected(context.sink, "oversized");
                return false;
            }
            return true;
        };
        // One request per connection: nothing may follow the frame.
        let admitted = if end + 1 == self.filled {
            serde_json::from_slice::<WireRequest<'_>>(&self.input[..end])
                .map_err(|_| "malformed")
                .and_then(|request| context.admit(request))
        } else {
            Err("malformed")
        };
        // The frame held the token; zero and release it before going on.
        self.input = Zeroizing::new(Vec::new());
        self.filled = 0;
        let (prompt, attempt) = match admitted {
            Ok(admitted) => admitted,
            Err(reason) => {
                rejected(context.sink, reason);
                return false;
            }
        };
        let request_id = match random_hex::<16>() {
            Ok(id) => id,
            Err(error) => {
                response_failed(context.sink, "register_request", &error);
                return false;
            }
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        registry().insert(request_id.clone(), sender);
        self.stage = Stage::Answer {
            _registration: Registration(request_id.clone()),
            receiver,
        };
        self.deadline = now + PROMPT_TIMEOUT;
        context.challenges.push(Challenge {
            request_id,
            secret: is_secret(&prompt),
            prompt,
            attempt,
        });
        true
    }

    /// Checks on the helper while its prompt waits for the user, so a helper
    /// that already left does not keep a prompt the host can still answer.
    fn helper_waiting(&mut self, sink: FailureSink) -> bool {
        match self.stream.read(&mut [0u8; 1]) {
            Err(error) if is_retry(&error) => true,
            Ok(0) => false,
            Ok(_) => {
                // One request per connection: nothing may follow the frame.
                rejected(sink, "malformed");
                false
            }
            Err(error) => {
                read_failed(sink, "watch_helper", &error);
                false
            }
        }
    }

    /// Discards anything the helper still sends until it closes.
    fn drain(&mut self, sink: FailureSink) -> bool {
        match self.stream.read(&mut [0u8; 64]) {
            Ok(0) => false,
            Ok(_) => true,
            Err(error) if is_retry(&error) => true,
            Err(error) => {
                read_failed(sink, "drain_helper", &error);
                false
            }
        }
    }
}

/// An entry in the answer registry, removed when dropped so that answers to
/// prompts that already ended fail instead of reaching a later prompt.
struct Registration(String);

impl Drop for Registration {
    fn drop(&mut self) {
        registry().remove(&self.0);
    }
}

fn is_retry(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

/// The helper closed or reset its end of the channel.
fn is_gone(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
}

/// Ends a connection whose read failed. A helper that left is the normal end
/// of its prompt; any other failure is recorded under `stage`.
fn read_failed(sink: FailureSink, stage: &'static str, error: &io::Error) {
    // log-exempt: Expected absence: the helper already closed or reset its end, so nobody is left to serve.
    if !is_gone(error) {
        response_failed(sink, stage, error);
    }
}

/// Usernames and host-key confirmations are shown; everything else is masked.
fn is_secret(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    !lower.contains("username") && !lower.contains("yes/no") && !lower.contains("(yes/")
}

/// Compares without stopping at the first difference, so timing does not
/// reveal how much of a guessed token matched.
fn same_secret(left: &[u8], right: &[u8]) -> bool {
    let difference = left
        .iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right));
    left.len() == right.len() && std::hint::black_box(difference) == 0
}

/// The answer followed by a newline, in a buffer that holds it exactly.
fn reply_frame(answer: &str) -> Zeroizing<Vec<u8>> {
    let mut frame = Zeroizing::new(Vec::with_capacity(answer.len() + 1));
    frame.extend_from_slice(answer.as_bytes());
    frame.push(b'\n');
    frame
}

/// Writes as much of the frame as the channel takes now. Returns whether the
/// whole frame has been written.
fn deliver(writer: &mut impl Write, frame: &[u8], written: &mut usize) -> io::Result<bool> {
    while *written < frame.len() {
        match writer.write(&frame[*written..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => *written += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

fn rejected(sink: FailureSink, reason: &'static str) {
    sink(
        "vcs.authentication.request_rejected",
        &[("reason", json!(reason))],
    );
}

/// Records only the stage and the stable I/O category, never the answer or
/// the error's display text.
fn response_failed(sink: FailureSink, stage: &'static str, error: &io::Error) {
    sink(
        "vcs.authentication.response_failed",
        &[
            ("stage", json!(stage)),
            ("error", json!("Git authentication response failed")),
            ("error_code", json!(format!("{:?}", error.kind()))),
            ("error_domain", json!("io")),
        ],
    );
}

/// Replies once to a currently owned prompt. Null means cancel; stale replies
/// and answers longer than 8192 bytes or containing NUL, CR, or LF fail.
pub fn respond(request_id: &str, answer: Option<String>) -> bool {
    let answer = answer.map(Zeroizing::new);
    if answer
        .as_ref()
        .is_some_and(|value| value.len() > MAX_ANSWER || value.contains(['\0', '\r', '\n']))
    {
        return false;
    }
    let sender = registry().remove(request_id);
    sender.is_some_and(|sender| sender.send(answer).is_ok())
}

/// Early application mode invoked by Git, before GUI/runtime initialization.
/// Prints the answer and returns 0, or returns 1 when there is none.
pub fn helper_main(prompt: &str) -> i32 {
    // The helper runs before any log writer exists; the session that owns the
    // channel records every rejection it sees.
    if helper(prompt).is_ok() {
        0
    } else {
        1
    }
}

fn helper(prompt: &str) -> io::Result<()> {
    let address = std::env::var_os(ADDRESS_VARIABLE).ok_or(io::ErrorKind::NotFound)?;
    let token = Zeroizing::new(std::env::var(TOKEN_VARIABLE).map_err(io::Error::other)?);
    ask(&address, &token, prompt, &mut io::stdout().lock())
}

/// Sends one prompt to the session at `address` and copies its answer line to
/// `out`. Fails when the session cancels, closes, or answers out of protocol.
fn ask(address: &OsStr, token: &str, prompt: &str, out: &mut impl Write) -> io::Result<()> {
    let mut request = Zeroizing::new(Vec::with_capacity(MAX_FRAME + 1));
    serde_json::to_writer(
        Bounded(&mut request),
        &WireRequest {
            token: Cow::Borrowed(token),
            prompt: Cow::Borrowed(prompt),
        },
    )?;
    request.push(b'\n');
    let mut stream = channel::connect(address)?;
    stream.write_all(&request)?;
    drop(request);
    let mut answer = Zeroizing::new(vec![0u8; MAX_ANSWER + 1]);
    let mut filled = 0;
    loop {
        if filled == answer.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let count = match stream.read(&mut answer[filled..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let start = filled;
        filled += count;
        if let Some(offset) = answer[start..filled].iter().position(|&byte| byte == b'\n') {
            // The session sends one line and nothing after it.
            if start + offset + 1 != filled {
                return Err(io::ErrorKind::InvalidData.into());
            }
            break;
        }
    }
    out.write_all(&answer[..filled])?;
    out.flush()
}

/// Appends to a buffer whose capacity already holds a whole frame, and fails
/// instead of growing it, so serialization never leaves a reallocated copy
/// of the token behind.
struct Bounded<'a>(&'a mut Vec<u8>);

impl Write for Bounded<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len() + bytes.len() > MAX_FRAME {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Owns an explicit retry decision after a failed process has already been reaped.
pub struct Confirmation {
    /// Identifies the decision when the host replies through [`respond`].
    pub request_id: String,
    receiver: mpsc::Receiver<Answer>,
    _registration: Registration,
}

impl Confirmation {
    /// Registers a new decision that the host answers through [`respond`].
    pub fn new() -> io::Result<Self> {
        let request_id = random_hex::<16>()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        registry().insert(request_id.clone(), sender);
        Ok(Self {
            _registration: Registration(request_id.clone()),
            request_id,
            receiver,
        })
    }

    /// Waits on a worker thread until the host decides, the owning request is
    /// cancelled, or the prompt deadline passes. Only the answer `retry`
    /// means yes.
    pub fn wait(&self, mut cancelled: impl FnMut() -> bool) -> io::Result<bool> {
        let deadline = Instant::now() + PROMPT_TIMEOUT;
        loop {
            if cancelled() {
                return Ok(false);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            match self.receiver.recv_timeout(remaining.min(CANCEL_CHECK)) {
                Ok(answer) => return Ok(answer.is_some_and(|value| value.as_str() == "retry")),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(false),
            }
        }
    }
}
