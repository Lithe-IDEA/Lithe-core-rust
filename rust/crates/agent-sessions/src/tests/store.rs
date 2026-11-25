//! Syncs, queries and the privacy of the cache.

use std::fs;
use std::path::{Path, PathBuf};

use lithe_protocol::ErrorCode;
use serde_json::json;

use super::{append, call_line, session_line, text_line, LinesParser, TempDir};
use crate::ingest::ChunkLimits;
use crate::model::ByteRange;
use crate::store::{
    database_file_name, CallEntry, CallFilter, SearchHit, SearchQuery, SessionEntry, SessionFilter,
    SessionStore, SyncReport,
};
use crate::{MAX_CHUNK_BYTES, MAX_LINE_BYTES};

const AGENT: &str = "test-agent";
const OTHER_AGENT: &str = "other-agent";

/// Every line in a chunk of its own.
const LINE_CHUNKS: ChunkLimits = ChunkLimits {
    max_chunk_bytes: 1,
    max_line_bytes: MAX_LINE_BYTES,
};

struct Fixture {
    dir: TempDir,
    store: SessionStore,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let dir = TempDir::new(prefix);
        fs::create_dir_all(dir.path().join("sessions")).expect("create the sessions directory");
        let store = SessionStore::open(&dir.path().join("cache")).expect("open the cache");
        Self { dir, store }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.path().join("sessions").join(name)
    }

    fn sync(&mut self, files: &[PathBuf]) -> SyncReport {
        sync_as(&mut self.store, AGENT, files)
    }

    fn calls(&self) -> Vec<CallEntry> {
        calls(&self.store, &CallFilter::default())
    }

    fn search(&self, text: &str) -> Vec<SearchHit> {
        search(&self.store, text, &[])
    }
}

fn sync_as(store: &mut SessionStore, agent: &str, files: &[PathBuf]) -> SyncReport {
    store
        .sync_agent(agent, files, &mut LinesParser::default())
        .expect("sync the session files")
}

fn calls(store: &SessionStore, filter: &CallFilter) -> Vec<CallEntry> {
    let mut entries = Vec::new();
    store
        .for_each_call(filter, |entry| {
            entries.push(entry);
            Ok(())
        })
        .expect("query the calls");
    entries
}

fn sessions(store: &SessionStore, filter: SessionFilter) -> Vec<SessionEntry> {
    store.sessions(&filter).expect("list the sessions")
}

fn every_session() -> SessionFilter {
    SessionFilter {
        limit: 100,
        ..SessionFilter::default()
    }
}

fn search(store: &SessionStore, text: &str, agents: &[&str]) -> Vec<SearchHit> {
    store
        .search(&SearchQuery {
            text: text.to_string(),
            agents: agents.iter().map(|agent| agent.to_string()).collect(),
            session_id: None,
            limit: 100,
        })
        .expect("search the sessions")
}

fn keys(entries: &[CallEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| entry.call.key.as_str())
        .collect()
}

fn session_ids(entries: &[SessionEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| entry.session_id.as_str())
        .collect()
}

fn hit_times(hits: &[SearchHit]) -> Vec<Option<i64>> {
    hits.iter().map(|hit| hit.at_ms).collect()
}

fn use_line(id: &str) -> String {
    format!("{}\n", json!({ "type": "use", "id": id }))
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("a Unicode test path")
}

fn size(path: &Path) -> u64 {
    fs::metadata(path).expect("read the file size").len()
}

#[test]
fn appended_lines_are_read_from_the_cursor() {
    let mut fixture = Fixture::new("append");
    let path = fixture.file("a.jsonl");
    append(
        &path,
        &(session_line("s1", "/work/app", 1_000) + &call_line("c1", 1_100, "model-a", 10, 20)),
    );
    let report = fixture.sync(std::slice::from_ref(&path));
    assert_eq!(
        (
            report.files_listed,
            report.files_read,
            report.sessions,
            report.calls
        ),
        (1, 1, 1, 1)
    );
    let first_size = size(&path);
    assert_eq!(report.bytes_read, first_size);

    append(&path, &call_line("c2", 1_200, "model-a", 5, 6));
    let report = fixture.sync(std::slice::from_ref(&path));
    assert_eq!(
        (report.files_read, report.files_restarted, report.calls),
        (1, 0, 1)
    );
    assert_eq!(report.bytes_read, size(&path) - first_size);
    let entries = fixture.calls();
    assert_eq!(keys(&entries), ["c1", "c2"]);
    // The parser state carried the session over to the appended line.
    assert_eq!(entries[1].call.session_id, "s1");
    assert_eq!(entries[1].cwd.as_deref(), Some("/work/app"));
    assert_eq!(entries[1].path, path_text(&path));

    let report = fixture.sync(&[path.clone(), path]);
    assert_eq!(
        (
            report.files_listed,
            report.files_unchanged,
            report.files_read,
            report.bytes_read
        ),
        (1, 1, 0, 0)
    );
}

#[test]
fn rewritten_and_truncated_files_are_read_again() {
    let mut fixture = Fixture::new("rewrite");
    let path = fixture.file("a.jsonl");
    append(
        &path,
        &(session_line("s1", "/w", 1_000)
            + &call_line("c1", 1_100, "m", 1, 2)
            + &call_line("c2", 1_200, "m", 1, 2)),
    );
    fixture.sync(std::slice::from_ref(&path));

    fs::write(
        &path,
        session_line("s2", "/w", 2_000)
            + &call_line("c3", 2_100, "m", 1, 2)
            + &call_line("c4", 2_200, "m", 1, 2)
            + &call_line("c5", 2_300, "m", 1, 2),
    )
    .expect("rewrite the file");
    let report = fixture.sync(std::slice::from_ref(&path));
    assert_eq!((report.files_read, report.files_restarted), (1, 1));
    assert_eq!(keys(&fixture.calls()), ["c3", "c4", "c5"]);

    fs::write(&path, session_line("s3", "/w", 3_000)).expect("truncate the file");
    let report = fixture.sync(&[path]);
    assert_eq!(report.files_restarted, 1);
    assert!(fixture.calls().is_empty());
    assert_eq!(
        session_ids(&sessions(&fixture.store, every_session())),
        ["s3"]
    );
}

#[test]
fn overlong_lines_are_skipped() {
    let mut fixture = Fixture::new("long-line");
    fixture.store.limits = ChunkLimits {
        max_chunk_bytes: MAX_CHUNK_BYTES,
        max_line_bytes: 256,
    };
    let path = fixture.file("a.jsonl");
    append(
        &path,
        &(session_line("s1", "/w", 1_000)
            + &text_line(1_050, &"word ".repeat(100))
            + &call_line("c1", 1_100, "m", 1, 2)),
    );
    let report = fixture.sync(std::slice::from_ref(&path));
    assert_eq!((report.skipped_lines, report.spans), (1, 0));
    assert_eq!(report.bytes_read, size(&path));
    assert_eq!(keys(&fixture.calls()), ["c1"]);
    assert!(fixture.search("word").is_empty());
}

#[test]
fn a_line_waits_for_its_newline() {
    let mut fixture = Fixture::new("partial");
    let path = fixture.file("a.jsonl");
    let call = call_line("c1", 1_100, "m", 1, 2);
    let (head, tail) = call.split_at(call.len() - 5);
    append(&path, &(session_line("s1", "/w", 1_000) + head));
    fixture.sync(std::slice::from_ref(&path));
    assert!(fixture.calls().is_empty());
    assert_eq!(fixture.sync(std::slice::from_ref(&path)).files_unchanged, 1);

    append(&path, tail);
    let report = fixture.sync(&[path]);
    assert_eq!(
        (report.files_read, report.files_restarted, report.calls),
        (1, 0, 1)
    );
    assert_eq!(keys(&fixture.calls()), ["c1"]);
}

#[test]
fn unlisted_deleted_and_removed_files_lose_their_records() {
    let mut fixture = Fixture::new("forget");
    let a = fixture.file("a.jsonl");
    let b = fixture.file("b.jsonl");
    let other = fixture.file("other.jsonl");
    append(
        &a,
        &(session_line("sa", "/w", 1_000) + &call_line("ca", 1_100, "m", 1, 2)),
    );
    append(
        &b,
        &(session_line("sb", "/w", 2_000) + &call_line("cb", 2_100, "m", 1, 2)),
    );
    append(
        &other,
        &(session_line("so", "/w", 3_000) + &call_line("co", 3_100, "m", 1, 2)),
    );
    fixture.sync(&[a.clone(), b.clone()]);
    sync_as(&mut fixture.store, OTHER_AGENT, &[other]);
    assert_eq!(keys(&fixture.calls()), ["ca", "cb", "co"]);

    assert_eq!(fixture.sync(std::slice::from_ref(&a)).files_removed, 1);
    assert_eq!(keys(&fixture.calls()), ["ca", "co"]);

    fixture.sync(&[a.clone(), b.clone()]);
    fs::remove_file(&b).expect("delete a session file");
    assert_eq!(fixture.sync(&[a.clone(), b]).files_removed, 1);
    assert_eq!(keys(&fixture.calls()), ["ca", "co"]);

    assert_eq!(
        fixture.store.remove_agent(AGENT).expect("remove the agent"),
        1
    );
    assert_eq!(keys(&fixture.calls()), ["co"]);
    assert_eq!(
        session_ids(&sessions(&fixture.store, every_session())),
        ["so"]
    );
    // A removed agent's files are read from the start when it comes back.
    let report = fixture.sync(&[a]);
    assert_eq!((report.files_read, report.files_restarted), (1, 0));
}

#[test]
fn a_failed_chunk_is_offered_again() {
    let mut fixture = Fixture::new("retry");
    fixture.store.limits = LINE_CHUNKS;
    let path = fixture.file("a.jsonl");
    append(
        &path,
        &(session_line("s1", "/w", 1_000)
            + &call_line("c1", 1_100, "m", 1, 2)
            + &text_line(1_150, "boom")
            + &call_line("c2", 1_200, "m", 1, 2)),
    );
    let mut failing = LinesParser {
        fail_on: Some("boom"),
        ..LinesParser::default()
    };
    let report = fixture
        .store
        .sync_agent(AGENT, std::slice::from_ref(&path), &mut failing)
        .expect("sync the session files");
    assert_eq!((report.files_read, report.files_failed), (1, 1));
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, path);
    assert!(matches!(report.failures[0].code, ErrorCode::ParseFailed));
    assert_eq!(keys(&fixture.calls()), ["c1"]);

    let mut parser = LinesParser::default();
    let report = fixture
        .store
        .sync_agent(AGENT, &[path], &mut parser)
        .expect("sync the session files");
    assert_eq!((report.files_failed, report.files_restarted), (0, 0));
    // Only the failed line and the one after it are read again.
    assert_eq!(parser.chunks, 2);
    let entries = fixture.calls();
    assert_eq!(keys(&entries), ["c1", "c2"]);
    assert_eq!(entries[1].call.session_id, "s1");
}

#[test]
fn invalid_records_fail_their_file_only() {
    let mut fixture = Fixture::new("invalid");
    let line = |value: serde_json::Value| format!("{value}\n");
    let sentence_label = fixture.file("a.jsonl");
    append(
        &sentence_label,
        &(session_line("s1", "/w", 1_000)
            + &line(json!({ "type": "call", "key": "c1", "at": 1, "model": "a whole sentence" }))),
    );
    let outside_range = fixture.file("b.jsonl");
    append(
        &outside_range,
        &(session_line("s2", "/w", 1_000)
            + &line(json!({ "type": "call", "key": "c2", "at": 1, "badRange": true }))),
    );
    let text_id = fixture.file("c.jsonl");
    append(
        &text_id,
        &line(json!({ "type": "session", "id": "第一个会话", "cwd": "/w", "at": 1 })),
    );
    let control_cwd = fixture.file("d.jsonl");
    append(
        &control_cwd,
        &line(json!({ "type": "session", "id": "s4", "cwd": "/w\nsecond line", "at": 1 })),
    );
    let valid = fixture.file("e.jsonl");
    append(
        &valid,
        &(session_line("s5", "/w", 1_000) + &call_line("ok", 2, "m", 1, 2)),
    );

    let report = fixture.sync(&[
        sentence_label.clone(),
        outside_range.clone(),
        text_id.clone(),
        control_cwd.clone(),
        valid,
    ]);
    assert_eq!(report.files_failed, 4);
    let failed: Vec<&Path> = report
        .failures
        .iter()
        .map(|failure| failure.path.as_path())
        .collect();
    assert_eq!(
        failed,
        [&sentence_label, &outside_range, &text_id, &control_cwd]
    );
    assert!(report
        .failures
        .iter()
        .all(|failure| matches!(failure.code, ErrorCode::ParseFailed)));
    assert_eq!(keys(&fixture.calls()), ["ok"]);
    assert_eq!(
        session_ids(&sessions(&fixture.store, every_session())),
        ["s5"]
    );
}

#[test]
fn caller_identifiers_are_checked() {
    let mut fixture = Fixture::new("caller-ids");
    let error = fixture
        .store
        .sync_agent("an agent", &[], &mut LinesParser::default())
        .expect_err("an agent ID with a space is refused");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    let error = fixture
        .store
        .remove_agent("")
        .expect_err("an empty agent ID is refused");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
}

#[test]
fn the_cache_holds_no_conversation_text() {
    let mut fixture = Fixture::new("privacy");
    let path = fixture.file("a.jsonl");
    append(
        &path,
        &(session_line("s1", "/w", 1_000)
            + &text_line(1_100, "The quick brown fox jumps over the lazy dog")
            + &text_line(1_200, "今天去公园散步")
            + &call_line("c1", 1_300, "m", 1, 2)),
    );
    fixture.sync(&[path]);
    assert_eq!(hit_times(&fixture.search("quick fox")), [Some(1_100)]);
    assert_eq!(hit_times(&fixture.search("公园")), [Some(1_200)]);

    let cache = fixture.dir.path().join("cache");
    let mut files = 0;
    for entry in fs::read_dir(&cache).expect("list the cache") {
        let path = entry.expect("list the cache").path();
        let bytes = fs::read(&path).expect("read a cache file");
        for needle in [
            "quick brown",
            "brown fox",
            "lazy dog",
            "今天去公园",
            "去公园散步",
            "公园散步",
        ] {
            assert!(
                !bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes()),
                "{} holds {needle:?}",
                path.display()
            );
        }
        files += 1;
    }
    assert!(files >= 1);
}

#[test]
fn copied_calls_are_counted_once() {
    let mut fixture = Fixture::new("copies");
    let original = fixture.file("a.jsonl");
    let resumed = fixture.file("b.jsonl");
    append(
        &original,
        &(session_line("s1", "/w", 1_000)
            + &call_line("c1", 1_100, "m", 10, 20)
            + &call_line("c2", 1_200, "m", 10, 20)),
    );
    // A resumed session copies an earlier call and carries on.
    append(
        &resumed,
        &(use_line("s1")
            + &call_line("c1", 1_100, "m", 10, 20)
            + &call_line("c3", 1_300, "m", 1, 2)),
    );
    let report = fixture.sync(&[original.clone(), resumed.clone()]);
    assert_eq!(report.calls, 4);
    let entries = fixture.calls();
    assert_eq!(keys(&entries), ["c1", "c2", "c3"]);
    assert_eq!(entries[0].path, path_text(&original));
    // The resumed file only mentions the session; its cwd comes from the other.
    assert_eq!(entries[2].cwd.as_deref(), Some("/w"));
    let listed = sessions(&fixture.store, every_session());
    assert_eq!(session_ids(&listed), ["s1"]);
    assert_eq!(listed[0].file_count, 2);

    fs::remove_file(&original).expect("delete the original");
    fixture.sync(&[original, resumed.clone()]);
    let entries = fixture.calls();
    assert_eq!(keys(&entries), ["c1", "c3"]);
    assert_eq!(entries[0].path, path_text(&resumed));
}

#[test]
fn sessions_merge_their_files_and_filter() {
    let mut fixture = Fixture::new("sessions");
    let continued = fixture.file("a.jsonl");
    let described = fixture.file("b.jsonl");
    let other = fixture.file("c.jsonl");
    let other_agent = fixture.file("d.jsonl");
    // `a` sorts first but only continues the session `b` describes.
    append(
        &continued,
        &(use_line("s1") + &call_line("c9", 5_000, "m", 1, 2)),
    );
    append(
        &described,
        &(session_line("s1", "/w", 1_000) + &call_line("c1", 1_100, "m", 1, 2)),
    );
    append(&other, &session_line("s2", "/x", 3_000));
    append(&other_agent, &session_line("s3", "/w", 2_000));
    fixture.sync(&[continued, described.clone(), other]);
    sync_as(&mut fixture.store, OTHER_AGENT, &[other_agent]);

    let listed = sessions(&fixture.store, every_session());
    assert_eq!(session_ids(&listed), ["s2", "s3", "s1"]);
    let merged = &listed[2];
    assert_eq!(merged.agent, AGENT);
    assert_eq!(merged.path, path_text(&described));
    assert_eq!(merged.file_count, 2);
    assert_eq!(merged.cwd.as_deref(), Some("/w"));
    assert_eq!(
        (merged.started_at_ms, merged.updated_at_ms),
        (Some(1_000), Some(1_000))
    );
    assert!(merged.title_range.is_some());

    let only = |filter: SessionFilter| session_ids(&sessions(&fixture.store, filter)).join(",");
    assert_eq!(
        only(SessionFilter {
            agents: vec![AGENT.to_string()],
            ..every_session()
        }),
        "s2,s1"
    );
    assert_eq!(
        only(SessionFilter {
            cwd: Some("/w".to_string()),
            ..every_session()
        }),
        "s3,s1"
    );
    assert_eq!(
        only(SessionFilter {
            active_since_ms: Some(2_000),
            ..every_session()
        }),
        "s2,s3"
    );
    assert_eq!(
        only(SessionFilter {
            offset: 1,
            limit: 1,
            ..SessionFilter::default()
        }),
        "s3"
    );
}

#[test]
fn calls_filter_by_agent_session_and_time() {
    let mut fixture = Fixture::new("call-filter");
    let a = fixture.file("a.jsonl");
    let b = fixture.file("b.jsonl");
    append(
        &a,
        &(session_line("s1", "/w", 1_000)
            + &call_line("c1", 1_000, "m", 1, 2)
            + &session_line("s2", "/w", 2_000)
            + &call_line("c2", 2_000, "m", 1, 2)),
    );
    append(
        &b,
        &(session_line("s3", "/w", 3_000) + &call_line("c3", 3_000, "m", 1, 2)),
    );
    fixture.sync(&[a]);
    sync_as(&mut fixture.store, OTHER_AGENT, &[b]);

    let only = |filter: CallFilter| {
        calls(&fixture.store, &filter)
            .iter()
            .map(|entry| entry.call.key.clone())
            .collect::<Vec<_>>()
            .join(",")
    };
    assert_eq!(only(CallFilter::default()), "c1,c2,c3");
    assert_eq!(
        only(CallFilter {
            agents: vec![OTHER_AGENT.to_string()],
            ..CallFilter::default()
        }),
        "c3"
    );
    assert_eq!(
        only(CallFilter {
            session_id: Some("s2".to_string()),
            ..CallFilter::default()
        }),
        "c2"
    );
    // The start is inclusive and the end exclusive.
    assert_eq!(
        only(CallFilter {
            since_ms: Some(2_000),
            until_ms: Some(3_000),
            ..CallFilter::default()
        }),
        "c2"
    );
}

#[test]
fn a_rebuild_with_other_chunks_gives_the_same_records() {
    let dir = TempDir::new("rebuild-equal");
    let sessions_dir = dir.path().join("sessions");
    fs::create_dir_all(&sessions_dir).expect("create the sessions directory");
    let a = sessions_dir.join("a.jsonl");
    let b = sessions_dir.join("b.jsonl");
    let files = [a.clone(), b.clone()];
    let first_half = session_line("s1", "/w", 1_000)
        + &call_line("c1", 1_100, "m", 10, 20)
        + &text_line(1_150, "数据库迁移 migration plan");
    let second_half = call_line("c2", 1_200, "m", 30, 40)
        + &session_line("s2", "/x", 2_000)
        + &text_line(2_100, "another migration")
        + &call_line("c3", 2_200, "n", 50, 60);
    append(&a, &first_half);
    append(&b, &(use_line("s1") + &call_line("c1", 1_100, "m", 10, 20)));

    // One cache follows the files line by line as they grow; the other reads
    // them once, whole.
    let mut incremental = SessionStore::open(&dir.path().join("incremental")).expect("open");
    incremental.limits = LINE_CHUNKS;
    sync_as(&mut incremental, AGENT, &files);
    append(&a, &second_half);
    append(&b, &call_line("c4", 3_000, "n", 1, 2));
    sync_as(&mut incremental, AGENT, &files);
    let mut whole = SessionStore::open(&dir.path().join("whole")).expect("open");
    sync_as(&mut whole, AGENT, &files);

    assert_eq!(
        calls(&incremental, &CallFilter::default()),
        calls(&whole, &CallFilter::default())
    );
    assert_eq!(
        keys(&calls(&whole, &CallFilter::default())),
        ["c1", "c2", "c3", "c4"]
    );
    assert_eq!(
        sessions(&incremental, every_session()),
        sessions(&whole, every_session())
    );
    for text in ["migration", "数据库", "数据库迁移", "an"] {
        assert_eq!(
            search(&incremental, text, &[]),
            search(&whole, text, &[]),
            "{text}"
        );
    }
    assert_eq!(search(&whole, "migration", &[]).len(), 2);
}

#[test]
fn search_finds_chinese_and_english_words() {
    let mut fixture = Fixture::new("search");
    let a = fixture.file("a.jsonl");
    let b = fixture.file("b.jsonl");
    append(
        &a,
        &(session_line("s1", "/w", 1_000)
            + &text_line(1_000, "数据库连接失败了")
            + &text_line(2_000, "Connection refused by the database")
            + &text_line(3_000, "我在中间")),
    );
    append(
        &b,
        &(session_line("s2", "/w", 4_000) + &text_line(4_000, "数据库很快")),
    );
    fixture.sync(&[a]);
    sync_as(&mut fixture.store, OTHER_AGENT, &[b]);

    assert_eq!(
        hit_times(&fixture.search("数据库")),
        [Some(4_000), Some(1_000)]
    );
    assert_eq!(
        hit_times(&search(&fixture.store, "数据库", &[AGENT])),
        [Some(1_000)]
    );
    assert_eq!(hit_times(&fixture.search("库连")), [Some(1_000)]);
    // One character finds every run it is in, wherever it sits.
    assert_eq!(hit_times(&fixture.search("中")), [Some(3_000)]);
    assert_eq!(hit_times(&fixture.search("据")), [Some(4_000), Some(1_000)]);
    assert_eq!(hit_times(&fixture.search("间")), [Some(3_000)]);
    // A spaced word matches as a prefix, case-insensitively.
    assert_eq!(hit_times(&fixture.search("CONNECT")), [Some(2_000)]);
    assert_eq!(
        hit_times(&fixture.search("connect database")),
        [Some(2_000)]
    );
    assert!(fixture.search("connect 数据库").is_empty());
    assert!(fixture.search("refusal").is_empty());
    assert!(fixture.search("!!! ...").is_empty());

    let hits = fixture
        .store
        .search(&SearchQuery {
            text: "数据库".to_string(),
            agents: Vec::new(),
            session_id: Some("s2".to_string()),
            limit: 100,
        })
        .expect("search the sessions");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (hits[0].agent.as_str(), hits[0].session_id.as_str()),
        (OTHER_AGENT, "s2")
    );
    let hits = fixture
        .store
        .search(&SearchQuery {
            text: "数据库".to_string(),
            limit: 1,
            ..SearchQuery::default()
        })
        .expect("search the sessions");
    assert_eq!(hit_times(&hits), [Some(4_000)]);
}

#[test]
fn read_range_returns_the_bytes_until_the_file_changes() {
    let mut fixture = Fixture::new("read-range");
    let path = fixture.file("a.jsonl");
    let text = text_line(1_100, "deploy the service");
    append(&path, &(session_line("s1", "/w", 1_000) + &text));
    fixture.sync(std::slice::from_ref(&path));
    let hits = fixture.search("deploy");
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    let read = |store: &SessionStore, range: ByteRange| store.read_range(AGENT, &hit.path, range);
    assert_eq!(
        read(&fixture.store, hit.range)
            .expect("read the range")
            .as_deref(),
        Some(text.as_bytes())
    );

    // Appending leaves the bytes already read as they were.
    append(&path, &call_line("c1", 1_200, "m", 1, 2));
    assert!(read(&fixture.store, hit.range)
        .expect("read the range")
        .is_some());

    for range in [
        ByteRange { start: 5, end: 5 },
        ByteRange {
            start: 0,
            end: size(&path),
        },
    ] {
        let error = read(&fixture.store, range).expect_err("a range past the cursor");
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
    }
    let error = fixture
        .store
        .read_range(AGENT, "/never/read.jsonl", hit.range)
        .expect_err("a file the cache has not read");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    let error = fixture
        .store
        .read_range(OTHER_AGENT, &hit.path, hit.range)
        .expect_err("a file of another agent");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));

    fs::write(&path, session_line("s9", "/w", 9_000) + &text).expect("rewrite the file");
    assert_eq!(
        read(&fixture.store, hit.range).expect("read the range"),
        None
    );
    fs::remove_file(&path).expect("delete the file");
    assert_eq!(
        read(&fixture.store, hit.range).expect("read the range"),
        None
    );
}

#[test]
fn old_and_damaged_caches_are_rebuilt() {
    let dir = TempDir::new("rebuild");
    let cache = dir.path().join("cache");
    fs::create_dir_all(&cache).expect("create the cache directory");
    let stale = cache.join("agent-sessions-v0.sqlite3");
    let stale_wal = cache.join("agent-sessions-v0.sqlite3-wal");
    let unrelated = cache.join("notes.txt");
    for path in [&stale, &stale_wal, &unrelated] {
        fs::write(path, b"left by someone else").expect("write a file");
    }
    let session = dir.path().join("a.jsonl");
    append(
        &session,
        &(session_line("s1", "/w", 1_000) + &call_line("c1", 1_100, "m", 1, 2)),
    );

    let mut store = SessionStore::open(&cache).expect("open the cache");
    assert!(!stale.exists() && !stale_wal.exists());
    assert!(unrelated.exists());
    sync_as(&mut store, AGENT, std::slice::from_ref(&session));
    drop(store);
    let reopened = SessionStore::open(&cache).expect("open the cache");
    assert_eq!(keys(&calls(&reopened, &CallFilter::default())), ["c1"]);
    drop(reopened);

    let database = cache.join(database_file_name());
    {
        let connection = rusqlite::Connection::open(&database).expect("open the database");
        connection
            .pragma_update(None, "user_version", 99)
            .expect("set another schema version");
    }
    let mut store = SessionStore::open(&cache).expect("rebuild the cache");
    assert!(calls(&store, &CallFilter::default()).is_empty());
    sync_as(&mut store, AGENT, std::slice::from_ref(&session));
    assert_eq!(keys(&calls(&store, &CallFilter::default())), ["c1"]);
    drop(store);

    for suffix in ["-wal", "-shm"] {
        let side_file = cache.join(format!("{}{suffix}", database_file_name()));
        if side_file.exists() {
            fs::remove_file(side_file).expect("delete a side file");
        }
    }
    fs::write(&database, vec![b'x'; 4096]).expect("damage the database");
    let mut store = SessionStore::open(&cache).expect("rebuild the cache");
    assert!(calls(&store, &CallFilter::default()).is_empty());
    sync_as(&mut store, AGENT, &[session]);
    assert_eq!(keys(&calls(&store, &CallFilter::default())), ["c1"]);
}
