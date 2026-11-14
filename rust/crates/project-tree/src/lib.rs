//! The rows the project tree shows, and its selection kept in step with them.
//!
//! The host lists folders with `workspace.listDirectories` and keeps the nodes it listed, which
//! folders are open, focus, scrolling and painting. When the shape of the tree changes (a
//! folder opens or closes, a listing arrives, a node goes away) it sends
//! `workspace.projectTree.rows` the loaded nodes in preorder with their depth and whether each
//! folder is open, the selection, and the path of the row at the top of the view. Core answers
//! with the rows to show, the selection repaired, and the row the top one is now; the host asks
//! once per change, never per paint, and nothing here reads the disk. A folder's children are
//! only what the host listed, so opening a folder never enumerates what is under it.
//!
//! A row shows when every folder above it is open; the root, the project folder, always shows.
//! A selected node that went away gives the selection to the nearest folder above it that is
//! still loaded, and a selected node a closed folder hides gives it to the nearest row above it
//! that shows, so the selection is always on a row the user sees. Marks and an anchor whose
//! node went away are dropped; marks inside a closed folder stay. The selection itself follows
//! `lithe-row-selection`. Fixtures live in `rust/fixtures/workspace/project-tree-rows-v1.json`.

use std::collections::HashMap;

use lithe_protocol::{CoreError, ErrorCode};
use lithe_row_selection::{ancestors, targets, Selection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upper bound of nodes in one request.
const MAX_NODES: usize = 1 << 20;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// The loaded nodes in preorder; the first is the root.
    nodes: Vec<Node>,
    #[serde(default)]
    selection: Selection,
    /// The path of the row at the top of the view, which should stay there.
    #[serde(default)]
    top: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Node {
    /// Workspace-relative with `/` separators; empty for the root.
    path: String,
    /// 0 for the root, and one more than the folder the node is in.
    depth: usize,
    /// The node is a folder the user opened.
    #[serde(default)]
    open: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    /// The positions in `nodes` of the rows to show, top to bottom.
    rows: Vec<usize>,
    selection: Selection,
    /// The selected row is not the one the request selected, because that node went away or
    /// a closed folder hides it.
    selection_moved: bool,
    /// The row `top` names, while it shows.
    top_row: Option<usize>,
    /// What a batch acts on, as `lithe-row-selection` says.
    targets: Vec<String>,
}

/// Handles `workspace.projectTree.rows`.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid project tree request")
            .with_details(error.to_string())
    })?;
    request.selection.validate()?;
    let nodes = &request.nodes;
    if nodes.len() > MAX_NODES {
        return Err(invalid("Too many project tree nodes for one request"));
    }
    match nodes.first() {
        Some(root) if root.path.is_empty() && root.depth == 0 => {}
        _ => return Err(invalid("The first project tree node must be the root")),
    }
    let mut position = HashMap::with_capacity(nodes.len());
    let mut shown = vec![false; nodes.len()];
    let mut rows = Vec::new();
    // `folders[d]` is the node at depth `d` above the current one, and `closed` the depth of
    // the shallowest folder above it that is closed.
    let mut folders: Vec<usize> = Vec::new();
    let mut closed: Option<usize> = None;
    for (index, node) in nodes.iter().enumerate() {
        if index > 0 {
            if node.depth == 0 || node.depth > folders.len() {
                return Err(invalid("A project tree node skips a depth"));
            }
            folders.truncate(node.depth);
            let parent = &nodes[folders[node.depth - 1]].path;
            if !is_child(parent, &node.path) {
                return Err(invalid("A project tree node is not in the folder above it"));
            }
        }
        if position.insert(node.path.as_str(), index).is_some() {
            return Err(invalid("A project tree path is listed twice"));
        }
        if closed.is_some_and(|depth| node.depth <= depth) {
            closed = None;
        }
        if closed.is_none() {
            shown[index] = true;
            rows.push(index);
            if !node.open {
                closed = Some(node.depth);
            }
        }
        folders.push(index);
    }

    let loaded = |path: &str| position.contains_key(path);
    let shows = |path: &str| position.get(path).is_some_and(|&index| shown[index]);
    let mut selection = request.selection;
    let before = selection.selected.clone();
    if let Some(path) = &selection.selected {
        if !shows(path) {
            // The root always shows, so there is always one.
            let nearest = ancestors(path)
                .find(|above| shows(above))
                .unwrap_or_default();
            selection.selected = Some(nearest.to_string());
        }
    }
    selection.marked.retain(|path| loaded(path));
    if selection
        .anchor
        .as_deref()
        .is_some_and(|path| !loaded(path))
    {
        selection.anchor = None;
    }
    let row_of: HashMap<usize, usize> = rows
        .iter()
        .enumerate()
        .map(|(row, &node)| (node, row))
        .collect();
    let top_row = request
        .top
        .as_deref()
        .and_then(|path| position.get(path))
        .and_then(|node| row_of.get(node).copied());
    let targets = targets(&selection);
    let selection_moved = selection.selected != before;
    Ok(serde_json::to_value(Response {
        rows,
        selection,
        selection_moved,
        top_row,
        targets,
    })
    .expect("project tree response encodes"))
}

/// Whether `path` names an entry directly inside the folder at `parent`.
fn is_child(parent: &str, path: &str) -> bool {
    let name = if parent.is_empty() {
        path
    } else {
        match path
            .strip_prefix(parent)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            Some(name) => name,
            None => return false,
        }
    };
    !name.is_empty() && !name.contains('/')
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_is_one_name_below_its_folder() {
        assert!(is_child("", "a"));
        assert!(is_child("a", "a/b"));
        assert!(!is_child("a", "ab/c"));
        assert!(!is_child("a", "a/b/c"));
        assert!(!is_child("", "a/b"));
        assert!(!is_child("a", "a/"));
    }
}
