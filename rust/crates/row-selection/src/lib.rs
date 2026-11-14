//! The multi-selection of rows that show workspace paths, such as the project tree and the
//! changes of a change list.
//!
//! A selection has three parts: the selected row, which the keyboard moves and a plain click
//! sets; the rows a Cmd or Ctrl click or a Shift click marked for a batch; and the anchor, the
//! row a Shift click extends from. A Cmd or Ctrl click marks or unmarks one row and makes it
//! the anchor. A Shift click marks every row from the anchor, or from the selected row when
//! there is no anchor, to the clicked one, and leaves the marks it does not cover. A plain
//! click selects its row and drops the marks and the anchor. The empty path is the workspace
//! root, which is never marked: a batch on the whole workspace is never what a click meant.
//!
//! The host keeps focus, painting and what a batch does; it sends the paths of the rows it
//! shows, top to bottom, with the selection and one action, and keeps the selection that comes
//! back. Marks of rows the host does not show, such as rows inside a closed folder, stay.
//! [`targets`] is what a batch acts on. Fixtures live in
//! `rust/fixtures/workspace/row-selection-v1.json`.

use std::collections::BTreeSet;

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upper bound of rows and of marks in one request.
pub const MAX_ROWS: usize = 1 << 20;

/// Rejection of an action on a path that is not one of the rows, as when the row went away
/// between the paint that showed it and the click.
pub const STALE: &str = "workspace.rowSelection.stale";

/// The selected row, the marked rows and the anchor, by path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    /// The path of the selected row; the empty path is the workspace root.
    #[serde(default)]
    pub selected: Option<String>,
    /// The paths marked for a batch, in path order; never the empty path.
    #[serde(default)]
    pub marked: BTreeSet<String>,
    /// The path a Shift click extends from; never the empty path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

impl Selection {
    /// Rejects marks and an anchor on the workspace root, and more marks than [`MAX_ROWS`].
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.marked.len() > MAX_ROWS {
            return Err(invalid("Too many marked rows for one request"));
        }
        if self.marked.contains("") {
            return Err(invalid("The workspace root cannot be marked"));
        }
        if self.anchor.as_deref() == Some("") {
            return Err(invalid("The workspace root cannot be the anchor"));
        }
        Ok(())
    }
}

/// What a batch acts on: the marked paths in path order, without those inside a marked folder,
/// since the folder stands for its contents. It is empty when nothing is marked; a host then
/// acts on the selected row alone.
pub fn targets(selection: &Selection) -> Vec<String> {
    selection
        .marked
        .iter()
        .filter(|path| !ancestors(path).any(|above| selection.marked.contains(above)))
        .cloned()
        .collect()
}

/// The paths above `path`, nearest first, ending with the workspace root.
pub fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    let mut rest = (!path.is_empty()).then_some(path);
    std::iter::from_fn(move || {
        let current = rest?;
        let above = current.rfind('/').map_or("", |slash| &current[..slash]);
        rest = (!above.is_empty()).then_some(above);
        Some(above)
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// The paths of the rows the host shows, top to bottom.
    rows: Vec<String>,
    #[serde(default)]
    selection: Selection,
    action: Action,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Action {
    /// A plain click on the row of `path`.
    Choose { path: String },
    /// A Cmd or Ctrl click on the row of `path`.
    Toggle { path: String },
    /// A Shift click on the row of `path`.
    Extend { path: String },
    /// The batch is over: the marks and the anchor go and the selected row stays.
    Clear,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    selection: Selection,
    /// What a batch acts on, as [`targets`] says.
    targets: Vec<String>,
    /// Why the action did not apply; the selection is then as sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    rejection: Option<&'static str>,
}

/// Handles `workspace.rowSelection.reduce`.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid row selection request")
            .with_details(error.to_string())
    })?;
    if request.rows.len() > MAX_ROWS {
        return Err(invalid("Too many rows for one request"));
    }
    request.selection.validate()?;
    let mut selection = request.selection;
    let rejection = apply(&request.rows, &mut selection, request.action);
    let targets = targets(&selection);
    Ok(serde_json::to_value(Response {
        selection,
        targets,
        rejection,
    })
    .expect("row selection response encodes"))
}

/// Applies `action` to `selection` over `rows`, or says why it does not apply.
fn apply(rows: &[String], selection: &mut Selection, action: Action) -> Option<&'static str> {
    let row = |path: &str| rows.iter().position(|row| row == path);
    match action {
        Action::Clear => {
            selection.marked.clear();
            selection.anchor = None;
        }
        Action::Choose { path } => {
            if row(&path).is_none() {
                return Some(STALE);
            }
            selection.marked.clear();
            selection.anchor = None;
            selection.selected = Some(path);
        }
        Action::Toggle { path } => {
            if row(&path).is_none() {
                return Some(STALE);
            }
            if path.is_empty() {
                return None;
            }
            if !selection.marked.remove(&path) {
                selection.marked.insert(path.clone());
            }
            selection.anchor = Some(path.clone());
            selection.selected = Some(path);
        }
        Action::Extend { path } => {
            let Some(at) = row(&path) else {
                return Some(STALE);
            };
            if path.is_empty() {
                return None;
            }
            let from = [selection.anchor.as_deref(), selection.selected.as_deref()]
                .into_iter()
                .flatten()
                .find_map(row)
                .unwrap_or(at);
            let (low, high) = (from.min(at), from.max(at));
            selection.marked.extend(
                rows[low..=high]
                    .iter()
                    .filter(|row| !row.is_empty())
                    .cloned(),
            );
            selection.selected = Some(path);
        }
    }
    None
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_end_with_the_root() {
        assert_eq!(ancestors("a/b/c").collect::<Vec<_>>(), ["a/b", "a", ""]);
        assert_eq!(ancestors("a").collect::<Vec<_>>(), [""]);
        assert_eq!(ancestors("").count(), 0);
    }

    #[test]
    fn a_marked_folder_stands_for_its_contents() {
        let selection = Selection {
            marked: ["a", "a-b", "a/b", "c/d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ..Selection::default()
        };
        assert_eq!(targets(&selection), ["a", "a-b", "c/d"]);
    }
}
