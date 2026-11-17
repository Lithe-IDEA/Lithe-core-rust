//! Problems tool-window filtering, ordering, grouping, and tree rows
//! (`lsp.problems.query`).
//!
//! Platforms submit the diagnostics they display together with the active
//! view options. Core returns indices into the submitted list, so the large
//! message payload never crosses the boundary a second time. File paths are
//! compared with the IDEA natural file-name order that the Project view uses,
//! which keeps `File2` before `File10` on both platforms.
//!
//! The tool window shows the groups the way IntelliJ IDEA's Problems window
//! does: a node per file (or severity) over its diagnostics. Platforms keep
//! which nodes are collapsed and which row is selected, and ask Core again
//! after a change; Core returns the rows that show and moves a selection that
//! a collapsed node hides to that node. A diagnostic's row key names its file,
//! position, and place among the diagnostics at that position rather than its
//! index, so the selection stays on it when the server republishes the list.

use lithe_core_rules::file_names::FileNameKey;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// LSP severities in display order; unknown (`0`) ranks after hints.
const SEVERITY_RANKS: [u8; 5] = [1, 2, 3, 4, 0];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    diagnostics: Vec<DiagnosticFacts>,
    #[serde(default)]
    group_by: GroupBy,
    #[serde(default)]
    sort_by: SortBy,
    #[serde(default)]
    filter: Filter,
    /// The keys of the group nodes the user closed; unknown keys are ignored.
    #[serde(default)]
    collapsed: Vec<String>,
    /// The key of the selected row; absent or unknown selects nothing.
    #[serde(default)]
    selected: Option<String>,
}

/// One displayed diagnostic. Only the fields that affect filtering and order
/// are submitted; code-action payloads stay with the platform.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiagnosticFacts {
    /// Opaque file identity chosen by the platform, using its native path
    /// equality (for example a case-folded Windows path).
    file_key: String,
    /// Workspace-relative path shown to the user; it decides file order.
    display_path: String,
    /// Zero-based start line.
    line: u64,
    /// Zero-based UTF-16 start column.
    column: u64,
    /// LSP `DiagnosticSeverity` (1–4); missing or out-of-range values are unknown.
    #[serde(default)]
    severity: Option<i64>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum GroupBy {
    #[default]
    File,
    Severity,
    None,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum SortBy {
    /// Severity first, then file, line, and column (IDEA's default).
    #[default]
    Severity,
    /// File, then line and column.
    File,
    /// Line and column across files; file order breaks ties.
    Position,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Filter {
    /// Severities to keep; `None` keeps all. `0` selects unknown severities.
    #[serde(default)]
    severities: Option<Vec<u8>>,
    /// Restricts results to one `fileKey` (the File scope or "current file only").
    #[serde(default)]
    file_key: Option<String>,
    /// Case-insensitive exact match on the diagnostic source.
    #[serde(default)]
    source: Option<String>,
    /// Case-insensitive substring over message, source, code, path, and
    /// the one-based `line:column` label.
    #[serde(default)]
    query: Option<String>,
}

/// Per-severity totals keyed by LSP severity name.
#[derive(Serialize, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct SeverityCounts {
    error: usize,
    warning: usize,
    information: usize,
    hint: usize,
    unknown: usize,
}

impl SeverityCounts {
    fn add(&mut self, severity: u8) {
        match severity {
            1 => self.error += 1,
            2 => self.warning += 1,
            3 => self.information += 1,
            4 => self.hint += 1,
            _ => self.unknown += 1,
        }
    }

    fn highest(&self) -> u8 {
        SEVERITY_RANKS
            .into_iter()
            .find(|severity| self.get(*severity) > 0)
            .unwrap_or(0)
    }

    fn get(&self, severity: u8) -> usize {
        match severity {
            1 => self.error,
            2 => self.warning,
            3 => self.information,
            4 => self.hint,
            _ => self.unknown,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    /// Stable key: `file:<fileKey>`, `severity:<n>`, or `all`.
    key: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_path: Option<String>,
    /// The last segment of a file group's `displayPath`.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// A file group's `displayPath` without its last segment; empty at the
    /// workspace root.
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<u8>,
    /// Indices into the submitted `diagnostics`, in display order.
    indices: Vec<usize>,
    counts: SeverityCounts,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum RowKind {
    Group,
    Diagnostic,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    /// The group's key, or `diagnostic:<line>:<column>:<ordinal>:<fileKey>`
    /// where `ordinal` counts the earlier submitted diagnostics at the same
    /// file and position; stable while nodes open and close.
    key: String,
    kind: RowKind,
    /// 0 for a group node and for a diagnostic of the ungrouped list, 1 for a
    /// diagnostic under a node.
    depth: u8,
    /// Whether a node's diagnostics show; absent on a diagnostic.
    #[serde(skip_serializing_if = "Option::is_none")]
    expanded: Option<bool>,
    /// The row's group.
    group: usize,
    /// The diagnostic the row previews and opens: itself, or the first
    /// diagnostic under a node.
    shows: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSummary {
    file_key: String,
    display_path: String,
    count: usize,
    highest_severity: u8,
    counts: SeverityCounts,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    groups: Vec<Group>,
    /// Files with visible diagnostics in file order, independent of `groupBy`.
    files: Vec<FileSummary>,
    visible_counts: SeverityCounts,
    total_counts: SeverityCounts,
    /// The rows that show, top to bottom; empty when nothing is visible.
    rows: Vec<Row>,
    /// The key of the selected row, always one of `rows`; absent when nothing
    /// is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
    /// The position of the selected row in `rows`.
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_row: Option<usize>,
}

struct Entry<'a> {
    index: usize,
    facts: &'a DiagnosticFacts,
    severity: u8,
}

/// Executes `lsp.problems.query`.
pub fn query_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid Problems query request")
            .with_details(error.to_string())
    })?;
    Ok(serde_json::to_value(query(&request)).expect("Problems query response should encode"))
}

fn query(request: &Request) -> Response {
    let mut total_counts = SeverityCounts::default();
    let query = request
        .filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(str::to_lowercase);
    let source = request.filter.source.as_deref().map(str::to_lowercase);
    let mut visible = Vec::new();
    for (index, facts) in request.diagnostics.iter().enumerate() {
        let severity = normalized_severity(facts.severity);
        total_counts.add(severity);
        if request
            .filter
            .severities
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(&severity))
        {
            continue;
        }
        if request
            .filter
            .file_key
            .as_ref()
            .is_some_and(|key| key != &facts.file_key)
        {
            continue;
        }
        if let Some(source) = &source {
            if facts.source.as_deref().map(str::to_lowercase).as_ref() != Some(source) {
                continue;
            }
        }
        if let Some(query) = &query {
            if !haystack(facts).contains(query.as_str()) {
                continue;
            }
        }
        visible.push(Entry {
            index,
            facts,
            severity,
        });
    }

    // Each file's display key is converted to UTF-16 once, not per comparison.
    let path_keys: BTreeMap<&str, FileNameKey> = visible
        .iter()
        .map(|entry| {
            (
                entry.facts.display_path.as_str(),
                FileNameKey::new(&entry.facts.display_path),
            )
        })
        .collect();
    let compare_files = |left: &DiagnosticFacts, right: &DiagnosticFacts| {
        path_keys[left.display_path.as_str()]
            .cmp(&path_keys[right.display_path.as_str()])
            .then_with(|| left.display_path.cmp(&right.display_path))
            .then_with(|| left.file_key.cmp(&right.file_key))
    };
    let position = |entry: &Entry| (entry.facts.line, entry.facts.column);
    visible.sort_by(|left, right| {
        let order = match request.sort_by {
            SortBy::Severity => severity_rank(left.severity)
                .cmp(&severity_rank(right.severity))
                .then_with(|| compare_files(left.facts, right.facts))
                .then_with(|| position(left).cmp(&position(right))),
            SortBy::File => compare_files(left.facts, right.facts)
                .then_with(|| position(left).cmp(&position(right))),
            SortBy::Position => position(left)
                .cmp(&position(right))
                .then_with(|| compare_files(left.facts, right.facts)),
        };
        // Submission order is the final tie-breaker, so equal facts stay stable.
        order.then_with(|| left.index.cmp(&right.index))
    });

    let mut visible_counts = SeverityCounts::default();
    for entry in &visible {
        visible_counts.add(entry.severity);
    }

    let files = file_groups(&visible, &compare_files);
    let groups = match request.group_by {
        GroupBy::File => files
            .iter()
            .map(|file| {
                let path = file.facts.display_path.as_str();
                let (folder, name) = match path.rfind('/') {
                    Some(at) => (&path[..at], &path[at + 1..]),
                    None => ("", path),
                };
                Group {
                    key: file_group_key(&file.facts.file_key),
                    kind: "file",
                    file_key: Some(file.facts.file_key.clone()),
                    display_path: Some(path.to_string()),
                    name: Some(name.to_string()),
                    folder: Some(folder.to_string()),
                    severity: None,
                    indices: file.indices.clone(),
                    counts: file.counts,
                }
            })
            .collect(),
        GroupBy::Severity => SEVERITY_RANKS
            .into_iter()
            .filter_map(|severity| {
                let indices: Vec<usize> = visible
                    .iter()
                    .filter(|entry| entry.severity == severity)
                    .map(|entry| entry.index)
                    .collect();
                (!indices.is_empty()).then(|| {
                    let mut counts = SeverityCounts::default();
                    indices.iter().for_each(|_| counts.add(severity));
                    Group {
                        key: format!("severity:{severity}"),
                        kind: "severity",
                        file_key: None,
                        display_path: None,
                        name: None,
                        folder: None,
                        severity: Some(severity),
                        indices,
                        counts,
                    }
                })
            })
            .collect(),
        GroupBy::None if visible.is_empty() => Vec::new(),
        GroupBy::None => vec![Group {
            key: "all".into(),
            kind: "all",
            file_key: None,
            display_path: None,
            name: None,
            folder: None,
            severity: None,
            indices: visible.iter().map(|entry| entry.index).collect(),
            counts: visible_counts,
        }],
    };
    let files = files
        .into_iter()
        .map(|file| FileSummary {
            file_key: file.facts.file_key.clone(),
            display_path: file.facts.display_path.clone(),
            count: file.indices.len(),
            highest_severity: file.counts.highest(),
            counts: file.counts,
        })
        .collect();
    let keys = diagnostic_keys(&request.diagnostics);
    let (rows, selected) = tree(request, &groups, &keys);
    let selected_row = selected
        .as_ref()
        .and_then(|key| rows.iter().position(|row| &row.key == key));
    Response {
        groups,
        files,
        visible_counts,
        total_counts,
        rows,
        selected,
        selected_row,
    }
}

/// The rows that show and the key of the selected one. Group nodes head
/// their diagnostics except in the ungrouped list, whose diagnostics sit at
/// the top level. A selected diagnostic that a closed node hides moves to
/// that node; one that is gone moves to its file's node when the list is
/// grouped by file and the file still shows, and is dropped otherwise.
fn tree(request: &Request, groups: &[Group], keys: &[String]) -> (Vec<Row>, Option<String>) {
    let headed = request.group_by != GroupBy::None;
    let collapsed: BTreeSet<&str> = request.collapsed.iter().map(String::as_str).collect();
    let expanded: Vec<bool> = groups
        .iter()
        .map(|group| !headed || !collapsed.contains(group.key.as_str()))
        .collect();
    let mut rows = Vec::new();
    for (slot, group) in groups.iter().enumerate() {
        if headed {
            rows.push(Row {
                key: group.key.clone(),
                kind: RowKind::Group,
                depth: 0,
                expanded: Some(expanded[slot]),
                group: slot,
                shows: group.indices[0],
            });
        }
        if !expanded[slot] {
            continue;
        }
        for index in &group.indices {
            rows.push(Row {
                key: keys[*index].clone(),
                kind: RowKind::Diagnostic,
                depth: u8::from(headed),
                expanded: None,
                group: slot,
                shows: *index,
            });
        }
    }

    let Some(wanted) = request.selected.as_deref() else {
        return (rows, None);
    };
    if headed && groups.iter().any(|group| group.key == wanted) {
        return (rows, Some(wanted.to_string()));
    }
    if let Some(index) = keys.iter().position(|key| key == wanted) {
        if let Some(slot) = groups
            .iter()
            .position(|group| group.indices.contains(&index))
        {
            let shown = if expanded[slot] {
                wanted.to_string()
            } else {
                groups[slot].key.clone()
            };
            return (rows, Some(shown));
        }
    }
    let file = (request.group_by == GroupBy::File)
        .then(|| diagnostic_file(wanted))
        .flatten()
        .map(file_group_key)
        .filter(|key| groups.iter().any(|group| &group.key == key));
    (rows, file)
}

/// Each submitted diagnostic's row key, in submission order.
fn diagnostic_keys(diagnostics: &[DiagnosticFacts]) -> Vec<String> {
    let mut seen = BTreeMap::<(&str, u64, u64), usize>::new();
    diagnostics
        .iter()
        .map(|facts| {
            let ordinal = seen
                .entry((facts.file_key.as_str(), facts.line, facts.column))
                .or_insert(0);
            let key = format!(
                "diagnostic:{}:{}:{}:{}",
                facts.line, facts.column, ordinal, facts.file_key
            );
            *ordinal += 1;
            key
        })
        .collect()
}

/// The `fileKey` a diagnostic row key names, or `None` for another key.
fn diagnostic_file(key: &str) -> Option<&str> {
    let mut parts = key.strip_prefix("diagnostic:")?.splitn(4, ':');
    for _ in 0..3 {
        parts.next()?.parse::<u64>().ok()?;
    }
    parts.next()
}

fn file_group_key(file_key: &str) -> String {
    format!("file:{file_key}")
}

struct FileGroup<'a> {
    /// The first visible diagnostic of the file names the group.
    facts: &'a DiagnosticFacts,
    indices: Vec<usize>,
    counts: SeverityCounts,
}

fn file_groups<'a>(
    visible: &[Entry<'a>],
    compare_files: &dyn Fn(&DiagnosticFacts, &DiagnosticFacts) -> Ordering,
) -> Vec<FileGroup<'a>> {
    let mut positions = BTreeMap::<&str, usize>::new();
    let mut groups: Vec<FileGroup<'a>> = Vec::new();
    for entry in visible {
        let slot = *positions
            .entry(entry.facts.file_key.as_str())
            .or_insert_with(|| {
                groups.push(FileGroup {
                    facts: entry.facts,
                    indices: Vec::new(),
                    counts: SeverityCounts::default(),
                });
                groups.len() - 1
            });
        groups[slot].indices.push(entry.index);
        groups[slot].counts.add(entry.severity);
    }
    groups.sort_by(|left, right| compare_files(left.facts, right.facts));
    groups
}

fn normalized_severity(severity: Option<i64>) -> u8 {
    match severity {
        Some(value @ 1..=4) => value as u8,
        _ => 0,
    }
}

fn severity_rank(severity: u8) -> usize {
    SEVERITY_RANKS
        .iter()
        .position(|candidate| *candidate == severity)
        .unwrap_or(SEVERITY_RANKS.len())
}

fn haystack(facts: &DiagnosticFacts) -> String {
    [
        facts.message.as_str(),
        facts.source.as_deref().unwrap_or(""),
        facts.code.as_deref().unwrap_or(""),
        facts.display_path.as_str(),
        &format!(
            "{}:{}",
            facts.line.saturating_add(1),
            facts.column.saturating_add(1)
        ),
    ]
    .join(" ")
    .to_lowercase()
}
