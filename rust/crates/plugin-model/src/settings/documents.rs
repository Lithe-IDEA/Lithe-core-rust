//! Bounded plugin preference documents, edit validation and source projections.
//!
//! Native adapters own storage, conflict checks and file publication. This
//! module never opens paths or grants permission to start a plugin process.

use super::localization::shown_section;
use super::{
    resolve, SettingAction, SettingDeclaration, SettingEffect, SettingLayers, SettingScope,
    SettingsLocalization,
};
use crate::names::is_plugin_id;
use crate::rejection::{PluginRejection, Validation};
use lithe_protocol::json_document::{parse_unique_json, UniqueMap};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Most UTF-8 bytes one stored layer or edited document may contain.
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// One value change or unparsed editor input. Without text, null removes the
/// override and exposes inheritance.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsEdit {
    /// Declaration identifier, unique within the complete edit batch.
    pub(crate) id: String,
    /// A declaration-validated value, or null to remove only this layer.
    pub(crate) value: Value,
    /// Unparsed editor text. When present, value must be null; Core converts
    /// integer text before applying the declaration's ordinary validation.
    #[serde(default)]
    pub(crate) text: Option<String>,
}

type Document = UniqueMap<UniqueMap<Value>>;

fn invalid_document(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.settings.invalidDocument",
        field,
        "Expected a bounded JSON object of plugin IDs and unique setting maps.",
    )
}

fn read_document(content: Option<&str>, field: &str) -> Validation<Document> {
    let Some(content) = content else {
        return Ok(Document::default());
    };
    if content.len() > MAX_DOCUMENT_BYTES {
        return Err(invalid_document(field));
    }
    let checked = parse_unique_json(content).map_err(|_| invalid_document(field))?;
    let document: Document =
        serde_json::from_value(checked).map_err(|_| invalid_document(field))?;
    for (plugin, settings) in &document.0 {
        if !is_plugin_id(plugin)
            || settings
                .0
                .keys()
                .any(|id| !super::is_setting_identifier(id))
        {
            return Err(invalid_document(field));
        }
    }
    Ok(document)
}

/// Projects declared rows and raw launch layers from complete native documents.
/// Unknown settings remain in storage but are not sent across the UI boundary.
/// `localization` translates the text rows, actions and section headings show;
/// IDs, values, grouping and order stay those of the manifest.
pub fn document_state(
    plugin: &str,
    declarations: &[SettingDeclaration],
    actions: &[SettingAction],
    global: Option<&str>,
    project: Option<&str>,
    localization: Option<&SettingsLocalization>,
) -> Validation<Value> {
    let global = read_document(global, "globalDocument")?;
    let project = read_document(project, "projectDocument")?;
    let empty = BTreeMap::new();
    let global = global.0.get(plugin).map(|group| &group.0).unwrap_or(&empty);
    let project = project
        .0
        .get(plugin)
        .map(|group| &group.0)
        .unwrap_or(&empty);
    let global_resolved = resolve(
        declarations,
        SettingLayers {
            global,
            project: &empty,
        },
    );
    let effective = resolve(declarations, SettingLayers { global, project });
    let shown = |declaration: &SettingDeclaration| {
        localization.map_or_else(
            || declaration.clone(),
            |localization| localization.declaration(declaration),
        )
    };
    let rows: Vec<_> = declarations.iter().zip(global_resolved.iter()).zip(effective.iter()).map(|((declaration, global_value), effective)| json!({
        "declaration": shown(declaration), "scope": declaration.scope(), "effect": declaration.effect(),
        "global": global_value, "effective": effective,
        "globalStored": global.contains_key(&declaration.id),
        "projectStored": project.contains_key(&declaration.id),
    })).collect();
    let declared_values = |values: &BTreeMap<String, Value>| -> BTreeMap<String, Value> {
        declarations
            .iter()
            .filter_map(|declaration| {
                values
                    .get(&declaration.id)
                    .map(|value| (declaration.id.clone(), value.clone()))
            })
            .collect()
    };
    let mut sections: BTreeMap<String, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    let mut global_editor = Vec::with_capacity(declarations.len());
    let mut project_editor = Vec::with_capacity(declarations.len());
    for (index, ((declaration, global_value), effective_value)) in declarations
        .iter()
        .zip(global_resolved.iter())
        .zip(effective.iter())
        .enumerate()
    {
        sections
            .entry(declaration.section.clone().unwrap_or_default())
            .or_default()
            .0
            .push(index);
        let invalid_global = global
            .get(&declaration.id)
            .is_some_and(|value| !declaration.accepts(value));
        let project_editable = declaration.scope() == SettingScope::Project;
        let invalid_project = if project_editable {
            project
                .get(&declaration.id)
                .is_some_and(|value| !declaration.accepts(value))
        } else {
            invalid_global
        };
        global_editor.push(json!({"value":global_value.value, "resetValue":declaration.default,
            "input":editor_input(declaration,&global_value.value), "resetInput":editor_input(declaration,&declaration.default),
            "source":global_value.source, "editable":true, "overridden":global.contains_key(&declaration.id), "invalidStored":invalid_global}));
        project_editor.push(json!({"value":effective_value.value, "resetValue":global_value.value,
            "input":editor_input(declaration,&effective_value.value), "resetInput":editor_input(declaration,&global_value.value),
            "source":effective_value.source, "editable":project_editable,
            "overridden":project_editable && project.contains_key(&declaration.id), "invalidStored":invalid_project}));
    }
    for (index, action) in actions.iter().enumerate() {
        sections
            .entry(action.section.clone().unwrap_or_default())
            .or_default()
            .1
            .push(index);
    }
    let sections: Vec<_> = sections
        .into_iter()
        .map(|(title, (rows, actions))| json!({"title":shown_section(localization, title), "rows":rows, "actions":actions}))
        .collect();
    let shown_actions: Vec<_> = actions
        .iter()
        .map(|action| {
            localization.map_or_else(
                || action.clone(),
                |localization| localization.action(action),
            )
        })
        .collect();
    Ok(
        json!({"pluginID":plugin, "rows":rows, "actions":shown_actions, "sections":sections,
        "pages":super::page_sections(declarations, actions, localization),
        "editors":{"global":global_editor,"project":project_editor},
        "settings":declared_values(global), "projectSettings":declared_values(project),
        "storage":{"globalRelativePath":"plugin-settings.json", "projectRelativePath":".lithe/plugin-settings.json", "maximumBytes":MAX_DOCUMENT_BYTES, "readRequired":!declarations.is_empty()}}),
    )
}

/// Widget input for an already validated resolved value or reset value.
/// Numeric text and option positions are presentation data, not host rules.
fn editor_input(declaration: &SettingDeclaration, value: &Value) -> Value {
    match declaration.kind.as_str() {
        super::SETTING_INTEGER => json!({"text":value.to_string()}),
        super::SETTING_BOOLEAN => json!({"on":value == &Value::Bool(true)}),
        super::SETTING_ENUM => {
            json!({"selected":declaration.options.iter().position(|option| value.as_str() == Some(option.value.as_str())).unwrap_or_default()})
        }
        super::SETTING_MULTI_SELECT => {
            json!({"checks":declaration.options.iter().map(|option| value.as_array().is_some_and(|values| values.iter().any(|value| value.as_str() == Some(option.value.as_str())))).collect::<Vec<_>>()})
        }
        _ => json!({"text":value.as_str().unwrap_or_default()}),
    }
}

/// Validates the entire batch before editing one layer. Siblings and dormant
/// values survive; the host receives text to publish, never an I/O instruction.
pub fn edit_document(
    plugin: &str,
    declarations: &[SettingDeclaration],
    scope: SettingScope,
    content: Option<&str>,
    edits: Vec<SettingsEdit>,
) -> Validation<Value> {
    let mut document = read_document(content, "document")?;
    if edits.is_empty() || edits.len() > super::MAX_SETTINGS {
        return Err(PluginRejection::at(
            "plugin.settings.invalidEdit",
            "edits",
            "Supply one bounded edit per declared setting.",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut prepared = Vec::with_capacity(edits.len());
    for (index, mut edit) in edits.into_iter().enumerate() {
        let field = format!("edits[{index}]");
        let declaration = declarations.iter().find(|setting| setting.id == edit.id);
        let Some(declaration) = declaration else {
            return Err(PluginRejection::at(
                "plugin.settings.invalidEdit",
                format!("{field}.id"),
                "The setting is not declared by this plugin.",
            ));
        };
        if !seen.insert(edit.id.clone())
            || (scope == SettingScope::Project && declaration.scope() != SettingScope::Project)
        {
            return Err(PluginRejection::at(
                "plugin.settings.invalidEdit",
                format!("{field}.id"),
                "The edit is repeated or has no project layer.",
            ));
        }
        if let Some(text) = edit.text.take() {
            if !edit.value.is_null() || text.len() > MAX_DOCUMENT_BYTES {
                return Err(PluginRejection::at(
                    "plugin.settings.invalidEdit",
                    format!("{field}.text"),
                    "Supply bounded editor text without a separate value.",
                ));
            }
            edit.value = if declaration.kind == super::SETTING_INTEGER {
                Value::from(text.trim().parse::<i64>().map_err(|_| {
                    PluginRejection::at(
                        "plugin.settings.invalidEdit",
                        format!("{field}.text"),
                        "Enter a whole number for this setting.",
                    )
                })?)
            } else {
                Value::String(text)
            };
        }
        if !edit.value.is_null() && !declaration.accepts(&edit.value) {
            return Err(PluginRejection::at(
                "plugin.settings.invalidEdit",
                format!("{field}.value"),
                "The value does not satisfy its setting declaration.",
            ));
        }
        prepared.push((edit, declaration));
    }
    let original = serde_json::to_value(&document).map_err(|_| invalid_document("document"))?;
    let group = document.0.entry(plugin.to_owned()).or_default();
    let mut effects = BTreeSet::new();
    for (edit, declaration) in prepared {
        let changed = if edit.value.is_null() {
            group.0.remove(&edit.id).is_some()
        } else if group.0.get(&edit.id) != Some(&edit.value) {
            group.0.insert(edit.id, edit.value);
            true
        } else {
            false
        };
        if changed {
            effects.insert(match declaration.effect() {
                SettingEffect::Restart => "restart",
                SettingEffect::Immediate => "immediate",
                SettingEffect::Reload => "reload",
                SettingEffect::NextRun => "nextRun",
            });
        }
    }
    if group.0.is_empty() {
        document.0.remove(plugin);
    }
    let changed =
        original != serde_json::to_value(&document).map_err(|_| invalid_document("document"))?;
    let text = if changed {
        let mut text =
            serde_json::to_string_pretty(&document).map_err(|_| invalid_document("document"))?;
        text.push('\n');
        text
    } else {
        content.unwrap_or("{}\n").to_owned()
    };
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(invalid_document("document"));
    }
    Ok(
        json!({"document":text, "delete":document.0.is_empty(), "changed":changed, "effects":effects}),
    )
}
