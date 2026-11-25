//! Tests of the session cache, with a synthetic agent whose session files are
//! JSON lines. The format belongs to no real agent: Core knows none.

mod discover;
mod pricing;
mod stats;
mod store;
mod text;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lithe_protocol::{CoreError, ErrorCode};
use serde_json::{json, Value};

use crate::model::{
    ByteRange, CallRecord, IndexSpan, ScanChunk, ScanOutput, SessionMeta, SessionParser, Tokens,
};

/// A directory removed when the test ends.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(prefix: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lithe-agent-sessions {prefix} {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // A crashed earlier run with the same process ID may have left it.
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove a stale temporary directory");
        }
        fs::create_dir_all(&path).expect("create a temporary directory");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("could not remove {}: {error}", self.0.display());
        }
    }
}

/// Parses the synthetic format: one JSON object per line, with a `type` of
/// `session`, `use`, `call` or `text`. Calls and texts belong to the last
/// `session` or `use` line, which the parser carries across chunks in its
/// state.
#[derive(Default)]
pub(crate) struct LinesParser {
    /// Fails any chunk that contains this text.
    pub(crate) fail_on: Option<&'static str>,
    /// Chunks scanned.
    pub(crate) chunks: u64,
}

impl SessionParser for LinesParser {
    fn scan(&mut self, chunk: ScanChunk<'_>) -> Result<ScanOutput, CoreError> {
        self.chunks += 1;
        let mut session = chunk
            .state
            .and_then(|state| state.get("session"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut output = ScanOutput::default();
        let mut position = chunk.offset;
        for line in chunk.bytes.split_inclusive(|byte| *byte == b'\n') {
            assert!(line.ends_with(b"\n"), "a chunk holds whole lines");
            let range = ByteRange {
                start: position,
                end: position + line.len() as u64,
            };
            position = range.end;
            if let Some(needle) = self.fail_on {
                if String::from_utf8_lossy(line).contains(needle) {
                    return Err(CoreError::new(
                        ErrorCode::ParseFailed,
                        "test parser failure",
                    ));
                }
            }
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let text = |field: &str| value.get(field).and_then(Value::as_str).map(str::to_string);
            let number = |field: &str| value.get(field).and_then(Value::as_u64);
            match value.get("type").and_then(Value::as_str) {
                Some("session") => {
                    let id = text("id").expect("session id");
                    session = Some(id.clone());
                    output.sessions.push(SessionMeta {
                        session_id: id,
                        parent_session_id: text("parent"),
                        cwd: text("cwd"),
                        started_at_ms: value.get("at").and_then(Value::as_i64),
                        updated_at_ms: value.get("at").and_then(Value::as_i64),
                        agent_version: text("version"),
                        title_range: value.get("title").map(|_| range),
                    });
                }
                // Continues a session without describing it, as a file that
                // carries on another file's session does.
                Some("use") => session = Some(text("id").expect("session id")),
                Some("call") => output.calls.push(CallRecord {
                    key: text("key").expect("call key"),
                    session_id: session.clone().expect("a session line first"),
                    thread_id: None,
                    started_at_ms: value.get("at").and_then(Value::as_i64).expect("call time"),
                    requested_model: text("requested"),
                    model: text("model"),
                    context_window: None,
                    effort: None,
                    provider: text("provider"),
                    tokens: Tokens {
                        input: number("input"),
                        output: number("output"),
                        cache_read: number("cacheRead"),
                        cache_write_5m: number("cacheWrite5m"),
                        cache_write_1h: number("cacheWrite1h"),
                        reasoning: number("reasoning"),
                    },
                    first_token_ms: number("firstTokenMs"),
                    duration_ms: number("durationMs"),
                    compaction: false,
                    request_id: None,
                    error_category: text("error"),
                    range: match value.get("badRange") {
                        Some(_) => ByteRange {
                            start: range.end + 10,
                            end: range.end + 20,
                        },
                        None => range,
                    },
                }),
                Some("text") => output.spans.push(IndexSpan {
                    session_id: session.clone().expect("a session line first"),
                    at_ms: value.get("at").and_then(Value::as_i64),
                    range,
                    text: text("text").expect("span text"),
                }),
                _ => {}
            }
        }
        output.state = session.map(|session| json!({ "session": session }));
        Ok(output)
    }
}

pub(crate) fn session_line(id: &str, cwd: &str, at: i64) -> String {
    format!(
        "{}\n",
        json!({ "type": "session", "id": id, "cwd": cwd, "at": at, "title": true })
    )
}

pub(crate) fn call_line(key: &str, at: i64, model: &str, input: u64, output: u64) -> String {
    format!(
        "{}\n",
        json!({ "type": "call", "key": key, "at": at, "model": model, "input": input, "output": output })
    )
}

pub(crate) fn text_line(at: i64, text: &str) -> String {
    format!("{}\n", json!({ "type": "text", "at": at, "text": text }))
}

pub(crate) fn append(path: &Path, text: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open a session file");
    file.write_all(text.as_bytes())
        .expect("append to a session file");
}
