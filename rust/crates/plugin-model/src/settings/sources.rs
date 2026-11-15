//! Bounded native settings forms and confirmed edits of workspace-owned sources.
//! Plugins interpret their tools; native adapters own files, staging, conditional
//! publication and read-back. Full source documents never cross this boundary.

use crate::{decode_payload, is_identifier, outcome, PluginRejection, Validation};
use lithe_protocol::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SOURCES: usize = 1024;
const MAX_FIELDS: usize = 32;
const MAX_TEXT: usize = 8192;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[cfg(test)]
mod tests;

/// Read-only projection supplied by the function of a `sourceEditor` action.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSettings {
    /// Discriminator `litheSettingsSources`; not an arbitrary function result.
    pub format: String,
    /// Source-editor protocol version, currently zero.
    pub version: u32,
    /// Plugin-localized editor window title.
    pub title: String,
    /// Ordered sections use shared form or table controls, never plugin styles.
    pub groups: Vec<SourceGroup>,
    /// File bindings owned by this read snapshot.
    pub sources: Vec<SettingsSource>,
}

/// A presentation section; its identifier binds sources without interpreting them.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceGroup {
    /// Stable section identity within this editor.
    pub id: String,
    /// Plugin-localized section heading.
    pub title: String,
    /// `form` presents labeled controls; `table` presents source labels as rows.
    pub layout: String,
    /// Localized heading of the first table column; empty uses the host default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub row_title: String,
}

/// One source file inside the captured workspace. Revisions cover exact bytes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsSource {
    /// Opaque binding reused by edits and staging receipts.
    pub id: String,
    /// Identity of an existing presentation section.
    pub group: String,
    /// Localized source or module name displayed in the first table column.
    pub title: String,
    /// Normalized workspace-relative path, interpreted only by native I/O.
    pub path: String,
    /// `missing`, or `sha256:` followed by 64 lowercase hexadecimal digits.
    pub revision: String,
    /// Whether the plugin permits explicit edits of this source.
    pub writable: bool,
    /// Allowlisted non-sensitive fields; unrelated source content stays private.
    pub fields: Vec<SourceField>,
}

/// One explicitly non-sensitive raw value and its upstream effective value.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceField {
    /// Opaque field binding interpreted only by the source's plugin.
    pub id: String,
    /// Localized label or table column heading.
    pub title: String,
    /// Explanation displayed by the host's information control.
    #[serde(default)]
    pub description: String,
    /// `text`, `enum`, `boolean`, `stringList`, or `fileList`.
    pub kind: String,
    /// Null means the source has no explicit override.
    pub value: Value,
    /// Current value selected by the upstream tool, including inheritance.
    pub effective_value: Value,
    /// Upstream value after removing this override; null means not determined.
    #[serde(default)]
    pub inherited_value: Value,
    /// Localized upstream origin, suitable for a compact source indicator.
    pub effective_source: String,
    /// Upstream-supported choices; empty for non-enumeration controls.
    #[serde(default)]
    pub options: Vec<SourceOption>,
}

/// An upstream-supported choice; its opaque value never becomes a command line.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOption {
    /// Opaque native value written only after confirmation.
    pub value: String,
    /// Localized choice label.
    pub title: String,
}

/// Native controls retain these unparsed drafts until an explicit Save.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceInput {
    /// Unparsed text or newline-separated list draft.
    #[serde(default)]
    pub text: String,
    /// Selected enumeration value; it must occur in the snapshot's options.
    #[serde(default)]
    pub choice: String,
    /// Boolean control draft, ignored by other control kinds.
    #[serde(default)]
    pub on: bool,
    /// Explicitly retain or restore absence of a native override.
    #[serde(default)]
    pub inherit: bool,
}

/// A source-bound collection of changed controls, not a source document.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceEdits {
    /// Existing source binding from the captured snapshot.
    #[serde(rename = "sourceID")]
    pub source_id: String,
    /// Explicitly edited controls; omitted fields remain untouched.
    pub values: BTreeMap<String, SourceInput>,
}

/// One confirmed write. The adapter stages it with the action's own function.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceWrite {
    /// Existing source binding to which the staging receipt must refer.
    #[serde(rename = "sourceID")]
    pub source_id: String,
    /// Workspace-relative publication target copied from the snapshot.
    pub path: String,
    /// Exact native source revision that publication must still observe.
    pub revision: String,
    /// Changed native values only; null removes the corresponding override.
    pub values: BTreeMap<String, Value>,
}

fn rejected(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.settings.invalidSourceEditor",
        field,
        "Expected bounded, source-bound non-sensitive workspace settings.",
    )
}

fn text(value: &str, limit: usize, empty: bool) -> bool {
    (empty || !value.trim().is_empty())
        && value.len() <= limit
        && !value.chars().any(char::is_control)
}

fn identifier(value: &str) -> bool {
    value.len() <= 128 && is_identifier(value)
}

fn relative_path(value: &str) -> bool {
    text(value, 4096, false)
        && !value.contains(['\\', ':'])
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Whether a revision identifies exact native bytes or an absent source.
pub fn valid_source_revision(value: &str) -> bool {
    value == "missing"
        || value.strip_prefix("sha256:").is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn value_valid(field: &SourceField, value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    match field.kind.as_str() {
        "boolean" => value.is_boolean(),
        "enum" => value
            .as_str()
            .is_some_and(|value| field.options.iter().any(|option| option.value == value)),
        "text" => value
            .as_str()
            .is_some_and(|value| text(value, MAX_TEXT, true)),
        "stringList" | "fileList" => value.as_array().is_some_and(|items| {
            items.len() <= 256
                && items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|item| text(item, 4096, false)))
        }),
        _ => false,
    }
}

impl SourceSettings {
    /// Rejects malformed files, aliases, controls, values and section references.
    pub fn validate(&self) -> Validation<()> {
        if serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > MAX_BYTES) {
            return Err(rejected("snapshot"));
        }
        if self.format != "litheSettingsSources"
            || self.version != 0
            || !text(&self.title, 256, false)
            || self.groups.is_empty()
            || self.groups.len() > 32
            || self.sources.len() > MAX_SOURCES
        {
            return Err(rejected("snapshot"));
        }
        let mut groups = BTreeSet::new();
        for group in &self.groups {
            if !identifier(&group.id)
                || !groups.insert(&group.id)
                || !text(&group.title, 256, false)
                || !matches!(group.layout.as_str(), "form" | "table")
                || !text(&group.row_title, 256, true)
            {
                return Err(rejected("groups"));
            }
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for source in &self.sources {
            if !identifier(&source.id)
                || !ids.insert(&source.id)
                || !paths.insert(&source.path)
                || !groups.contains(&source.group)
                || !text(&source.title, 256, false)
                || !relative_path(&source.path)
                || !valid_source_revision(&source.revision)
                || source.fields.is_empty()
                || source.fields.len() > MAX_FIELDS
            {
                return Err(rejected("sources"));
            }
            let mut fields = BTreeSet::new();
            for field in &source.fields {
                if !identifier(&field.id)
                    || !fields.insert(&field.id)
                    || !text(&field.title, 256, false)
                    || !text(&field.description, 2048, true)
                    || !text(&field.effective_source, 1024, true)
                    || !matches!(
                        field.kind.as_str(),
                        "text" | "enum" | "boolean" | "stringList" | "fileList"
                    )
                    || !value_valid(field, &field.value)
                    || !value_valid(field, &field.effective_value)
                    || !value_valid(field, &field.inherited_value)
                {
                    return Err(rejected("fields"));
                }
                if field.kind == "enum" {
                    let mut choices = BTreeSet::new();
                    if field.options.is_empty() || field.options.len() > 64 {
                        return Err(rejected("options"));
                    }
                    for option in &field.options {
                        if !text(&option.value, 128, false)
                            || !choices.insert(&option.value)
                            || !text(&option.title, 256, false)
                        {
                            return Err(rejected("options"));
                        }
                    }
                } else if !field.options.is_empty() {
                    return Err(rejected("options"));
                }
            }
        }
        Ok(())
    }
}

impl SourceField {
    /// Projects an inherited value without making it an explicit override.
    pub fn input(&self) -> SourceInput {
        let mut input = SourceInput {
            inherit: self.value.is_null(),
            ..SourceInput::default()
        };
        let value = if input.inherit {
            &self.effective_value
        } else {
            &self.value
        };
        match self.kind.as_str() {
            "enum" => input.choice = value.as_str().unwrap_or_default().to_owned(),
            "boolean" => input.on = value.as_bool().unwrap_or_default(),
            "stringList" | "fileList" => {
                input.text = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            _ => input.text = value.as_str().unwrap_or_default().to_owned(),
        }
        input
    }

    fn edited_value(&self, input: &SourceInput) -> Validation<Value> {
        if input.inherit {
            return Ok(Value::Null);
        }
        let value = match self.kind.as_str() {
            "enum" => json!(input.choice),
            "boolean" => json!(input.on),
            "text" => json!(input.text),
            "stringList" | "fileList" => {
                if input.text.len() > 256 * 4096 {
                    return Err(rejected("input"));
                }
                json!(input
                    .text
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect::<Vec<_>>())
            }
            _ => return Err(rejected("input")),
        };
        if !value_valid(self, &value) {
            return Err(rejected("input"));
        }
        Ok(value)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateRequest {
    result: Value,
}

/// Validates a source-editor read before any native control sees its contents.
pub fn state_command(payload: Value) -> Result<Value, CoreError> {
    let request: StateRequest = decode_payload(payload, "plugins.settings.sourceState")?;
    Ok(outcome((|| {
        if request.result.to_string().len() > MAX_BYTES {
            return Err(rejected("result"));
        }
        let snapshot: SourceSettings =
            serde_json::from_value(request.result).map_err(|_| rejected("result"))?;
        snapshot.validate()?;
        let inputs: BTreeMap<_, BTreeMap<_, _>> = snapshot
            .sources
            .iter()
            .map(|source| {
                (
                    source.id.clone(),
                    source
                        .fields
                        .iter()
                        .map(|field| (field.id.clone(), field.input()))
                        .collect(),
                )
            })
            .collect();
        let inherited_inputs: BTreeMap<_, BTreeMap<_, _>> = snapshot
            .sources
            .iter()
            .map(|source| {
                (
                    source.id.clone(),
                    source
                        .fields
                        .iter()
                        .map(|field| {
                            let mut inherited = field.clone();
                            inherited.effective_value = if field.value.is_null() {
                                field.effective_value.clone()
                            } else {
                                field.inherited_value.clone()
                            };
                            inherited.value = Value::Null;
                            (field.id.clone(), inherited.input())
                        })
                        .collect(),
                )
            })
            .collect();
        let effective_displays: BTreeMap<_, BTreeMap<_, Value>> = snapshot
            .sources
            .iter()
            .map(|source| {
                let displays = source.fields.iter().filter(|field| {
                    (field.value.is_null() && field.kind != "enum")
                        || (!field.value.is_null() && field.value != field.effective_value)
                }).map(|field| {
                    let value = &field.effective_value;
                    let mut display = if value.is_null() {
                        json!({"kind":"unknown"})
                    } else {
                        match field.kind.as_str() {
                            "enum" => {
                                let title = field.options.iter().find(|option| Some(option.value.as_str()) == value.as_str())
                                    .map(|option| option.title.as_str()).unwrap_or_default();
                                json!({"kind":"text","text":title})
                            }
                            "boolean" => json!({"kind":"boolean","on":value.as_bool().unwrap_or_default()}),
                            "fileList" | "stringList" => json!({"kind":"list","count":value.as_array().map_or(0, Vec::len)}),
                            _ => json!({"kind":"text","text":value.as_str().unwrap_or_default()}),
                        }
                    };
                    display["inherited"] = json!(field.value.is_null());
                    (field.id.clone(), display)
                }).collect();
                (source.id.clone(), displays)
            }).collect();
        let layout: Vec<_> = snapshot
            .groups
            .iter()
            .enumerate()
            .map(|(index, group)| {
                let sources: Vec<_> = snapshot
                    .sources
                    .iter()
                    .enumerate()
                    .filter_map(|(index, source)| (source.group == group.id).then_some(index))
                    .collect();
                json!({"group":index,"sources":sources})
            })
            .collect();
        Ok((
            "state",
            json!({"snapshot":snapshot,"inputs":inputs,"inheritedInputs":inherited_inputs,"effectiveDisplays":effective_displays,"layout":layout,"limits":{
                "maximumSourceBytes":MAX_BYTES,"maximumTotalBytes":4*MAX_BYTES
            }}),
        ))
    })()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditRequest {
    snapshot: SourceSettings,
    edits: Vec<SourceEdits>,
    confirmed: bool,
}

/// Derives only changed raw values; native file access and publication are absent.
pub fn edit_command(payload: Value) -> Result<Value, CoreError> {
    let request: EditRequest = decode_payload(payload, "plugins.settings.sourceEdit")?;
    Ok(outcome((|| {
        request.snapshot.validate()?;
        if !request.confirmed || request.edits.len() > MAX_SOURCES {
            return Err(rejected("confirmed"));
        }
        let mut ids = BTreeSet::new();
        let mut writes = Vec::new();
        for edit in request.edits {
            if !ids.insert(edit.source_id.clone()) || edit.values.len() > MAX_FIELDS {
                return Err(rejected("edits"));
            }
            let source = request
                .snapshot
                .sources
                .iter()
                .find(|source| source.id == edit.source_id)
                .ok_or_else(|| rejected("sourceID"))?;
            let mut values = BTreeMap::new();
            for (id, input) in edit.values {
                let field = source
                    .fields
                    .iter()
                    .find(|field| field.id == id)
                    .ok_or_else(|| rejected("fieldID"))?;
                let value = field.edited_value(&input)?;
                if value != field.value {
                    values.insert(id, value);
                }
            }
            if values.is_empty() {
                continue;
            }
            if !source.writable {
                return Err(rejected("writable"));
            }
            writes.push(SourceWrite {
                source_id: source.id.clone(),
                path: source.path.clone(),
                revision: source.revision.clone(),
                values,
            });
        }
        let order: BTreeMap<_, _> = request
            .snapshot
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| (&source.id, index))
            .collect();
        writes.sort_by_key(|write| order.get(&write.source_id).copied());
        Ok(("writes", json!(writes)))
    })()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    format: String,
    version: u32,
    #[serde(rename = "sourceID")]
    source_id: String,
    revision: String,
    changed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptRequest {
    write: SourceWrite,
    result: Value,
}

/// Binds a private staged-file receipt to the confirmed source and exact digest.
pub fn receipt_command(payload: Value) -> Result<Value, CoreError> {
    let request: ReceiptRequest = decode_payload(payload, "plugins.settings.sourceReceipt")?;
    Ok(outcome((|| {
        let receipt: Receipt =
            serde_json::from_value(request.result).map_err(|_| rejected("result"))?;
        if receipt.format != "litheSettingsSourceEdit"
            || receipt.version != 0
            || receipt.source_id != request.write.source_id
            || !valid_source_revision(&receipt.revision)
            || receipt.revision == "missing"
            || !receipt.changed && receipt.revision != request.write.revision
        {
            return Err(rejected("receipt"));
        }
        Ok((
            "receipt",
            json!({"sourceID":receipt.source_id,"revision":receipt.revision,"changed":receipt.changed}),
        ))
    })()))
}
