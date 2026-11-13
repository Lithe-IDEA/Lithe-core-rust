//! `keymap.*` commands: keymap import, contexts, conflicts and schemes.

use super::{plugin_response, rules_response};
use crate::protocol::{CoreResponse, KeymapCommand};
/// The manifest type every domain command validates plugin manifests with.
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
use serde_json::Value;

/// Runs one `keymap.*` command.
pub(super) fn dispatch(command: KeymapCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match command {
        KeymapCommand::DecodeImport => {
            plugin_response(id, payload, lithe_plugin_keymap_import::decode_command)
        }
        KeymapCommand::PlanImportSources => plugin_response(
            id,
            payload,
            lithe_plugin_keymap_import::plan_sources_command::<Manifest>,
        ),
        KeymapCommand::PlanImportBindings => plugin_response(
            id,
            payload,
            lithe_plugin_keymap_import::plan_bindings_command,
        ),
        KeymapCommand::ParseContext => rules_response(id, "keymap.parseContext", payload),
        KeymapCommand::EvaluateContext => rules_response(id, "keymap.evaluateContext", payload),
        KeymapCommand::AnalyzeConflicts => rules_response(id, "keymap.analyzeConflicts", payload),
        KeymapCommand::ModifierGestureStep => {
            rules_response(id, "keymap.modifierGestureStep", payload)
        }
        KeymapCommand::SchemesReduce => rules_response(id, "keymap.schemes.reduce", payload),
    }
}
