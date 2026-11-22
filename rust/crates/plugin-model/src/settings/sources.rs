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
/// Entries of one `entryList` value, matching the list bound of `fileList`.
const MAX_ENTRIES: usize = 256;
/// Nesting bound of `sidebar` sources; deeper trees are not a presentation the host supports.
const MAX_DEPTH: usize = 8;

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
    /// `form` presents labeled controls; `table` presents source labels as rows;
    /// `sidebar` lists the sources for navigation and shows one selected source.
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
    /// A remote helper explicitly selects the workspace or remote home. A
    /// host-readable source outside those scopes is always read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RemoteSourceScope>,
    /// `missing`, or `sha256:` followed by 64 lowercase hexadecimal digits.
    pub revision: String,
    /// Whether the plugin permits explicit edits of this source.
    pub writable: bool,
    /// Allowlisted non-sensitive fields; unrelated source content stays private.
    pub fields: Vec<SourceField>,
    /// Earlier source of the same `sidebar` group under which the host nests
    /// this one; empty for a top-level source.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent: String,
}

/// File namespace used by remote native settings helpers.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum RemoteSourceScope {
    /// Relative to the remote workspace.
    Workspace,
    /// Relative to the remote account home.
    Home,
    /// An absolute host path the helper may display but the host never replaces.
    HostReadOnly,
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
    /// `text`, `enum`, `boolean`, `stringList`, `fileList`, or `entryList`.
    pub kind: String,
    /// Localized tab (`sidebar` group) or subheading (`form` group) holding the
    /// field; empty places it before any section. Table columns have none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub section: String,
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
    /// Kinds of entry an `entryList` holds, in the order of the host's add menu.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entry_kinds: Vec<EntryKind>,
    /// Per-entry columns of an `entryList`, such as whether an entry is exported.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<EntryAttribute>,
}

/// One kind of `entryList` entry and how the host lets the user add it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryKind {
    /// Opaque kind identity stored in each entry.
    pub id: String,
    /// Localized add-menu item, such as "JAR files".
    pub title: String,
    /// `file` and `directory` open the host's native picker, whose absolute
    /// native paths the plugin turns into its own spelling on write; `choice`
    /// offers `choices`.
    pub input: String,
    /// File-name extensions a `file` picker offers, without dots; empty allows any file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// Values a `choice` kind may add; empty when nothing can be added now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<EntryChoice>,
}

/// A value the user may add as a `choice` entry, such as a library or module.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryChoice {
    /// Opaque native value written only after confirmation.
    pub value: String,
    /// Localized label.
    pub title: String,
    /// Localized secondary text, such as a location or a count.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// A per-entry column; absent values mean the plugin's default for the entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryAttribute {
    /// Opaque attribute identity, a key of each entry's `attributes`.
    pub id: String,
    /// Localized column heading.
    pub title: String,
    /// `boolean` or `enum`.
    pub kind: String,
    /// Choices of an `enum` attribute; empty for `boolean`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<SourceOption>,
}

/// One ordered item of an `entryList` value. Only `kind`, `value` and
/// `attributes` are native data; `title` and `detail` are display text that
/// edits and comparisons ignore.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceEntry {
    /// Identity of one of the field's `entryKinds`.
    pub kind: String,
    /// Opaque native value: a path as the plugin spells it, or a choice value.
    pub value: String,
    /// Localized label; empty shows `value`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Localized secondary text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Declared attribute values; an absent key keeps the plugin's default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
}

impl SourceEntry {
    /// The native part of an entry, as edits carry it to the plugin.
    fn native(&self) -> Value {
        let mut value = json!({"kind":self.kind,"value":self.value});
        if !self.attributes.is_empty() {
            value["attributes"] = json!(self.attributes);
        }
        value
    }
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
    /// Ordered `entryList` draft, ignored by other control kinds.
    #[serde(default)]
    pub entries: Vec<SourceEntry>,
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
        "entryList" => value.as_array().is_some_and(|items| {
            let mut seen = BTreeSet::new();
            items.len() <= MAX_ENTRIES
                && items.iter().all(|item| {
                    entry(item).is_some_and(|item| {
                        entry_valid(field, &item) && seen.insert((item.kind, item.value))
                    })
                })
        }),
        _ => false,
    }
}

fn entry(value: &Value) -> Option<SourceEntry> {
    serde_json::from_value(value.clone()).ok()
}

fn entry_valid(field: &SourceField, entry: &SourceEntry) -> bool {
    field.entry_kinds.iter().any(|kind| kind.id == entry.kind)
        && text(&entry.value, 4096, false)
        && text(&entry.title, 256, true)
        && text(&entry.detail, 1024, true)
        && entry.attributes.iter().all(|(id, value)| {
            field
                .attributes
                .iter()
                .find(|attribute| &attribute.id == id)
                .is_some_and(|attribute| match attribute.kind.as_str() {
                    "boolean" => value.is_boolean(),
                    _ => value.as_str().is_some_and(|value| {
                        attribute.options.iter().any(|option| option.value == value)
                    }),
                })
        })
}

fn options_valid(options: &[SourceOption]) -> bool {
    let mut choices = BTreeSet::new();
    !options.is_empty()
        && options.len() <= 64
        && options.iter().all(|option| {
            text(&option.value, 128, false)
                && choices.insert(&option.value)
                && text(&option.title, 256, false)
        })
}

/// Validates the add menu and columns of an `entryList`, or their absence elsewhere.
fn entry_declarations_valid(field: &SourceField) -> bool {
    if field.kind != "entryList" {
        return field.entry_kinds.is_empty() && field.attributes.is_empty();
    }
    let mut kinds = BTreeSet::new();
    let mut attributes = BTreeSet::new();
    (1..=16).contains(&field.entry_kinds.len())
        && field.entry_kinds.iter().all(|kind| {
            let mut choices = BTreeSet::new();
            identifier(&kind.id)
                && kinds.insert(&kind.id)
                && text(&kind.title, 256, false)
                && matches!(kind.input.as_str(), "file" | "directory" | "choice")
                && (kind.input == "file" || kind.extensions.is_empty())
                && kind.extensions.len() <= 16
                && kind.extensions.iter().all(|extension| {
                    !extension.is_empty()
                        && extension.len() <= 32
                        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
                && (kind.input == "choice" || kind.choices.is_empty())
                && kind.choices.len() <= 1024
                && kind.choices.iter().all(|choice| {
                    text(&choice.value, 4096, false)
                        && choices.insert(&choice.value)
                        && text(&choice.title, 256, false)
                        && text(&choice.detail, 1024, true)
                })
        })
        && field.attributes.len() <= 4
        && field.attributes.iter().all(|attribute| {
            identifier(&attribute.id)
                && attributes.insert(&attribute.id)
                && text(&attribute.title, 256, false)
                && match attribute.kind.as_str() {
                    "boolean" => attribute.options.is_empty(),
                    "enum" => options_valid(&attribute.options),
                    _ => false,
                }
        })
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
        let mut groups = BTreeMap::new();
        for group in &self.groups {
            if !identifier(&group.id)
                || groups.insert(&group.id, group.layout.as_str()).is_some()
                || !text(&group.title, 256, false)
                || !matches!(group.layout.as_str(), "form" | "table" | "sidebar")
                || !text(&group.row_title, 256, true)
            {
                return Err(rejected("groups"));
            }
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        // Depth of each `sidebar` source; a parent must precede its children,
        // which also rules out cycles.
        let mut depths: BTreeMap<&str, (&str, usize)> = BTreeMap::new();
        for source in &self.sources {
            let Some(&layout) = groups.get(&source.group) else {
                return Err(rejected("sources"));
            };
            if layout == "sidebar" {
                let depth = if source.parent.is_empty() {
                    0
                } else {
                    match depths.get(source.parent.as_str()) {
                        Some((group, depth))
                            if *group == source.group && *depth + 1 < MAX_DEPTH =>
                        {
                            depth + 1
                        }
                        _ => return Err(rejected("parent")),
                    }
                };
                depths.insert(&source.id, (&source.group, depth));
            } else if !source.parent.is_empty() {
                return Err(rejected("parent"));
            }
            if !identifier(&source.id)
                || !ids.insert(&source.id)
                || !paths.insert((
                    source.scope.unwrap_or(RemoteSourceScope::Workspace),
                    &source.path,
                ))
                || !text(&source.title, 256, false)
                || match source.scope {
                    Some(RemoteSourceScope::HostReadOnly) => {
                        source.writable || !source.path.strip_prefix('/').is_some_and(relative_path)
                    }
                    _ => !relative_path(&source.path),
                }
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
                        "text" | "enum" | "boolean" | "stringList" | "fileList" | "entryList"
                    )
                    || !text(&field.section, 64, true)
                    || layout == "table" && (!field.section.is_empty() || field.kind == "entryList")
                    || !entry_declarations_valid(field)
                    || !value_valid(field, &field.value)
                    || !value_valid(field, &field.effective_value)
                    || !value_valid(field, &field.inherited_value)
                {
                    return Err(rejected("fields"));
                }
                if field.kind == "enum" {
                    if !options_valid(&field.options) {
                        return Err(rejected("options"));
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
            "entryList" => {
                input.entries = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(entry)
                    .collect();
            }
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
            "entryList" => {
                // A new choice entry must be one the plugin offers now or one
                // the source already holds; the host cannot invent containers.
                let known = |added: &SourceEntry| {
                    [&self.value, &self.effective_value]
                        .into_iter()
                        .filter_map(Value::as_array)
                        .flatten()
                        .filter_map(entry)
                        .any(|known| known.kind == added.kind && known.value == added.value)
                };
                for entry in &input.entries {
                    let offered = self.entry_kinds.iter().any(|kind| {
                        kind.id == entry.kind
                            && (kind.input != "choice"
                                || kind
                                    .choices
                                    .iter()
                                    .any(|choice| choice.value == entry.value))
                    });
                    if !offered && !known(entry) {
                        return Err(rejected("input"));
                    }
                }
                json!(input.entries)
            }
            _ => return Err(rejected("input")),
        };
        if !value_valid(self, &value) {
            return Err(rejected("input"));
        }
        Ok(self.native(&value))
    }

    /// Drops the display text of `entryList` entries, which never reaches a
    /// write and never makes two values differ.
    fn native(&self, value: &Value) -> Value {
        match (self.kind.as_str(), value.as_array()) {
            ("entryList", Some(items)) => items
                .iter()
                .map(|item| entry(item).map_or_else(|| item.clone(), |item| item.native()))
                .collect(),
            _ => value.clone(),
        }
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
                        || (!field.value.is_null()
                            && field.native(&field.value) != field.native(&field.effective_value))
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
                            "fileList" | "stringList" | "entryList" => json!({"kind":"list","count":value.as_array().map_or(0, Vec::len)}),
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
                if value != field.native(&field.value) {
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
