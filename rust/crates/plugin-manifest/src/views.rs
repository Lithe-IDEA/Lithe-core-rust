//! `plugins.ui.applyResult`: checks what a plugin backend returned for one
//! of its views and gives the host the whole view to draw.
//!
//! A view lives in a tool window body or a settings region the manifest
//! declares. The backend answers each view event with a whole view, patches
//! against the view the host drew last, or nothing. Core checks the result
//! against the surface's vocabulary and the plugin's declared settings,
//! actions and tools, so both products draw the same nodes and refuse the
//! same results; the host only renders.

use crate::{validate_manifest_value, DeclarativePluginManifest};
use lithe_plugin_model::{capabilities, decode_payload, outcome, PluginRejection, Validation};
use lithe_plugin_ui::{Context, Surface, INVALID_VIEW};
use lithe_protocol::CoreError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// The declared place a view is drawn in.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewSurface {
    /// A tool window whose body is a view.
    #[serde(default, rename = "toolWindowID")]
    tool_window_id: Option<String>,
    /// A settings region that draws a view.
    #[serde(default, rename = "settingRegionID")]
    setting_region_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyResultPayload {
    manifest: Value,
    surface: ViewSurface,
    /// The view the host drew last; absent before the first result.
    #[serde(default)]
    view: Option<Value>,
    /// What the backend returned: `{view}`, `{patches}` or `{}`.
    result: Value,
}

/// `plugins.ui.applyResult`: the checked whole view the host draws next and
/// whether it differs from the one it drew. A refused result is a rejected
/// outcome with code `plugin.ui.invalidView` and the JSON path of the first
/// problem; the host keeps drawing the previous view.
pub fn apply_result_command(payload: Value) -> Result<Value, CoreError> {
    let payload: ApplyResultPayload = decode_payload(payload, "plugins.ui.applyResult")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            let surface = view_surface(&manifest, &payload.surface)?;
            let tools = manifest.view_tools();
            let context = Context {
                surface,
                settings: &manifest.settings,
                actions: &manifest.setting_actions,
                tools: &tools,
                ui_version: manifest.capability_version(capabilities::UI),
            };
            let (view, changed) =
                lithe_plugin_ui::apply(&context, payload.view.as_ref(), &payload.result)?;
            Ok(("view", json!({"view": view, "changed": changed})))
        },
    )))
}

/// The surface the request names, which must be a declared place that draws
/// a view.
fn view_surface(
    manifest: &DeclarativePluginManifest,
    surface: &ViewSurface,
) -> Validation<Surface> {
    match (&surface.tool_window_id, &surface.setting_region_id) {
        (Some(id), None) => manifest
            .ui
            .as_ref()
            .and_then(|ui| ui.tool_windows.iter().find(|window| &window.id == id))
            .filter(|window| window.view.is_some())
            .map(|_| Surface::ToolWindow)
            .ok_or_else(|| {
                PluginRejection::at(
                    INVALID_VIEW,
                    "surface.toolWindowID",
                    "The plugin declares no tool window with this ID whose body is a view.",
                )
            }),
        (None, Some(id)) => manifest
            .setting_regions
            .iter()
            .find(|region| &region.id == id)
            .filter(|region| region.view.is_some())
            .map(|_| Surface::Settings)
            .ok_or_else(|| {
                PluginRejection::at(
                    INVALID_VIEW,
                    "surface.settingRegionID",
                    "The plugin declares no settings region with this ID that draws a view.",
                )
            }),
        _ => Err(PluginRejection::at(
            INVALID_VIEW,
            "surface",
            "Name exactly one of toolWindowID and settingRegionID.",
        )),
    }
}

impl DeclarativePluginManifest {
    /// Tools a view may show as a tool row: those the plugin defines and
    /// those it requires.
    fn view_tools(&self) -> BTreeSet<String> {
        self.tool_definitions
            .iter()
            .filter_map(|definition| definition["id"].as_str())
            .map(str::to_owned)
            .chain(self.requires_tools.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Value {
        json!({
            "schemaVersion": 0,
            "id": "io.github.example.views",
            "displayName": "Views",
            "version": "1.0.0",
            "pluginAPIVersion": 0,
            "requires": {"backend": 0, "settings": 0, "ui": 0},
            "vendor": {"id": "org.example", "displayName": "Example", "signatureRequirement": "publisherPackage"},
            "permissions": ["backend.launch", "ui.contribute"],
            "activationEvents": ["onView:io.github.example.views.panel", "onView:io.github.example.views.text"],
            "languages": [],
            "requiresTools": ["node"],
            "settings": [
                {"id": "mode", "type": "enum", "title": "Mode", "default": "fast",
                 "options": [{"value": "fast", "title": "Fast"}, {"value": "safe", "title": "Safe"}]}
            ],
            "settingRegions": [
                {"id": "overview", "keywords": ["overview"], "view": {"function": "settings.overview"}},
                {"id": "search", "page": {"buildSystemID": "example"}, "keywords": ["example"]}
            ],
            "ui": {"toolWindows": [
                {"id": "io.github.example.views.panel", "title": "Panel", "placement": "right",
                 "view": {"function": "panel.draw"}},
                {"id": "io.github.example.views.text", "title": "Text", "placement": "bottom", "text": "Hello"}
            ]},
            "backend": {"launch": {"kind": "packaged", "path": "bin/backend"}}
        })
    }

    fn apply(surface: Value, view: Option<Value>, result: Value) -> Value {
        let mut payload = json!({"manifest": manifest(), "surface": surface, "result": result});
        if let Some(view) = view {
            payload["view"] = view;
        }
        apply_result_command(payload).expect("outcome")
    }

    #[test]
    fn a_declared_view_surface_returns_the_checked_view() {
        let drawn = json!([{"type": "label", "id": "title", "text": "Hello"}]);
        let outcome = apply(
            json!({"toolWindowID": "io.github.example.views.panel"}),
            None,
            json!({"view": drawn}),
        );
        assert_eq!(outcome["status"], "ok", "{outcome}");
        assert_eq!(outcome["view"], json!({"view": drawn, "changed": true}));

        let outcome = apply(
            json!({"toolWindowID": "io.github.example.views.panel"}),
            Some(drawn.clone()),
            json!({"patches": [{"op": "set", "id": "title", "fields": {"text": "Bye"}}]}),
        );
        assert_eq!(outcome["view"]["view"][0]["text"], "Bye", "{outcome}");
        assert_eq!(outcome["view"]["changed"], true);
    }

    #[test]
    fn settings_regions_bind_declared_settings() {
        let outcome = apply(
            json!({"settingRegionID": "overview"}),
            None,
            json!({"view": [{"type": "segmented", "id": "mode", "setting": "mode"}]}),
        );
        assert_eq!(outcome["status"], "ok", "{outcome}");
        let outcome = apply(
            json!({"settingRegionID": "overview"}),
            None,
            json!({"view": [{"type": "toggle", "id": "missing", "setting": "missing"}]}),
        );
        assert_eq!(outcome["status"], "rejected", "{outcome}");
        assert_eq!(outcome["error"]["code"], INVALID_VIEW);
    }

    #[test]
    fn a_tool_picker_follows_the_declared_ui_version() {
        let picker = json!({"view": [{"type": "tool", "tool": "node", "choose": true}]});
        let outcome = apply(json!({"settingRegionID": "overview"}), None, picker.clone());
        assert_eq!(outcome["status"], "rejected", "{outcome}");
        assert_eq!(
            outcome["error"]["field"], "result.view[0].choose",
            "{outcome}"
        );

        let mut manifest = manifest();
        manifest["requires"]["ui"] = json!(1);
        let outcome = apply_result_command(json!({
            "manifest": manifest,
            "surface": {"settingRegionID": "overview"},
            "result": picker,
        }))
        .expect("outcome");
        assert_eq!(outcome["status"], "ok", "{outcome}");
    }

    #[test]
    fn only_declared_view_surfaces_accept_results() {
        for surface in [
            json!({"toolWindowID": "io.github.example.views.text"}),
            json!({"toolWindowID": "io.github.example.views.unknown"}),
            json!({"settingRegionID": "search"}),
            json!({}),
            json!({"toolWindowID": "io.github.example.views.panel", "settingRegionID": "overview"}),
        ] {
            let outcome = apply(surface.clone(), None, json!({"view": []}));
            assert_eq!(outcome["status"], "rejected", "{surface}: {outcome}");
            assert_eq!(outcome["error"]["code"], INVALID_VIEW, "{surface}");
        }
    }

    #[test]
    fn a_malformed_envelope_is_an_invalid_request() {
        let error = apply_result_command(json!({"manifest": manifest(), "result": {}}))
            .expect_err("surface is required");
        assert!(matches!(
            error.code,
            lithe_protocol::ErrorCode::InvalidRequest
        ));
    }
}
