//! Bounded one-shot commands for tests that run a transport chain end to end.
//! Those tests drive real shells, so the module exists on Unix hosts only.

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Output bytes kept per stream. Test commands print a few lines; anything
/// larger is a broken command and only its beginning is useful.
const CAPTURE_LIMIT_BYTES: usize = 1024 * 1024;

/// Interval between exit checks. The tested commands run local shells, so
/// this adds no noticeable latency.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long output pipes may stay open after the command exited. A
/// descendant that inherited them can keep them open indefinitely.
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);

/// Result of a finished one-shot command.
#[derive(Debug)]
pub struct CommandOutput {
    /// Exit code; `None` when the command was terminated by a signal.
    pub status: Option<i32>,
    /// Standard output, at most the first 1 MiB.
    pub stdout: Vec<u8>,
    /// Standard error, at most the first 1 MiB.
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    /// Standard error for assertion messages.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

/// Runs `argv` locally with no input and kills it after `timeout`.
pub fn run(argv: &[String], timeout: Duration) -> io::Result<CommandOutput> {
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = capture(child.stdout.take());
    let stderr = capture(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            crate::failure_log::kill(
                "remote.client.operation_failed",
                "process_kill",
                child.kill(),
            );
            crate::failure_log::io(
                "remote.client.operation_failed",
                "process_wait",
                child.wait(),
            );
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{program} did not finish within {} seconds",
                    timeout.as_secs()
                ),
            ));
        }
        // test-stability: allow(rust-real-sleep) reason: a child process has no waitable handle with a timeout in std; the loop has a monotonic deadline.
        thread::sleep(EXIT_POLL_INTERVAL);
    };
    let drain_deadline = Instant::now() + OUTPUT_DRAIN_GRACE;
    Ok(CommandOutput {
        status: status.code(),
        stdout: stdout.collect(drain_deadline),
        stderr: stderr.collect(drain_deadline),
    })
}

struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
    finished: Receiver<()>,
}

impl Capture {
    /// Returns what was read once the pipe closed, or at `deadline` if a
    /// descendant still holds it open.
    fn collect(self, deadline: Instant) -> Vec<u8> {
        crate::failure_log::outcome(
            "remote.client.operation_failed",
            "capture_drain",
            self.finished
                .recv_timeout(deadline.saturating_duration_since(Instant::now())),
            "completion_failed",
            "task",
        );
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        bytes.clone()
    }
}

fn capture(pipe: Option<impl Read + Send + 'static>) -> Capture {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (finished_sender, finished) = mpsc::channel();
    let shared = Arc::clone(&bytes);
    thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut buffer = [0_u8; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let mut captured = shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let room = CAPTURE_LIMIT_BYTES.saturating_sub(captured.len());
                        captured.extend_from_slice(&buffer[..read.min(room)]);
                    }
                }
            }
        }
        // log-exempt: User cancellation: the command caller already stopped collecting output and no longer waits for the capture completion notification.
        let _ = finished_sender.send(());
    });
    Capture { bytes, finished }
}
