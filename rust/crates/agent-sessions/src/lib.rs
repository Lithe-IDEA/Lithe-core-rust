//! Local agent session records: the model calls, token counts and search
//! index Lithe derives from the session files agent CLIs keep on this machine.
//!
//! Agent CLIs record every conversation in their own files. This crate reads
//! those files incrementally and keeps a cache that can be deleted at any time
//! and rebuilt from them; Lithe never writes the agents' files.
//!
//! Core knows no agent. Which directories hold sessions and how their lines are
//! read both come from the agent's plugin: [`discover`] lists the files a
//! declared source matches, and a [`SessionParser`], backed by the plugin's
//! backend, turns whole lines into the records in [`model`]. Each plugin only
//! sees the files of its own agents.
//!
//! The cache ([`SessionStore`]) is one SQLite database in the platform's Caches
//! directory. It holds, per file, a cursor of what has been read; per call,
//! metadata and token counts; and a full-text index that keeps no text. The
//! index stores the words of each indexed span, not the span: its rows hold no
//! positions, so neither the text nor the order of its words can be read back.
//! Every record points at a byte range of its source file, which is read again
//! when a message is shown.
//!
//! The cost, cache hit rate and speed of calls are computed once, here, in
//! [`stats`], with prices from a models.dev table ([`pricing`]).

mod discover;
mod ingest;
pub mod model;
pub mod pricing;
pub mod stats;
mod store;
mod text;

#[cfg(test)]
mod tests;

pub use discover::{discover, Discovery, SessionSource, MAX_DISCOVERY_DEPTH, MAX_FILES_PER_SOURCE};
pub use ingest::{MAX_CHUNK_BYTES, MAX_LINE_BYTES};
pub use model::{
    ByteRange, CallRecord, IndexSpan, ScanChunk, ScanOutput, SessionMeta, SessionParser, Tokens,
};
pub use store::{
    database_file_name, CallEntry, CallFilter, FileFailure, SearchHit, SearchQuery, SessionCounts,
    SessionEntry, SessionFilter, SessionGroup, SessionOrder, SessionStore, SyncReport,
    MAX_FILTER_DIRECTORIES, MAX_ID_BYTES, MAX_INDEXED_SPAN_BYTES, MAX_LABEL_BYTES, MAX_READ_BYTES,
    MAX_RECORDS_PER_CHUNK, MAX_SEARCH_HITS, MAX_SESSIONS_PAGE, MAX_SESSION_GROUPS, MAX_STATE_BYTES,
    SCHEMA_VERSION,
};
