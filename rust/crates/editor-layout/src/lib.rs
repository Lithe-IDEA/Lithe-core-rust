//! How the editor area is divided among its tab groups, and what opening a tab or dropping a
//! dragged one does to the groups and to that division.
//!
//! The layout is a binary tree: a leaf shows one group, and a split lays its two children side
//! by side (`row`) or one over the other. A host keeps the tree together with what its own
//! widgets need, such as the ratio of each splitter, and sends only the shape: groups by ID and
//! splits by a `node` number the host chose. Core never sees ratios, pixels or a drag in
//! progress; a split it makes gets the number the host offered, and the host starts its
//! splitter in the middle.
//!
//! `editor.groupLayout.reduce` runs one transition over the groups and the tree together:
//!
//! * any action of `editor.workbenchState.reduce`, after which a leaf whose group left the
//!   groups leaves the tree and its sibling takes the room;
//! * `openTab`, which opens a tab in the group the host prefers while the tree shows it, else
//!   in the first group of the tree, else in a new group that becomes the whole tree;
//! * `dropTab`, the release of a dragged tab on a place `editor.groupLayout.dropTargets`
//!   offered; a place that no longer fits the groups is a rejection, never a different move;
//! * `removeEmptyGroups`, which drops named groups that have no tab.
//!
//! `editor.groupLayout.dropTargets` says, once per drag, which places of each group accept the
//! dragged tab. The host does the hit testing against what it painted. A group docked in a
//! tool window is never split and is never removed when its last tab leaves; only a dockable
//! tab may join it. Fixtures live in `rust/fixtures/editor/group-layout-v1.json`.

use std::collections::HashSet;

use lithe_core_rules::editor::{reduce, Action, EmptyGroups, Group, SelectionFallback, State};
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upper bound of groups and of layout leaves in one request.
const MAX_GROUPS: usize = 1024;

/// Rejection of a drop whose member or place no longer fits the groups, as when a tab closed
/// or a group went away while the tab was dragged.
pub const STALE: &str = "editor.groupLayout.stale";

/// Rejection of a drop on a place the drop rules refuse.
pub const REFUSED: &str = "editor.groupLayout.refused";

/// The shape of the layout tree.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
enum Layout {
    /// A leaf that shows one group.
    Leaf(Leaf),
    /// A split of the room between two children.
    Split(Split),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Leaf {
    group: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Split {
    /// The host's number for this split, unique in the tree.
    node: u64,
    /// The children are side by side; otherwise the first is over the second.
    #[serde(default)]
    row: bool,
    first: Box<Layout>,
    second: Box<Layout>,
}

impl Layout {
    fn leaf(group: &str) -> Self {
        Layout::Leaf(Leaf {
            group: group.to_string(),
        })
    }

    fn contains(&self, group: &str) -> bool {
        match self {
            Layout::Leaf(leaf) => leaf.group == group,
            Layout::Split(split) => split.first.contains(group) || split.second.contains(group),
        }
    }

    fn has_node(&self, node: u64) -> bool {
        match self {
            Layout::Leaf(_) => false,
            Layout::Split(split) => {
                split.node == node || split.first.has_node(node) || split.second.has_node(node)
            }
        }
    }

    fn first_group(&self) -> &str {
        match self {
            Layout::Leaf(leaf) => &leaf.group,
            Layout::Split(split) => split.first.first_group(),
        }
    }

    /// Checks that groups and split numbers are unique and groups not empty, counting leaves.
    fn check(
        &self,
        groups: &mut HashSet<String>,
        nodes: &mut HashSet<u64>,
    ) -> Result<(), CoreError> {
        match self {
            Layout::Leaf(leaf) => {
                if leaf.group.is_empty() {
                    return Err(invalid("A layout leaf has an empty group"));
                }
                if !groups.insert(leaf.group.clone()) {
                    return Err(invalid("A group is in the layout twice"));
                }
                if groups.len() > MAX_GROUPS {
                    return Err(invalid("The layout has too many groups"));
                }
                Ok(())
            }
            Layout::Split(split) => {
                if !nodes.insert(split.node) {
                    return Err(invalid("A split number is in the layout twice"));
                }
                split.first.check(groups, nodes)?;
                split.second.check(groups, nodes)
            }
        }
    }
}

/// The tree without the leaves `keep` refuses; a split that loses a child is replaced by the
/// other child, and losing every leaf leaves no tree.
fn prune(layout: Option<Layout>, keep: &impl Fn(&str) -> bool) -> Option<Layout> {
    match layout? {
        Layout::Leaf(leaf) => keep(&leaf.group).then_some(Layout::Leaf(leaf)),
        Layout::Split(split) => {
            let Split {
                node,
                row,
                first,
                second,
            } = split;
            match (prune(Some(*first), keep), prune(Some(*second), keep)) {
                (Some(first), Some(second)) => Some(Layout::Split(Split {
                    node,
                    row,
                    first: Box::new(first),
                    second: Box::new(second),
                })),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            }
        }
    }
}

/// The tree with the leaf of `target` replaced by split `node` that holds it and a leaf of
/// `added` on side `edge`.
fn split_leaf(layout: Layout, target: &str, added: &str, edge: Edge, node: u64) -> Layout {
    match layout {
        Layout::Leaf(leaf) if leaf.group == target => {
            let fresh = Box::new(Layout::leaf(added));
            let old = Box::new(Layout::Leaf(leaf));
            let (first, second) = if edge.leading() {
                (fresh, old)
            } else {
                (old, fresh)
            };
            Layout::Split(Split {
                node,
                row: edge.side_by_side(),
                first,
                second,
            })
        }
        Layout::Leaf(leaf) => Layout::Leaf(leaf),
        Layout::Split(split) => Layout::Split(Split {
            node: split.node,
            row: split.row,
            first: Box::new(split_leaf(*split.first, target, added, edge, node)),
            second: Box::new(split_leaf(*split.second, target, added, edge, node)),
        }),
    }
}

/// A side of a group, where a dropped tab splits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    fn side_by_side(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }

    /// The new group goes before the old one: to its left or above it.
    fn leading(self) -> bool {
        matches!(self, Edge::Left | Edge::Top)
    }
}

/// Where a dragged tab is released.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum DropPlace {
    /// Into `group` as its tab at `index`, counted among its tabs without the dragged one.
    Join { group: String, index: i64 },
    /// Into a new group on side `edge` of `group`.
    Split { group: String, edge: Edge },
    /// Into the first group of an editor area that has none.
    Empty,
}

/// The transitions only this command has; every other `kind` is a workbench state action.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum LayoutAction {
    /// Opens `member` and shows it.
    OpenTab {
        member: String,
        /// The group the host would rather use, such as the one a file was last shown in.
        #[serde(default)]
        preferred_group: Option<String>,
        /// The ID of the group made when the layout has none.
        new_group: String,
    },
    /// Releases the dragged `member` on `target`.
    DropTab {
        member: String,
        target: DropPlace,
        #[serde(default)]
        docked: Vec<String>,
        #[serde(default)]
        dockable: bool,
        /// The ID of the group a split or an empty editor makes.
        #[serde(default)]
        new_group: Option<String>,
        /// The number of the split a split makes.
        #[serde(default)]
        new_node: Option<u64>,
    },
    /// Removes the named groups that have no tab; the others stay.
    RemoveEmptyGroups { groups: Vec<String> },
}

const LAYOUT_ACTIONS: [&str; 3] = ["openTab", "dropTab", "removeEmptyGroups"];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReduceRequest {
    #[serde(default)]
    state: State,
    #[serde(default)]
    layout: Option<Layout>,
    action: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReduceResponse {
    state: State,
    /// `null` when the editor area shows no group.
    layout: Option<Layout>,
    /// Groups that left the state, in their previous order.
    removed_group_ids: Vec<String>,
    /// The group the tab is in after `openTab` or `dropTab`.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    /// Why a drop did not apply; the state and the layout are then as sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    rejection: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetsRequest {
    #[serde(default)]
    state: State,
    #[serde(default)]
    layout: Option<Layout>,
    member: String,
    #[serde(default)]
    docked: Vec<String>,
    #[serde(default)]
    dockable: bool,
}

/// Which places of one group accept the dragged tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaceRules {
    /// The strip of tabs: the tab joins at the place it is dropped.
    join: bool,
    /// The edges of the body: the tab splits the group on that side.
    split: bool,
    /// The middle of the body: the tab joins at the end.
    append: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupTargets {
    group: String,
    #[serde(flatten)]
    rules: PlaceRules,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetsResponse {
    /// Every group of the state, in its order.
    groups: Vec<GroupTargets>,
    /// The editor area has no group, and the tab may open its first one.
    empty: bool,
}

/// Handles `editor.groupLayout.reduce`.
///
/// The layout first loses the leaves of groups the state does not have. A rejected drop
/// returns the state and that layout without change.
pub fn reduce_command(payload: Value) -> Result<Value, CoreError> {
    let request: ReduceRequest = serde_json::from_value(payload).map_err(|error| {
        invalid("Invalid editor group layout request").with_details(error.to_string())
    })?;
    check(&request.state, request.layout.as_ref())?;
    let ReduceRequest {
        state,
        layout,
        action,
    } = request;
    let layout = prune(layout, &|group| has_group(&state, group));
    let mut run = Run {
        state,
        layout,
        removed: Vec::new(),
        group: None,
    };
    let kind = action
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !LAYOUT_ACTIONS.contains(&kind) {
        let action: Action = serde_json::from_value(action).map_err(|error| {
            invalid("Invalid editor group layout action").with_details(error.to_string())
        })?;
        run.apply(action);
        return Ok(run.finish(None));
    }
    let action: LayoutAction = serde_json::from_value(action).map_err(|error| {
        invalid("Invalid editor group layout action").with_details(error.to_string())
    })?;
    match action {
        LayoutAction::OpenTab {
            member,
            preferred_group,
            new_group,
        } => run.open_tab(&member, preferred_group.as_deref(), &new_group)?,
        LayoutAction::DropTab {
            member,
            target,
            docked,
            dockable,
            new_group,
            new_node,
        } => {
            if docked.len() > MAX_GROUPS {
                return Err(invalid("Too many docked groups"));
            }
            let drop = Drop {
                member: &member,
                docked: &docked,
                dockable,
                new_group: new_group.as_deref(),
                new_node,
            };
            if let Err(rejection) = run.drop_tab(&drop, target)? {
                return Ok(run.finish(Some(rejection)));
            }
        }
        LayoutAction::RemoveEmptyGroups { groups } => run.remove_empty(&groups),
    }
    Ok(run.finish(None))
}

/// Handles `editor.groupLayout.dropTargets`.
///
/// A tab that is in no group has no place. Otherwise, for each group: a group whose only tab
/// is the dragged one accepts nothing, since dropping it there changes nothing, and neither
/// does a docked group for a tab that is not dockable. Every other group accepts the tab in
/// its strip; its body accepts a split unless it is docked or not in the layout, and accepts
/// the tab at its end unless the tab is already one of its tabs.
pub fn drop_targets_command(payload: Value) -> Result<Value, CoreError> {
    let request: TargetsRequest = serde_json::from_value(payload).map_err(|error| {
        invalid("Invalid editor drop targets request").with_details(error.to_string())
    })?;
    check(&request.state, request.layout.as_ref())?;
    if request.docked.len() > MAX_GROUPS {
        return Err(invalid("Too many docked groups"));
    }
    let TargetsRequest {
        state,
        layout,
        member,
        docked,
        dockable,
    } = request;
    let layout = prune(layout, &|group| has_group(&state, group));
    let open = group_of(&state, &member).is_some();
    let drop = Drop {
        member: &member,
        docked: &docked,
        dockable,
        new_group: None,
        new_node: None,
    };
    let groups = state
        .groups
        .iter()
        .map(|group| GroupTargets {
            group: group.id.clone(),
            rules: if open {
                drop.rules(group, layout.as_ref())
            } else {
                PlaceRules::default()
            },
        })
        .collect();
    Ok(encode(&TargetsResponse {
        groups,
        empty: open && layout.is_none(),
    }))
}

/// A drop's dragged tab and what the host offers for it.
struct Drop<'a> {
    member: &'a str,
    docked: &'a [String],
    dockable: bool,
    new_group: Option<&'a str>,
    new_node: Option<u64>,
}

impl Drop<'_> {
    fn is_docked(&self, group: &str) -> bool {
        self.docked.iter().any(|id| id == group)
    }

    fn rules(&self, group: &Group, layout: Option<&Layout>) -> PlaceRules {
        let docked = self.is_docked(&group.id);
        let has = group.members.iter().any(|id| id == self.member);
        let solo = has && group.members.iter().all(|id| id == self.member);
        let refused = solo || (docked && !self.dockable);
        PlaceRules {
            join: !refused,
            split: !refused && !docked && layout.is_some_and(|layout| layout.contains(&group.id)),
            append: !refused && !has,
        }
    }

    /// What a group the tab leaves does when it is empty: a docked group stays.
    fn source_policy(&self, group: &str) -> EmptyGroups {
        if self.is_docked(group) {
            EmptyGroups::Keep
        } else {
            EmptyGroups::Remove
        }
    }
}

/// One transition in progress: the groups, the layout and what changed.
struct Run {
    state: State,
    layout: Option<Layout>,
    removed: Vec<String>,
    group: Option<String>,
}

impl Run {
    fn apply(&mut self, action: Action) {
        let reduction = reduce(std::mem::take(&mut self.state), action);
        self.state = reduction.state;
        self.removed.extend(reduction.removed_group_ids);
    }

    /// Adds the empty group `id` after the others; it must be new.
    fn add_group(&mut self, id: Option<&str>) -> Result<String, CoreError> {
        let id = id.unwrap_or_default();
        if id.is_empty() || has_group(&self.state, id) {
            return Err(invalid("The new group has no ID or already exists"));
        }
        if self.state.groups.len() >= MAX_GROUPS {
            return Err(invalid("Too many editor groups"));
        }
        self.state.groups.push(Group {
            id: id.to_string(),
            ..Group::default()
        });
        Ok(id.to_string())
    }

    fn open_tab(
        &mut self,
        member: &str,
        preferred: Option<&str>,
        new_group: &str,
    ) -> Result<(), CoreError> {
        if member.is_empty() {
            return Err(invalid("The tab to open has no member"));
        }
        let group = match (&self.layout, preferred) {
            (Some(layout), Some(preferred)) if layout.contains(preferred) => preferred.to_string(),
            (Some(layout), _) => layout.first_group().to_string(),
            (None, _) => {
                let group = self.add_group(Some(new_group))?;
                self.layout = Some(Layout::leaf(&group));
                group
            }
        };
        self.apply(Action::Open {
            group: group.clone(),
            member: member.to_string(),
            activate: true,
        });
        self.group = Some(group);
        Ok(())
    }

    /// Carries a drop out, or names why it does not apply before changing anything.
    fn drop_tab(
        &mut self,
        drop: &Drop,
        target: DropPlace,
    ) -> Result<Result<(), &'static str>, CoreError> {
        let Some(source) = group_of(&self.state, drop.member).map(|group| group.id.clone()) else {
            return Ok(Err(STALE));
        };
        let member = drop.member.to_string();
        let move_to = |to: String, before: Option<String>| Action::Move {
            member: member.clone(),
            from: source.clone(),
            to,
            before,
            after: None,
            activate: true,
            empty_groups: drop.source_policy(&source),
            selection_fallback: SelectionFallback::Adjacent,
        };
        match target {
            DropPlace::Join { group, index } => {
                let Some(dest) = find(&self.state, &group) else {
                    return Ok(Err(STALE));
                };
                if !drop.rules(dest, self.layout.as_ref()).join {
                    return Ok(Err(REFUSED));
                }
                if source == group {
                    let at = dest
                        .members
                        .iter()
                        .position(|id| *id == member)
                        .unwrap_or_default() as i64;
                    let to = index.clamp(0, dest.members.len() as i64 - 1);
                    if at != to {
                        self.apply(Action::Reorder {
                            group: group.clone(),
                            from_index: at,
                            to_index: to,
                        });
                    }
                    self.apply(Action::Activate {
                        group: group.clone(),
                        member: Some(member.clone()),
                    });
                } else {
                    let before = usize::try_from(index.max(0))
                        .ok()
                        .and_then(|index| dest.members.get(index))
                        .cloned();
                    self.apply(move_to(group.clone(), before));
                }
                self.group = Some(group);
            }
            DropPlace::Split { group, edge } => {
                let Some(dest) = find(&self.state, &group) else {
                    return Ok(Err(STALE));
                };
                if !self
                    .layout
                    .as_ref()
                    .is_some_and(|layout| layout.contains(&group))
                {
                    return Ok(Err(STALE));
                }
                if !drop.rules(dest, self.layout.as_ref()).split {
                    return Ok(Err(REFUSED));
                }
                let node = drop
                    .new_node
                    .filter(|node| {
                        !self
                            .layout
                            .as_ref()
                            .is_some_and(|layout| layout.has_node(*node))
                    })
                    .ok_or_else(|| invalid("The new split has no number or one already in use"))?;
                let added = self.add_group(drop.new_group)?;
                self.apply(move_to(added.clone(), None));
                let state = &self.state;
                self.layout = prune(self.layout.take(), &|id| has_group(state, id))
                    .map(|layout| split_leaf(layout, &group, &added, edge, node));
                self.group = Some(added);
            }
            DropPlace::Empty => {
                if self.layout.is_some() {
                    return Ok(Err(STALE));
                }
                let added = self.add_group(drop.new_group)?;
                self.apply(move_to(added.clone(), None));
                self.layout = Some(Layout::leaf(&added));
                self.group = Some(added);
            }
        }
        Ok(Ok(()))
    }

    fn remove_empty(&mut self, named: &[String]) {
        let groups = std::mem::take(&mut self.state.groups);
        for group in groups {
            if group.members.is_empty() && named.contains(&group.id) {
                self.removed.push(group.id);
            } else {
                self.state.groups.push(group);
            }
        }
        if self
            .state
            .focused_group
            .as_ref()
            .is_some_and(|focused| self.removed.contains(focused))
        {
            self.state.focused_group = self.state.groups.first().map(|group| group.id.clone());
        }
    }

    fn finish(self, rejection: Option<&'static str>) -> Value {
        let Run {
            state,
            layout,
            removed,
            group,
        } = self;
        let layout = prune(layout, &|id| has_group(&state, id));
        encode(&ReduceResponse {
            state,
            layout,
            removed_group_ids: removed,
            group,
            rejection,
        })
    }
}

fn check(state: &State, layout: Option<&Layout>) -> Result<(), CoreError> {
    if state.groups.len() > MAX_GROUPS {
        return Err(invalid("Too many editor groups"));
    }
    let mut ids = HashSet::new();
    for group in &state.groups {
        if group.id.is_empty() || !ids.insert(group.id.as_str()) {
            return Err(invalid("An editor group has an empty or repeated ID"));
        }
    }
    if let Some(layout) = layout {
        layout.check(&mut HashSet::new(), &mut HashSet::new())?;
    }
    Ok(())
}

fn has_group(state: &State, id: &str) -> bool {
    find(state, id).is_some()
}

fn find<'a>(state: &'a State, id: &str) -> Option<&'a Group> {
    state.groups.iter().find(|group| group.id == id)
}

fn group_of<'a>(state: &'a State, member: &str) -> Option<&'a Group> {
    state
        .groups
        .iter()
        .find(|group| group.members.iter().any(|id| id == member))
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

fn encode(value: &impl Serialize) -> Value {
    serde_json::to_value(value).expect("editor group layout response encodes")
}
