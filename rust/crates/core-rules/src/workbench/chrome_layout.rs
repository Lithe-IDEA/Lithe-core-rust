//! Placement rules for the rearrangeable buttons of the workbench chrome.
//!
//! Three surfaces of the workbench carry buttons a user may drag: the tool stripes on both window
//! edges (six zones, one for each tool window anchor of the IDEA new UI: Left Top, Left Bottom and
//! Bottom Left on the left stripe, Right Top, Right Bottom and Bottom Right on the right one), the
//! trailing end of the title bar and the trailing end of the status bar. Every host persists the
//! same fact: the zone of each button, the order inside each zone, which stripe buttons the user
//! hid, and which tool panels are open.
//! This module owns that fact and every transition of it. The host keeps only what exists while a
//! gesture runs (pointer, drag ghost, insertion marker) and sends one [`Action`] when the gesture
//! ends.
//!
//! The host describes what it can show with a catalog of [`CatalogItem`]s. The reducer never
//! invents items: it drops saved placements of items the catalog no longer lists, and puts catalog
//! items the saved state does not mention yet at the end of their default zone, so a saved layout
//! survives added and removed buttons. Items never leave their surface: a stripe button cannot move
//! into the status bar. Fixed chrome, such as the status bar path or the project and branch
//! switchers, is not part of the model.
//!
//! Each stripe zone owns one panel slot, so at most one panel item per zone is open. Opening a
//! panel closes the other open panel of its zone, and moving an open panel takes its open state
//! into the target zone. Which part of the window shows a zone's panel is presentation and stays
//! in the host. The names of the left and right zones are easy to mix up, so the pairs are spelled
//! out: `LeftBottom` is the lower group of the stripe whose panel opens in the lower half of the
//! left column, while `BottomLeft` is the group packed at the stripe's bottom whose panel opens in
//! the left half of the bottom area, and likewise `RightBottom` and `BottomRight` on the right.
//!
//! A user may hide a tool stripe button, as IDEA's Hide Entry does. A hidden item keeps its zone
//! and its place in the zone order, so showing it again puts the button back where it was. Hiding
//! closes the item's panel; a host may still open the panel of a hidden item, from a shortcut or a
//! command, and the item stays hidden. Where the host lists hidden items for the user to restore
//! is presentation. Title bar and status bar items cannot be hidden.

use crate::{RuleError, RuleErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Command name of the reducer in the Core command namespace.
pub const REDUCE_CHROME_LAYOUT: &str = "workbench.chromeLayout.reduce";

/// Part of the window chrome whose buttons belong together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Surface {
    /// The two tool stripes on the window edges.
    ToolStripe,
    /// The trailing end of the title bar.
    TitleBar,
    /// The trailing end of the status bar.
    StatusBar,
}

/// One place a button can sit in.
///
/// Zones are listed in the canonical order the reducer emits placements in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Zone {
    /// Upper group of the left stripe; its panel opens in the left column, in the top half when
    /// `LeftBottom` also has a panel open.
    LeftTop,
    /// Group of the left stripe directly under `LeftTop`; its panel opens in the lower half of the
    /// left column. This is IDEA's Left Bottom anchor, not `BottomLeft`.
    LeftBottom,
    /// Group packed at the bottom of the left stripe; its panel opens in the left half of the
    /// bottom area. This is IDEA's Bottom Left anchor, not `LeftBottom`.
    BottomLeft,
    /// Upper group of the right stripe; its panel opens in the right column, in the top half when
    /// `RightBottom` also has a panel open.
    RightTop,
    /// Group of the right stripe directly under `RightTop`; its panel opens in the lower half of
    /// the right column. This is IDEA's Right Bottom anchor, not `BottomRight`.
    RightBottom,
    /// Group packed at the bottom of the right stripe; its panel opens in the right half of the
    /// bottom area. This is IDEA's Bottom Right anchor, not `RightBottom`.
    BottomRight,
    /// Trailing end of the title bar.
    TitleTrailing,
    /// Trailing end of the status bar.
    StatusTrailing,
}

const ZONE_COUNT: usize = 8;

const ZONES: [Zone; ZONE_COUNT] = [
    Zone::LeftTop,
    Zone::LeftBottom,
    Zone::BottomLeft,
    Zone::RightTop,
    Zone::RightBottom,
    Zone::BottomRight,
    Zone::TitleTrailing,
    Zone::StatusTrailing,
];

impl Zone {
    /// Surface this zone belongs to.
    pub fn surface(self) -> Surface {
        match self {
            Zone::LeftTop
            | Zone::LeftBottom
            | Zone::BottomLeft
            | Zone::RightTop
            | Zone::RightBottom
            | Zone::BottomRight => Surface::ToolStripe,
            Zone::TitleTrailing => Surface::TitleBar,
            Zone::StatusTrailing => Surface::StatusBar,
        }
    }

    /// Position of this zone in [`ZONES`], which indexes the per-zone arrays of a layout.
    fn rank(self) -> usize {
        match self {
            Zone::LeftTop => 0,
            Zone::LeftBottom => 1,
            Zone::BottomLeft => 2,
            Zone::RightTop => 3,
            Zone::RightBottom => 4,
            Zone::BottomRight => 5,
            Zone::TitleTrailing => 6,
            Zone::StatusTrailing => 7,
        }
    }
}

/// One button the host is able to show.
///
/// The catalog order is the default order inside each zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogItem {
    /// Stable host identifier, persisted in placements.
    pub id: String,
    /// Zone of the default layout. Its surface is the only surface the item may sit in.
    pub default_zone: Zone,
    /// The button opens a tool panel in its zone's panel slot. Only stripe items can.
    #[serde(default)]
    pub panel: bool,
    /// The panel is open in the default layout. Requires `panel`.
    #[serde(default)]
    pub open_by_default: bool,
}

/// Zone of one item. The order of placements with the same zone is the display order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Placement {
    /// Catalog identifier.
    pub id: String,
    /// Zone the item sits in.
    pub zone: Zone,
}

/// The persisted chrome layout.
///
/// A state returned by the reducer lists every catalog item exactly once, zone by zone in
/// [`Zone`] order, `open` names at most one panel item per zone, also in zone order, and `hidden`
/// names tool stripe items only, in the order of `placements`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct State {
    /// Every item with its zone.
    #[serde(default)]
    pub placements: Vec<Placement>,
    /// Panel items whose panel is shown.
    #[serde(default)]
    pub open: Vec<String>,
    /// Tool stripe items whose button the user hid. They keep their placement; a state saved
    /// before hiding existed has none.
    #[serde(default)]
    pub hidden: Vec<String>,
}

/// One state transition requested by a host.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Repairs a saved state against the catalog without changing anything else.
    Normalize,
    /// Replaces the state with the default layout of the catalog. Hosts without a saved layout
    /// start here.
    Reset,
    /// Moves `id` into `zone`, in front of `before` when that item sits in `zone`, otherwise at the
    /// end. Moving an item in front of itself keeps its position.
    Move {
        /// Item to move.
        id: String,
        /// Target zone; must belong to the item's surface.
        zone: Zone,
        /// Item the moved one is inserted in front of.
        #[serde(default)]
        before: Option<String>,
    },
    /// Opens the panel of `id` when it is closed and closes it when it is open.
    Toggle {
        /// Panel item.
        id: String,
    },
    /// Opens or closes the panel of `id`.
    SetOpen {
        /// Panel item.
        id: String,
        /// Desired state.
        open: bool,
    },
    /// Hides or shows the button of the tool stripe item `id`. Hiding also closes its panel;
    /// showing leaves the panel as it is.
    SetHidden {
        /// Tool stripe item.
        id: String,
        /// Desired state.
        hidden: bool,
    },
}

/// Outcome of one reduction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reduction {
    /// The complete normalized state.
    pub state: State,
    /// `state` differs from the state the host sent, so the host should persist it.
    pub changed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    catalog: Vec<CatalogItem>,
    #[serde(default)]
    state: State,
    action: Action,
}

/// Decodes a JSON payload `{catalog, state?, action}` and returns the encoded [`Reduction`].
pub fn reduce_json(payload: Value) -> Result<Value, RuleError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        RuleError::new(
            RuleErrorCode::InvalidRequest,
            "Invalid chrome layout request",
        )
        .with_details(error.to_string())
    })?;
    let reduction = reduce(&request.catalog, request.state, request.action)?;
    serde_json::to_value(reduction).map_err(|error| {
        RuleError::new(
            RuleErrorCode::InvalidRequest,
            "Chrome layout could not be encoded",
        )
        .with_details(error.to_string())
    })
}

/// Applies `action` to `state` after repairing `state` against `catalog`.
///
/// Fails with `invalid_request` when the catalog is malformed (empty or repeated identifiers, a
/// panel outside the stripes, a default-open item that is not a panel), or when the action names
/// an unknown item, moves an item to another surface, opens an item that is not a panel, or hides
/// an item that is not on a tool stripe.
pub fn reduce(
    catalog: &[CatalogItem],
    state: State,
    action: Action,
) -> Result<Reduction, RuleError> {
    let index = catalog_index(catalog)?;
    let mut layout = match action {
        Action::Reset => Layout::defaults(catalog),
        _ => Layout::repaired(catalog, &index, &state),
    };
    match action {
        Action::Normalize | Action::Reset => {}
        Action::Move { id, zone, before } => {
            let item = lookup(&index, &id)?;
            let target = before.and_then(|before| index.get(before.as_str()).copied());
            layout.move_item(catalog, item, zone, target)?;
        }
        Action::Toggle { id } => {
            let item = lookup(&index, &id)?;
            let open = !layout.is_open(item);
            layout.set_open(catalog, item, open)?;
        }
        Action::SetOpen { id, open } => {
            let item = lookup(&index, &id)?;
            layout.set_open(catalog, item, open)?;
        }
        Action::SetHidden { id, hidden } => {
            let item = lookup(&index, &id)?;
            layout.set_hidden(catalog, item, hidden)?;
        }
    }
    let next = layout.into_state(catalog);
    Ok(Reduction {
        changed: next != state,
        state: next,
    })
}

fn invalid(message: &str) -> RuleError {
    RuleError::new(RuleErrorCode::InvalidRequest, message)
}

fn catalog_index(catalog: &[CatalogItem]) -> Result<HashMap<&str, usize>, RuleError> {
    let mut index = HashMap::with_capacity(catalog.len());
    for (position, item) in catalog.iter().enumerate() {
        if item.id.is_empty() {
            return Err(invalid("Chrome catalog item has an empty id"));
        }
        if item.panel && item.default_zone.surface() != Surface::ToolStripe {
            return Err(
                invalid("Only tool stripe items can open a panel").with_details(item.id.clone())
            );
        }
        if item.open_by_default && !item.panel {
            return Err(
                invalid("Only panel items can be open by default").with_details(item.id.clone())
            );
        }
        if index.insert(item.id.as_str(), position).is_some() {
            return Err(invalid("Chrome catalog repeats an id").with_details(item.id.clone()));
        }
    }
    Ok(index)
}

fn lookup(index: &HashMap<&str, usize>, id: &str) -> Result<usize, RuleError> {
    index
        .get(id)
        .copied()
        .ok_or_else(|| invalid("Unknown chrome item").with_details(id.to_string()))
}

/// Working form of [`State`]: catalog positions per zone, the open panel of each zone, and
/// which items are hidden, indexed by catalog position.
struct Layout {
    zones: [Vec<usize>; ZONE_COUNT],
    open: [Option<usize>; ZONE_COUNT],
    zone_of: Vec<Zone>,
    hidden: Vec<bool>,
}

impl Layout {
    fn empty(catalog: &[CatalogItem]) -> Self {
        Self {
            zones: Default::default(),
            open: [None; ZONE_COUNT],
            zone_of: catalog.iter().map(|item| item.default_zone).collect(),
            hidden: vec![false; catalog.len()],
        }
    }

    fn defaults(catalog: &[CatalogItem]) -> Self {
        let mut layout = Self::empty(catalog);
        for (item, entry) in catalog.iter().enumerate() {
            layout.zones[entry.default_zone.rank()].push(item);
        }
        for (item, entry) in catalog.iter().enumerate() {
            let slot = &mut layout.open[entry.default_zone.rank()];
            if entry.open_by_default && slot.is_none() {
                *slot = Some(item);
            }
        }
        layout
    }

    /// Keeps the first valid placement of every known item, then appends unplaced items to their
    /// default zone in catalog order, then keeps the first listed open panel of every zone, then
    /// hides the known tool stripe items `hidden` names.
    fn repaired(catalog: &[CatalogItem], index: &HashMap<&str, usize>, state: &State) -> Self {
        let mut layout = Self::empty(catalog);
        let mut placed = vec![false; catalog.len()];
        for placement in &state.placements {
            let Some(&item) = index.get(placement.id.as_str()) else {
                continue;
            };
            if placed[item] || placement.zone.surface() != catalog[item].default_zone.surface() {
                continue;
            }
            placed[item] = true;
            layout.zone_of[item] = placement.zone;
            layout.zones[placement.zone.rank()].push(item);
        }
        for (item, entry) in catalog.iter().enumerate() {
            if !placed[item] {
                layout.zones[entry.default_zone.rank()].push(item);
            }
        }
        for id in &state.open {
            let Some(&item) = index.get(id.as_str()) else {
                continue;
            };
            let slot = &mut layout.open[layout.zone_of[item].rank()];
            if catalog[item].panel && slot.is_none() {
                *slot = Some(item);
            }
        }
        for id in &state.hidden {
            if let Some(&item) = index.get(id.as_str()) {
                layout.hidden[item] = hideable(&catalog[item]);
            }
        }
        layout
    }

    fn is_open(&self, item: usize) -> bool {
        self.open[self.zone_of[item].rank()] == Some(item)
    }

    fn move_item(
        &mut self,
        catalog: &[CatalogItem],
        item: usize,
        zone: Zone,
        before: Option<usize>,
    ) -> Result<(), RuleError> {
        if zone.surface() != catalog[item].default_zone.surface() {
            return Err(invalid("Chrome item cannot leave its surface")
                .with_details(catalog[item].id.clone()));
        }
        if before == Some(item) && self.zone_of[item] == zone {
            return Ok(());
        }
        let source = self.zone_of[item];
        let was_open = self.is_open(item);
        self.zones[source.rank()].retain(|&member| member != item);
        if was_open {
            self.open[source.rank()] = None;
        }
        let members = &mut self.zones[zone.rank()];
        let position = before
            .and_then(|target| members.iter().position(|&member| member == target))
            .unwrap_or(members.len());
        members.insert(position, item);
        self.zone_of[item] = zone;
        if was_open {
            self.open[zone.rank()] = Some(item);
        }
        Ok(())
    }

    fn set_open(
        &mut self,
        catalog: &[CatalogItem],
        item: usize,
        open: bool,
    ) -> Result<(), RuleError> {
        if !catalog[item].panel {
            return Err(invalid("Chrome item has no panel").with_details(catalog[item].id.clone()));
        }
        let slot = &mut self.open[self.zone_of[item].rank()];
        if open {
            *slot = Some(item);
        } else if *slot == Some(item) {
            *slot = None;
        }
        Ok(())
    }

    /// Hides or shows the button of `item`. Hiding closes its panel, because an open panel whose
    /// button disappears would leave nothing on the stripe to close it with.
    fn set_hidden(
        &mut self,
        catalog: &[CatalogItem],
        item: usize,
        hidden: bool,
    ) -> Result<(), RuleError> {
        if !hideable(&catalog[item]) {
            return Err(invalid("Only tool stripe items can be hidden")
                .with_details(catalog[item].id.clone()));
        }
        self.hidden[item] = hidden;
        if hidden && self.is_open(item) {
            self.open[self.zone_of[item].rank()] = None;
        }
        Ok(())
    }

    fn into_state(self, catalog: &[CatalogItem]) -> State {
        let mut placements = Vec::with_capacity(catalog.len());
        let mut hidden = Vec::new();
        for zone in ZONES {
            for &item in &self.zones[zone.rank()] {
                placements.push(Placement {
                    id: catalog[item].id.clone(),
                    zone,
                });
                if self.hidden[item] {
                    hidden.push(catalog[item].id.clone());
                }
            }
        }
        let open = self
            .open
            .iter()
            .flatten()
            .map(|&item| catalog[item].id.clone())
            .collect();
        State {
            placements,
            open,
            hidden,
        }
    }
}

/// Only tool stripe buttons can be hidden: the title bar and status bar have no place that
/// lists hidden items.
fn hideable(item: &CatalogItem) -> bool {
    item.default_zone.surface() == Surface::ToolStripe
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, zone: Zone, panel: bool, open: bool) -> CatalogItem {
        CatalogItem {
            id: id.to_string(),
            default_zone: zone,
            panel,
            open_by_default: open,
        }
    }

    fn catalog() -> Vec<CatalogItem> {
        vec![
            item("project", Zone::LeftTop, true, true),
            item("commit", Zone::LeftTop, true, false),
            item("git", Zone::BottomLeft, true, true),
            item("terminal", Zone::BottomLeft, true, false),
            item("notifications", Zone::RightTop, true, false),
            item("update", Zone::BottomRight, false, false),
            item("run", Zone::TitleTrailing, false, false),
            item("search", Zone::TitleTrailing, false, false),
            item("caret", Zone::StatusTrailing, false, false),
            item("memory", Zone::StatusTrailing, false, false),
        ]
    }

    fn ids(state: &State, zone: Zone) -> Vec<&str> {
        state
            .placements
            .iter()
            .filter(|placement| placement.zone == zone)
            .map(|placement| placement.id.as_str())
            .collect()
    }

    fn defaults() -> State {
        reduce(&catalog(), State::default(), Action::Reset)
            .unwrap()
            .state
    }

    fn moved(state: State, id: &str, zone: Zone, before: Option<&str>) -> State {
        let action = Action::Move {
            id: id.to_string(),
            zone,
            before: before.map(str::to_string),
        };
        reduce(&catalog(), state, action).unwrap().state
    }

    fn toggled(state: State, id: &str) -> State {
        reduce(&catalog(), state, Action::Toggle { id: id.into() })
            .unwrap()
            .state
    }

    #[test]
    fn reset_lays_out_the_catalog_and_opens_default_panels() {
        let state = defaults();
        assert_eq!(ids(&state, Zone::LeftTop), ["project", "commit"]);
        assert_eq!(ids(&state, Zone::BottomLeft), ["git", "terminal"]);
        assert!(ids(&state, Zone::LeftBottom).is_empty());
        assert!(ids(&state, Zone::RightBottom).is_empty());
        assert_eq!(ids(&state, Zone::BottomRight), ["update"]);
        assert_eq!(ids(&state, Zone::StatusTrailing), ["caret", "memory"]);
        assert_eq!(state.open, ["project", "git"]);
    }

    #[test]
    fn zones_are_emitted_in_rank_order_under_their_wire_names() {
        for (rank, zone) in ZONES.iter().enumerate() {
            assert_eq!(zone.rank(), rank, "{zone:?}");
        }
        for (zone, name) in [
            (Zone::LeftTop, "leftTop"),
            (Zone::LeftBottom, "leftBottom"),
            (Zone::BottomLeft, "bottomLeft"),
            (Zone::RightTop, "rightTop"),
            (Zone::RightBottom, "rightBottom"),
            (Zone::BottomRight, "bottomRight"),
            (Zone::TitleTrailing, "titleTrailing"),
            (Zone::StatusTrailing, "statusTrailing"),
        ] {
            assert_eq!(serde_json::to_value(zone).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<Zone>(Value::from(name)).unwrap(),
                zone
            );
        }
    }

    #[test]
    fn left_bottom_and_bottom_left_are_different_zones() {
        let left_bottom = moved(defaults(), "commit", Zone::LeftBottom, None);
        assert_eq!(ids(&left_bottom, Zone::LeftBottom), ["commit"]);
        assert_eq!(ids(&left_bottom, Zone::BottomLeft), ["git", "terminal"]);
        let bottom_left = moved(defaults(), "commit", Zone::BottomLeft, None);
        assert!(ids(&bottom_left, Zone::LeftBottom).is_empty());
        assert_eq!(
            ids(&bottom_left, Zone::BottomLeft),
            ["git", "terminal", "commit"]
        );
        assert_ne!(left_bottom, bottom_left);

        let right_bottom = moved(defaults(), "notifications", Zone::RightBottom, None);
        assert_eq!(ids(&right_bottom, Zone::RightBottom), ["notifications"]);
        assert_eq!(ids(&right_bottom, Zone::BottomRight), ["update"]);
        let bottom_right = moved(defaults(), "notifications", Zone::BottomRight, None);
        assert!(ids(&bottom_right, Zone::RightBottom).is_empty());
        assert_eq!(
            ids(&bottom_right, Zone::BottomRight),
            ["update", "notifications"]
        );
        assert_ne!(right_bottom, bottom_right);
    }

    #[test]
    fn normalize_drops_unknown_repeated_and_foreign_placements_and_appends_new_items() {
        let saved = State {
            placements: vec![
                Placement {
                    id: "gone".into(),
                    zone: Zone::LeftTop,
                },
                Placement {
                    id: "terminal".into(),
                    zone: Zone::RightTop,
                },
                Placement {
                    id: "terminal".into(),
                    zone: Zone::LeftTop,
                },
                Placement {
                    id: "caret".into(),
                    zone: Zone::LeftTop,
                },
                Placement {
                    id: "memory".into(),
                    zone: Zone::StatusTrailing,
                },
            ],
            open: vec![
                "gone".into(),
                "update".into(),
                "terminal".into(),
                "notifications".into(),
            ],
            hidden: Vec::new(),
        };
        let reduction = reduce(&catalog(), saved, Action::Normalize).unwrap();
        let state = reduction.state;
        assert!(reduction.changed);
        assert_eq!(ids(&state, Zone::RightTop), ["terminal", "notifications"]);
        assert_eq!(ids(&state, Zone::StatusTrailing), ["memory", "caret"]);
        assert_eq!(ids(&state, Zone::LeftTop), ["project", "commit"]);
        assert_eq!(state.open, ["terminal"]);
        assert_eq!(state.placements.len(), catalog().len());
    }

    #[test]
    fn normalizing_a_result_changes_nothing() {
        let state = moved(defaults(), "git", Zone::RightBottom, None);
        let again = reduce(&catalog(), state.clone(), Action::Normalize).unwrap();
        assert!(!again.changed);
        assert_eq!(again.state, state);
    }

    #[test]
    fn move_inserts_in_front_of_the_target_or_at_the_end() {
        let state = moved(defaults(), "search", Zone::TitleTrailing, Some("run"));
        assert_eq!(ids(&state, Zone::TitleTrailing), ["search", "run"]);
        let state = moved(state, "search", Zone::TitleTrailing, None);
        assert_eq!(ids(&state, Zone::TitleTrailing), ["run", "search"]);
        let same = moved(state.clone(), "run", Zone::TitleTrailing, Some("run"));
        assert_eq!(same, state);
        let elsewhere = moved(state, "caret", Zone::StatusTrailing, Some("project"));
        assert_eq!(ids(&elsewhere, Zone::StatusTrailing), ["memory", "caret"]);
    }

    #[test]
    fn moving_an_open_panel_takes_the_slot_of_the_target_zone() {
        let state = moved(defaults(), "git", Zone::LeftTop, Some("commit"));
        assert_eq!(ids(&state, Zone::LeftTop), ["project", "git", "commit"]);
        assert_eq!(state.open, ["git"]);
        let state = moved(state, "commit", Zone::RightTop, None);
        assert_eq!(state.open, ["git"]);
    }

    #[test]
    fn items_never_leave_their_surface() {
        for (id, zone) in [
            ("run", Zone::LeftTop),
            ("project", Zone::StatusTrailing),
            ("caret", Zone::TitleTrailing),
        ] {
            let action = Action::Move {
                id: id.into(),
                zone,
                before: None,
            };
            let error = reduce(&catalog(), defaults(), action).unwrap_err();
            assert_eq!(error.code, RuleErrorCode::InvalidRequest, "{id}");
        }
    }

    #[test]
    fn toggle_opens_one_panel_per_zone() {
        let state = toggled(defaults(), "commit");
        assert_eq!(state.open, ["commit", "git"]);
        let state = toggled(state, "commit");
        assert_eq!(state.open, ["git"]);
        let state = toggled(state, "notifications");
        assert_eq!(state.open, ["git", "notifications"]);
        let closed = reduce(
            &catalog(),
            state,
            Action::SetOpen {
                id: "project".into(),
                open: false,
            },
        )
        .unwrap();
        assert!(!closed.changed);
    }

    #[test]
    fn panels_of_the_three_anchors_of_a_stripe_stay_open_side_by_side() {
        // The project panel is open in LeftTop and the git panel in BottomLeft.
        let state = moved(defaults(), "commit", Zone::LeftBottom, None);
        assert_eq!(state.open, ["project", "git"]);
        let state = toggled(state, "commit");
        assert_eq!(state.open, ["project", "commit", "git"]);
        // Closing the lower panel of the left column leaves the upper one and the bottom one.
        let state = toggled(state, "commit");
        assert_eq!(state.open, ["project", "git"]);
        // An open panel moved into LeftBottom takes that slot and keeps LeftTop's panel open.
        let state = moved(state, "git", Zone::LeftBottom, None);
        assert_eq!(state.open, ["project", "git"]);
        assert_eq!(ids(&state, Zone::LeftBottom), ["commit", "git"]);
        assert_eq!(ids(&state, Zone::BottomLeft), ["terminal"]);
    }

    #[test]
    fn only_panels_open_and_only_known_items_act() {
        for action in [
            Action::Toggle {
                id: "update".into(),
            },
            Action::SetOpen {
                id: "caret".into(),
                open: true,
            },
            Action::Toggle {
                id: "missing".into(),
            },
            Action::Move {
                id: "missing".into(),
                zone: Zone::LeftTop,
                before: None,
            },
        ] {
            assert!(reduce(&catalog(), defaults(), action).is_err());
        }
    }

    fn hidden(state: State, id: &str, hidden: bool) -> State {
        let action = Action::SetHidden {
            id: id.into(),
            hidden,
        };
        reduce(&catalog(), state, action).unwrap().state
    }

    #[test]
    fn hiding_a_button_closes_its_panel_and_keeps_its_place() {
        let state = hidden(defaults(), "project", true);
        assert_eq!(state.hidden, ["project"]);
        assert_eq!(state.open, ["git"]);
        assert_eq!(ids(&state, Zone::LeftTop), ["project", "commit"]);
        // Showing it again puts the button back in front of commit and leaves the panel closed.
        let state = hidden(state, "project", false);
        assert_eq!(state, toggled(defaults(), "project"));
    }

    #[test]
    fn a_hidden_item_keeps_its_hidden_state_while_its_panel_opens_and_it_moves() {
        let state = hidden(defaults(), "commit", true);
        // A shortcut or a command may still open the panel of a hidden item.
        let state = toggled(state, "commit");
        assert_eq!(state.open, ["commit", "git"]);
        assert_eq!(state.hidden, ["commit"]);
        let state = moved(state, "commit", Zone::RightBottom, None);
        assert_eq!(ids(&state, Zone::RightBottom), ["commit"]);
        assert_eq!(state.open, ["git", "commit"]);
        assert_eq!(state.hidden, ["commit"]);
        // Hiding an item that is already hidden still closes its panel.
        let state = hidden(state, "commit", true);
        assert_eq!(state.open, ["git"]);
    }

    #[test]
    fn hidden_items_are_listed_in_placement_order_and_reset_shows_them() {
        let state = hidden(defaults(), "update", true);
        let state = hidden(state, "terminal", true);
        let state = hidden(state, "commit", true);
        assert_eq!(state.hidden, ["commit", "terminal", "update"]);
        let again = reduce(&catalog(), state.clone(), Action::Normalize).unwrap();
        assert!(!again.changed);
        let reset = reduce(&catalog(), state, Action::Reset).unwrap().state;
        assert!(reset.hidden.is_empty());
    }

    #[test]
    fn normalize_drops_unknown_repeated_and_foreign_hidden_ids() {
        let saved = State {
            hidden: vec![
                "gone".into(),
                "terminal".into(),
                "run".into(),
                "caret".into(),
                "terminal".into(),
                "project".into(),
            ],
            ..defaults()
        };
        let reduction = reduce(&catalog(), saved, Action::Normalize).unwrap();
        assert!(reduction.changed);
        assert_eq!(reduction.state.hidden, ["project", "terminal"]);
        // A saved hidden panel that is open stays open: only hiding it closes it.
        assert_eq!(reduction.state.open, ["project", "git"]);
    }

    #[test]
    fn only_tool_stripe_items_can_be_hidden() {
        for id in ["run", "caret", "missing"] {
            let action = Action::SetHidden {
                id: id.into(),
                hidden: true,
            };
            let error = reduce(&catalog(), defaults(), action).unwrap_err();
            assert_eq!(error.code, RuleErrorCode::InvalidRequest, "{id}");
        }
    }

    #[test]
    fn a_state_saved_before_hiding_existed_decodes_and_answers_hidden() {
        let request = serde_json::json!({
            "catalog": [{"id": "project", "defaultZone": "leftTop", "panel": true}],
            "state": {"placements": [{"id": "project", "zone": "leftTop"}], "open": []},
            "action": {"kind": "setHidden", "id": "project", "hidden": true},
        });
        let answer = reduce_json(request).unwrap();
        assert_eq!(answer["state"]["hidden"], serde_json::json!(["project"]));
        assert_eq!(answer["changed"], true);
    }

    #[test]
    fn malformed_catalogs_are_rejected() {
        for broken in [
            vec![item("", Zone::LeftTop, false, false)],
            vec![
                item("a", Zone::LeftTop, false, false),
                item("a", Zone::LeftBottom, false, false),
            ],
            vec![item("a", Zone::TitleTrailing, true, false)],
            vec![item("a", Zone::LeftTop, false, true)],
        ] {
            assert!(reduce(&broken, State::default(), Action::Normalize).is_err());
        }
    }
}
