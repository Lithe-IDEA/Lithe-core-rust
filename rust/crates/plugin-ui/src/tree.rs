//! The host-control part of a view: layout, text, inputs and read-only
//! displays. Hosts draw every node with their own native controls and theme;
//! a node carries meaning (tone, style, width in characters), never pixels or
//! colors.

use crate::fields::{
    array, at, boolean, choice, fail, identifier, integer, line, object, string, text, Checked,
    Object,
};
use crate::{
    canvas, Context, Surface, MAX_INFO_CHARS, MAX_LABEL_CHARS, MAX_TEXT_CHARS, MAX_TREE_DEPTH,
    MAX_TREE_ITEMS, MAX_UI_NODES,
};
use lithe_plugin_model::UIIcon;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Text tones: meaning the host maps to its theme.
pub(crate) const TONES: &[&str] = &[
    "secondary",
    "muted",
    "accent",
    "success",
    "warning",
    "error",
];
/// Label text styles the host maps to its type scale.
const LABEL_STYLES: &[&str] = &["title", "strong", "body", "caption", "mono"];
/// Most options of a select.
const MAX_SELECT_OPTIONS: usize = 128;
/// Fewest and most options of a segmented control.
const SEGMENTED_OPTIONS: (usize, usize) = (2, 8);
/// Widest label or text field, in characters.
const MAX_WIDTH_CHARS: u64 = 120;
/// Most rows of a key-value list.
const MAX_KEY_VALUE_ITEMS: usize = 64;
/// Most columns and rows of a table.
const MAX_TABLE_COLUMNS: usize = 16;
const MAX_TABLE_ROWS: usize = 256;

/// One pass over a view: identifiers seen and budgets spent so far.
pub(crate) struct Walk<'c> {
    pub(crate) context: &'c Context<'c>,
    ids: BTreeSet<String>,
    nodes: usize,
    pub(crate) canvases: usize,
}

impl<'c> Walk<'c> {
    pub(crate) fn new(context: &'c Context<'c>) -> Self {
        Self {
            context,
            ids: BTreeSet::new(),
            nodes: 0,
            canvases: 0,
        }
    }

    /// Checks an array of view nodes at `path`, `depth` levels deep.
    pub(crate) fn nodes(&mut self, value: &Value, path: &str, depth: usize) -> Checked<()> {
        let nodes = value
            .as_array()
            .ok_or_else(|| fail(path, "Expected an array of view nodes."))?;
        for (index, node) in nodes.iter().enumerate() {
            self.node(node, &format!("{path}[{index}]"), depth)?;
        }
        Ok(())
    }

    /// Records a node identifier, unique across the view's host controls.
    pub(crate) fn id(&mut self, map: &Object, path: &str, required: bool) -> Checked<()> {
        if let Some(id) = identifier(map, path, "id", required)? {
            if !self.ids.insert(id.to_owned()) {
                return Err(fail(at(path, "id"), "View node identifiers are unique."));
            }
        }
        Ok(())
    }

    fn node(&mut self, value: &Value, path: &str, depth: usize) -> Checked<()> {
        if depth > MAX_TREE_DEPTH {
            return Err(fail(path, "Views nest at most 16 levels deep."));
        }
        self.nodes += 1;
        if self.nodes > MAX_UI_NODES {
            return Err(fail(path, "A view has at most 512 control nodes."));
        }
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(at(path, "type"), "A view node names its type."))?;
        match kind {
            "row" => {
                let map = object(value, path, &["type", "id", "children"])?;
                self.id(map, path, false)?;
                self.children(map, path, depth)
            }
            "section" => {
                let map = object(value, path, &["type", "id", "title", "info", "children"])?;
                self.id(map, path, false)?;
                line(map, path, "title", MAX_LABEL_CHARS, false)?;
                info(map, path)?;
                self.children(map, path, depth)
            }
            "label" => {
                let map = object(
                    value,
                    path,
                    &["type", "id", "text", "tone", "style", "width", "info"],
                )?;
                self.id(map, path, false)?;
                text(map, path, "text", MAX_TEXT_CHARS, true)?;
                choice(map, path, "tone", TONES, false)?;
                choice(map, path, "style", LABEL_STYLES, false)?;
                integer(map, path, "width", (1, MAX_WIDTH_CHARS), false)?;
                info(map, path)
            }
            "textField" => self.text_field(value, path),
            "select" | "segmented" => self.select(value, path, kind == "segmented"),
            "toggle" => {
                let map = object(
                    value,
                    path,
                    &["type", "id", "label", "info", "value", "setting", "enabled"],
                )?;
                self.control(map, path)?;
                line(map, path, "label", MAX_LABEL_CHARS, true)?;
                if self.binding(map, path, &["boolean"])?.is_none() {
                    boolean(map, path, "value")?;
                }
                Ok(())
            }
            "button" => self.button(value, path),
            "keyValue" => {
                let map = object(value, path, &["type", "id", "items"])?;
                self.id(map, path, false)?;
                let items = array(map, path, "items", (1, MAX_KEY_VALUE_ITEMS), true)?;
                for (index, item) in items.into_iter().flatten().enumerate() {
                    let item_path = format!("{path}.items[{index}]");
                    let item = object(item, &item_path, &["label", "value", "tone"])?;
                    line(item, &item_path, "label", MAX_LABEL_CHARS, true)?;
                    text(item, &item_path, "value", MAX_TEXT_CHARS, true)?;
                    choice(item, &item_path, "tone", TONES, false)?;
                }
                Ok(())
            }
            "table" => self.table(value, path),
            "banner" => {
                let map = object(value, path, &["type", "id", "text", "tone"])?;
                self.id(map, path, false)?;
                text(map, path, "text", MAX_TEXT_CHARS, true)?;
                choice(map, path, "tone", TONES, true)?;
                Ok(())
            }
            "setting" => {
                self.settings_only(path, kind)?;
                let map = object(value, path, &["type", "id", "setting"])?;
                self.id(map, path, false)?;
                self.declared_setting(map, path)?;
                Ok(())
            }
            "tool" => {
                self.settings_only(path, kind)?;
                let map = object(value, path, &["type", "id", "tool", "choose"])?;
                self.id(map, path, false)?;
                let tool = string(map, path, "tool", true)?.unwrap_or_default();
                if !self.context.tools.contains(tool) {
                    return Err(fail(
                        at(path, "tool"),
                        "A tool row names a tool this plugin defines or requires.",
                    ));
                }
                boolean(map, path, "choose")?;
                if map.contains_key("choose") && self.context.ui_version < 1 {
                    return Err(fail(
                        at(path, "choose"),
                        "A tool row with choose needs ui 1 in requires.",
                    ));
                }
                Ok(())
            }
            "tree" => self.tree(value, path),
            "canvas" => canvas::check(self, value, path),
            other => Err(fail(
                at(path, "type"),
                format!("Unknown view node type {other}."),
            )),
        }
    }

    fn children(&mut self, map: &Object, path: &str, depth: usize) -> Checked<()> {
        let children = map
            .get("children")
            .ok_or_else(|| fail(at(path, "children"), "Required."))?;
        self.nodes(children, &at(path, "children"), depth + 1)
    }

    /// Fields every input shares: a required ID, label text and enablement.
    fn control(&mut self, map: &Object, path: &str) -> Checked<()> {
        self.id(map, path, true)?;
        info(map, path)?;
        boolean(map, path, "enabled")?;
        Ok(())
    }

    fn text_field(&mut self, value: &Value, path: &str) -> Checked<()> {
        let map = object(
            value,
            path,
            &[
                "type",
                "id",
                "label",
                "info",
                "value",
                "setting",
                "placeholder",
                "width",
                "mono",
                "enabled",
            ],
        )?;
        self.control(map, path)?;
        line(map, path, "label", MAX_LABEL_CHARS, false)?;
        line(map, path, "placeholder", MAX_LABEL_CHARS, false)?;
        boolean(map, path, "mono")?;
        match map.get("width") {
            None => {}
            Some(Value::String(width)) if width == "fill" => {}
            Some(_) => {
                integer(map, path, "width", (1, MAX_WIDTH_CHARS), false).map_err(|_| {
                    fail(
                        at(path, "width"),
                        "Expected \"fill\" or a width of 1 to 120 characters.",
                    )
                })?;
            }
        }
        if self
            .binding(map, path, &["text", "file", "directory", "integer"])?
            .is_none()
        {
            text(map, path, "value", MAX_TEXT_CHARS, false)?;
        }
        Ok(())
    }

    fn select(&mut self, value: &Value, path: &str, segmented: bool) -> Checked<()> {
        let map = object(
            value,
            path,
            &[
                "type", "id", "label", "info", "value", "setting", "options", "enabled",
            ],
        )?;
        self.control(map, path)?;
        line(map, path, "label", MAX_LABEL_CHARS, false)?;
        if let Some(setting) = self.binding(map, path, &["enum"])? {
            if map.contains_key("options") {
                return Err(fail(
                    at(path, "options"),
                    "A bound control shows the options the setting declares.",
                ));
            }
            let count = setting.option_values().count();
            if segmented && !(SEGMENTED_OPTIONS.0..=SEGMENTED_OPTIONS.1).contains(&count) {
                return Err(fail(
                    at(path, "setting"),
                    "A segmented control shows 2 to 8 options.",
                ));
            }
            return Ok(());
        }
        let count = if segmented {
            SEGMENTED_OPTIONS
        } else {
            (1, MAX_SELECT_OPTIONS)
        };
        let options = array(map, path, "options", count, true)?
            .into_iter()
            .flatten();
        let mut values = BTreeSet::new();
        for (index, option) in options.enumerate() {
            let option_path = format!("{path}.options[{index}]");
            let option = object(option, &option_path, &["value", "label"])?;
            let value = line(option, &option_path, "value", MAX_LABEL_CHARS, true)?;
            line(option, &option_path, "label", MAX_LABEL_CHARS, true)?;
            if !values.insert(value.unwrap_or_default().to_owned()) {
                return Err(fail(at(&option_path, "value"), "Option values are unique."));
            }
        }
        if let Some(value) = string(map, path, "value", false)? {
            if !values.contains(value) {
                return Err(fail(at(path, "value"), "The value is one of the options."));
            }
        }
        Ok(())
    }

    fn button(&mut self, value: &Value, path: &str) -> Checked<()> {
        let map = object(
            value,
            path,
            &["type", "id", "text", "tone", "enabled", "action", "icon"],
        )?;
        self.id(map, path, true)?;
        line(map, path, "text", MAX_LABEL_CHARS, true)?;
        choice(map, path, "tone", &["primary"], false)?;
        boolean(map, path, "enabled")?;
        if let Some(action) = string(map, path, "action", false)? {
            self.settings_only(&at(path, "action"), "settingAction button")?;
            if !self
                .context
                .actions
                .iter()
                .any(|declared| declared.id == action)
            {
                return Err(fail(
                    at(path, "action"),
                    "A button runs one of the plugin's declared setting actions.",
                ));
            }
        }
        if let Some(icon) = string(map, path, "icon", false)? {
            let host: Option<UIIcon> = serde_json::from_value(json!({"host": icon})).ok();
            if !host.is_some_and(|icon| icon.is_valid_host_icon()) {
                return Err(fail(at(path, "icon"), "A button icon names a host icon."));
            }
        }
        Ok(())
    }

    fn table(&mut self, value: &Value, path: &str) -> Checked<()> {
        let map = object(value, path, &["type", "id", "columns", "rows"])?;
        self.id(map, path, false)?;
        let columns =
            array(map, path, "columns", (1, MAX_TABLE_COLUMNS), true)?.map_or(0, Vec::len);
        for (index, column) in map["columns"].as_array().into_iter().flatten().enumerate() {
            let column_path = format!("{path}.columns[{index}]");
            let column = object(column, &column_path, &["title", "align"])?;
            line(column, &column_path, "title", MAX_LABEL_CHARS, true)?;
            choice(
                column,
                &column_path,
                "align",
                &["start", "center", "end"],
                false,
            )?;
        }
        let rows = array(map, path, "rows", (0, MAX_TABLE_ROWS), true)?;
        for (index, row) in rows.into_iter().flatten().enumerate() {
            let row_path = format!("{path}.rows[{index}]");
            let cells = row
                .as_array()
                .filter(|cells| cells.len() == columns)
                .ok_or_else(|| fail(&row_path, "A row has one string per column."))?;
            for (cell_index, cell) in cells.iter().enumerate() {
                let cell = cell.as_str().filter(|cell| {
                    cell.chars().count() <= MAX_TEXT_CHARS && !cell.chars().any(char::is_control)
                });
                if cell.is_none() {
                    return Err(fail(
                        format!("{row_path}[{cell_index}]"),
                        "A cell is single-line text of at most 1024 characters.",
                    ));
                }
            }
        }
        Ok(())
    }

    fn tree(&mut self, value: &Value, path: &str) -> Checked<()> {
        if self.context.surface != Surface::ToolWindow {
            return Err(fail(path, "Trees appear in tool windows."));
        }
        let map = object(value, path, &["type", "id", "label", "items"])?;
        self.id(map, path, true)?;
        line(map, path, "label", MAX_LABEL_CHARS, false)?;
        let items = map
            .get("items")
            .ok_or_else(|| fail(at(path, "items"), "Required."))?;
        let mut ids = BTreeSet::new();
        let mut count = 0;
        tree_items(items, &at(path, "items"), 1, &mut ids, &mut count)
    }

    /// The declared setting a control edits through the host draft, when it
    /// names one; `kinds` are the setting types the control can edit.
    fn binding(
        &self,
        map: &Object,
        path: &str,
        kinds: &[&str],
    ) -> Checked<Option<&'c lithe_plugin_model::settings::SettingDeclaration>> {
        if !map.contains_key("setting") {
            return Ok(None);
        }
        if map.contains_key("value") {
            return Err(fail(
                at(path, "value"),
                "A control shows either its own value or a declared setting.",
            ));
        }
        self.settings_only(&at(path, "setting"), "setting binding")?;
        let setting = self.declared_setting(map, path)?;
        if !kinds.contains(&setting.kind()) {
            return Err(fail(
                at(path, "setting"),
                format!(
                    "This control cannot edit a {} setting; it edits {}.",
                    setting.kind(),
                    kinds.join(", ")
                ),
            ));
        }
        Ok(Some(setting))
    }

    fn declared_setting(
        &self,
        map: &Object,
        path: &str,
    ) -> Checked<&'c lithe_plugin_model::settings::SettingDeclaration> {
        let id = string(map, path, "setting", true)?.unwrap_or_default();
        self.context
            .settings
            .iter()
            .find(|setting| setting.id == id)
            .ok_or_else(|| {
                fail(
                    at(path, "setting"),
                    "The plugin declares no setting with this ID.",
                )
            })
    }

    fn settings_only(&self, path: &str, what: &str) -> Checked<()> {
        if self.context.surface != Surface::Settings {
            return Err(fail(
                path,
                format!("A {what} appears only in a settings region."),
            ));
        }
        Ok(())
    }
}

/// The ⓘ explanation a host shows beside a control or heading.
fn info(map: &Object, path: &str) -> Checked<()> {
    text(map, path, "info", MAX_INFO_CHARS, false).map(drop)
}

fn tree_items(
    value: &Value,
    path: &str,
    depth: usize,
    ids: &mut BTreeSet<String>,
    count: &mut usize,
) -> Checked<()> {
    if depth > MAX_TREE_DEPTH {
        return Err(fail(path, "Tree items nest at most 16 levels deep."));
    }
    let items = value
        .as_array()
        .ok_or_else(|| fail(path, "Expected an array of tree items."))?;
    for (index, item) in items.iter().enumerate() {
        let item_path = format!("{path}[{index}]");
        *count += 1;
        if *count > MAX_TREE_ITEMS {
            return Err(fail(&item_path, "A tree has at most 2000 items."));
        }
        let map = object(
            item,
            &item_path,
            &["id", "label", "icon", "detail", "expanded", "children"],
        )?;
        let id = identifier(map, &item_path, "id", true)?.unwrap_or_default();
        if !ids.insert(id.to_owned()) {
            return Err(fail(
                at(&item_path, "id"),
                "Tree item identifiers are unique.",
            ));
        }
        line(map, &item_path, "label", MAX_LABEL_CHARS * 2, true)?;
        line(map, &item_path, "detail", MAX_LABEL_CHARS * 2, false)?;
        boolean(map, &item_path, "expanded")?;
        if let Some(icon) = string(map, &item_path, "icon", false)? {
            let host: Option<UIIcon> = serde_json::from_value(json!({"host": icon})).ok();
            if !host.is_some_and(|icon| icon.is_valid_host_icon()) {
                return Err(fail(
                    at(&item_path, "icon"),
                    "A tree icon names a host icon.",
                ));
            }
        }
        if let Some(children) = map.get("children") {
            tree_items(children, &at(&item_path, "children"), depth + 1, ids, count)?;
        }
    }
    Ok(())
}
