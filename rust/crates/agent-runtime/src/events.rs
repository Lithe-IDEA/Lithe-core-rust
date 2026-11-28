//! Lithe's agent session events: what a plugin backend reports with
//! `lithe/host/agents/report`, the checks Core runs on them, and the bounded
//! queue the host reads with `agents.waitEvents`.
//!
//! The model is the same for every agent. Names follow the Agent Client
//! Protocol where it has one, so an ACP session maps onto it item by item; a
//! field the agent does not report stays absent instead of being estimated.

use lithe_agent_sessions::Tokens;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Most events one report carries.
pub(crate) const MAX_EVENTS_PER_REPORT: usize = 256;

/// Longest text of one event, in bytes: a reply, a tool's output or a diff
/// side. A longer text is refused; a backend splits it into deltas.
const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Longest ID, title, label or model name, in bytes.
const MAX_LABEL_BYTES: usize = 1024;

/// Most options of a permission request.
const MAX_PERMISSION_OPTIONS: usize = 8;

/// Most permission requests a session waits on at once.
const MAX_PENDING_PERMISSIONS: usize = 16;

/// Most file locations of one tool call.
const MAX_LOCATIONS: usize = 64;

/// Most entries of a plan.
const MAX_PLAN_ENTRIES: usize = 256;

/// Most windows of a rate-limit snapshot.
pub(crate) const MAX_RATE_WINDOWS: usize = 8;

/// Most settings a session reports at once.
const MAX_CONFIG_OPTIONS: usize = 16;

/// Most values of one setting.
const MAX_CONFIG_VALUES: usize = 128;

/// Most events the host queue keeps. The host reads continuously; when it
/// falls this far behind, the oldest events are dropped and the next read
/// says so.
const MAX_QUEUED_EVENTS: usize = 8192;

/// Most events one `agents.waitEvents` returns.
const MAX_EVENTS_PER_WAIT: usize = 512;

/// Longest wait of one `agents.waitEvents`.
const MAX_WAIT: Duration = Duration::from_secs(30);

/// A session event a plugin backend reports.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum SessionEvent {
    Session(SessionInfo),
    Message(MessageEvent),
    ToolCall(ToolCallEvent),
    Plan(PlanEvent),
    PermissionRequested(PermissionRequest),
    Usage(UsageEvent),
    RateLimits(RateLimits),
    TurnEnded(TurnEnded),
    Notice(Notice),
    ConfigOptions(ConfigOptions),
}

/// What the agent says about its session.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionInfo {
    /// The agent CLI's own session ID, used to resume and to find the session
    /// in the local records.
    #[serde(rename = "agentSessionID")]
    pub agent_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MessageRole {
    /// The agent's reply.
    Assistant,
    /// The agent's reasoning, as the CLI exposes it.
    Thought,
}

/// Text of the agent's reply or reasoning. `delta` text extends the message
/// with the same ID; otherwise it replaces it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MessageEvent {
    pub turn: u64,
    pub role: MessageRole,
    #[serde(rename = "messageID")]
    pub message_id: String,
    pub text: String,
    #[serde(default)]
    pub delta: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolLocation {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolDiff {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    pub new_text: String,
}

/// A tool call of the agent, reported again whenever its status changes.
/// Later reports of the same ID replace the fields they carry.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolCallEvent {
    pub turn: u64,
    #[serde(rename = "toolCallID")]
    pub tool_call_id: String,
    pub status: ToolStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<ToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<ToolLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<ToolDiff>,
    /// What the tool printed, for example a command's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}

/// The agent's plan for the turn; each report replaces the last.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanEvent {
    pub turn: u64,
    pub entries: Vec<PlanEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PermissionOption {
    pub id: String,
    pub label: String,
    pub option_kind: PermissionOptionKind,
}

/// The agent asks the user before it acts. The user picks one of `options`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PermissionRequest {
    pub turn: u64,
    pub permission: String,
    pub title: String,
    #[serde(
        rename = "toolCallID",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_call_id: Option<String>,
    pub options: Vec<PermissionOption>,
}

/// Token counts of one model call or of a whole turn, as the agent reports
/// them. `contextUsed` and `contextWindow` describe the context afterwards.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UsageEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// What the CLI itself says the turn cost, in US dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// One rate-limit window of the agent's account.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RateWindow {
    /// Stable ID of the window within the agent, for example `primary`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Share of the window used, 0 to 100.
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<i64>,
}

/// The agent's rate-limit state. Core keeps the latest snapshot per agent and
/// window for the usage view.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RateLimits {
    pub windows: Vec<RateWindow>,
    /// The plan the account is on, as the agent names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum StopReason {
    Completed,
    Cancelled,
    MaxTokens,
    MaxTurns,
    Refused,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TurnEnded {
    pub turn: u64,
    pub reason: StopReason,
    /// What the agent said about a failure, shown as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// Something the agent tells the user outside a reply, for example a model
/// fallback or a context compaction.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Notice {
    pub level: NoticeLevel,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// What a setting means to the host, so it can place the setting where users
/// expect it. The names are ACP's categories: ACP's `thought_level` is
/// `thoughtLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ConfigCategory {
    /// The model the session uses.
    Model,
    /// How hard the model thinks, its values listed from least to most.
    ThoughtLevel,
    /// The agent's mode, for example asking before edits or planning only.
    Mode,
}

/// One value a setting may take.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConfigValue {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A setting of the session the user may change, as ACP's session config
/// option: the value it has now and the values it may take.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConfigOption {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<ConfigCategory>,
    pub current_value: String,
    pub options: Vec<ConfigValue>,
}

/// The settings of the session the user may change. Each report replaces the
/// last; an empty list means there are none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConfigOptions {
    pub config_options: Vec<ConfigOption>,
}

/// Why Core refused a report: a stable code for `error.data.code` and a
/// message that names the event by index, never its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportRefusal {
    pub code: &'static str,
    pub message: String,
}

/// The turns and permission requests of one session, which the events of a
/// report must fit.
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnState {
    /// The last turn Core started.
    pub last_turn: u64,
    /// The turn in progress, if any.
    pub active_turn: Option<u64>,
    /// Permission requests waiting for the user: ID to the turn and options.
    pub pending: BTreeMap<String, (u64, Vec<PermissionOption>)>,
    /// The agent CLI's own session ID, once reported.
    pub agent_session_id: Option<String>,
    /// The settings the backend last reported: ID to the values it may take.
    pub config: BTreeMap<String, Vec<String>>,
}

impl TurnState {
    /// Starts the next turn. `None` while a turn is still in progress.
    pub fn start_turn(&mut self) -> Option<u64> {
        if self.active_turn.is_some() {
            return None;
        }
        self.last_turn += 1;
        self.active_turn = Some(self.last_turn);
        Some(self.last_turn)
    }

    /// Checks `events` against this state and applies them. Nothing is applied
    /// when one event is refused.
    pub fn apply(&mut self, events: &[SessionEvent]) -> Result<(), ReportRefusal> {
        let mut next = self.clone();
        for (index, event) in events.iter().enumerate() {
            next.apply_one(event)
                .map_err(|(code, problem)| ReportRefusal {
                    code,
                    message: format!("Event {index} of the report {problem}."),
                })?;
        }
        *self = next;
        Ok(())
    }

    fn apply_one(&mut self, event: &SessionEvent) -> Result<(), (&'static str, String)> {
        check_bounds(event).map_err(|problem| (INVALID_PARAMS, problem))?;
        match event {
            SessionEvent::Session(info) => {
                self.agent_session_id = Some(info.agent_session_id.clone());
            }
            SessionEvent::Message(message) => self.require_active(message.turn)?,
            SessionEvent::ToolCall(call) => self.require_active(call.turn)?,
            SessionEvent::Plan(plan) => self.require_active(plan.turn)?,
            SessionEvent::PermissionRequested(request) => {
                self.require_active(request.turn)?;
                if self.pending.contains_key(&request.permission) {
                    return Err((
                        INVALID_PARAMS,
                        "repeats a permission ID that is still waiting".to_string(),
                    ));
                }
                if self.pending.len() >= MAX_PENDING_PERMISSIONS {
                    return Err((
                        LIMIT_EXCEEDED,
                        "exceeds the permission requests a session may wait on".to_string(),
                    ));
                }
                self.pending.insert(
                    request.permission.clone(),
                    (request.turn, request.options.clone()),
                );
            }
            SessionEvent::Usage(usage) => {
                if let Some(turn) = usage.turn {
                    if turn == 0 || turn > self.last_turn {
                        return Err((
                            PRECONDITION_FAILED,
                            "names a turn Core has not started".to_string(),
                        ));
                    }
                }
            }
            SessionEvent::RateLimits(_) | SessionEvent::Notice(_) => {}
            SessionEvent::ConfigOptions(options) => {
                self.config = options
                    .config_options
                    .iter()
                    .map(|option| {
                        let values = option.options.iter().map(|v| v.value.clone()).collect();
                        (option.id.clone(), values)
                    })
                    .collect();
            }
            SessionEvent::TurnEnded(ended) => {
                self.require_active(ended.turn)?;
                self.active_turn = None;
                self.pending.retain(|_, (turn, _)| *turn != ended.turn);
            }
        }
        Ok(())
    }

    fn require_active(&self, turn: u64) -> Result<(), (&'static str, String)> {
        if self.active_turn == Some(turn) {
            Ok(())
        } else {
            Err((
                PRECONDITION_FAILED,
                "does not belong to the turn in progress".to_string(),
            ))
        }
    }

    /// Takes the user's answer to a waiting permission request. The option
    /// must be one the request listed.
    pub fn decide(&mut self, permission: &str, option: &str) -> Result<u64, &'static str> {
        let Some((turn, options)) = self.pending.get(permission) else {
            return Err("agents.permissionNotPending");
        };
        if !options.iter().any(|candidate| candidate.id == option) {
            return Err("agents.unknownPermissionOption");
        }
        let turn = *turn;
        self.pending.remove(permission);
        Ok(turn)
    }

    /// Checks a value the user picked for a setting: the setting and the
    /// value must be ones the backend last reported.
    pub fn check_config(&self, option: &str, value: &str) -> Result<(), &'static str> {
        let Some(values) = self.config.get(option) else {
            return Err("agents.unknownConfigOption");
        };
        if values.iter().any(|candidate| candidate == value) {
            Ok(())
        } else {
            Err("agents.unknownConfigValue")
        }
    }
}

const INVALID_PARAMS: &str = lithe_plugin_rpc::codes::HOST_INVALID_PARAMS;
const PRECONDITION_FAILED: &str = lithe_plugin_rpc::codes::HOST_PRECONDITION_FAILED;
const LIMIT_EXCEEDED: &str = lithe_plugin_rpc::codes::HOST_LIMIT_EXCEEDED;

fn check_bounds(event: &SessionEvent) -> Result<(), String> {
    let label = |value: &str, field: &str| {
        if value.is_empty() || value.len() > MAX_LABEL_BYTES {
            Err(format!("has an empty or overlong {field}"))
        } else {
            Ok(())
        }
    };
    let optional_label = |value: &Option<String>, field: &str| match value {
        Some(value) => label(value, field),
        None => Ok(()),
    };
    let text = |value: &str, field: &str| {
        if value.len() > MAX_TEXT_BYTES {
            Err(format!("has a {field} longer than 1 MiB"))
        } else {
            Ok(())
        }
    };
    match event {
        SessionEvent::Session(info) => {
            label(&info.agent_session_id, "agentSessionID")?;
            optional_label(&info.model, "model")?;
            optional_label(&info.effort, "effort")?;
            optional_label(&info.agent_version, "agentVersion")?;
        }
        SessionEvent::Message(message) => {
            label(&message.message_id, "messageID")?;
            text(&message.text, "text")?;
        }
        SessionEvent::ToolCall(call) => {
            label(&call.tool_call_id, "toolCallID")?;
            optional_label(&call.title, "title")?;
            if call.locations.len() > MAX_LOCATIONS {
                return Err("has too many locations".to_string());
            }
            for location in &call.locations {
                text(&location.path, "location path")?;
            }
            if let Some(diff) = &call.diff {
                text(&diff.path, "diff path")?;
                text(diff.old_text.as_deref().unwrap_or_default(), "diff")?;
                text(&diff.new_text, "diff")?;
            }
            text(call.output.as_deref().unwrap_or_default(), "output")?;
        }
        SessionEvent::Plan(plan) => {
            if plan.entries.len() > MAX_PLAN_ENTRIES {
                return Err("has too many plan entries".to_string());
            }
            for entry in &plan.entries {
                text(&entry.content, "plan entry")?;
            }
        }
        SessionEvent::PermissionRequested(request) => {
            label(&request.permission, "permission")?;
            text(&request.title, "title")?;
            optional_label(&request.tool_call_id, "toolCallID")?;
            if request.options.is_empty() || request.options.len() > MAX_PERMISSION_OPTIONS {
                return Err("needs one to eight options".to_string());
            }
            for option in &request.options {
                label(&option.id, "option ID")?;
                label(&option.label, "option label")?;
            }
            let mut ids: Vec<&str> = request.options.iter().map(|o| o.id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            if ids.len() != request.options.len() {
                return Err("repeats an option ID".to_string());
            }
        }
        SessionEvent::Usage(usage) => {
            optional_label(&usage.model, "model")?;
            if usage
                .cost_usd
                .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
            {
                return Err("has an invalid cost".to_string());
            }
        }
        SessionEvent::RateLimits(limits) => check_rate_limits(limits)?,
        SessionEvent::TurnEnded(ended) => {
            text(ended.message.as_deref().unwrap_or_default(), "message")?;
        }
        SessionEvent::Notice(notice) => {
            text(&notice.text, "text")?;
            optional_label(&notice.code, "code")?;
        }
        SessionEvent::ConfigOptions(options) => check_config_options(options)?,
    }
    Ok(())
}

/// Checks the settings a session reports: bounded labels, IDs and values
/// unique, and each current value one of the setting's values.
fn check_config_options(options: &ConfigOptions) -> Result<(), String> {
    let label = |value: &str| !value.is_empty() && value.len() <= MAX_LABEL_BYTES;
    let optional = |value: &Option<String>| value.as_deref().is_none_or(label);
    if options.config_options.len() > MAX_CONFIG_OPTIONS {
        return Err("has more than 16 settings".to_string());
    }
    let mut ids = Vec::with_capacity(options.config_options.len());
    for option in &options.config_options {
        if !label(&option.id)
            || !label(&option.name)
            || !optional(&option.description)
            || !label(&option.current_value)
        {
            return Err("has a setting with an empty or overlong label".to_string());
        }
        if option.options.is_empty() || option.options.len() > MAX_CONFIG_VALUES {
            return Err("has a setting without 1 to 128 values".to_string());
        }
        let mut values = Vec::with_capacity(option.options.len());
        for value in &option.options {
            if !label(&value.value) || !label(&value.name) || !optional(&value.description) {
                return Err("has a setting value with an empty or overlong label".to_string());
            }
            values.push(value.value.as_str());
        }
        if !values.contains(&option.current_value.as_str()) {
            return Err("has a setting whose current value is not one of its values".to_string());
        }
        values.sort_unstable();
        values.dedup();
        if values.len() != option.options.len() {
            return Err("repeats a setting value".to_string());
        }
        ids.push(option.id.as_str());
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != options.config_options.len() {
        return Err("repeats a setting ID".to_string());
    }
    Ok(())
}

/// Checks a rate-limit snapshot, from a session event or a session file scan.
pub(crate) fn check_rate_limits(limits: &RateLimits) -> Result<(), String> {
    if limits.windows.is_empty() || limits.windows.len() > MAX_RATE_WINDOWS {
        return Err("needs one to eight rate-limit windows".to_string());
    }
    if limits
        .plan
        .as_deref()
        .is_some_and(|plan| plan.is_empty() || plan.len() > MAX_LABEL_BYTES)
    {
        return Err("has an empty or overlong plan".to_string());
    }
    for window in &limits.windows {
        if window.id.is_empty()
            || window.id.len() > MAX_LABEL_BYTES
            || window
                .label
                .as_deref()
                .is_some_and(|label| label.is_empty() || label.len() > MAX_LABEL_BYTES)
        {
            return Err("has an empty or overlong window ID or label".to_string());
        }
        if !window.used_percent.is_finite() || !(0.0..=100.0).contains(&window.used_percent) {
            return Err("has a used percentage outside 0 to 100".to_string());
        }
    }
    Ok(())
}

/// One event the host reads: Lithe's own lifecycle events and the events
/// backends report, in the order Core accepted them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HostEvent {
    pub sequence: u64,
    pub session: String,
    pub event: Value,
}

#[derive(Default)]
struct QueueState {
    events: VecDeque<HostEvent>,
    next_sequence: u64,
}

/// The events of every session, for `agents.waitEvents`.
#[derive(Default)]
pub(crate) struct EventQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl EventQueue {
    /// Queues `event` of `session` and wakes a waiting reader.
    pub fn push(&self, session: &str, event: Value) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.next_sequence += 1;
        let sequence = state.next_sequence;
        state.events.push_back(HostEvent {
            sequence,
            session: session.to_string(),
            event,
        });
        while state.events.len() > MAX_QUEUED_EVENTS {
            state.events.pop_front();
        }
        drop(state);
        self.ready.notify_all();
    }

    /// The events after `after`, waiting up to `timeout` for the first one.
    /// `missed` is true when events after `after` were already dropped.
    pub fn wait(&self, after: u64, timeout: Duration, limit: usize) -> Value {
        let deadline = Instant::now() + timeout.min(MAX_WAIT);
        let limit = limit.clamp(1, MAX_EVENTS_PER_WAIT);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.next_sequence <= after {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            state = self
                .ready
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        let oldest = state.events.front().map(|event| event.sequence);
        let missed = oldest.is_some_and(|oldest| oldest > after + 1) && after < state.next_sequence;
        let events: Vec<&HostEvent> = state
            .events
            .iter()
            .filter(|event| event.sequence > after)
            .take(limit)
            .collect();
        let next = events
            .last()
            .map_or(after.min(state.next_sequence), |e| e.sequence);
        json!({ "events": events, "next": next, "missed": missed })
    }
}
