//! Plugin views: the event a view function receives and the result it returns.
//!
//! A tool window or settings region names a view function in `view.function`
//! ([plugin views](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/shared/plugins/contract/v1/README.md#plugin-views)).
//! Declare it with [`Backend::view`](crate::Backend::view): the SDK reads the
//! params into a [`ViewEvent`] and answers the handler's [`ViewResult`]. View
//! and canvas nodes stay JSON values built with `json!`, shaped as
//! `plugin-ui.schema.json` describes; Core checks every result before the host
//! draws it.

use crate::error::SdkError;
use serde_json::{Map, Value};

const INVALID_EVENT: &str = "sdk.view.invalidEvent";

/// Why the host called the view function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ViewEventKind {
    /// The view became visible; answer a whole view.
    Open,
    /// The view went away; the result is ignored.
    Close,
    Click,
    Change,
    Select,
    Activate,
    Viewport,
    /// The effective or draft setting values changed.
    Settings,
}

impl ViewEventKind {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "open" => Self::Open,
            "close" => Self::Close,
            "click" => Self::Click,
            "change" => Self::Change,
            "select" => Self::Select,
            "activate" => Self::Activate,
            "viewport" => Self::Viewport,
            "settings" => Self::Settings,
            _ => return None,
        })
    }
}

/// The declaration being drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewSurface {
    ToolWindow(String),
    SettingRegion(String),
}

/// The visible part of a canvas, in view-box coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The params of one call to a view function.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ViewEvent {
    pub kind: ViewEventKind,
    /// Host-chosen; each [`ViewEventKind::Open`] starts a new instance.
    pub instance: String,
    pub surface: ViewSurface,
    /// Interface language tag, such as `en` or `zh-Hans`; return view text in it.
    pub language: String,
    /// Saved setting values in force, by setting ID.
    pub effective: Map<String, Value>,
    /// Values changed in the open settings window but not applied; empty in a tool window.
    pub draft: Map<String, Value>,
    /// The node acted on: the group inside a canvas, the tree for an item.
    pub node: Option<String>,
    /// A click inside a canvas, and a viewport event: the canvas ID.
    pub canvas: Option<String>,
    /// A change: a string, a boolean for a toggle, or the bound setting's new draft value.
    pub value: Option<Value>,
    /// A change of a control bound to a setting: the setting ID.
    pub setting: Option<String>,
    /// A select or activate event: the tree item ID.
    pub item: Option<String>,
    pub viewport: Option<Viewport>,
}

impl ViewEvent {
    /// Reads the params the host sent; fails with `sdk.view.invalidEvent`
    /// when a required field is missing or a field has the wrong type. An
    /// optional field that is `null` counts as absent.
    pub fn parse(params: &Value) -> Result<Self, SdkError> {
        let invalid = || SdkError::new(INVALID_EVENT, "Invalid view event.");
        let object = params.as_object().ok_or_else(invalid)?;
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
        let string = |fields: &Map<String, Value>, key: &str| match fields.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(invalid()),
        };
        let optional = |key: &str| string(object, key);
        let kind = text("event")
            .and_then(|name| ViewEventKind::parse(&name))
            .ok_or_else(invalid)?;
        let surface = object
            .get("surface")
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        let surface = match (
            string(surface, "toolWindowID")?,
            string(surface, "settingRegionID")?,
        ) {
            (Some(id), None) => ViewSurface::ToolWindow(id),
            (None, Some(id)) => ViewSurface::SettingRegion(id),
            _ => return Err(invalid()),
        };
        let settings = object
            .get("settings")
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        let values = |key: &str| {
            settings
                .get(key)
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(invalid)
        };
        let viewport = match object.get("viewport") {
            None | Some(Value::Null) => None,
            Some(viewport) => {
                let number = |key: &str| {
                    viewport
                        .get(key)
                        .and_then(Value::as_f64)
                        .ok_or_else(invalid)
                };
                Some(Viewport {
                    x: number("x")?,
                    y: number("y")?,
                    width: number("width")?,
                    height: number("height")?,
                })
            }
        };
        let value = match object.get("value") {
            None | Some(Value::Null) => None,
            Some(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => {
                Some(value.clone())
            }
            Some(_) => return Err(invalid()),
        };
        Ok(Self {
            kind,
            instance: text("instance").ok_or_else(invalid)?,
            surface,
            language: text("language").ok_or_else(invalid)?,
            effective: values("effective")?,
            draft: values("draft")?,
            node: optional("node")?,
            canvas: optional("canvas")?,
            value,
            setting: optional("setting")?,
            item: optional("item")?,
            viewport,
        })
    }
}

/// What a view function answers. The first result after
/// [`ViewEventKind::Open`] is a whole view.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewResult {
    /// The whole view: its nodes, top to bottom.
    View(Vec<Value>),
    /// Edits to the view the host is drawing, applied in order.
    Patches(Vec<Patch>),
    /// Nothing changed.
    Unchanged,
}

impl From<ViewResult> for Value {
    fn from(result: ViewResult) -> Self {
        let mut object = Map::new();
        match result {
            ViewResult::View(nodes) => {
                object.insert("view".to_string(), Value::Array(nodes));
            }
            ViewResult::Patches(patches) => {
                let patches = patches.into_iter().map(Value::from).collect();
                object.insert("patches".to_string(), Value::Array(patches));
            }
            ViewResult::Unchanged => {}
        }
        Value::Object(object)
    }
}

/// One edit, addressed by node ID within the host controls, or within one
/// canvas scene ([`Patch::in_canvas`]) or one tree's items ([`Patch::in_tree`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Patch(Map<String, Value>);

impl Patch {
    fn new(op: &str, id: Option<&str>) -> Self {
        let mut patch = Map::new();
        patch.insert("op".to_string(), Value::from(op));
        if let Some(id) = id {
            patch.insert("id".to_string(), Value::from(id));
        }
        Self(patch)
    }

    fn with(mut self, key: &str, value: Value) -> Self {
        self.0.insert(key.to_string(), value);
        self
    }

    /// Merges `fields`, a JSON object, into the node; a `null` field removes it.
    pub fn set(id: &str, fields: Value) -> Self {
        Self::new("set", Some(id)).with("fields", fields)
    }

    /// Swaps the node, with its subtree, for `node`.
    pub fn replace(id: &str, node: Value) -> Self {
        Self::new("replace", Some(id)).with("node", node)
    }

    /// Deletes the node and its subtree.
    pub fn remove(id: &str) -> Self {
        Self::new("remove", Some(id))
    }

    /// Adds `node` at the end of the top level; see [`Patch::under`] and [`Patch::at`].
    pub fn insert(node: Value) -> Self {
        Self::new("insert", None).with("node", node)
    }

    /// An insert into the children of `parent`.
    pub fn under(self, parent: &str) -> Self {
        self.with("parent", Value::from(parent))
    }

    /// An insert at `index` instead of the end.
    pub fn at(self, index: usize) -> Self {
        self.with("index", Value::from(index))
    }

    /// Addresses the nodes of canvas `canvas`.
    pub fn in_canvas(self, canvas: &str) -> Self {
        self.with("canvas", Value::from(canvas))
    }

    /// Addresses the items of tree `tree`.
    pub fn in_tree(self, tree: &str) -> Self {
        self.with("tree", Value::from(tree))
    }
}

impl From<Patch> for Value {
    fn from(patch: Patch) -> Self {
        Value::Object(patch.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(extra: Value) -> Value {
        let mut params = json!({
            "event": "change",
            "instance": "i1",
            "surface": { "settingRegionID": "otp" },
            "language": "zh-Hans",
            "settings": { "effective": { "otpRelease": "26" }, "draft": { "otpRelease": "27" } },
        });
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        params
    }

    #[test]
    fn reads_every_field_the_host_sends() {
        let parsed = ViewEvent::parse(&event(json!({
            "node": "otp", "setting": "otpRelease", "value": "27",
            "viewport": { "x": 0, "y": 10.5, "width": 1200, "height": 640 },
        })))
        .unwrap();
        assert_eq!(parsed.kind, ViewEventKind::Change);
        assert_eq!(parsed.surface, ViewSurface::SettingRegion("otp".into()));
        assert_eq!(parsed.language, "zh-Hans");
        assert_eq!(parsed.draft["otpRelease"], "27");
        assert_eq!(parsed.setting.as_deref(), Some("otpRelease"));
        assert_eq!(parsed.value, Some(json!("27")));
        assert_eq!(parsed.viewport.unwrap().y, 10.5);
        assert_eq!(parsed.item, None);
        // An optional field sent as null counts as absent.
        let absent = ViewEvent::parse(&event(
            json!({ "node": null, "value": null, "viewport": null }),
        ))
        .unwrap();
        assert_eq!(
            (absent.node, absent.value, absent.viewport),
            (None, None, None)
        );
    }

    #[test]
    fn refuses_events_without_their_required_shape() {
        for params in [
            json!(null),
            event(json!({ "event": "hover" })),
            event(json!({ "surface": { "toolWindowID": "a", "settingRegionID": "b" } })),
            event(json!({ "surface": { "toolWindowID": "a", "settingRegionID": 3 } })),
            event(json!({ "surface": {} })),
            event(json!({ "settings": { "effective": {} } })),
            event(json!({ "node": 3 })),
            event(json!({ "value": ["a"] })),
            event(json!({ "viewport": { "x": 0 } })),
        ] {
            let error = ViewEvent::parse(&params).unwrap_err();
            assert_eq!(error.code(), INVALID_EVENT, "{params}");
        }
    }

    #[test]
    fn results_and_patches_serialize_as_the_schema_names_them() {
        assert_eq!(Value::from(ViewResult::Unchanged), json!({}));
        assert_eq!(
            Value::from(ViewResult::View(vec![
                json!({ "type": "label", "text": "a" })
            ])),
            json!({ "view": [{ "type": "label", "text": "a" }] })
        );
        let patches = ViewResult::Patches(vec![
            Patch::set("filter", json!({ "enabled": false })),
            Patch::remove("guava").in_canvas("graph"),
            Patch::insert(json!({ "id": "web", "label": "web" }))
                .in_tree("modules")
                .under("app")
                .at(0),
            Patch::replace(
                "refresh",
                json!({ "type": "button", "id": "refresh", "text": "Stop" }),
            ),
        ]);
        assert_eq!(
            Value::from(patches),
            json!({ "patches": [
                { "op": "set", "id": "filter", "fields": { "enabled": false } },
                { "op": "remove", "id": "guava", "canvas": "graph" },
                { "op": "insert", "node": { "id": "web", "label": "web" }, "tree": "modules", "parent": "app", "index": 0 },
                { "op": "replace", "id": "refresh", "node": { "type": "button", "id": "refresh", "text": "Stop" } },
            ] })
        );
    }
}
