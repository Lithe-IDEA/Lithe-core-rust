//! The local session records of agent CLIs: syncing the cache from the files
//! the agents keep, and the lists, searches, reads and statistics the host and
//! plugins ask for.
//!
//! Core reads the files; the agent's plugin only interprets lines Core hands
//! it, through the `scan` and `read` functions its session sources declare.
//! The cache lives in the directory the host names, which is in the platform's
//! Caches directory and never in the application bundle; deleting it loses
//! nothing the agents' own files do not hold, except the last rate-limit
//! snapshots in `rate-limits.json`.
//!
//! Two connections to the cache keep a long sync from blocking the views: the
//! writer syncs and forgets, the reader lists, searches, reads and sums. The
//! writer lock is held across calls to plugin backends; nothing a backend can
//! ask for while it is called takes that lock.

use crate::backend::BackendLink;
use crate::catalog::{record_key, split_record_key, CatalogAgent};
use crate::events::{check_rate_limits, RateLimits, RateWindow, MAX_RATE_WINDOWS};
use crate::{log, now_ms, Runtime};
use lithe_agent_sessions::pricing::PriceTable;
use lithe_agent_sessions::stats::{GroupBy, SpeedRanker, Summarizer};
use lithe_agent_sessions::{
    discover, ByteRange, CallFilter, ScanChunk, ScanOutput, SearchQuery, SessionFilter,
    SessionOrder, SessionParser, SessionSource, SessionStore, MAX_FILTER_DIRECTORIES,
    MAX_SEARCH_HITS, MAX_SESSIONS_PAGE,
};
use lithe_lsp_engine::agent_host::AgentHostFailure;
use lithe_plugin_agents::SessionSourceDeclaration;
use lithe_plugin_rpc::codes;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

/// How long one `scan` call may take.
const SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one `read` call may take.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Longest title Core passes on, in bytes.
const MAX_TITLE_BYTES: usize = 512;

/// Most messages one read item returns.
const MAX_MESSAGES_PER_ITEM: usize = 2048;

/// Longest text of one read message, in bytes.
const MAX_MESSAGE_TEXT_BYTES: usize = 1024 * 1024;

/// Longest range `lithe/host/agents/usage` sums, in days.
const MAX_USAGE_DAYS: i64 = 366;

/// Longest directory `agentSessions.list`'s `cwdUnder` takes, in bytes.
const MAX_FILTER_DIRECTORY_BYTES: usize = 4096;

/// Largest `rate-limits.json` Core loads.
const MAX_RATE_FILE_BYTES: u64 = 1024 * 1024;

const RATE_FILE: &str = "rate-limits.json";

const MILLISECONDS_PER_DAY: i64 = 86_400_000;

/// Where the cache is and what the session roots resolve against.
#[derive(Debug, Clone)]
struct Config {
    cache_directory: PathBuf,
    user_home: String,
    /// The variables the agents' session roots name, from the host.
    environment: BTreeMap<String, String>,
}

/// The latest rate-limit state of one agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RateSnapshot {
    /// When the agent reported it, in Unix milliseconds.
    at_ms: i64,
    windows: Vec<RateWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
}

impl RateSnapshot {
    fn new(limits: RateLimits, at_ms: i64) -> Self {
        Self {
            at_ms,
            windows: limits.windows,
            plan: limits.plan,
        }
    }

    fn check(&self) -> Result<(), String> {
        check_rate_limits(&RateLimits {
            windows: self.windows.clone(),
            plan: self.plan.clone(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateFile {
    version: u32,
    agents: BTreeMap<String, RateSnapshot>,
}

#[derive(Default)]
pub(crate) struct Records {
    writer: Mutex<Option<SessionStore>>,
    reader: Mutex<Option<SessionStore>>,
    config: RwLock<Option<Config>>,
    prices: RwLock<Option<PriceTable>>,
    rate_limits: Mutex<BTreeMap<String, RateSnapshot>>,
}

fn not_configured() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "The agent session cache is not configured.",
    )
    .with_details("agentSessions.notConfigured")
}

fn refusal(code: ErrorCode, details: &'static str, message: &str) -> CoreError {
    CoreError::new(code, message).with_details(details)
}

impl Records {
    fn config(&self) -> Option<Config> {
        self.config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn lock_reader(&self) -> MutexGuard<'_, Option<SessionStore>> {
        self.reader.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_writer(&self) -> MutexGuard<'_, Option<SessionStore>> {
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_rates(&self) -> MutexGuard<'_, BTreeMap<String, RateSnapshot>> {
        self.rate_limits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// `agentSessions.configure`.
    pub fn configure(&self, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct ConfigureRequest {
            /// A directory of the platform's Caches directory for the cache.
            cache_directory: String,
            user_home: String,
            /// Values of the variables in the catalog's
            /// `sessionRootVariables`.
            #[serde(default)]
            environment: BTreeMap<String, String>,
            /// A models.dev `api.json` the host keeps, to price calls.
            #[serde(default)]
            prices_file: Option<String>,
        }
        let request: ConfigureRequest = crate::decode(payload)?;
        let cache_directory = PathBuf::from(&request.cache_directory);
        if !cache_directory.is_absolute() || !Path::new(&request.user_home).is_absolute() {
            return Err(refusal(
                ErrorCode::InvalidRequest,
                "agentSessions.invalidDirectory",
                "The cache directory and user home must be absolute paths.",
            ));
        }
        if request.environment.len() > 256
            || request
                .environment
                .iter()
                .any(|(name, value)| name.is_empty() || name.len() > 256 || value.len() > 32 * 1024)
        {
            return Err(refusal(
                ErrorCode::InvalidRequest,
                "agentSessions.invalidEnvironment",
                "The environment for session roots is invalid.",
            ));
        }

        let same_directory = self
            .config()
            .is_some_and(|config| config.cache_directory == cache_directory);
        if !same_directory {
            let writer = SessionStore::open(&cache_directory)?;
            let reader = SessionStore::open(&cache_directory)?;
            *self.lock_writer() = Some(writer);
            *self.lock_reader() = Some(reader);
            *self.lock_rates() = load_rate_file(&cache_directory);
        }
        *self.config.write().unwrap_or_else(PoisonError::into_inner) = Some(Config {
            cache_directory,
            user_home: request.user_home,
            environment: request.environment,
        });

        let mut answer = json!({});
        match request.prices_file.as_deref().map(load_prices) {
            None => {
                *self.prices.write().unwrap_or_else(PoisonError::into_inner) = None;
            }
            Some(Ok(table)) => {
                answer["pricedModels"] = json!(table.len());
                *self.prices.write().unwrap_or_else(PoisonError::into_inner) = Some(table);
            }
            Some(Err(error)) => {
                *self.prices.write().unwrap_or_else(PoisonError::into_inner) = None;
                answer["pricesError"] = json!(error.code);
            }
        }
        Ok(answer)
    }

    /// `agentSessions.sync`: brings the records of one agent up to date.
    pub fn sync(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        let request: AgentRequest = crate::decode(payload)?;
        let started = Instant::now();
        let outcome = self.sync_checked(runtime, &request);
        match &outcome {
            Ok(report) => log(
                "agent.records.updated",
                [
                    ("plugin", json!(request.plugin_id)),
                    ("agent", json!(request.agent_id)),
                    ("files_read", report["filesRead"].clone()),
                    ("files_failed", report["filesFailed"].clone()),
                    (
                        "duration",
                        json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
                    ),
                ],
            ),
            Err(error) => log(
                "agent.records.sync_failed",
                [
                    ("plugin", json!(request.plugin_id)),
                    ("agent", json!(request.agent_id)),
                    (
                        "reason",
                        json!(error
                            .details
                            .as_deref()
                            .unwrap_or("agentSessions.syncFailed")),
                    ),
                ],
            ),
        }
        outcome
    }

    fn sync_checked(&self, runtime: &Runtime, request: &AgentRequest) -> Result<Value, CoreError> {
        let config = self.config().ok_or_else(not_configured)?;
        let agent = readable_agent(runtime, &request.plugin_id, &request.agent_id)?;
        let mut routes = Vec::new();
        let mut files = Vec::new();
        let mut truncated = Vec::new();
        let mut unreadable_entries = 0u64;
        for source in &agent.agent.session_sources {
            let Some(root) = lithe_plugin_agents::resolve_root(
                &source.roots,
                &config.user_home,
                &config.environment,
            ) else {
                continue;
            };
            let discovery = discover(&SessionSource {
                root: root.clone(),
                include: source.include.clone(),
            })
            .map_err(|error| {
                let code = error.code;
                CoreError::new(code, "Could not list the agent's session files.")
                    .with_details("agentSessions.discoveryFailed")
            })?;
            if discovery.truncated {
                truncated.push(source.id.clone());
            }
            unreadable_entries += discovery.unreadable_entries;
            files.extend(discovery.files);
            let backend = backend_for(&*runtime.link, &agent.plugin_id, &source.scan)?;
            routes.push(Route {
                root,
                source: source.clone(),
                backend,
            });
        }

        let key = agent.record_key();
        let mut parser = PluginParser {
            link: &*runtime.link,
            plugin_id: &agent.plugin_id,
            agent_id: &agent.agent.id,
            routes: &routes,
            rate_limits: None,
        };
        let report = {
            let mut writer = self.lock_writer();
            let store = writer.as_mut().ok_or_else(not_configured)?;
            store.sync_agent(&key, &files, &mut parser)?
        };
        if let Some(snapshot) = parser.rate_limits.take() {
            self.keep_rate_snapshot(&key, snapshot);
        }
        let mut answer = serde_json::to_value(&report).unwrap_or_else(|_| json!({}));
        answer["truncatedSources"] = json!(truncated);
        answer["unreadableEntries"] = json!(unreadable_entries);
        Ok(answer)
    }

    /// `agentSessions.forget`: drops the records of one agent, for example
    /// when the user withdraws access to its sessions.
    pub fn forget(&self, payload: Value) -> Result<Value, CoreError> {
        let request: AgentRequest = crate::decode(payload)?;
        let key = record_key(&request.plugin_id, &request.agent_id);
        let files = {
            let mut writer = self.lock_writer();
            let store = writer.as_mut().ok_or_else(not_configured)?;
            store.remove_agent(&key)?
        };
        let removed_limits = self.lock_rates().remove(&key).is_some();
        if removed_limits {
            self.save_rate_file();
        }
        log(
            "agent.records.removed",
            [
                ("plugin", json!(request.plugin_id)),
                ("agent", json!(request.agent_id)),
                ("files", json!(files)),
            ],
        );
        Ok(json!({ "files": files }))
    }

    /// `agentSessions.list`: a page of the readable agents' sessions, most
    /// recently active or started first, with their titles and the counts of
    /// the whole list when asked.
    pub fn list(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize, Default, PartialEq, Eq)]
        #[serde(rename_all = "camelCase")]
        enum ListOrder {
            #[default]
            Updated,
            Started,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct ListRequest {
            #[serde(default)]
            agents: Option<Vec<String>>,
            #[serde(default)]
            cwd: Option<String>,
            #[serde(default)]
            cwd_under: Vec<String>,
            #[serde(default)]
            active_since_ms: Option<i64>,
            #[serde(rename = "sessionID", default)]
            session_id: Option<String>,
            #[serde(default)]
            order: ListOrder,
            #[serde(default)]
            offset: u64,
            #[serde(default)]
            limit: Option<usize>,
            #[serde(default)]
            titles: bool,
            #[serde(default)]
            counts: bool,
        }
        let request: ListRequest = crate::decode(payload)?;
        if request.cwd_under.len() > MAX_FILTER_DIRECTORIES
            || request.cwd_under.iter().any(|directory| {
                directory.is_empty()
                    || directory.len() > MAX_FILTER_DIRECTORY_BYTES
                    || directory.chars().any(char::is_control)
            })
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid agent command payload: cwdUnder",
            )
            .with_details("agents.invalidPayload"));
        }
        let Some(agents) = allowed_agents(runtime, request.agents) else {
            let mut answer = json!({ "sessions": [] });
            if request.counts {
                answer["total"] = json!(0);
                answer["groups"] = json!([]);
                answer["groupsTruncated"] = json!(false);
            }
            return Ok(answer);
        };
        let filter = SessionFilter {
            agents,
            cwd: request.cwd,
            cwd_under: request.cwd_under,
            active_since_ms: request.active_since_ms,
            session_id: request.session_id,
            order: match request.order {
                ListOrder::Updated => SessionOrder::Updated,
                ListOrder::Started => SessionOrder::Started,
            },
            offset: request.offset,
            limit: request.limit.unwrap_or(100).clamp(1, MAX_SESSIONS_PAGE),
        };
        let (entries, title_bytes, counts) = {
            let reader = self.lock_reader();
            let store = reader.as_ref().ok_or_else(not_configured)?;
            let entries = store.sessions(&filter)?;
            let counts = if request.counts {
                Some(store.session_counts(&filter)?)
            } else {
                None
            };
            let mut title_bytes = Vec::with_capacity(entries.len());
            for entry in &entries {
                let bytes = match (request.titles, entry.title_range) {
                    (true, Some(range)) => store
                        .read_range(&entry.agent, &entry.path, range)
                        .ok()
                        .flatten(),
                    _ => None,
                };
                title_bytes.push(bytes);
            }
            (entries, title_bytes, counts)
        };

        let mut titles: Vec<Option<String>> = vec![None; entries.len()];
        if request.titles {
            let config = self.config().ok_or_else(not_configured)?;
            // One read call per agent and source, with every title of it.
            let mut groups: BTreeMap<(String, usize), Vec<usize>> = BTreeMap::new();
            let mut routes: BTreeMap<String, Option<(CatalogAgent, Vec<RootedSource>)>> =
                BTreeMap::new();
            for (index, entry) in entries.iter().enumerate() {
                if title_bytes[index].is_none() {
                    continue;
                }
                let rooted = routes.entry(entry.agent.clone()).or_insert_with(|| {
                    let agent = runtime.catalog.find_key(&entry.agent)?;
                    let sources = rooted_sources(&agent, &config);
                    Some((agent, sources))
                });
                let Some((_, sources)) = rooted else { continue };
                if let Some(source) = source_index(sources, Path::new(&entry.path)) {
                    groups
                        .entry((entry.agent.clone(), source))
                        .or_default()
                        .push(index);
                }
            }
            for ((key, source), indexes) in groups {
                let Some(Some((agent, sources))) = routes.get(&key) else {
                    continue;
                };
                let items: Vec<Value> = indexes
                    .iter()
                    .map(|index| {
                        json!({
                            "path": entries[*index].path,
                            "lines": text_lines(title_bytes[*index].as_deref().unwrap_or_default()),
                        })
                    })
                    .collect();
                let Ok(answers) = call_read(
                    &*runtime.link,
                    agent,
                    &sources[source].source,
                    "titles",
                    items,
                ) else {
                    continue;
                };
                for (index, answer) in indexes.iter().zip(answers) {
                    titles[*index] = decode_title(&answer);
                }
            }
        }

        let sessions: Vec<Value> = entries
            .iter()
            .zip(titles)
            .map(|(entry, title)| {
                let mut value = serde_json::to_value(entry).unwrap_or_else(|_| json!({}));
                add_agent_ids(&mut value, &entry.agent);
                if let Some(title) = title {
                    value["title"] = json!(title);
                }
                value
            })
            .collect();
        let mut answer = json!({ "sessions": sessions });
        if let Some(counts) = counts {
            answer["total"] = json!(counts.total);
            answer["groups"] = serde_json::to_value(&counts.groups).unwrap_or_else(|_| json!([]));
            answer["groupsTruncated"] = json!(counts.groups_truncated);
        }
        Ok(answer)
    }

    /// `agentSessions.search`: where the readable agents' sessions contain
    /// the words of a query. The host reads the hits it shows.
    pub fn search(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct SearchRequest {
            text: String,
            #[serde(default)]
            agents: Option<Vec<String>>,
            #[serde(rename = "sessionID", default)]
            session_id: Option<String>,
            #[serde(default)]
            limit: Option<usize>,
        }
        let request: SearchRequest = crate::decode(payload)?;
        let Some(agents) = allowed_agents(runtime, request.agents) else {
            return Ok(json!({ "hits": [] }));
        };
        let hits = {
            let reader = self.lock_reader();
            let store = reader.as_ref().ok_or_else(not_configured)?;
            store.search(&SearchQuery {
                text: request.text,
                agents,
                session_id: request.session_id,
                limit: request.limit.unwrap_or(50).clamp(1, MAX_SEARCH_HITS),
            })?
        };
        let hits: Vec<Value> = hits
            .iter()
            .map(|hit| {
                let mut value = serde_json::to_value(hit).unwrap_or_else(|_| json!({}));
                add_agent_ids(&mut value, &hit.agent);
                value
            })
            .collect();
        Ok(json!({ "hits": hits }))
    }

    /// `agentSessions.read`: the messages of a byte range of a session file,
    /// as the agent's plugin reads them. A range that ends inside a line is
    /// read up to its last whole line, and `end` says where that is, so a host
    /// reads a long session a page at a time and no line is cut in two.
    pub fn read(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct ReadRequest {
            agent: String,
            path: String,
            range: ByteRange,
        }
        let request: ReadRequest = crate::decode(payload)?;
        let config = self.config().ok_or_else(not_configured)?;
        let (plugin_id, agent_id) = split_record_key(&request.agent).ok_or_else(|| {
            refusal(
                ErrorCode::InvalidRequest,
                "agentSessions.unknownAgent",
                "No installed plugin declares this agent.",
            )
        })?;
        let agent = readable_agent(runtime, plugin_id, agent_id)?;
        let sources = rooted_sources(&agent, &config);
        let source = source_index(&sources, Path::new(&request.path)).ok_or_else(|| {
            refusal(
                ErrorCode::InvalidRequest,
                "agentSessions.outsideSources",
                "The file is not in a session directory of the agent.",
            )
        })?;
        let (bytes, ends_line) = {
            let reader = self.lock_reader();
            let store = reader.as_ref().ok_or_else(not_configured)?;
            let bytes = store.read_range(&request.agent, &request.path, request.range)?;
            // A range ends a line when its last byte is a newline or the byte
            // after it begins one; a span a plugin recorded without its
            // newline is read whole.
            let ends_line = match &bytes {
                Some(bytes) if bytes.last() != Some(&b'\n') => {
                    let next = ByteRange {
                        start: request.range.end,
                        end: request.range.end.saturating_add(1),
                    };
                    !matches!(
                        store.read_range(&request.agent, &request.path, next),
                        Ok(Some(byte)) if byte != b"\n" && byte != b"\r"
                    )
                }
                _ => true,
            };
            (bytes, ends_line)
        };
        let Some(mut bytes) = bytes else {
            return Ok(json!({ "changed": true, "messages": [] }));
        };
        if !ends_line {
            bytes.truncate(whole_lines(&bytes));
        }
        // A range inside one line reads nothing and answers its own start, so
        // the caller asks again with a larger range.
        let end = request.range.start + bytes.len() as u64;
        if bytes.is_empty() {
            return Ok(json!({ "changed": false, "messages": [], "end": end }));
        }
        let items = vec![json!({ "path": request.path, "lines": text_lines(&bytes) })];
        let answers = call_read(
            &*runtime.link,
            &agent,
            &sources[source].source,
            "messages",
            items,
        )?;
        let messages = answers
            .into_iter()
            .next()
            .map(decode_messages)
            .transpose()?
            .unwrap_or_default();
        Ok(json!({ "changed": false, "messages": messages, "end": end }))
    }

    /// `agentSessions.stats`: usage of the readable agents for the host.
    pub fn stats(&self, runtime: &Runtime, payload: Value) -> Result<Value, CoreError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct StatsRequest {
            #[serde(default)]
            agents: Option<Vec<String>>,
            #[serde(rename = "sessionID", default)]
            session_id: Option<String>,
            #[serde(default)]
            since_ms: Option<i64>,
            #[serde(default)]
            until_ms: Option<i64>,
            group_by: String,
            #[serde(default)]
            utc_offset_seconds: i32,
        }
        let request: StatsRequest = crate::decode(payload)?;
        let group_by = parse_group_by(&request.group_by, request.utc_offset_seconds, true)
            .ok_or_else(|| {
                refusal(
                    ErrorCode::InvalidRequest,
                    "agentSessions.invalidGroup",
                    "Group by day, agent, model, session or cwd.",
                )
            })?;
        let (summaries, speeds) = match allowed_agents(runtime, request.agents) {
            None => (json!([]), json!([])),
            Some(agents) => {
                let filter = CallFilter {
                    agents,
                    session_id: request.session_id,
                    since_ms: request.since_ms,
                    until_ms: request.until_ms,
                };
                let reader = self.lock_reader();
                let store = reader.as_ref().ok_or_else(not_configured)?;
                self.summarize(store, &filter, group_by)?
            }
        };
        Ok(json!({
            "summaries": summaries,
            "speeds": speeds,
            "rateLimits": self.rate_limit_entries(runtime),
        }))
    }

    /// The summaries of the calls `filter` selects, grouped by `group_by`,
    /// and the models ranked by median output speed over the same calls.
    fn summarize(
        &self,
        store: &SessionStore,
        filter: &CallFilter,
        group_by: GroupBy,
    ) -> Result<(Value, Value), CoreError> {
        let prices = self.prices.read().unwrap_or_else(PoisonError::into_inner);
        let mut summarizer = Summarizer::new(group_by, prices.as_ref());
        let mut speeds = SpeedRanker::default();
        store.for_each_call(filter, |entry| {
            summarizer.add(&entry);
            speeds.add(&entry.call);
            Ok(())
        })?;
        Ok((
            serde_json::to_value(summarizer.finish()).unwrap_or_else(|_| json!([])),
            serde_json::to_value(speeds.finish()).unwrap_or_else(|_| json!([])),
        ))
    }

    /// Answers `lithe/host/agents/usage` for a plugin granted
    /// `agents.usage.read`: totals of the readable agents, grouped by day,
    /// agent or model, the models' median output speeds, and the agents'
    /// rate limits. It carries no conversation text,
    /// titles, session IDs or directories. Runs with the asking backend's
    /// session lock held, so it never waits for the cache.
    pub fn usage(&self, runtime: &Runtime, params: &Value) -> Result<Value, AgentHostFailure> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct UsageRequest {
            #[serde(default)]
            since_ms: Option<i64>,
            #[serde(default)]
            until_ms: Option<i64>,
            group_by: String,
            #[serde(default)]
            utc_offset_seconds: i32,
        }
        let invalid = |message: &str| AgentHostFailure::new(codes::HOST_INVALID_PARAMS, message);
        let request: UsageRequest = serde_json::from_value(params.clone())
            .map_err(|error| invalid(&format!("Invalid usage request: {error}")))?;
        let group_by = parse_group_by(&request.group_by, request.utc_offset_seconds, false)
            .ok_or_else(|| invalid("Group usage by day, agent or model."))?;
        let until_ms = request
            .until_ms
            .unwrap_or_else(|| now_ms() + MILLISECONDS_PER_DAY);
        let since_ms = request
            .since_ms
            .unwrap_or(until_ms - 30 * MILLISECONDS_PER_DAY);
        if since_ms >= until_ms || until_ms - since_ms > MAX_USAGE_DAYS * MILLISECONDS_PER_DAY {
            return Err(invalid("The usage range is empty or longer than 366 days."));
        }
        let agents: Vec<String> = runtime
            .catalog
            .readable()
            .iter()
            .map(CatalogAgent::record_key)
            .collect();
        let (summaries, speeds) = if agents.is_empty() {
            (json!([]), json!([]))
        } else {
            let reader = match self.reader.try_lock() {
                Ok(reader) => reader,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err(AgentHostFailure::new(
                        codes::HOST_BUSY,
                        "The session records are busy; try again shortly.",
                    ))
                }
            };
            let Some(store) = reader.as_ref() else {
                return Err(AgentHostFailure::new(
                    codes::HOST_UNAVAILABLE,
                    "The session records are not available yet.",
                ));
            };
            let filter = CallFilter {
                agents,
                session_id: None,
                since_ms: Some(since_ms),
                until_ms: Some(until_ms),
            };
            self.summarize(store, &filter, group_by).map_err(|_| {
                AgentHostFailure::new(codes::HOST_FAILED, "Could not sum the session records.")
            })?
        };
        Ok(json!({
            "sinceMs": since_ms,
            "untilMs": until_ms,
            "summaries": summaries,
            "speeds": speeds,
            "rateLimits": self.rate_limit_entries(runtime),
        }))
    }

    /// Keeps a rate-limit snapshot an agent reported while it ran.
    pub fn note_rate_limits(&self, key: &str, limits: RateLimits, at_ms: i64) {
        self.keep_rate_snapshot(key, RateSnapshot::new(limits, at_ms));
    }

    fn keep_rate_snapshot(&self, key: &str, snapshot: RateSnapshot) {
        let changed = {
            let mut rates = self.lock_rates();
            match rates.get(key) {
                Some(current) if current.at_ms > snapshot.at_ms || *current == snapshot => false,
                _ => {
                    rates.insert(key.to_string(), snapshot);
                    true
                }
            }
        };
        if changed {
            self.save_rate_file();
        }
    }

    /// The latest rate limits of the agents the user enabled.
    fn rate_limit_entries(&self, runtime: &Runtime) -> Vec<Value> {
        let rates = self.lock_rates().clone();
        rates
            .into_iter()
            .filter_map(|(key, snapshot)| {
                let agent = runtime.catalog.find_key(&key)?;
                if !agent.may_run && !agent.may_read_sessions {
                    return None;
                }
                let mut value = serde_json::to_value(&snapshot).ok()?;
                value["pluginID"] = json!(agent.plugin_id);
                value["agentID"] = json!(agent.agent.id);
                Some(value)
            })
            .collect()
    }

    fn save_rate_file(&self) {
        let Some(config) = self.config() else { return };
        let file = RateFile {
            version: 1,
            agents: self.lock_rates().clone(),
        };
        let Ok(bytes) = serde_json::to_vec(&file) else {
            return;
        };
        // Best effort: a lost snapshot comes back with the agent's next report.
        let path = config.cache_directory.join(RATE_FILE);
        let partial = config.cache_directory.join(format!("{RATE_FILE}.partial"));
        if fs::write(&partial, bytes).is_ok() {
            let _ = fs::rename(&partial, &path);
        }
    }
}

fn load_rate_file(directory: &Path) -> BTreeMap<String, RateSnapshot> {
    let path = directory.join(RATE_FILE);
    let readable = fs::metadata(&path).is_ok_and(|meta| meta.len() <= MAX_RATE_FILE_BYTES);
    let file: Option<RateFile> = readable
        .then(|| fs::read(&path).ok())
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let Some(file) = file.filter(|file| file.version == 1) else {
        return BTreeMap::new();
    };
    file.agents
        .into_iter()
        .filter(|(key, snapshot)| split_record_key(key).is_some() && snapshot.check().is_ok())
        .collect()
}

fn load_prices(path: &str) -> Result<PriceTable, CoreError> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The price table path must be absolute.",
        ));
    }
    let bytes = fs::read(path)
        .map_err(|_| CoreError::new(ErrorCode::EntryNotFound, "Could not read the price table."))?;
    PriceTable::from_models_dev(&bytes)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRequest {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    #[serde(rename = "agentID")]
    agent_id: String,
}

/// The agent, when the user lets Lithe read its sessions.
fn readable_agent(
    runtime: &Runtime,
    plugin_id: &str,
    agent_id: &str,
) -> Result<CatalogAgent, CoreError> {
    let agent = runtime.catalog.find(plugin_id, agent_id).ok_or_else(|| {
        refusal(
            ErrorCode::EntryNotFound,
            "agentSessions.unknownAgent",
            "No installed plugin declares this agent.",
        )
    })?;
    if !agent.may_read_sessions {
        return Err(refusal(
            ErrorCode::PermissionDenied,
            "agentSessions.readNotGranted",
            "The user has not allowed Lithe to read this agent's sessions.",
        ));
    }
    if agent.agent.session_sources.is_empty() {
        return Err(refusal(
            ErrorCode::NotSupported,
            "agentSessions.noSources",
            "The agent declares no session records.",
        ));
    }
    Ok(agent)
}

/// The record keys a request may cover: the requested agents the user lets
/// Lithe read, or all of them. `None` when that leaves none, because an empty
/// filter would cover every agent in the cache.
fn allowed_agents(runtime: &Runtime, requested: Option<Vec<String>>) -> Option<Vec<String>> {
    let readable: Vec<String> = runtime
        .catalog
        .readable()
        .iter()
        .map(CatalogAgent::record_key)
        .collect();
    let agents = match requested {
        Some(requested) => requested
            .into_iter()
            .filter(|key| readable.contains(key))
            .collect(),
        None => readable,
    };
    (!agents.is_empty()).then_some(agents)
}

fn add_agent_ids(value: &mut Value, key: &str) {
    if let Some((plugin_id, agent_id)) = split_record_key(key) {
        value["pluginID"] = json!(plugin_id);
        value["agentID"] = json!(agent_id);
    }
}

fn parse_group_by(name: &str, utc_offset_seconds: i32, host: bool) -> Option<GroupBy> {
    if !(-14 * 3600..=14 * 3600).contains(&utc_offset_seconds) {
        return None;
    }
    match name {
        "day" => Some(GroupBy::Day { utc_offset_seconds }),
        "agent" => Some(GroupBy::Agent),
        "model" => Some(GroupBy::Model),
        "session" if host => Some(GroupBy::Session),
        "cwd" if host => Some(GroupBy::Cwd),
        _ => None,
    }
}

/// A session source with its root resolved.
struct RootedSource {
    root: PathBuf,
    source: SessionSourceDeclaration,
}

fn rooted_sources(agent: &CatalogAgent, config: &Config) -> Vec<RootedSource> {
    agent
        .agent
        .session_sources
        .iter()
        .filter_map(|source| {
            let root = lithe_plugin_agents::resolve_root(
                &source.roots,
                &config.user_home,
                &config.environment,
            )?;
            Some(RootedSource {
                root,
                source: source.clone(),
            })
        })
        .collect()
}

/// The source whose root holds `path`, the deepest one when roots nest.
fn source_index(sources: &[RootedSource], path: &Path) -> Option<usize> {
    if !path.is_absolute() {
        return None;
    }
    sources
        .iter()
        .enumerate()
        .filter(|(_, source)| path.starts_with(&source.root))
        .max_by_key(|(_, source)| source.root.components().count())
        .map(|(index, _)| index)
}

fn backend_for(
    link: &dyn BackendLink,
    plugin_id: &str,
    function_id: &str,
) -> Result<String, CoreError> {
    link.session_for(plugin_id, function_id, None)
        .ok_or_else(|| {
            refusal(
                ErrorCode::InvalidRequest,
                "agentSessions.backendUnavailable",
                "The agent plugin's backend is not running.",
            )
        })
}

/// How many of `bytes`, which end inside a line, make whole lines: up to the
/// last newline, none when they hold no newline.
fn whole_lines(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1)
}

/// The lines of `bytes` as text, without their line breaks.
fn text_lines(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    body.split(|byte| *byte == b'\n')
        .map(|line| String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line)).into_owned())
        .collect()
}

/// Calls a source's `read` function with `items` and returns one answer
/// item per request item.
fn call_read(
    link: &dyn BackendLink,
    agent: &CatalogAgent,
    source: &SessionSourceDeclaration,
    mode: &str,
    items: Vec<Value>,
) -> Result<Vec<Value>, CoreError> {
    let backend = backend_for(link, &agent.plugin_id, &source.read)?;
    let count = items.len();
    let params = json!({
        "agentID": agent.agent.id,
        "sourceID": source.id,
        "mode": mode,
        "items": items,
    });
    let answer = link
        .call(
            &backend,
            &agent.plugin_id,
            &source.read,
            params,
            READ_TIMEOUT,
        )
        .map_err(|_| {
            refusal(
                ErrorCode::ProcessFailed,
                "agentSessions.readFailed",
                "The agent plugin could not read the session.",
            )
        })?;
    let invalid = || {
        refusal(
            ErrorCode::ParseFailed,
            "agentSessions.invalidRead",
            "The agent plugin returned an invalid reading of the session.",
        )
    };
    let Some(Value::Array(items)) = answer.get("items").cloned() else {
        return Err(invalid());
    };
    if items.len() != count || answer.as_object().is_some_and(|object| object.len() != 1) {
        return Err(invalid());
    }
    Ok(items)
}

fn decode_title(item: &Value) -> Option<String> {
    let title = item.get("title")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    let mut end = title.len().min(MAX_TITLE_BYTES);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    Some(title[..end].to_string())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadMessage {
    /// `user`, `assistant`, `thought` or `tool`.
    role: String,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadItem {
    #[serde(default)]
    messages: Vec<ReadMessage>,
}

fn decode_messages(item: Value) -> Result<Vec<ReadMessage>, CoreError> {
    let invalid = || {
        refusal(
            ErrorCode::ParseFailed,
            "agentSessions.invalidRead",
            "The agent plugin returned an invalid reading of the session.",
        )
    };
    let item: ReadItem = serde_json::from_value(item).map_err(|_| invalid())?;
    let valid = item.messages.len() <= MAX_MESSAGES_PER_ITEM
        && item.messages.iter().all(|message| {
            matches!(
                message.role.as_str(),
                "user" | "assistant" | "thought" | "tool"
            ) && message.text.len() <= MAX_MESSAGE_TEXT_BYTES
                && message
                    .tool_name
                    .as_deref()
                    .is_none_or(|name| !name.is_empty() && name.len() <= 256)
        });
    if valid {
        Ok(item.messages)
    } else {
        Err(invalid())
    }
}

/// Where the files of one source go to be scanned.
struct Route {
    root: PathBuf,
    source: SessionSourceDeclaration,
    backend: String,
}

/// A [`SessionParser`] that asks the agent plugin's backend.
struct PluginParser<'a> {
    link: &'a dyn BackendLink,
    plugin_id: &'a str,
    agent_id: &'a str,
    routes: &'a [Route],
    /// The newest rate-limit snapshot the scans found.
    rate_limits: Option<RateSnapshot>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScannedRateLimits {
    at_ms: i64,
    windows: Vec<RateWindow>,
    #[serde(default)]
    plan: Option<String>,
}

impl PluginParser<'_> {
    fn route(&self, path: &Path) -> Option<&Route> {
        self.routes
            .iter()
            .filter(|route| path.starts_with(&route.root))
            .max_by_key(|route| route.root.components().count())
    }

    fn note(&mut self, limits: Value) -> Result<(), CoreError> {
        let invalid = || {
            CoreError::new(
                ErrorCode::ParseFailed,
                "The agent plugin returned invalid rate limits.",
            )
            .with_details("agentSessions.invalidScan")
        };
        let limits: ScannedRateLimits = serde_json::from_value(limits).map_err(|_| invalid())?;
        if limits.windows.len() > MAX_RATE_WINDOWS {
            return Err(invalid());
        }
        let snapshot = RateSnapshot {
            at_ms: limits.at_ms,
            windows: limits.windows,
            plan: limits.plan,
        };
        snapshot.check().map_err(|_| invalid())?;
        if self
            .rate_limits
            .as_ref()
            .is_none_or(|current| current.at_ms <= snapshot.at_ms)
        {
            self.rate_limits = Some(snapshot);
        }
        Ok(())
    }
}

/// The whole lines of a chunk: each line's file offset, its length with the
/// line break, and its text. Text that is not UTF-8 is replaced, so a parser
/// computes ranges from `offset` and `length`, never from the text.
fn chunk_lines(offset: u64, bytes: &[u8]) -> Vec<Value> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| start + index + 1);
        let line = &bytes[start..end];
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        lines.push(json!({
            "offset": offset + start as u64,
            "length": line.len(),
            "text": String::from_utf8_lossy(content),
        }));
        start = end;
    }
    lines
}

impl SessionParser for PluginParser<'_> {
    fn scan(&mut self, chunk: ScanChunk<'_>) -> Result<ScanOutput, CoreError> {
        let Some(route) = self.route(chunk.path) else {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "The file is not in a session directory of the agent.",
            ));
        };
        let mut params = json!({
            "agentID": self.agent_id,
            "sourceID": route.source.id,
            "path": chunk.path.to_string_lossy(),
            "lines": chunk_lines(chunk.offset, chunk.bytes),
        });
        if let Some(state) = chunk.state {
            params["state"] = state.clone();
        }
        let mut answer = self
            .link
            .call(
                &route.backend,
                self.plugin_id,
                &route.source.scan,
                params,
                SCAN_TIMEOUT,
            )
            .map_err(|_| {
                CoreError::new(
                    ErrorCode::ProcessFailed,
                    "The agent plugin could not read a session file.",
                )
                .with_details("agentSessions.scanFailed")
            })?;
        if let Some(limits) = answer
            .as_object_mut()
            .and_then(|object| object.remove("rateLimits"))
        {
            self.note(limits)?;
        }
        serde_json::from_value(answer).map_err(|_| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "The agent plugin returned invalid session records.",
            )
            .with_details("agentSessions.invalidScan")
        })
    }
}

#[cfg(test)]
pub(crate) fn chunk_lines_for_test(offset: u64, bytes: &[u8]) -> Vec<Value> {
    chunk_lines(offset, bytes)
}

#[cfg(test)]
pub(crate) fn text_lines_for_test(bytes: &[u8]) -> Vec<String> {
    text_lines(bytes)
}
