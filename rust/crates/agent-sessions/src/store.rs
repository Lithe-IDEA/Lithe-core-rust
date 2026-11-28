//! The SQLite cache of session records.
//!
//! Rows are kept per source file: a file's calls, sessions and indexed spans
//! are dropped together when the file is rewritten, deleted or no longer
//! listed, and read again from it. Queries remove the copies a session file
//! copied from another one (an agent that resumes or forks a session can copy
//! its history), so dropping one file never loses a call another file also
//! records, and a cache rebuilt from scratch answers the same.
//!
//! The database holds no conversation text. Parsers return text only in
//! [`IndexSpan::text`], which becomes index terms here; the full-text table
//! is contentless and stores no positions. Identifiers and labels must be
//! ASCII without spaces, so a sentence cannot be stored in them.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lithe_protocol::{cancellation, CoreError, ErrorCode};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::Serialize;

use crate::ingest::{self, ChunkLimits, Cursor, FileStamp, Plan};
use crate::model::{
    ByteRange, CallRecord, IndexSpan, ScanChunk, ScanOutput, SessionParser, Tokens,
};
use crate::text;

/// Version of the database layout. A cache of another version is deleted and
/// rebuilt from the session files, so it is never migrated.
pub const SCHEMA_VERSION: u32 = 1;

/// Longest identifier, in bytes: agent, session, call, thread and request IDs.
pub const MAX_ID_BYTES: usize = 256;

/// Longest label, in bytes: model, provider, effort, context window, error
/// category and agent version.
pub const MAX_LABEL_BYTES: usize = 256;

/// Longest working directory, in bytes.
const MAX_CWD_BYTES: usize = 4096;

/// Largest parser state, as JSON, kept with a file's cursor.
pub const MAX_STATE_BYTES: usize = 64 * 1024;

/// Bytes of an indexed span's text that are split into terms; the rest is not
/// searchable.
pub const MAX_INDEXED_SPAN_BYTES: usize = 256 * 1024;

/// Most sessions, calls and spans together that one chunk may produce.
pub const MAX_RECORDS_PER_CHUNK: usize = 100_000;

/// Most hits one search returns.
pub const MAX_SEARCH_HITS: usize = 200;

/// Most sessions one page of [`SessionStore::sessions`] lists.
pub const MAX_SESSIONS_PAGE: usize = 500;

/// Most directories one [`SessionFilter::cwd_under`] names.
pub const MAX_FILTER_DIRECTORIES: usize = 64;

/// Most working directories [`SessionStore::session_counts`] counts apart.
pub const MAX_SESSION_GROUPS: usize = 500;

/// Most bytes [`SessionStore::read_range`] returns at once.
pub const MAX_READ_BYTES: u64 = 16 * 1024 * 1024;

/// Most failed files a [`SyncReport`] names; it counts the rest.
const MAX_REPORTED_FAILURES: usize = 32;

/// How long a write waits for another connection, for example a second Lithe
/// window syncing the same cache.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const DATABASE_PREFIX: &str = "agent-sessions-v";

const READ_BUFFER_BYTES: usize = 64 * 1024;

/// File name of the database in the cache directory. It carries
/// [`SCHEMA_VERSION`], so a Lithe of another version uses its own file.
pub fn database_file_name() -> String {
    format!("{DATABASE_PREFIX}{SCHEMA_VERSION}.sqlite3")
}

const SCHEMA: &str = "
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    agent TEXT NOT NULL,
    path TEXT NOT NULL,
    cursor_offset INTEGER NOT NULL,
    stamp_size INTEGER,
    stamp_modified_ms INTEGER,
    fingerprint BLOB NOT NULL,
    state TEXT,
    UNIQUE (agent, path)
) STRICT;

CREATE TABLE sessions (
    file_id INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    described INTEGER NOT NULL,
    parent_session_id TEXT,
    cwd TEXT,
    started_at_ms INTEGER,
    updated_at_ms INTEGER,
    agent_version TEXT,
    title_start INTEGER,
    title_end INTEGER,
    PRIMARY KEY (file_id, session_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX sessions_by_id ON sessions (session_id);

CREATE TABLE calls (
    file_id INTEGER NOT NULL,
    call_key TEXT NOT NULL,
    session_id TEXT NOT NULL,
    thread_id TEXT,
    started_at_ms INTEGER NOT NULL,
    requested_model TEXT,
    model TEXT,
    context_window TEXT,
    effort TEXT,
    provider TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_5m_tokens INTEGER,
    cache_write_1h_tokens INTEGER,
    reasoning_tokens INTEGER,
    first_token_ms INTEGER,
    duration_ms INTEGER,
    compaction INTEGER NOT NULL,
    request_id TEXT,
    error_category TEXT,
    range_start INTEGER NOT NULL,
    range_end INTEGER NOT NULL,
    PRIMARY KEY (file_id, call_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX calls_by_key ON calls (call_key);

CREATE TABLE spans (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    at_ms INTEGER,
    range_start INTEGER NOT NULL,
    range_end INTEGER NOT NULL
) STRICT;
CREATE INDEX spans_by_file ON spans (file_id);

-- Terms only: no content, no positions. The rowid is the span's id.
CREATE VIRTUAL TABLE span_terms USING fts5 (
    terms,
    tokenize = 'ascii',
    content = '',
    contentless_delete = 1,
    detail = 'none'
);
";

/// The local cache of session records.
pub struct SessionStore {
    connection: Connection,
    pub(crate) limits: ChunkLimits,
}

/// What one [`SessionStore::sync_agent`] did.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    /// Distinct files the sync was given.
    pub files_listed: u64,
    /// Files with the size and modification time of their last full read.
    pub files_unchanged: u64,
    /// Files read, from the start or from where the last sync stopped.
    pub files_read: u64,
    /// Files that had changed and were read again from the start.
    pub files_restarted: u64,
    /// Files no longer listed or no longer on disk, whose records were dropped.
    pub files_removed: u64,
    /// Files that could not be read or parsed. Their cursor stays where it
    /// was, so the next sync tries the same bytes again.
    pub files_failed: u64,
    /// The first failed files, at most 32.
    pub failures: Vec<FileFailure>,
    /// Bytes read from session files.
    pub bytes_read: u64,
    /// Lines skipped for being longer than
    /// [`MAX_LINE_BYTES`](crate::MAX_LINE_BYTES).
    pub skipped_lines: u64,
    /// Session descriptions written.
    pub sessions: u64,
    /// Calls written, copies included.
    pub calls: u64,
    /// Indexed spans written.
    pub spans: u64,
}

/// A file a sync could not read or parse.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFailure {
    /// The file.
    pub path: PathBuf,
    /// Why: for example [`ErrorCode::ParseFailed`] for invalid parser output.
    pub code: ErrorCode,
}

/// Which calls [`SessionStore::for_each_call`] visits. Empty and absent
/// fields do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallFilter {
    /// Agents to include; empty includes every agent.
    pub agents: Vec<String>,
    /// Only calls of this session.
    pub session_id: Option<String>,
    /// Only calls that started at or after this time, in Unix milliseconds.
    pub since_ms: Option<i64>,
    /// Only calls that started before this time, in Unix milliseconds.
    pub until_ms: Option<i64>,
}

/// A call with what the statistics group it by.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEntry {
    /// The agent whose session recorded the call.
    pub agent: String,
    /// The session file the call was read from.
    pub path: String,
    /// The working directory of the call's session, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The call.
    pub call: CallRecord,
}

/// Which sessions [`SessionStore::sessions`] lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Agents to include; empty includes every agent.
    pub agents: Vec<String>,
    /// Only sessions that ran in this working directory.
    pub cwd: Option<String>,
    /// Only sessions that ran in one of these directories or below one; empty
    /// does not filter. A directory matches its own path and the paths that
    /// continue it after a `/` or `\`; trailing separators are ignored. At
    /// most [`MAX_FILTER_DIRECTORIES`].
    pub cwd_under: Vec<String>,
    /// Only sessions last active at or after this time, in Unix milliseconds.
    pub active_since_ms: Option<i64>,
    /// Only the session with this ID, in the agent's own ID format.
    pub session_id: Option<String>,
    /// Which sessions come first.
    pub order: SessionOrder,
    /// Sessions to skip, for the next page.
    pub offset: u64,
    /// Sessions to list, at most [`MAX_SESSIONS_PAGE`].
    pub limit: usize,
}

/// The order of [`SessionStore::sessions`]. Sessions without the time come
/// last; ties go by agent and session ID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionOrder {
    /// Most recently active first: by the latest change, or the start when no
    /// file records a change.
    #[default]
    Updated,
    /// Most recently started first.
    Started,
}

/// How many sessions a [`SessionFilter`] selects, ignoring its page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCounts {
    /// Every session the filter selects.
    pub total: u64,
    /// The sessions of each working directory, most recently active first, at
    /// most [`MAX_SESSION_GROUPS`].
    pub groups: Vec<SessionGroup>,
    /// Whether directories were left out of `groups`.
    pub groups_truncated: bool,
}

/// The sessions of one working directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGroup {
    /// The directory; absent for sessions whose directory is not known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Sessions that ran in it.
    pub count: u64,
    /// The latest activity of those sessions, in Unix milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
}

/// A session, merged from every file that describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    /// The agent that ran the session.
    pub agent: String,
    /// The session, in the agent's own ID format.
    #[serde(rename = "sessionID")]
    pub session_id: String,
    /// The file the other fields come from: the first, by path, of the files
    /// that describe the session, or of those that only mention it.
    pub path: String,
    /// Files that mention the session.
    pub file_count: u64,
    /// The session this one was resumed or forked from.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentSessionID")]
    pub parent_session_id: Option<String>,
    /// The working directory the agent ran in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The earliest start any file records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    /// The latest change any file records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
    /// The version of the agent CLI that wrote `path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    /// The bytes of `path` that hold the title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_range: Option<ByteRange>,
    /// How many bytes of `path` the cache has read: every whole line up to
    /// here, so [`SessionStore::read_range`] takes any range that ends by it.
    pub read_bytes: u64,
    /// The session's messages in `path`: the prompts and replies its plugin
    /// reported as searchable text. Tool output and other files' copies are
    /// not counted.
    pub message_count: u64,
    /// Threads besides the main one that made model calls in the session, in
    /// any of its files: the agent's subagents.
    pub subagent_count: u64,
    /// The model of the session's latest call: the main thread's when it made
    /// one, otherwise a subagent's; the requested model when the response
    /// named none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The bytes of `path` that hold the session's last message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_range: Option<ByteRange>,
}

/// The sessions a [`SessionFilter`] selects, before its order and page, one
/// row per session: the copy the other fields come from. Parameters: `?1`
/// agents, `?2` cwd, `?3` active since, `?4` session ID, `?5` the
/// directories of `cwd_under`.
macro_rules! selected_sessions {
    () => {
        "WITH ranked AS (
            SELECT f.agent, f.path, f.cursor_offset, s.*,
                   ROW_NUMBER() OVER (
                       PARTITION BY f.agent, s.session_id
                       ORDER BY s.described DESC, f.path
                   ) AS copy_rank,
                   COUNT(*) OVER whole AS file_count,
                   MIN(s.started_at_ms) OVER whole AS first_started_ms,
                   MAX(s.updated_at_ms) OVER whole AS last_updated_ms
            FROM sessions s JOIN files f ON f.id = s.file_id
            WHERE (json_array_length(?1) = 0
                   OR f.agent IN (SELECT value FROM json_each(?1)))
              AND (?4 IS NULL OR s.session_id = ?4)
            WINDOW whole AS (PARTITION BY f.agent, s.session_id)
        ),
        selected AS (
            SELECT * FROM ranked
            WHERE copy_rank = 1
              AND (?2 IS NULL OR cwd = ?2)
              AND (?3 IS NULL OR COALESCE(last_updated_ms, first_started_ms) >= ?3)
              AND (json_array_length(?5) = 0
                   OR EXISTS (SELECT 1 FROM json_each(?5) d
                              WHERE cwd = d.value
                                 OR substr(cwd, 1, length(d.value) + 1)
                                    IN (d.value || '/', d.value || char(92))))
        )"
    };
}

/// A full-text search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Words to find; a span matches when it contains every term of them.
    /// Words of a spaced script also match as prefixes of longer words.
    pub text: String,
    /// Agents to include; empty includes every agent.
    pub agents: Vec<String>,
    /// Only spans of this session.
    pub session_id: Option<String>,
    /// Hits to return, at most [`MAX_SEARCH_HITS`].
    pub limit: usize,
}

/// Where a search found its terms. The text is read again from `path` with
/// [`SessionStore::read_range`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// The agent whose session holds the text.
    pub agent: String,
    /// The session file.
    pub path: String,
    /// The session the text belongs to.
    #[serde(rename = "sessionID")]
    pub session_id: String,
    /// When the text was written, when the parser knew.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,
    /// The bytes of `path` the span covers.
    pub range: ByteRange,
}

/// How a failure while syncing one file affects the rest of the sync.
enum SyncFailure {
    /// The file could not be read or parsed; the sync goes on with the next.
    File(CoreError),
    /// The cache could not be written, or the operation was cancelled.
    Abort(CoreError),
}

impl From<CoreError> for SyncFailure {
    fn from(error: CoreError) -> Self {
        SyncFailure::File(error)
    }
}

/// A file the cache has read, as its row holds it.
struct KnownFile {
    id: i64,
    cursor: Cursor,
    state: Option<serde_json::Value>,
}

enum OpenFailure {
    /// The file is not a cache of this version; delete it and start again.
    Rebuild,
    Failed(CoreError),
}

impl SessionStore {
    /// Opens the cache in `cache_dir`, which must be in the platform's caches
    /// directory, never in the application bundle. Caches of other schema
    /// versions in it are deleted, and so is a damaged cache.
    pub fn open(cache_dir: &Path) -> Result<Self, CoreError> {
        fs::create_dir_all(cache_dir).map_err(|error| {
            io_error("Could not create the agent session cache directory", error)
        })?;
        remove_other_versions(cache_dir)?;
        let path = cache_dir.join(database_file_name());
        let connection = match open_database(&path) {
            Ok(connection) => connection,
            Err(OpenFailure::Rebuild) => {
                remove_database(&path)?;
                match open_database(&path) {
                    Ok(connection) => connection,
                    Err(OpenFailure::Rebuild) => {
                        return Err(CoreError::new(
                            ErrorCode::Unknown,
                            "Could not rebuild the agent session cache",
                        ))
                    }
                    Err(OpenFailure::Failed(error)) => return Err(error),
                }
            }
            Err(OpenFailure::Failed(error)) => return Err(error),
        };
        Ok(Self {
            connection,
            limits: ChunkLimits::default(),
        })
    }

    /// Brings the records of `agent` up to date with `files`, its session
    /// files as [`discover`](crate::discover) listed them. Files of the agent
    /// that are not listed any more are dropped. A file that fails is counted
    /// in the report and the sync goes on; cancellation and cache write
    /// failures stop it, keeping what was written so far.
    pub fn sync_agent(
        &mut self,
        agent: &str,
        files: &[PathBuf],
        parser: &mut dyn SessionParser,
    ) -> Result<SyncReport, CoreError> {
        check_caller_id("agent", agent)?;
        let mut report = SyncReport::default();
        let mut listed: Vec<(&str, &Path)> = Vec::with_capacity(files.len());
        for path in files {
            match path.to_str() {
                Some(text) => listed.push((text, path.as_path())),
                None => report.record_failure(path, ErrorCode::InvalidRequest),
            }
        }
        listed.sort_unstable_by(|left, right| left.0.cmp(right.0));
        listed.dedup_by(|left, right| left.0 == right.0);
        report.files_listed = listed.len() as u64;

        let known = self.known_files(agent)?;
        let transaction = self.transaction()?;
        for (id, path) in &known {
            if listed
                .binary_search_by(|(text, _)| (*text).cmp(path.as_str()))
                .is_err()
            {
                forget_file(&transaction, *id)?;
                report.files_removed += 1;
            }
        }
        commit(transaction)?;

        for (text, path) in listed {
            cancellation::check()?;
            match self.sync_file(agent, text, path, parser, &mut report) {
                Ok(()) => {}
                Err(SyncFailure::File(error)) => report.record_failure(path, error.code),
                Err(SyncFailure::Abort(error)) => return Err(error),
            }
        }
        Ok(report)
    }

    /// Drops every record of `agent`, for example when its plugin is removed
    /// or the user withdraws access to its sessions. Returns the files dropped.
    pub fn remove_agent(&mut self, agent: &str) -> Result<u64, CoreError> {
        check_caller_id("agent", agent)?;
        let known = self.known_files(agent)?;
        let transaction = self.transaction()?;
        for (id, _) in &known {
            forget_file(&transaction, *id)?;
        }
        commit(transaction)?;
        Ok(known.len() as u64)
    }

    /// Visits the calls `filter` selects, each once, in order of start time.
    /// A call recorded in several files is visited as its copy in the first
    /// file by path. Returns the calls visited.
    pub fn for_each_call(
        &self,
        filter: &CallFilter,
        mut visit: impl FnMut(CallEntry) -> Result<(), CoreError>,
    ) -> Result<u64, CoreError> {
        const SQL: &str = "
            WITH ranked AS (
                SELECT f.agent, f.path, c.*,
                       ROW_NUMBER() OVER (
                           PARTITION BY f.agent, c.call_key
                           ORDER BY f.path, c.range_start
                       ) AS copy_rank
                FROM calls c JOIN files f ON f.id = c.file_id
                WHERE json_array_length(?1) = 0
                   OR f.agent IN (SELECT value FROM json_each(?1))
            )
            SELECT r.agent, r.path,
                   COALESCE(
                       (SELECT s.cwd FROM sessions s
                        WHERE s.file_id = r.file_id AND s.session_id = r.session_id),
                       (SELECT s.cwd FROM sessions s JOIN files sf ON sf.id = s.file_id
                        WHERE s.session_id = r.session_id AND sf.agent = r.agent
                          AND s.cwd IS NOT NULL
                        ORDER BY sf.path LIMIT 1)),
                   r.call_key, r.session_id, r.thread_id, r.started_at_ms,
                   r.requested_model, r.model, r.context_window, r.effort, r.provider,
                   r.input_tokens, r.output_tokens, r.cache_read_tokens,
                   r.cache_write_5m_tokens, r.cache_write_1h_tokens, r.reasoning_tokens,
                   r.first_token_ms, r.duration_ms, r.compaction, r.request_id,
                   r.error_category, r.range_start, r.range_end
            FROM ranked r
            WHERE r.copy_rank = 1
              AND (?2 IS NULL OR r.session_id = ?2)
              AND (?3 IS NULL OR r.started_at_ms >= ?3)
              AND (?4 IS NULL OR r.started_at_ms < ?4)
            ORDER BY r.started_at_ms, r.agent, r.call_key";
        let mut statement = self
            .connection
            .prepare_cached(SQL)
            .map_err(|error| sql_error("Could not query agent calls", error))?;
        let mut rows = statement
            .query(params![
                agents_json(&filter.agents)?,
                filter.session_id,
                filter.since_ms,
                filter.until_ms
            ])
            .map_err(|error| sql_error("Could not query agent calls", error))?;
        let mut visited = 0;
        while let Some(row) = rows
            .next()
            .map_err(|error| sql_error("Could not query agent calls", error))?
        {
            cancellation::check()?;
            let entry =
                call_entry(row).map_err(|error| sql_error("Could not query agent calls", error))?;
            visit(entry)?;
            visited += 1;
        }
        Ok(visited)
    }

    /// Lists a page of sessions in the filter's order.
    pub fn sessions(&self, filter: &SessionFilter) -> Result<Vec<SessionEntry>, CoreError> {
        const SQL: &str = concat!(
            selected_sessions!(),
            "
            SELECT agent, session_id, path, file_count, parent_session_id, cwd,
                   first_started_ms, last_updated_ms, agent_version, title_start, title_end,
                   cursor_offset, file_id
            FROM selected
            ORDER BY CASE WHEN ?6 = 1 THEN first_started_ms
                          ELSE COALESCE(last_updated_ms, first_started_ms) END DESC NULLS LAST,
                     agent, session_id
            LIMIT ?7 OFFSET ?8"
        );
        let list = |error| sql_error("Could not list agent sessions", error);
        let limit = filter.limit.min(MAX_SESSIONS_PAGE);
        let mut entries = {
            let mut statement = self.connection.prepare_cached(SQL).map_err(list)?;
            let rows = statement
                .query_map(
                    params![
                        agents_json(&filter.agents)?,
                        filter.cwd,
                        filter.active_since_ms,
                        filter.session_id,
                        directories_json(&filter.cwd_under)?,
                        i64::from(filter.order == SessionOrder::Started),
                        limit,
                        filter.offset
                    ],
                    |row| {
                        let entry = SessionEntry {
                            agent: row.get(0)?,
                            session_id: row.get(1)?,
                            path: row.get(2)?,
                            file_count: row.get(3)?,
                            parent_session_id: row.get(4)?,
                            cwd: row.get(5)?,
                            started_at_ms: row.get(6)?,
                            updated_at_ms: row.get(7)?,
                            agent_version: row.get(8)?,
                            title_range: optional_range(row.get(9)?, row.get(10)?),
                            read_bytes: row.get(11)?,
                            message_count: 0,
                            subagent_count: 0,
                            model: None,
                            last_message_range: None,
                        };
                        Ok((row.get::<_, i64>(12)?, entry))
                    },
                )
                .map_err(list)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(list)?
        };
        for (file_id, entry) in &mut entries {
            cancellation::check()?;
            self.describe_activity(*file_id, entry).map_err(list)?;
        }
        Ok(entries.into_iter().map(|(_, entry)| entry).collect())
    }

    /// Fills in the messages, subagents and model of a listed session.
    fn describe_activity(&self, file_id: i64, entry: &mut SessionEntry) -> rusqlite::Result<()> {
        // The count is taken over every span before the limit keeps the last.
        const MESSAGES: &str = "
            SELECT range_start, range_end, COUNT(*) OVER ()
            FROM spans WHERE file_id = ?1 AND session_id = ?2
            ORDER BY range_start DESC LIMIT 1";
        // Every file that records calls of the session has a row in sessions.
        const THREADS: &str = "
            SELECT COUNT(DISTINCT c.thread_id)
            FROM sessions s
            JOIN files f ON f.id = s.file_id
            JOIN calls c ON c.file_id = s.file_id AND c.session_id = s.session_id
            WHERE s.session_id = ?1 AND f.agent = ?2";
        const MODEL: &str = "
            SELECT COALESCE(c.model, c.requested_model)
            FROM sessions s
            JOIN files f ON f.id = s.file_id
            JOIN calls c ON c.file_id = s.file_id AND c.session_id = s.session_id
            WHERE s.session_id = ?1 AND f.agent = ?2
              AND COALESCE(c.model, c.requested_model) IS NOT NULL
            ORDER BY c.thread_id IS NOT NULL, c.started_at_ms DESC, c.call_key DESC
            LIMIT 1";
        let last = self
            .connection
            .prepare_cached(MESSAGES)?
            .query_row(params![file_id, entry.session_id], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })
            .optional()?;
        if let Some((start, end, count)) = last {
            entry.message_count = count;
            entry.last_message_range = Some(ByteRange { start, end });
        }
        entry.subagent_count = self
            .connection
            .prepare_cached(THREADS)?
            .query_row(params![entry.session_id, entry.agent], |row| row.get(0))?;
        entry.model = self
            .connection
            .prepare_cached(MODEL)?
            .query_row(params![entry.session_id, entry.agent], |row| row.get(0))
            .optional()?;
        Ok(())
    }

    /// Counts the sessions `filter` selects, in all and by working directory,
    /// ignoring its order and page.
    pub fn session_counts(&self, filter: &SessionFilter) -> Result<SessionCounts, CoreError> {
        const TOTAL: &str = concat!(selected_sessions!(), " SELECT COUNT(*) FROM selected");
        const GROUPS: &str = concat!(
            selected_sessions!(),
            "
            SELECT cwd, COUNT(*), MAX(COALESCE(last_updated_ms, first_started_ms)) AS active
            FROM selected
            GROUP BY cwd
            ORDER BY active DESC NULLS LAST, cwd
            LIMIT ?6"
        );
        let count = |error| sql_error("Could not count agent sessions", error);
        let agents = agents_json(&filter.agents)?;
        let directories = directories_json(&filter.cwd_under)?;
        let total = self
            .connection
            .prepare_cached(TOTAL)
            .map_err(count)?
            .query_row(
                params![
                    agents,
                    filter.cwd,
                    filter.active_since_ms,
                    filter.session_id,
                    directories
                ],
                |row| row.get(0),
            )
            .map_err(count)?;
        let mut statement = self.connection.prepare_cached(GROUPS).map_err(count)?;
        let rows = statement
            .query_map(
                params![
                    agents,
                    filter.cwd,
                    filter.active_since_ms,
                    filter.session_id,
                    directories,
                    MAX_SESSION_GROUPS + 1
                ],
                |row| {
                    Ok(SessionGroup {
                        cwd: row.get(0)?,
                        count: row.get(1)?,
                        updated_at_ms: row.get(2)?,
                    })
                },
            )
            .map_err(count)?;
        let mut groups = rows.collect::<Result<Vec<_>, _>>().map_err(count)?;
        let groups_truncated = groups.len() > MAX_SESSION_GROUPS;
        groups.truncate(MAX_SESSION_GROUPS);
        Ok(SessionCounts {
            total,
            groups,
            groups_truncated,
        })
    }

    /// Finds the spans that contain every term of `query.text`, newest first.
    /// A query without letters or digits finds nothing.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, CoreError> {
        const SQL: &str = "
            SELECT f.agent, f.path, s.session_id, s.at_ms, s.range_start, s.range_end
            FROM span_terms
            JOIN spans s ON s.id = span_terms.rowid
            JOIN files f ON f.id = s.file_id
            WHERE span_terms MATCH ?1
              AND (json_array_length(?2) = 0
                   OR f.agent IN (SELECT value FROM json_each(?2)))
              AND (?3 IS NULL OR s.session_id = ?3)
            ORDER BY s.at_ms DESC NULLS LAST, f.path, s.range_start
            LIMIT ?4";
        let Some(expression) = text::match_expression(&query.text) else {
            return Ok(Vec::new());
        };
        let limit = query.limit.min(MAX_SEARCH_HITS);
        let mut statement = self
            .connection
            .prepare_cached(SQL)
            .map_err(|error| sql_error("Could not search agent sessions", error))?;
        let rows = statement
            .query_map(
                params![
                    expression,
                    agents_json(&query.agents)?,
                    query.session_id,
                    limit
                ],
                |row| {
                    Ok(SearchHit {
                        agent: row.get(0)?,
                        path: row.get(1)?,
                        session_id: row.get(2)?,
                        at_ms: row.get(3)?,
                        range: ByteRange {
                            start: row.get(4)?,
                            end: row.get(5)?,
                        },
                    })
                },
            )
            .map_err(|error| sql_error("Could not search agent sessions", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql_error("Could not search agent sessions", error))
    }

    /// Reads `range` of a session file of `agent` the cache has read. Returns
    /// `None` when the file is gone or changed since, so the range may no
    /// longer hold what the record says; a sync brings the records up to date.
    pub fn read_range(
        &self,
        agent: &str,
        path: &str,
        range: ByteRange,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        check_caller_id("agent", agent)?;
        let Some(known) = self.known_file(agent, path)? else {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "The agent session cache has not read this file",
            ));
        };
        if range.start >= range.end
            || range.end > known.cursor.offset
            || range.end - range.start > MAX_READ_BYTES
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "The byte range is outside what the cache has read of the file",
            ));
        }
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error("Could not open an agent session file", error)),
        };
        let size = file.metadata().map_err(ingest::read_failed)?.len();
        if size < known.cursor.offset
            || ingest::fingerprint(&mut file, known.cursor.offset).map_err(ingest::read_failed)?
                != known.cursor.fingerprint
        {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(range.start))
            .map_err(ingest::read_failed)?;
        let mut bytes = vec![0; usize::try_from(range.end - range.start).unwrap_or(usize::MAX)];
        file.read_exact(&mut bytes).map_err(ingest::read_failed)?;
        Ok(Some(bytes))
    }

    fn sync_file(
        &mut self,
        agent: &str,
        path_text: &str,
        path: &Path,
        parser: &mut dyn SessionParser,
        report: &mut SyncReport,
    ) -> Result<(), SyncFailure> {
        let known = self
            .known_file(agent, path_text)
            .map_err(SyncFailure::Abort)?;
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Deleted after it was listed: the same as not listed.
                if let Some(known) = known {
                    let transaction = self.transaction().map_err(SyncFailure::Abort)?;
                    forget_file(&transaction, known.id).map_err(SyncFailure::Abort)?;
                    commit(transaction).map_err(SyncFailure::Abort)?;
                    report.files_removed += 1;
                }
                return Ok(());
            }
            Err(error) => {
                return Err(io_error("Could not open an agent session file", error).into())
            }
        };
        let metadata = file.metadata().map_err(ingest::read_failed)?;
        if !metadata.is_file() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "An agent session path is not a regular file",
            )
            .into());
        }
        let stamp = FileStamp::of(&metadata);
        let plan = ingest::plan(&mut file, stamp, known.as_ref().map(|known| &known.cursor))
            .map_err(ingest::read_failed)?;
        let (file_id, start, mut state) = match (plan, known) {
            (Plan::Unchanged, _) => {
                report.files_unchanged += 1;
                return Ok(());
            }
            (Plan::Append { offset }, Some(known)) => (known.id, offset, known.state),
            (_, known) => {
                let empty = ingest::fingerprint(&mut file, 0).map_err(ingest::read_failed)?;
                let restarted = known.is_some();
                let id = self
                    .restart_file(agent, path_text, known.map(|known| known.id), &empty)
                    .map_err(SyncFailure::Abort)?;
                if restarted {
                    report.files_restarted += 1;
                }
                (id, 0, None)
            }
        };
        report.files_read += 1;

        let mut reader = File::open(path).map_err(ingest::read_failed)?;
        reader
            .seek(SeekFrom::Start(start))
            .map_err(ingest::read_failed)?;
        let mut reader = BufReader::with_capacity(READ_BUFFER_BYTES, reader);
        // Read up to the size the stamp records, so a file that grows during
        // the pass is read on from there next time.
        let limit = stamp.size - start;
        let limits = self.limits;
        let connection = &mut self.connection;
        let progress = ingest::read_lines(&mut reader, start, limit, limits, |offset, bytes| {
            cancellation::check().map_err(SyncFailure::Abort)?;
            let output = parser.scan(ScanChunk {
                path,
                offset,
                bytes,
                state: state.as_ref(),
            })?;
            let end = offset + bytes.len() as u64;
            let output = validate(output, offset, end)?;
            let fingerprint = ingest::fingerprint(&mut file, end).map_err(ingest::read_failed)?;
            let state_json = state_json(output.state.as_ref())?;
            write_chunk(connection, file_id, &output, report)
                .and_then(|transaction| {
                    update_cursor(
                        &transaction,
                        file_id,
                        end,
                        &fingerprint,
                        state_json.as_deref(),
                        None,
                    )?;
                    commit(transaction)
                })
                .map_err(SyncFailure::Abort)?;
            state = output.state;
            Ok::<(), SyncFailure>(())
        })?;
        report.bytes_read += progress.bytes_read;
        report.skipped_lines += progress.skipped_lines;

        // The stamp is stored only when the pass reached the size it records;
        // otherwise the file shrank meanwhile and the next sync checks it.
        let finished = (progress.bytes_read == limit).then_some(stamp);
        let fingerprint =
            ingest::fingerprint(&mut file, progress.end).map_err(ingest::read_failed)?;
        let state_json = state_json(state.as_ref())?;
        let transaction = self.transaction().map_err(SyncFailure::Abort)?;
        update_cursor(
            &transaction,
            file_id,
            progress.end,
            &fingerprint,
            state_json.as_deref(),
            finished,
        )
        .map_err(SyncFailure::Abort)?;
        commit(transaction).map_err(SyncFailure::Abort)
    }

    /// Drops the records of a file and resets its cursor, or adds the file.
    fn restart_file(
        &mut self,
        agent: &str,
        path: &str,
        known_id: Option<i64>,
        empty_fingerprint: &[u8],
    ) -> Result<i64, CoreError> {
        let transaction = self.transaction()?;
        let id = match known_id {
            Some(id) => {
                clear_file(&transaction, id)?;
                update_cursor(&transaction, id, 0, empty_fingerprint, None, None)?;
                id
            }
            None => transaction
                .query_row(
                    "INSERT INTO files (agent, path, cursor_offset, fingerprint)
                     VALUES (?1, ?2, 0, ?3) RETURNING id",
                    params![agent, path, empty_fingerprint],
                    |row| row.get(0),
                )
                .map_err(|error| sql_error("Could not write the agent session cache", error))?,
        };
        commit(transaction)?;
        Ok(id)
    }

    fn known_files(&self, agent: &str) -> Result<Vec<(i64, String)>, CoreError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT id, path FROM files WHERE agent = ?1")
            .map_err(|error| sql_error("Could not read the agent session cache", error))?;
        let rows = statement
            .query_map([agent], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| sql_error("Could not read the agent session cache", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql_error("Could not read the agent session cache", error))
    }

    fn known_file(&self, agent: &str, path: &str) -> Result<Option<KnownFile>, CoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT id, cursor_offset, stamp_size, stamp_modified_ms, fingerprint, state
                 FROM files WHERE agent = ?1 AND path = ?2",
                params![agent, path],
                |row| {
                    let stamp_size: Option<u64> = row.get(2)?;
                    let stamp_modified_ms: Option<i64> = row.get(3)?;
                    Ok((
                        row.get::<_, i64>(0)?,
                        Cursor {
                            offset: row.get(1)?,
                            stamp: stamp_size
                                .zip(stamp_modified_ms)
                                .map(|(size, modified_ms)| FileStamp { size, modified_ms }),
                            fingerprint: row.get(4)?,
                        },
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| sql_error("Could not read the agent session cache", error))?;
        let Some((id, cursor, state)) = row else {
            return Ok(None);
        };
        let state = match state {
            Some(state) => Some(serde_json::from_str(&state).map_err(|_| {
                CoreError::new(ErrorCode::Unknown, "The agent session cache is damaged")
            })?),
            None => None,
        };
        Ok(Some(KnownFile { id, cursor, state }))
    }

    fn transaction(&mut self) -> Result<Transaction<'_>, CoreError> {
        self.connection
            .transaction()
            .map_err(|error| sql_error("Could not write the agent session cache", error))
    }
}

impl SyncReport {
    fn record_failure(&mut self, path: &Path, code: ErrorCode) {
        self.files_failed += 1;
        if self.failures.len() < MAX_REPORTED_FAILURES {
            self.failures.push(FileFailure {
                path: path.to_path_buf(),
                code,
            });
        }
    }
}

fn open_database(path: &Path) -> Result<Connection, OpenFailure> {
    let connection = Connection::open(path).map_err(open_failure)?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(open_failure)?;
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(open_failure)?;
    connection
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
        .map_err(open_failure)?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(open_failure)?;
    match version {
        SCHEMA_VERSION => Ok(connection),
        0 => {
            // The schema and its version are written in one transaction, so
            // a cache without the version has none of the tables either.
            connection
                .execute_batch(&format!(
                    "BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
                ))
                .map_err(|_| OpenFailure::Rebuild)?;
            Ok(connection)
        }
        _ => Err(OpenFailure::Rebuild),
    }
}

fn open_failure(error: rusqlite::Error) -> OpenFailure {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) => {
            OpenFailure::Rebuild
        }
        _ => OpenFailure::Failed(sql_error("Could not open the agent session cache", error)),
    }
}

fn remove_database(path: &Path) -> Result<(), CoreError> {
    let name = database_file_name();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        remove_if_present(&path.with_file_name(format!("{name}{suffix}")))?;
    }
    Ok(())
}

/// Deletes the caches of other schema versions in `cache_dir`.
fn remove_other_versions(cache_dir: &Path) -> Result<(), CoreError> {
    let current = database_file_name();
    let entries = fs::read_dir(cache_dir)
        .map_err(|error| io_error("Could not list the agent session cache directory", error))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| io_error("Could not list the agent session cache directory", error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(DATABASE_PREFIX) && !name.starts_with(&current) {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                // Another Lithe of that version may still have it open, which
                // Windows does not allow deleting. It uses its own file, so
                // leaving it until a later start is harmless.
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(io_error(
                        "Could not delete an old agent session cache",
                        error,
                    ))
                }
            }
        }
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), CoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("Could not delete the agent session cache", error)),
    }
}

fn commit(transaction: Transaction<'_>) -> Result<(), CoreError> {
    transaction
        .commit()
        .map_err(|error| sql_error("Could not write the agent session cache", error))
}

/// Drops the records of a file and the file itself.
fn forget_file(transaction: &Transaction<'_>, file_id: i64) -> Result<(), CoreError> {
    clear_file(transaction, file_id)?;
    transaction
        .execute("DELETE FROM files WHERE id = ?1", [file_id])
        .map_err(|error| sql_error("Could not write the agent session cache", error))?;
    Ok(())
}

/// Drops the records of a file, keeping its row.
fn clear_file(transaction: &Transaction<'_>, file_id: i64) -> Result<(), CoreError> {
    let write =
        |error: rusqlite::Error| sql_error("Could not write the agent session cache", error);
    {
        // Rows of a contentless index are deleted by rowid one at a time; a
        // scan of the index cannot select them.
        let mut spans = transaction
            .prepare_cached("SELECT id FROM spans WHERE file_id = ?1")
            .map_err(write)?;
        let ids = spans
            .query_map([file_id], |row| row.get::<_, i64>(0))
            .map_err(write)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(write)?;
        let mut delete = transaction
            .prepare_cached("DELETE FROM span_terms WHERE rowid = ?1")
            .map_err(write)?;
        for id in ids {
            delete.execute([id]).map_err(write)?;
        }
    }
    for table in ["spans", "calls", "sessions"] {
        transaction
            .execute(
                &format!("DELETE FROM {table} WHERE file_id = ?1"),
                [file_id],
            )
            .map_err(write)?;
    }
    Ok(())
}

fn update_cursor(
    transaction: &Transaction<'_>,
    file_id: i64,
    offset: u64,
    fingerprint: &[u8],
    state: Option<&str>,
    stamp: Option<FileStamp>,
) -> Result<(), CoreError> {
    transaction
        .prepare_cached(
            "UPDATE files SET cursor_offset = ?2, fingerprint = ?3, state = ?4,
                              stamp_size = ?5, stamp_modified_ms = ?6
             WHERE id = ?1",
        )
        .and_then(|mut statement| {
            statement.execute(params![
                file_id,
                offset,
                fingerprint,
                state,
                stamp.map(|stamp| stamp.size),
                stamp.map(|stamp| stamp.modified_ms)
            ])
        })
        .map_err(|error| sql_error("Could not write the agent session cache", error))?;
    Ok(())
}

/// Writes the records of one chunk in a transaction the caller commits
/// together with the file's cursor.
fn write_chunk<'a>(
    connection: &'a mut Connection,
    file_id: i64,
    output: &ScanOutput,
    report: &mut SyncReport,
) -> Result<Transaction<'a>, CoreError> {
    let write =
        |error: rusqlite::Error| sql_error("Could not write the agent session cache", error);
    let transaction = connection.transaction().map_err(write)?;
    {
        let mut describe = transaction
            .prepare_cached(
                "INSERT INTO sessions (file_id, session_id, described, parent_session_id, cwd,
                                       started_at_ms, updated_at_ms, agent_version,
                                       title_start, title_end)
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (file_id, session_id) DO UPDATE SET
                     described = 1,
                     parent_session_id = COALESCE(excluded.parent_session_id, parent_session_id),
                     cwd = COALESCE(excluded.cwd, cwd),
                     started_at_ms = COALESCE(excluded.started_at_ms, started_at_ms),
                     updated_at_ms = COALESCE(excluded.updated_at_ms, updated_at_ms),
                     agent_version = COALESCE(excluded.agent_version, agent_version),
                     title_start = COALESCE(excluded.title_start, title_start),
                     title_end = CASE WHEN excluded.title_start IS NULL
                                      THEN title_end ELSE excluded.title_end END",
            )
            .map_err(write)?;
        for session in &output.sessions {
            describe
                .execute(params![
                    file_id,
                    session.session_id,
                    session.parent_session_id,
                    session.cwd,
                    session.started_at_ms,
                    session.updated_at_ms,
                    session.agent_version,
                    session.title_range.map(|range| range.start),
                    session.title_range.map(|range| range.end)
                ])
                .map_err(write)?;
            report.sessions += 1;
        }

        let mut mention = transaction
            .prepare_cached(
                "INSERT INTO sessions (file_id, session_id, described) VALUES (?1, ?2, 0)
                 ON CONFLICT (file_id, session_id) DO NOTHING",
            )
            .map_err(write)?;
        let mut call = transaction
            .prepare_cached(
                "INSERT OR REPLACE INTO calls (
                     file_id, call_key, session_id, thread_id, started_at_ms,
                     requested_model, model, context_window, effort, provider,
                     input_tokens, output_tokens, cache_read_tokens,
                     cache_write_5m_tokens, cache_write_1h_tokens, reasoning_tokens,
                     first_token_ms, duration_ms, compaction, request_id, error_category,
                     range_start, range_end)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            )
            .map_err(write)?;
        for record in &output.calls {
            mention
                .execute(params![file_id, record.session_id])
                .map_err(write)?;
            let tokens = &record.tokens;
            call.execute(params![
                file_id,
                record.key,
                record.session_id,
                record.thread_id,
                record.started_at_ms,
                record.requested_model,
                record.model,
                record.context_window,
                record.effort,
                record.provider,
                tokens.input,
                tokens.output,
                tokens.cache_read,
                tokens.cache_write_5m,
                tokens.cache_write_1h,
                tokens.reasoning,
                record.first_token_ms,
                record.duration_ms,
                record.compaction,
                record.request_id,
                record.error_category,
                record.range.start,
                record.range.end
            ])
            .map_err(write)?;
            report.calls += 1;
        }

        let mut span = transaction
            .prepare_cached(
                "INSERT INTO spans (file_id, session_id, at_ms, range_start, range_end)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(write)?;
        let mut terms = transaction
            .prepare_cached("INSERT INTO span_terms (rowid, terms) VALUES (?1, ?2)")
            .map_err(write)?;
        for indexed in &output.spans {
            let indexed_terms = text::index_terms(indexed_prefix(indexed));
            if indexed_terms.is_empty() {
                continue;
            }
            mention
                .execute(params![file_id, indexed.session_id])
                .map_err(write)?;
            span.execute(params![
                file_id,
                indexed.session_id,
                indexed.at_ms,
                indexed.range.start,
                indexed.range.end
            ])
            .map_err(write)?;
            terms
                .execute(params![transaction.last_insert_rowid(), indexed_terms])
                .map_err(write)?;
            report.spans += 1;
        }
    }
    Ok(transaction)
}

/// The part of a span's text that is indexed, cut at a character boundary.
fn indexed_prefix(span: &IndexSpan) -> &str {
    let text = span.text.as_str();
    if text.len() <= MAX_INDEXED_SPAN_BYTES {
        return text;
    }
    let mut end = MAX_INDEXED_SPAN_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn state_json(state: Option<&serde_json::Value>) -> Result<Option<String>, CoreError> {
    let Some(state) = state else {
        return Ok(None);
    };
    let json = serde_json::to_string(state).map_err(|_| invalid_output("state is not JSON"))?;
    if json.len() > MAX_STATE_BYTES {
        return Err(invalid_output("state is too large"));
    }
    Ok(Some(json))
}

/// Checks the records of a chunk that spans `[start, end)` of its file.
fn validate(output: ScanOutput, start: u64, end: u64) -> Result<ScanOutput, CoreError> {
    if output.sessions.len() + output.calls.len() + output.spans.len() > MAX_RECORDS_PER_CHUNK {
        return Err(invalid_output("a chunk produced too many records"));
    }
    let in_chunk =
        |range: ByteRange| range.start < range.end && start <= range.start && range.end <= end;
    for session in &output.sessions {
        check_id(&session.session_id, "sessions[].sessionID")?;
        check_optional_id(
            session.parent_session_id.as_deref(),
            "sessions[].parentSessionId",
        )?;
        check_optional_label(session.agent_version.as_deref(), "sessions[].agentVersion")?;
        if let Some(cwd) = &session.cwd {
            if cwd.is_empty() || cwd.len() > MAX_CWD_BYTES || cwd.chars().any(char::is_control) {
                return Err(invalid_output("sessions[].cwd is not a directory path"));
            }
        }
        if session.title_range.is_some_and(|range| !in_chunk(range)) {
            return Err(invalid_output("sessions[].titleRange is outside the chunk"));
        }
    }
    for call in &output.calls {
        check_id(&call.key, "calls[].key")?;
        check_id(&call.session_id, "calls[].sessionID")?;
        check_optional_id(call.thread_id.as_deref(), "calls[].threadID")?;
        check_optional_id(call.request_id.as_deref(), "calls[].requestID")?;
        for (label, field) in [
            (&call.requested_model, "calls[].requestedModel"),
            (&call.model, "calls[].model"),
            (&call.context_window, "calls[].contextWindow"),
            (&call.effort, "calls[].effort"),
            (&call.provider, "calls[].provider"),
            (&call.error_category, "calls[].errorCategory"),
        ] {
            check_optional_label(label.as_deref(), field)?;
        }
        check_tokens(&call.tokens)?;
        if call.first_token_ms.is_some_and(too_large) || call.duration_ms.is_some_and(too_large) {
            return Err(invalid_output("calls[] has a time out of range"));
        }
        if !in_chunk(call.range) {
            return Err(invalid_output("calls[].range is outside the chunk"));
        }
    }
    for span in &output.spans {
        check_id(&span.session_id, "spans[].sessionID")?;
        if !in_chunk(span.range) {
            return Err(invalid_output("spans[].range is outside the chunk"));
        }
    }
    Ok(output)
}

fn check_tokens(tokens: &Tokens) -> Result<(), CoreError> {
    let counts = [
        tokens.input,
        tokens.output,
        tokens.cache_read,
        tokens.cache_write_5m,
        tokens.cache_write_1h,
        tokens.reasoning,
    ];
    if counts.into_iter().flatten().any(too_large) {
        return Err(invalid_output("calls[].tokens has a count out of range"));
    }
    if let (Some(reasoning), Some(output)) = (tokens.reasoning, tokens.output) {
        if reasoning > output {
            return Err(invalid_output(
                "calls[].tokens.reasoning is more than calls[].tokens.output",
            ));
        }
    }
    Ok(())
}

fn too_large(value: u64) -> bool {
    i64::try_from(value).is_err()
}

fn check_id(value: &str, field: &str) -> Result<(), CoreError> {
    if is_token(value, MAX_ID_BYTES) {
        Ok(())
    } else {
        Err(invalid_output(&format!("{field} is not an identifier")))
    }
}

fn check_optional_id(value: Option<&str>, field: &str) -> Result<(), CoreError> {
    value.map_or(Ok(()), |value| check_id(value, field))
}

fn check_optional_label(value: Option<&str>, field: &str) -> Result<(), CoreError> {
    match value {
        Some(value) if !is_token(value, MAX_LABEL_BYTES) => {
            Err(invalid_output(&format!("{field} is not a label")))
        }
        _ => Ok(()),
    }
}

/// Checks an identifier a caller passed, such as an agent ID.
fn check_caller_id(field: &str, value: &str) -> Result<(), CoreError> {
    if is_token(value, MAX_ID_BYTES) {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("The {field} is not an identifier"),
        ))
    }
}

/// Non-empty printable ASCII without spaces, at most `max_bytes` long.
fn is_token(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// `field` names the record field, never its value: the value may be text the
/// parser should not have returned.
fn invalid_output(field: &str) -> CoreError {
    CoreError::new(
        ErrorCode::ParseFailed,
        "An agent plugin returned an invalid session record",
    )
    .with_details(field.to_string())
}

fn agents_json(agents: &[String]) -> Result<String, CoreError> {
    serde_json::to_string(agents)
        .map_err(|_| CoreError::new(ErrorCode::InvalidRequest, "The agent filter is not valid"))
}

/// The directories of [`SessionFilter::cwd_under`] as a JSON array, without
/// trailing separators: a root becomes the empty string, which every
/// absolute path continues after its separator.
fn directories_json(directories: &[String]) -> Result<String, CoreError> {
    if directories.len() > MAX_FILTER_DIRECTORIES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many directories in one session filter",
        ));
    }
    let trimmed: Vec<&str> = directories
        .iter()
        .map(|directory| directory.trim_end_matches(['/', '\\']))
        .collect();
    serde_json::to_string(&trimmed)
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Could not encode the directory filter"))
}

fn optional_range(start: Option<u64>, end: Option<u64>) -> Option<ByteRange> {
    start.zip(end).map(|(start, end)| ByteRange { start, end })
}

fn call_entry(row: &Row<'_>) -> rusqlite::Result<CallEntry> {
    Ok(CallEntry {
        agent: row.get(0)?,
        path: row.get(1)?,
        cwd: row.get(2)?,
        call: CallRecord {
            key: row.get(3)?,
            session_id: row.get(4)?,
            thread_id: row.get(5)?,
            started_at_ms: row.get(6)?,
            requested_model: row.get(7)?,
            model: row.get(8)?,
            context_window: row.get(9)?,
            effort: row.get(10)?,
            provider: row.get(11)?,
            tokens: Tokens {
                input: row.get(12)?,
                output: row.get(13)?,
                cache_read: row.get(14)?,
                cache_write_5m: row.get(15)?,
                cache_write_1h: row.get(16)?,
                reasoning: row.get(17)?,
            },
            first_token_ms: row.get(18)?,
            duration_ms: row.get(19)?,
            compaction: row.get(20)?,
            request_id: row.get(21)?,
            error_category: row.get(22)?,
            range: ByteRange {
                start: row.get(23)?,
                end: row.get(24)?,
            },
        },
    })
}

/// An I/O failure, without the path: the caller knows which file it asked for.
pub(crate) fn io_error(message: &str, error: io::Error) -> CoreError {
    let code = match error.kind() {
        io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        _ => ErrorCode::Unknown,
    };
    CoreError::new(code, message).with_details(error.to_string())
}

/// A SQLite failure. SQLite's own message can quote a statement or a value,
/// so only its result code is kept.
fn sql_error(message: &str, error: rusqlite::Error) -> CoreError {
    let details = match &error {
        rusqlite::Error::SqliteFailure(failure, _)
        | rusqlite::Error::SqlInputError { error: failure, .. } => {
            format!(
                "SQLite result code {} ({:?})",
                failure.extended_code, failure.code
            )
        }
        _ => "SQLite access failed".to_string(),
    };
    CoreError::new(ErrorCode::Unknown, message).with_details(details)
}
