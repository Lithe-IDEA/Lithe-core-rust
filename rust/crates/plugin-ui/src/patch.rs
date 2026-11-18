//! Patches: small edits a backend returns instead of a whole view, so a click
//! on a large canvas or tree does not resend everything. Each patch addresses
//! nodes by ID inside one namespace: the view's host controls, one canvas
//! scene (`canvas`), or one tree's items (`tree`). Core applies the batch in
//! order to a copy of the host's view; the caller checks the patched view.
//!
//! - `set` merges `fields` into a node; a null field removes it.
//! - `replace` swaps a node for `node`.
//! - `remove` deletes a node.
//! - `insert` adds `node` to the children of `parent`, or to the top of the
//!   namespace, at `index` or at the end.

use crate::fields::{at, fail, identifier, integer, object, Checked, Object};
use crate::{MAX_PATCHES, MAX_TREE_DEPTH};
use serde_json::Value;

/// Applies `patches`, the array at `path`, in order to `view`.
pub(crate) fn apply(view: &mut Value, patches: &Value, path: &str) -> Checked<()> {
    let patches = patches
        .as_array()
        .ok_or_else(|| fail(path, "Expected an array of patches."))?;
    if patches.len() > MAX_PATCHES {
        return Err(fail(path, "A result has at most 512 patches."));
    }
    for (index, patch) in patches.iter().enumerate() {
        one(view, patch, &format!("{path}[{index}]"))?;
    }
    Ok(())
}

fn one(view: &mut Value, patch: &Value, path: &str) -> Checked<()> {
    let op = patch.get("op").and_then(Value::as_str).unwrap_or_default();
    let keys: &[&str] = match op {
        "set" => &["op", "canvas", "tree", "id", "fields"],
        "replace" => &["op", "canvas", "tree", "id", "node"],
        "remove" => &["op", "canvas", "tree", "id"],
        "insert" => &["op", "canvas", "tree", "parent", "index", "node"],
        _ => {
            return Err(fail(
                at(path, "op"),
                "Expected one of set, replace, remove, insert.",
            ))
        }
    };
    let map = object(patch, path, keys)?;
    let (top, tree) = namespace(view, map, path)?;
    if op == "insert" {
        let node = required(map, path, "node")?;
        let siblings = match identifier(map, path, "parent", false)? {
            None => top,
            Some(parent) => {
                let (siblings, index) =
                    locate(top, parent).ok_or_else(|| missing(at(path, "parent")))?;
                children(&mut siblings[index], tree)
                    .ok_or_else(|| fail(at(path, "parent"), "The parent holds no children."))?
            }
        };
        let end = siblings.len();
        let index = integer(map, path, "index", (0, end as u64), false)?
            .map_or(end, |index| index as usize);
        siblings.insert(index, node.clone());
        return Ok(());
    }
    let id = identifier(map, path, "id", true)?.unwrap_or_default();
    let (siblings, index) = locate(top, id).ok_or_else(|| missing(at(path, "id")))?;
    match op {
        "remove" => {
            siblings.remove(index);
        }
        "replace" => siblings[index] = required(map, path, "node")?.clone(),
        _ => {
            let fields_path = at(path, "fields");
            let fields = required(map, path, "fields")?
                .as_object()
                .filter(|fields| !fields.is_empty())
                .ok_or_else(|| fail(&fields_path, "Expected an object of fields to change."))?;
            if let Some(key) = fields.keys().find(|key| *key == "type" || *key == "id") {
                return Err(fail(
                    at(&fields_path, key),
                    "A patch keeps a node's type and ID; replace the node instead.",
                ));
            }
            let target = siblings[index]
                .as_object_mut()
                .ok_or_else(|| missing(at(path, "id")))?;
            for (key, value) in fields {
                if value.is_null() {
                    target.shift_remove(key);
                } else {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
    }
    Ok(())
}

/// The top-level nodes of the namespace a patch addresses, and whether it is
/// a tree, whose items may gain children they did not have.
fn namespace<'v>(
    view: &'v mut Value,
    map: &Object,
    path: &str,
) -> Checked<(&'v mut Vec<Value>, bool)> {
    let top = view
        .as_array_mut()
        .ok_or_else(|| fail("view", "Expected an array of view nodes."))?;
    let (key, nodes) = match (map.contains_key("canvas"), map.contains_key("tree")) {
        (false, false) => return Ok((top, false)),
        (true, false) => ("canvas", "nodes"),
        (false, true) => ("tree", "items"),
        (true, true) => {
            return Err(fail(
                path,
                "A patch addresses one canvas or one tree, not both.",
            ))
        }
    };
    let id = identifier(map, path, key, true)?.unwrap_or_default();
    let unknown = || fail(at(path, key), format!("No {key} has this ID."));
    let (siblings, index) = locate(top, id).ok_or_else(unknown)?;
    let node = &mut siblings[index];
    if node.get("type").and_then(Value::as_str) != Some(key) {
        return Err(unknown());
    }
    let items = node
        .get_mut(nodes)
        .and_then(Value::as_array_mut)
        .ok_or_else(unknown)?;
    Ok((items, key == "tree"))
}

/// The array holding the node `id` and its position, searching `children`
/// at most [`MAX_TREE_DEPTH`] levels deep.
fn locate<'v>(top: &'v mut Vec<Value>, id: &str) -> Option<(&'v mut Vec<Value>, usize)> {
    let route = route(top, id, 1)?;
    let (last, parents) = route.split_last()?;
    let mut siblings = top;
    for index in parents {
        siblings = siblings
            .get_mut(*index)?
            .get_mut("children")?
            .as_array_mut()?;
    }
    Some((siblings, *last))
}

fn route(nodes: &[Value], id: &str, depth: usize) -> Option<Vec<usize>> {
    if depth > MAX_TREE_DEPTH {
        return None;
    }
    for (index, node) in nodes.iter().enumerate() {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(vec![index]);
        }
        let children = node.get("children").and_then(Value::as_array);
        if let Some(mut route) = children.and_then(|children| route(children, id, depth + 1)) {
            route.insert(0, index);
            return Some(route);
        }
    }
    None
}

/// The children of an insert parent; tree items gain the array on demand.
fn children(node: &mut Value, tree: bool) -> Option<&mut Vec<Value>> {
    let map = node.as_object_mut()?;
    if tree && !map.contains_key("children") {
        map.insert("children".to_owned(), Value::Array(Vec::new()));
    }
    map.get_mut("children")?.as_array_mut()
}

fn required<'v>(map: &'v Object, path: &str, key: &str) -> Checked<&'v Value> {
    map.get(key).ok_or_else(|| fail(at(path, key), "Required."))
}

fn missing(path: String) -> lithe_plugin_model::PluginRejection {
    fail(
        path,
        "No node has this ID in the view the patches apply to.",
    )
}
