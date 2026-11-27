//! The records a session parser derives from an agent's session file, and the
//! parser interface Core calls with whole lines of that file.
//!
//! The types decode from the camelCase JSON a plugin backend returns, so the
//! adapter that calls the backend needs no types of its own. Absent fields stay
//! absent: a parser that cannot see a value leaves it out instead of guessing,
//! and nothing downstream fills it in.
//!
//! None of these records carries conversation text except [`IndexSpan::text`],
//! which the store splits into index terms and drops.

use std::path::Path;

use lithe_protocol::CoreError;
use serde::{Deserialize, Serialize};

/// A half-open byte range `[start, end)` of a session file. Records point at
/// the bytes they came from, so a viewer reads the original lines again
/// instead of the cache keeping a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ByteRange {
    /// Offset of the first byte, from the start of the file.
    pub start: u64,
    /// Offset just past the last byte; greater than `start`.
    pub end: u64,
}

/// Token counts of one model call. A count the agent does not record is
/// absent, which differs from zero: statistics leave absent counts out
/// instead of treating them as nothing used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tokens {
    /// Input tokens read without the prompt cache: neither cache reads nor
    /// cache writes. A parser for an agent whose count includes cached tokens
    /// subtracts them, so the cache hit rate has one definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    /// Output tokens, reasoning tokens included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    /// Input tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    /// Input tokens written to a prompt cache entry that lives five minutes.
    /// A provider with a single cache lifetime reports its writes here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_5m: Option<u64>,
    /// Input tokens written to a prompt cache entry that lives one hour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
    /// The part of `output` spent on reasoning, when the agent records it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
}

impl Tokens {
    /// Whether the call reports any prompt cache count. Calls that report none
    /// come from providers without cache data and stay out of the hit rate.
    pub fn reports_cache(&self) -> bool {
        self.cache_read.is_some() || self.cache_write_5m.is_some() || self.cache_write_1h.is_some()
    }
}

/// One model request an agent made, as its session file records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallRecord {
    /// Identifies the request within its agent, for example the response ID
    /// the provider returned. The same request recorded twice, in one file or
    /// in a session file copied into another, has the same key and is counted
    /// once.
    pub key: String,
    /// The session the request belongs to, in the agent's own ID format.
    #[serde(rename = "sessionID")]
    pub session_id: String,
    /// The thread or sub-agent within the session, when the agent has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "threadID")]
    pub thread_id: Option<String>,
    /// When the request started, in milliseconds since the Unix epoch (UTC).
    pub started_at_ms: i64,
    /// The model the agent asked for, which may be an alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    /// The model that answered, as the provider reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The context window the request selected. A parser for an agent that
    /// writes it into the model name splits it off, so `model` names the
    /// model alone and prices can be looked up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<String>,
    /// The reasoning effort the request selected, as the agent names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The provider that served the request, as a models.dev provider ID.
    /// Only set when the session file identifies it; never inferred from the
    /// model name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Token counts of the request.
    #[serde(default)]
    pub tokens: Tokens,
    /// Milliseconds from the start of the request to its first output token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    /// Milliseconds from the start of the request to its last output token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Whether the agent made the request to compact its context rather than
    /// to answer the user.
    #[serde(default)]
    pub compaction: bool,
    /// The provider's request ID, kept for matching with provider dashboards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "requestID")]
    pub request_id: Option<String>,
    /// A short category of the failure when the request failed, as the agent
    /// names it (for example `rateLimited`). Never the error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    /// The bytes of the session file that record the request.
    pub range: ByteRange,
}

/// What a session file says about a session. A parser reports it whenever
/// it learns more; the store keeps the latest present value of each field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMeta {
    /// The session, in the agent's own ID format.
    #[serde(rename = "sessionID")]
    pub session_id: String,
    /// The session this one was resumed or forked from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentSessionID")]
    pub parent_session_id: Option<String>,
    /// The working directory the agent ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// When the session started, in milliseconds since the Unix epoch (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    /// When the session last changed, in milliseconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
    /// The version of the agent CLI that wrote the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    /// The bytes that hold the session's title. A title is conversation text,
    /// so the cache keeps where it is, and a viewer reads it like a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_range: Option<ByteRange>,
}

/// Text of a session file to make searchable, with the bytes it came from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexSpan {
    /// The session the text belongs to.
    #[serde(rename = "sessionID")]
    pub session_id: String,
    /// When the text was written, in milliseconds since the Unix epoch.
    /// Search lists newer text first.
    #[serde(default)]
    pub at_ms: Option<i64>,
    /// The bytes a search hit points at.
    pub range: ByteRange,
    /// The text to index. The store splits it into terms and keeps neither
    /// the text nor the order of its terms; only the first
    /// [`MAX_INDEXED_SPAN_BYTES`](crate::MAX_INDEXED_SPAN_BYTES) are indexed.
    pub text: String,
}

/// Whole lines of a session file, handed to a parser.
#[derive(Debug, Clone, Copy)]
pub struct ScanChunk<'a> {
    /// The session file.
    pub path: &'a Path,
    /// The file offset of `bytes[0]`. Ranges the parser returns are file
    /// offsets, so they add this to positions within `bytes`.
    pub offset: u64,
    /// One or more complete lines, each ending with `\n`. Lines longer than
    /// [`MAX_LINE_BYTES`](crate::MAX_LINE_BYTES) are never handed over, and a
    /// line still being written waits for its newline.
    pub bytes: &'a [u8],
    /// What the parser returned as [`ScanOutput::state`] after the previous
    /// chunk of this file; absent at the start of the file.
    pub state: Option<&'a serde_json::Value>,
}

/// What a parser derived from one chunk.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanOutput {
    /// Sessions the chunk describes.
    #[serde(default)]
    pub sessions: Vec<SessionMeta>,
    /// Model requests the chunk records.
    #[serde(default)]
    pub calls: Vec<CallRecord>,
    /// Text of the chunk to make searchable.
    #[serde(default)]
    pub spans: Vec<IndexSpan>,
    /// What the parser needs to read the next chunk of the file, at most
    /// [`MAX_STATE_BYTES`](crate::MAX_STATE_BYTES) as JSON, for example the
    /// current session ID or a request whose usage arrives on a later line.
    /// It is stored with the file's cursor, so it must hold no conversation
    /// text.
    #[serde(default)]
    pub state: Option<serde_json::Value>,
}

/// Turns whole lines of an agent's session file into records. Core implements
/// it with a call to the agent plugin's backend; tests implement it directly.
///
/// A parser must give the same records for a file however its lines are split
/// into chunks: an incremental read and a rebuild from scratch then agree.
pub trait SessionParser {
    /// Derives the records of one chunk. An error stops reading the file
    /// without moving its cursor, so the same chunk is offered again by the
    /// next sync.
    fn scan(&mut self, chunk: ScanChunk<'_>) -> Result<ScanOutput, CoreError>;
}
