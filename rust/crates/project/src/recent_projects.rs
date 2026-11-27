//! Retention and ordering rules for the recently opened project list.
//!
//! Hosts own persistence, display metadata (names, icons, branches), and the
//! decision of which roots are worth recording at all. Core owns which paths
//! survive, in what order, and how pins interact with the retention capacity,
//! so every platform keeps the same set after the same sequence of actions.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Unpinned projects kept when the caller does not override the capacity.
pub const DEFAULT_RECENT_PROJECT_CAPACITY: usize = 20;
/// Largest capacity a caller may request.
const MAX_CAPACITY: usize = 200;
/// Bounds the stored list a host may hand over in one reduction.
const MAX_ENTRIES: usize = 1024;
/// Bounds one action's path list (removals and imports).
const MAX_ACTION_PATHS: usize = 1024;
/// Upper bound for one path identity in UTF-8 bytes.
const MAX_PATH_BYTES: usize = 4096;
const MAX_TAGS: usize = 16;
const MAX_TAG_BYTES: usize = 64;
const MAX_REMOTE_ID_BYTES: usize = 1024;
// The generated key also includes the length prefix: "remote:1024:" at most.
const MAX_RECENT_KEY_BYTES: usize = "remote:1024:".len() + MAX_REMOTE_ID_BYTES + MAX_PATH_BYTES;

/// One retained project identity. `path` is the host's normalized root path
/// and is compared byte for byte; `lastOpenedAt` is milliseconds since the
/// Unix epoch supplied by the host clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentProjectEntry {
    /// Host-normalized root path, compared byte for byte.
    pub path: String,
    /// Missing timestamps from older stores sort as the oldest entries.
    #[serde(default)]
    pub last_opened_at: i64,
    /// Pinned projects sort first and never count against the capacity.
    #[serde(default)]
    pub pinned: bool,
    /// Explicit pin order. Older entries without an order retain their recency order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_order: Option<u64>,
    /// User-provided category names, in the order they entered them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Reopen information for a remote project. Local projects omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RecentRemoteProject>,
}

/// A remote folder and its reconnect route. Credentials stay in the host's connection store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentRemoteProject {
    /// Stable identity of the full transport chain.
    pub connection_id: String,
    /// Display name of the connection at the time of the last open.
    pub name: String,
    /// Absolute path on the remote host.
    pub folder: String,
    /// Saved SSH connection ID, for SSH and containers on an SSH host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection_id: Option<String>,
    /// Windows distribution and optional account, when using a WSL transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsl: Option<RecentWslRoute>,
    /// Engine and container ID, when the folder is inside a container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<RecentContainerRoute>,
}

/// Host-owned WSL locator. Remote connection validation runs when the host reopens the item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentWslRoute {
    /// Name used by the platform to select a distribution.
    pub distribution: String,
    /// Optional account in that distribution.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
}

/// Host-owned container locator. Remote connection validation runs when the host reopens the item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentContainerRoute {
    /// CLI engine that owns the container.
    pub engine: String,
    /// Container identifier used by the engine.
    pub container: String,
    /// Optional engine context.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    /// Optional account in the container.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
}

/// One user or host action applied to the stored list.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RecentProjectsAction {
    /// The project was opened at `openedAt`; it moves first and keeps its pin.
    Record {
        /// Root path of the opened project.
        path: String,
        /// Open time in milliseconds since the Unix epoch.
        #[serde(rename = "openedAt")]
        opened_at: i64,
    },
    /// Record a successfully opened folder using its transport and remote path as identity.
    RecordRemote {
        /// The folder and reconnect route.
        remote: RecentRemoteProject,
        /// Open time in milliseconds since the Unix epoch.
        #[serde(rename = "openedAt")]
        opened_at: i64,
    },
    /// Replace the tags on an existing project.
    SetTags {
        /// Entry identity returned by an earlier reduction.
        path: String,
        /// Raw labels to trim, validate, and deduplicate.
        tags: Vec<String>,
    },
    /// Projects removed by the user or found unavailable by the host.
    Remove {
        /// Root paths to drop.
        paths: Vec<String>,
    },
    /// Flips the pin of an existing project; unknown paths are ignored.
    TogglePinned {
        /// Root path whose pin flips.
        path: String,
    },
    /// Applies an explicit menu intent, even if its window was showing an older list.
    SetPinned {
        /// Entry identity returned by an earlier reduction.
        path: String,
        /// Desired pin state.
        pinned: bool,
    },
    /// Adds projects discovered elsewhere (for example another IDE) that are
    /// not already listed. The first import is stamped `openedAt` and each
    /// later one a millisecond older, preserving the source order.
    Import {
        /// Discovered root paths in source order.
        paths: Vec<String>,
        /// Time stamped on the first import, in milliseconds since the Unix epoch.
        #[serde(rename = "openedAt")]
        opened_at: i64,
    },
    /// Merge dated local entries from an older client while retaining current metadata.
    ImportEntries {
        /// Imported local project paths and their open times.
        entries: Vec<ImportedRecentProject>,
    },
    /// Project a text and tag filter over the supplied entries, without mutating them.
    Filter {
        /// Case-insensitive substring matched against path, connection name and labels.
        query: String,
        /// Case-insensitive exact category, or empty to include all categories.
        tag: String,
    },
    /// Re-applies ordering, deduplication, and capacity to a loaded list.
    Normalize,
}

/// Local history imported from an earlier client.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedRecentProject {
    /// Host-normalized local project root.
    pub path: String,
    /// Open time recorded by the earlier client.
    pub last_opened_at: i64,
}

/// `workspace.recentProjects.reduce` request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentProjectsRequest {
    /// Stored list before the action.
    pub entries: Vec<RecentProjectEntry>,
    /// Action to apply.
    pub action: RecentProjectsAction,
    /// Unpinned capacity; defaults to [`DEFAULT_RECENT_PROJECT_CAPACITY`].
    #[serde(default)]
    pub capacity: Option<usize>,
}

/// Ordered retained list plus the paths this reduction added or dropped, so
/// hosts can attach or discard their own per-project metadata.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectsResponse {
    /// Retained list: pinned first, then most recently opened.
    pub entries: Vec<RecentProjectEntry>,
    /// Paths this reduction added.
    pub added: Vec<String>,
    /// Paths this reduction dropped.
    pub removed: Vec<String>,
    /// Row offsets in the supplied list, present only for a filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<Vec<usize>>,
    /// Distinct category names across the supplied list, present only for a filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Current display spelling of the requested tag, when that tag still exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_tag: Option<String>,
}

/// JSON entry point for `workspace.recentProjects.reduce`.
pub fn reduce_command(payload: Value) -> Result<Value, CoreError> {
    let request: RecentProjectsRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid recent projects request")
            .with_details(error.to_string())
    })?;
    let response = reduce(request)?;
    Ok(serde_json::to_value(response).expect("recent projects response should encode"))
}

/// Applies one action and returns the retained list, pinned projects first,
/// then by most recent `lastOpenedAt`; equal times keep the caller's order.
pub fn reduce(request: RecentProjectsRequest) -> Result<RecentProjectsResponse, CoreError> {
    let capacity = request.capacity.unwrap_or(DEFAULT_RECENT_PROJECT_CAPACITY);
    if capacity == 0 || capacity > MAX_CAPACITY {
        return Err(invalid("Recent project capacity is out of range"));
    }
    if request.entries.len() > MAX_ENTRIES {
        return Err(invalid("Too many recent projects"));
    }
    for entry in &request.entries {
        validate_time(entry.last_opened_at)?;
        validate_tags(&entry.tags)?;
        if let Some(remote) = &entry.remote {
            validate_remote(remote)?;
            if entry.path != remote_identity(remote) {
                return Err(invalid("Remote project identity does not match its route"));
            }
        } else {
            validate_path(&entry.path)?;
        }
    }

    if let RecentProjectsAction::Filter { query, tag } = request.action {
        return Ok(filter(request.entries, &query, &tag));
    }

    let mut entries = deduplicate(request.entries);
    // Deduplicated paths in the caller's order, used to report removals.
    let original: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
    match request.action {
        RecentProjectsAction::Record { path, opened_at } => {
            validate_path(&path)?;
            validate_time(opened_at)?;
            record(&mut entries, path, opened_at, None);
        }
        RecentProjectsAction::RecordRemote { remote, opened_at } => {
            validate_remote(&remote)?;
            validate_time(opened_at)?;
            let path = remote_identity(&remote);
            record(&mut entries, path, opened_at, Some(remote));
        }
        RecentProjectsAction::SetTags { path, tags } => {
            validate_recent_key(&path)?;
            let tags = normalize_tags(tags)?;
            if let Some(entry) = entries.iter_mut().find(|entry| entry.path == path) {
                entry.tags = tags;
            }
        }
        RecentProjectsAction::Remove { paths } => {
            validate_path_count(&paths)?;
            paths
                .iter()
                .try_for_each(|path| validate_recent_key(path))?;
            let removed: HashSet<&str> = paths.iter().map(String::as_str).collect();
            entries.retain(|entry| !removed.contains(entry.path.as_str()));
        }
        RecentProjectsAction::TogglePinned { path } => {
            validate_recent_key(&path)?;
            if let Some(index) = entries.iter().position(|entry| entry.path == path) {
                let pinned = !entries[index].pinned;
                set_pinned(&mut entries, index, pinned);
            }
        }
        RecentProjectsAction::SetPinned { path, pinned } => {
            validate_recent_key(&path)?;
            if let Some(index) = entries.iter().position(|entry| entry.path == path) {
                set_pinned(&mut entries, index, pinned);
            }
        }
        RecentProjectsAction::Import { paths, opened_at } => {
            validate_path_count(&paths)?;
            paths.iter().try_for_each(|path| validate_path(path))?;
            validate_time(opened_at)?;
            let mut known: HashSet<String> = entries.iter().map(|e| e.path.clone()).collect();
            let mut offset = 0;
            for path in paths {
                if known.insert(path.clone()) {
                    entries.push(RecentProjectEntry {
                        path,
                        last_opened_at: opened_at.saturating_sub(offset).max(0),
                        pinned: false,
                        pin_order: None,
                        tags: Vec::new(),
                        remote: None,
                    });
                    offset += 1;
                }
            }
        }
        RecentProjectsAction::ImportEntries { entries: imported } => {
            if imported.len() > MAX_ACTION_PATHS {
                return Err(invalid("Too many imported recent projects"));
            }
            for entry in imported {
                validate_path(&entry.path)?;
                validate_time(entry.last_opened_at)?;
                if let Some(known) = entries.iter_mut().find(|known| known.path == entry.path) {
                    if known.remote.is_none() {
                        known.last_opened_at = known.last_opened_at.max(entry.last_opened_at);
                    }
                } else {
                    record(&mut entries, entry.path, entry.last_opened_at, None);
                }
            }
        }
        RecentProjectsAction::Normalize => {}
        RecentProjectsAction::Filter { .. } => unreachable!("filter returns before mutation"),
    }

    let retained = order_and_limit(entries, capacity);
    let previous: HashSet<&str> = original.iter().map(String::as_str).collect();
    let kept: HashSet<&str> = retained.iter().map(|entry| entry.path.as_str()).collect();
    let added = retained
        .iter()
        .filter(|entry| !previous.contains(entry.path.as_str()))
        .map(|entry| entry.path.clone())
        .collect();
    let removed = original
        .iter()
        .filter(|path| !kept.contains(path.as_str()))
        .cloned()
        .collect();
    Ok(RecentProjectsResponse {
        entries: retained,
        added,
        removed,
        visible: None,
        tags: None,
        selected_tag: None,
    })
}

fn set_pinned(entries: &mut Vec<RecentProjectEntry>, index: usize, pinned: bool) {
    if entries[index].pinned == pinned {
        return;
    }
    if !pinned {
        entries[index].pinned = false;
        entries[index].pin_order = None;
        return;
    }
    let mut entry = entries.remove(index);
    entry.pinned = true;
    entry.pin_order = Some(
        entries
            .iter()
            .filter_map(|item| item.pin_order)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    );
    entries.insert(0, entry);
}

fn record(
    entries: &mut Vec<RecentProjectEntry>,
    path: String,
    opened_at: i64,
    remote: Option<RecentRemoteProject>,
) {
    let mut entry = match entries.iter().position(|entry| entry.path == path) {
        Some(index) if entries[index].last_opened_at > opened_at => return,
        Some(index) => entries.remove(index),
        None => RecentProjectEntry {
            path,
            last_opened_at: opened_at,
            pinned: false,
            pin_order: None,
            tags: Vec::new(),
            remote: None,
        },
    };
    entry.last_opened_at = opened_at;
    if remote.is_some() {
        entry.remote = remote;
    }
    entries.insert(0, entry);
}

fn filter(entries: Vec<RecentProjectEntry>, query: &str, tag: &str) -> RecentProjectsResponse {
    let query = query.trim().to_lowercase();
    let tag = tag.trim().to_lowercase();
    let mut tags = std::collections::BTreeMap::new();
    let mut visible = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut tag_matches = tag.is_empty();
        let mut query_matches = if let Some(remote) = &entry.remote {
            remote.folder.to_lowercase().contains(&query)
                || remote.name.to_lowercase().contains(&query)
        } else {
            entry.path.to_lowercase().contains(&query)
        };
        for label in &entry.tags {
            let key = label.to_lowercase();
            tag_matches |= key == tag;
            query_matches |= key.contains(&query);
            tags.entry(key).or_insert_with(|| label.clone());
        }
        if tag_matches && query_matches {
            visible.push(index);
        }
    }
    let selected_tag = tags.get(&tag).cloned();
    RecentProjectsResponse {
        entries: Vec::new(),
        added: Vec::new(),
        removed: Vec::new(),
        visible: Some(visible),
        tags: Some(tags.into_values().collect()),
        selected_tag,
    }
}

fn remote_identity(remote: &RecentRemoteProject) -> String {
    // Length-prefix the transport to disambiguate delimiters inside IDs and paths.
    format!(
        "remote:{}:{}{}",
        remote.connection_id.len(),
        remote.connection_id,
        remote.folder
    )
}

fn validate_remote(remote: &RecentRemoteProject) -> Result<(), CoreError> {
    if remote.connection_id.is_empty()
        || remote.connection_id.len() > MAX_REMOTE_ID_BYTES
        || remote.connection_id.contains('\0')
        || remote.name.len() > MAX_REMOTE_ID_BYTES
        || !remote.folder.starts_with('/')
        || (remote.ssh_connection_id.is_none()
            && remote.wsl.is_none()
            && remote.container.is_none())
        || (remote.ssh_connection_id.is_some() && remote.wsl.is_some())
    {
        return Err(invalid("Remote project route is invalid"));
    }
    validate_path(&remote.folder)
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, CoreError> {
    let mut seen = HashSet::new();
    let tags: Vec<String> = tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty() && seen.insert(tag.to_lowercase()))
        .map(str::to_owned)
        .collect();
    validate_tags(&tags)?;
    Ok(tags)
}

fn validate_tags(tags: &[String]) -> Result<(), CoreError> {
    if tags.len() > MAX_TAGS
        || tags.iter().any(|tag| {
            tag.is_empty()
                || tag.len() > MAX_TAG_BYTES
                || tag.contains(',')
                || tag.chars().any(char::is_control)
        })
    {
        return Err(invalid("Recent project tags are invalid"));
    }
    Ok(())
}

/// Keeps the newest record of each path; the first occurrence wins a tie.
fn deduplicate(entries: Vec<RecentProjectEntry>) -> Vec<RecentProjectEntry> {
    let mut result: Vec<RecentProjectEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        match result.iter_mut().find(|kept| kept.path == entry.path) {
            Some(kept) if entry.last_opened_at > kept.last_opened_at => *kept = entry,
            Some(_) => {}
            None => result.push(entry),
        }
    }
    result
}

fn order_and_limit(entries: Vec<RecentProjectEntry>, capacity: usize) -> Vec<RecentProjectEntry> {
    let (mut pinned, mut unpinned): (Vec<_>, Vec<_>) =
        entries.into_iter().partition(|entry| entry.pinned);
    // `sort_by_key` is stable, so equal timestamps keep the caller's order.
    pinned.sort_by_key(|entry| {
        std::cmp::Reverse((
            entry.pin_order,
            entry.pin_order.is_none().then_some(entry.last_opened_at),
        ))
    });
    unpinned.sort_by_key(|entry| std::cmp::Reverse(entry.last_opened_at));
    unpinned.truncate(capacity);
    // Pins are exempt from the capacity but still bounded by the input limit.
    pinned.truncate(MAX_ENTRIES);
    pinned.extend(unpinned);
    pinned
}

fn validate_path_count(paths: &[String]) -> Result<(), CoreError> {
    if paths.len() > MAX_ACTION_PATHS {
        return Err(invalid("Too many recent project paths"));
    }
    Ok(())
}

fn validate_recent_key(path: &str) -> Result<(), CoreError> {
    let limit = if path.starts_with("remote:") {
        MAX_RECENT_KEY_BYTES
    } else {
        MAX_PATH_BYTES
    };
    validate_path_with_limit(path, limit)
}

fn validate_path(path: &str) -> Result<(), CoreError> {
    validate_path_with_limit(path, MAX_PATH_BYTES)
}

fn validate_path_with_limit(path: &str, limit: usize) -> Result<(), CoreError> {
    if path.is_empty() || path.len() > limit || path.contains('\0') {
        return Err(invalid("Recent project path is invalid"));
    }
    Ok(())
}

fn validate_time(value: i64) -> Result<(), CoreError> {
    if value < 0 {
        return Err(invalid("Recent project time is invalid"));
    }
    Ok(())
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}
