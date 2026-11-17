//! References tool-window grouping, tree rows, and usage lines
//! (`lsp.references.query`, `lsp.references.lines`).
//!
//! Find Usages answers with a flat list of locations. The References tool
//! window shows them the way IntelliJ IDEA's Find tool window does: a Usages
//! node over one node per file, each over its usages. Platforms keep which
//! nodes are collapsed and which row is selected, and ask Core again after a
//! change; Core returns the groups, the visible rows with the usage each row
//! previews and the usages before and after it, and moves a selection that a
//! collapsed node hides to the nearest row that shows.
//!
//! Files are ordered with the IDEA natural file-name order that the Project
//! view and the Problems tool window use; the usages of a file by line and
//! column. Every submitted location is kept: none is filtered or merged with
//! another on the same line.
//!
//! Platforms read the usages' lines themselves (from an open editor or the
//! disk) and submit them to `lsp.references.lines`, which shows them with the
//! rule search results use ([`lithe_text::line_preview`]).

use lithe_core_rules::file_names::FileNameKey;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_text::{line_preview, LinePreview, Utf16Span};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Bytes a usage's line keeps after it was cut in front of the usage.
pub const USAGE_LINE_MAX_BYTES: usize = 240;

/// The key of the Usages node.
const ROOT_KEY: &str = "root";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryRequest {
    locations: Vec<LocationFacts>,
    /// Hides every file under the Usages node.
    #[serde(default)]
    root_collapsed: bool,
    /// The `fileKey`s of the file nodes the user closed; unknown keys are ignored.
    #[serde(default)]
    collapsed: Vec<String>,
    /// The key of the selected row; absent or unknown selects the first usage.
    #[serde(default)]
    selected: Option<String>,
}

/// One location of the answer.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocationFacts {
    /// Opaque file identity chosen by the platform with its native path
    /// equality, or the URI of a location that is not a local file.
    file_key: String,
    /// The path shown to the user, with `/` separators: relative to the
    /// workspace for a file in it. It decides the file order.
    display_path: String,
    /// Zero-based start line.
    line: u64,
    /// Zero-based UTF-16 start column.
    column: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    /// The key of the file's node, `file:<fileKey>`.
    key: String,
    file_key: String,
    display_path: String,
    /// The last segment of `displayPath`.
    name: String,
    /// `displayPath` without its last segment; empty at the workspace root.
    folder: String,
    /// Indices into the submitted `locations`, in display order.
    indices: Vec<usize>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum RowKind {
    Root,
    File,
    Usage,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    /// `root`, `file:<fileKey>`, or `usage:<index>`; stable while nodes open
    /// and close.
    key: String,
    kind: RowKind,
    /// 0 for the Usages node, 1 for a file, 2 for a usage.
    depth: u8,
    /// Whether a node's children show; absent on a usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    expanded: Option<bool>,
    /// The group of a file or usage row.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<usize>,
    /// The location the row previews and opens: the usage itself, or the
    /// first usage under a node.
    shows: usize,
    /// The usage before the row in tree order, counting usages under closed
    /// nodes; a node sits just before its first usage. Absent at the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    previous: Option<usize>,
    /// The usage after the row in the same order; absent at the end.
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryResponse {
    /// Files in natural path order.
    groups: Vec<Group>,
    /// The rows that show, top to bottom; empty for an empty answer.
    rows: Vec<Row>,
    /// The key of the selected row, always one of `rows`; absent when `rows` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
    /// The position of the selected row in `rows`.
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_row: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinesRequest {
    /// The distinct line texts, so a long line with several usages crosses
    /// the boundary once.
    texts: Vec<String>,
    usages: Vec<UsageLine>,
}

/// One usage and the line its range starts on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UsageLine {
    /// Index into `texts` of the usage's start line.
    text: usize,
    line: u64,
    column: u64,
    end_line: u64,
    end_column: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinesResponse {
    /// One preview per submitted usage, in the same order.
    lines: Vec<LinePreview>,
}

/// Executes `lsp.references.query`.
pub fn query_command(payload: Value) -> Result<Value, CoreError> {
    let request: QueryRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid References query request",
        )
        .with_details(error.to_string())
    })?;
    Ok(serde_json::to_value(query(&request)).expect("References query response should encode"))
}

/// Executes `lsp.references.lines`.
pub fn lines_command(payload: Value) -> Result<Value, CoreError> {
    let request: LinesRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid References lines request",
        )
        .with_details(error.to_string())
    })?;
    let lines = request
        .usages
        .iter()
        .map(|usage| {
            let text = request.texts.get(usage.text).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid References lines request",
                )
                .with_details(format!("usage text {} is not in texts", usage.text))
            })?;
            Ok(usage_line(text, usage))
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(serde_json::to_value(LinesResponse { lines })
        .expect("References lines response should encode"))
}

fn query(request: &QueryRequest) -> QueryResponse {
    let groups = groups(&request.locations);
    if groups.is_empty() {
        return QueryResponse {
            groups,
            rows: Vec::new(),
            selected: None,
            selected_row: None,
        };
    }
    let collapsed: BTreeSet<&str> = request.collapsed.iter().map(String::as_str).collect();
    let expanded: Vec<bool> = groups
        .iter()
        .map(|group| !collapsed.contains(group.file_key.as_str()))
        .collect();

    // Usages in tree order, and where each group's usages start in it.
    let order: Vec<usize> = groups
        .iter()
        .flat_map(|group| group.indices.iter().copied())
        .collect();
    let mut starts = Vec::with_capacity(groups.len());
    let mut position_of = vec![0; request.locations.len()];
    let mut slot_of = vec![0; request.locations.len()];
    let mut count = 0;
    for (slot, group) in groups.iter().enumerate() {
        starts.push(count);
        for index in &group.indices {
            position_of[*index] = count;
            slot_of[*index] = slot;
            count += 1;
        }
    }
    let previous = |at: usize| at.checked_sub(1).map(|at| order[at]);
    let next = |at: usize| order.get(at).copied();

    let mut rows = vec![Row {
        key: ROOT_KEY.into(),
        kind: RowKind::Root,
        depth: 0,
        expanded: Some(!request.root_collapsed),
        group: None,
        shows: order[0],
        previous: None,
        next: next(0),
    }];
    if !request.root_collapsed {
        for (slot, group) in groups.iter().enumerate() {
            let start = starts[slot];
            rows.push(Row {
                key: group.key.clone(),
                kind: RowKind::File,
                depth: 1,
                expanded: Some(expanded[slot]),
                group: Some(slot),
                shows: group.indices[0],
                previous: previous(start),
                next: next(start),
            });
            if !expanded[slot] {
                continue;
            }
            for index in &group.indices {
                let at = position_of[*index];
                rows.push(Row {
                    key: usage_key(*index),
                    kind: RowKind::Usage,
                    depth: 2,
                    expanded: None,
                    group: Some(slot),
                    shows: *index,
                    previous: previous(at),
                    next: next(at + 1),
                });
            }
        }
    }

    // A hidden selection moves to the nearest node that shows: a usage's file,
    // or the Usages node when it is closed.
    let wanted = request
        .selected
        .as_deref()
        .and_then(|key| parse_key(key, &groups, request.locations.len()))
        .unwrap_or(Selection::Usage(order[0]));
    let selection = match wanted {
        Selection::Usage(index) => {
            let slot = slot_of[index];
            if request.root_collapsed {
                ROOT_KEY.to_string()
            } else if !expanded[slot] {
                groups[slot].key.clone()
            } else {
                usage_key(index)
            }
        }
        Selection::File(slot) if !request.root_collapsed => groups[slot].key.clone(),
        Selection::File(_) | Selection::Root => ROOT_KEY.to_string(),
    };
    let selected_row = rows.iter().position(|row| row.key == selection);
    QueryResponse {
        groups,
        rows,
        selected: Some(selection),
        selected_row,
    }
}

/// The locations grouped by file, files in natural path order and usages by
/// position, with the submission order as the final tie-breaker.
fn groups(locations: &[LocationFacts]) -> Vec<Group> {
    let mut slots = BTreeMap::<&str, usize>::new();
    let mut groups: Vec<(&LocationFacts, Vec<usize>)> = Vec::new();
    for (index, location) in locations.iter().enumerate() {
        let slot = *slots.entry(location.file_key.as_str()).or_insert_with(|| {
            groups.push((location, Vec::new()));
            groups.len() - 1
        });
        groups[slot].1.push(index);
    }
    // Each file's display key is converted to UTF-16 once, not per comparison.
    let keys: Vec<FileNameKey> = groups
        .iter()
        .map(|(first, _)| FileNameKey::new(&first.display_path))
        .collect();
    let mut sorted: Vec<usize> = (0..groups.len()).collect();
    sorted.sort_by(|left, right| {
        let (left_first, right_first) = (groups[*left].0, groups[*right].0);
        keys[*left]
            .cmp(&keys[*right])
            .then_with(|| left_first.display_path.cmp(&right_first.display_path))
            .then_with(|| left_first.file_key.cmp(&right_first.file_key))
    });
    sorted
        .into_iter()
        .map(|slot| {
            let (first, indices) = &groups[slot];
            let mut indices = indices.clone();
            indices.sort_by_key(|index| (locations[*index].line, locations[*index].column, *index));
            let (folder, name) = match first.display_path.rfind('/') {
                Some(at) => (&first.display_path[..at], &first.display_path[at + 1..]),
                None => ("", first.display_path.as_str()),
            };
            Group {
                key: format!("file:{}", first.file_key),
                file_key: first.file_key.clone(),
                display_path: first.display_path.clone(),
                name: name.into(),
                folder: folder.into(),
                indices,
            }
        })
        .collect()
}

enum Selection {
    Root,
    File(usize),
    Usage(usize),
}

/// The row a submitted key names, or `None` when it names nothing in this answer.
fn parse_key(key: &str, groups: &[Group], locations: usize) -> Option<Selection> {
    if key == ROOT_KEY {
        return Some(Selection::Root);
    }
    if let Some(index) = key.strip_prefix("usage:") {
        return index
            .parse::<usize>()
            .ok()
            .filter(|index| *index < locations)
            .map(Selection::Usage);
    }
    groups
        .iter()
        .position(|group| group.key == key)
        .map(Selection::File)
}

fn usage_key(index: usize) -> String {
    format!("usage:{index}")
}

/// The preview of a usage's line: the usage is highlighted from its start to
/// its end, or to the end of the line when it runs onto later lines.
fn usage_line(text: &str, usage: &UsageLine) -> LinePreview {
    let width = text.trim_end_matches(['\r', '\n']).encode_utf16().count() as u64;
    let start = usage.column.min(width);
    let end = if usage.end_line == usage.line {
        usage.end_column.min(width)
    } else if usage.end_line > usage.line {
        width
    } else {
        start
    };
    let highlights = if end > start {
        vec![Utf16Span {
            start: start as usize,
            length: (end - start) as usize,
        }]
    } else {
        Vec::new()
    };
    line_preview(text, &highlights, Some(USAGE_LINE_MAX_BYTES))
}
