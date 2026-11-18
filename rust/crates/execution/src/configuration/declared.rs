//! Run configuration kinds a plugin declares (`build.run-type`).
//!
//! A build system may declare kinds of run configuration as data: the fields
//! their editor shows and the command line the values make
//! (`buildSystems[].runConfigurationTypes`). The build crate checks the
//! declaration and turns it into a [`DeclaredRunType`]; this module draws the
//! fields of such a kind, checks and stores the values the editor returns, and
//! reads them back for a launch, so neither the host nor the plugin parses a
//! saved entry. A saved entry keeps the plugin, build system, kind and field
//! values in `extensions.build`; its arguments, working directory and
//! environment are the top-level keys every run has.

use super::process::{self, FieldError};
use super::*;
use serde_json::Map;

/// Provider of a configuration of a plugin-declared kind.
pub const RUN_TYPE_PROVIDER: &str = "build.run-type";

/// Prefix of a declared field's ID among the editor's own fields, so a
/// plugin field never takes the ID of a field every run has.
const FIELD_PREFIX: &str = "field:";
const MAX_IDENTIFIER_BYTES: usize = 200;
/// Matches the most fields a kind may declare.
const MAX_VALUES: usize = 16;
const MAX_VALUE_BYTES: usize = 4096;
const MAX_VALUE_ARGUMENTS: usize = 64;

/// The identity of a declared kind: the plugin, its build system, and the
/// kind's ID within it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunTypeKey {
    /// Plugin that declares the build system.
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    /// Build system within that plugin.
    #[serde(rename = "buildSystemID")]
    pub build_system_id: String,
    /// Kind ID within the build system.
    #[serde(rename = "type")]
    pub type_id: String,
}

/// What a saved entry of a declared kind keeps in `extensions.build`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunTypeReference {
    /// Plugin that declares the build system.
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    /// Build system within that plugin.
    #[serde(rename = "buildSystemID")]
    pub build_system_id: String,
    /// Kind ID within the build system.
    #[serde(rename = "type")]
    pub type_id: String,
    /// Field values by declared field ID: a string, a boolean, or a list of
    /// strings. Hidden fields are not saved.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub values: Map<String, Value>,
}

impl RunTypeReference {
    /// The kind the entry belongs to.
    pub fn key(&self) -> RunTypeKey {
        RunTypeKey {
            plugin_id: self.plugin_id.clone(),
            build_system_id: self.build_system_id.clone(),
            type_id: self.type_id.clone(),
        }
    }

    /// The reference of a saved `build.run-type` entry.
    pub(crate) fn of(configuration: &RunConfiguration) -> Result<Self, CoreError> {
        let reference: Self = serde_json::from_value(
            configuration
                .extensions
                .get("build")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .map_err(|_| invalid_reference())?;
        reference.validate()?;
        Ok(reference)
    }

    /// Persistence checks shapes and sizes only; whether the kind is still
    /// declared and the values still fit it is checked by the editor and at
    /// launch.
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value.len() <= MAX_IDENTIFIER_BYTES
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        };
        let text = |value: &str| value.len() <= MAX_VALUE_BYTES && !value.contains('\0');
        let value = |value: &Value| match value {
            Value::Bool(_) => true,
            Value::String(value) => text(value),
            Value::Array(items) => {
                items.len() <= MAX_VALUE_ARGUMENTS
                    && items.iter().all(|item| item.as_str().is_some_and(&text))
            }
            _ => false,
        };
        if [&self.plugin_id, &self.build_system_id, &self.type_id]
            .iter()
            .any(|id| !identifier(id))
            || self.values.len() > MAX_VALUES
            || self
                .values
                .iter()
                .any(|(id, item)| !identifier(id) || !value(item))
        {
            return Err(invalid_reference());
        }
        Ok(())
    }
}

fn invalid_reference() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid run configuration type reference",
    )
}

/// A kind of run configuration a plugin declares, checked by the build crate.
#[derive(Debug, Clone)]
pub struct DeclaredRunType {
    /// Its identity.
    pub key: RunTypeKey,
    /// Shown in the + menu and as the default name of a new configuration.
    pub display_name: String,
    /// One sentence on what the kind runs.
    pub description: Option<String>,
    /// The plugin's `UIIcon`, passed to the host unchanged.
    pub icon: Option<Value>,
    /// How a started configuration behaves; never `service` or `group`.
    pub execution: Execution,
    /// Whether its command line takes the user's program arguments; the
    /// editor shows the program arguments field only then.
    pub takes_arguments: bool,
    /// The fields of its editor, in display order.
    pub fields: Vec<DeclaredField>,
}

/// What a declared field edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredFieldKind {
    /// One line of text.
    Text,
    /// One of the declared choices.
    Choice,
    /// On or off.
    Checkbox,
    /// A file, saved project-relative when it is inside the project.
    File,
    /// A directory, saved like a file.
    Directory,
    /// A command line split into arguments.
    Arguments,
}

impl DeclaredFieldKind {
    /// The editor field kind a host draws.
    fn editor_kind(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Choice => "choice",
            Self::Checkbox => "checkbox",
            Self::File => "file",
            Self::Directory => "directory",
            Self::Arguments => "arguments",
        }
    }
}

/// One field of a declared kind.
#[derive(Debug, Clone)]
pub struct DeclaredField {
    /// ID within the kind.
    pub id: String,
    /// What it edits.
    pub kind: DeclaredFieldKind,
    /// Its label.
    pub title: String,
    /// A sentence shown with it.
    pub description: Option<String>,
    /// Shown while it is empty.
    pub placeholder: Option<String>,
    /// The values of a choice field.
    pub choices: Vec<DeclaredChoice>,
    /// The value of a new configuration; checked against `kind` by the build
    /// crate. Absent means empty, off, or the first choice.
    pub default: Option<Value>,
    /// A launch needs a value.
    pub required: bool,
    /// Shown, saved and used only while an earlier choice or checkbox field
    /// has this value.
    pub visible_when: Option<DeclaredCondition>,
}

/// One value of a choice field.
#[derive(Debug, Clone)]
pub struct DeclaredChoice {
    /// The saved value.
    pub value: String,
    /// Its label; the value itself when absent.
    pub title: Option<String>,
}

/// When a field is shown.
#[derive(Debug, Clone)]
pub struct DeclaredCondition {
    /// An earlier choice or checkbox field.
    pub field: String,
    /// The value it must have.
    pub equals: Value,
}

/// The value a launch uses of one field. A field without one — empty, off,
/// or hidden — adds nothing to the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredValue {
    /// A text, choice, file or directory value; files and directories are absolute.
    Text(String),
    /// A checkbox that is on.
    Checked,
    /// The arguments of an arguments field.
    Arguments(Vec<String>),
}

/// A saved value a launch cannot use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredValueError {
    /// Declared field ID.
    pub field: String,
    /// `required` when a required field is empty, `invalidValue` when the
    /// value does not fit the field.
    pub code: &'static str,
}

impl DeclaredField {
    /// The value of a new configuration.
    fn initial(&self) -> Value {
        if let Some(value) = &self.default {
            return value.clone();
        }
        match self.kind {
            DeclaredFieldKind::Choice => self
                .choices
                .first()
                .map_or_else(|| json!(""), |choice| json!(choice.value)),
            DeclaredFieldKind::Checkbox => json!(false),
            DeclaredFieldKind::Arguments => json!([]),
            DeclaredFieldKind::Text | DeclaredFieldKind::File | DeclaredFieldKind::Directory => {
                json!("")
            }
        }
    }

    /// The editor key of the field.
    fn editor_key(&self) -> String {
        format!("{FIELD_PREFIX}{}", self.id)
    }

    /// The editor value of a saved value: arguments become argument text.
    fn editor_value(&self, value: Value) -> Value {
        match (self.kind, &value) {
            (DeclaredFieldKind::Arguments, Value::Array(items)) => {
                json!(super::form::argument_text(
                    &items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                ))
            }
            _ => value,
        }
    }
}

impl DeclaredRunType {
    fn field(&self, id: &str) -> Option<&DeclaredField> {
        self.fields.iter().find(|field| field.id == id)
    }

    /// Whether `field` is shown for `values`, keyed by declared field ID; a
    /// missing value is the field's initial one.
    fn visible(&self, field: &DeclaredField, values: &Map<String, Value>) -> bool {
        let Some(condition) = &field.visible_when else {
            return true;
        };
        let Some(controller) = self.field(&condition.field) else {
            return false;
        };
        let current = values
            .get(&controller.id)
            .cloned()
            .unwrap_or_else(|| controller.initial());
        current == condition.equals && self.visible(controller, values)
    }

    /// The editor values of a new configuration, by editor key.
    pub(super) fn default_values(&self) -> Map<String, Value> {
        self.fields
            .iter()
            .map(|field| (field.editor_key(), field.editor_value(field.initial())))
            .collect()
    }

    /// The editor values of saved field values, by editor key. A field the
    /// entry does not hold, such as one a newer plugin added, starts with its
    /// initial value.
    pub(super) fn saved_values(&self, reference: &RunTypeReference) -> Map<String, Value> {
        self.fields
            .iter()
            .map(|field| {
                let value = reference
                    .values
                    .get(&field.id)
                    .cloned()
                    .unwrap_or_else(|| field.initial());
                (field.editor_key(), field.editor_value(value))
            })
            .collect()
    }

    /// The field descriptions of the editor, after the header fields.
    pub(super) fn editor_fields(&self) -> Vec<Value> {
        self.fields
            .iter()
            .map(|field| {
                let mut item = json!({
                    "id": field.editor_key(),
                    "kind": field.kind.editor_kind(),
                    "label": field.title,
                });
                if let Some(description) = &field.description {
                    item["description"] = json!(description);
                }
                if let Some(placeholder) = &field.placeholder {
                    item["placeholder"] = json!(placeholder);
                }
                if field.required {
                    item["required"] = json!(true);
                }
                if field.kind == DeclaredFieldKind::Choice {
                    item["options"] = json!(field
                        .choices
                        .iter()
                        .map(|choice| choice.value.as_str())
                        .collect::<Vec<_>>());
                    item["optionLabels"] = json!(field
                        .choices
                        .iter()
                        .filter_map(|choice| Some((choice.value.clone(), choice.title.clone()?)))
                        .collect::<BTreeMap<_, _>>());
                }
                if let Some(condition) = &field.visible_when {
                    item["visibleWhen"] = json!({
                        "field": format!("{FIELD_PREFIX}{}", condition.field),
                        "equals": condition.equals,
                    });
                }
                item
            })
            .collect()
    }

    /// Checks the editor values and returns the field values in their saved
    /// form, with every value that is wrong. Hidden fields are not saved.
    /// A value of the wrong JSON type is a host fault, not a field error.
    pub(super) fn store(
        &self,
        root: &Path,
        editor: &Map<String, Value>,
    ) -> Result<(Map<String, Value>, Vec<FieldError>), CoreError> {
        let mut current = Map::new();
        for field in &self.fields {
            let key = field.editor_key();
            let value = match editor.get(&key) {
                None | Some(Value::Null) => field.editor_value(field.initial()),
                Some(value) => value.clone(),
            };
            let fits = match field.kind {
                DeclaredFieldKind::Checkbox => value.is_boolean(),
                DeclaredFieldKind::Choice => value
                    .as_str()
                    .is_some_and(|value| field.choices.iter().any(|choice| choice.value == value)),
                _ => value.is_string(),
            };
            if !fits {
                return Err(super::form::invalid_value(&key));
            }
            current.insert(field.id.clone(), value);
        }
        let mut stored = Map::new();
        let mut errors = Vec::new();
        for field in &self.fields {
            if !self.visible(field, &current) {
                continue;
            }
            let value = &current[&field.id];
            let key = field.editor_key();
            let saved = match field.kind {
                DeclaredFieldKind::Checkbox | DeclaredFieldKind::Choice => Some(value.clone()),
                DeclaredFieldKind::Arguments => {
                    let arguments =
                        super::launch::split_arguments(value.as_str().unwrap_or_default());
                    match process::arguments_limit_error(
                        &arguments,
                        MAX_VALUE_ARGUMENTS,
                        MAX_VALUE_BYTES,
                    ) {
                        Some(code) => {
                            errors.push(FieldError::new(key, code));
                            None
                        }
                        None => Some(json!(arguments)),
                    }
                }
                DeclaredFieldKind::Text
                | DeclaredFieldKind::File
                | DeclaredFieldKind::Directory => {
                    let text = value.as_str().unwrap_or_default().trim();
                    let checked = if text.is_empty() {
                        if field.required {
                            Err("required")
                        } else {
                            Ok(String::new())
                        }
                    } else {
                        match field.kind {
                            DeclaredFieldKind::File => super::form::stored_file(root, text, false),
                            DeclaredFieldKind::Directory => {
                                super::form::stored_directory(root, text)
                            }
                            _ => process::text_error(text, MAX_VALUE_BYTES)
                                .map_or_else(|| Ok(text.to_string()), Err),
                        }
                    };
                    match checked {
                        Ok(text) => Some(json!(text)),
                        Err(code) => {
                            errors.push(FieldError::new(key, code));
                            None
                        }
                    }
                }
            };
            if let Some(saved) = saved {
                stored.insert(field.id.clone(), saved);
            }
        }
        Ok((stored, errors))
    }

    /// The values a launch of an entry with the saved `values` uses, in
    /// field order. Files and directories become absolute against
    /// `workspace_root`; a saved value the kind no longer declares is
    /// ignored.
    pub fn launch_values(
        &self,
        values: &Map<String, Value>,
        workspace_root: &Path,
    ) -> Result<Vec<(String, DeclaredValue)>, DeclaredValueError> {
        let mut launch = Vec::new();
        for field in &self.fields {
            if !self.visible(field, values) {
                continue;
            }
            let error = |code| DeclaredValueError {
                field: field.id.clone(),
                code,
            };
            let value = values
                .get(&field.id)
                .cloned()
                .unwrap_or_else(|| field.initial());
            let used = match (field.kind, value) {
                (DeclaredFieldKind::Checkbox, Value::Bool(on)) => {
                    on.then_some(DeclaredValue::Checked)
                }
                (DeclaredFieldKind::Arguments, Value::Array(items)) => {
                    let arguments = items
                        .iter()
                        .map(|item| item.as_str().map(str::to_string))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| error("invalidValue"))?;
                    (!arguments.is_empty()).then_some(DeclaredValue::Arguments(arguments))
                }
                (
                    DeclaredFieldKind::Text
                    | DeclaredFieldKind::Choice
                    | DeclaredFieldKind::File
                    | DeclaredFieldKind::Directory,
                    Value::String(text),
                ) => {
                    if text.is_empty() {
                        if field.required {
                            return Err(error("required"));
                        }
                        None
                    } else if field.kind == DeclaredFieldKind::Choice
                        && !field.choices.iter().any(|choice| choice.value == text)
                    {
                        return Err(error("invalidValue"));
                    } else if matches!(
                        field.kind,
                        DeclaredFieldKind::File | DeclaredFieldKind::Directory
                    ) {
                        let path = process::file_path(workspace_root, &text, Platform::HOST);
                        Some(DeclaredValue::Text(path.to_string_lossy().into_owned()))
                    } else {
                        Some(DeclaredValue::Text(text))
                    }
                }
                _ => return Err(error("invalidValue")),
            };
            if let Some(used) = used {
                launch.push((field.id.clone(), used));
            }
        }
        Ok(launch)
    }

    /// The entry of this kind in `runConfig.configurationTypes`.
    pub(super) fn type_item(&self) -> Value {
        let mut item = json!({
            "provider": RUN_TYPE_PROVIDER,
            "runType": self.key,
            "displayName": self.display_name,
            "execution": self.execution,
        });
        if let Some(description) = &self.description {
            item["description"] = json!(description);
        }
        if let Some(icon) = &self.icon {
            item["icon"] = icon.clone();
        }
        item
    }
}

/// The declared kind `key` among `types`; `NotSupported` when no installed
/// plugin declares it, such as after the plugin was removed.
pub(super) fn declared<'a>(
    types: &'a [DeclaredRunType],
    key: &RunTypeKey,
) -> Result<&'a DeclaredRunType, CoreError> {
    types.iter().find(|item| &item.key == key).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotSupported,
            "No installed plugin declares this run configuration type",
        )
        .with_details(format!(
            "{}/{}/{}",
            key.plugin_id, key.build_system_id, key.type_id
        ))
    })
}

/// What a launch of a saved `build.run-type` entry uses besides the plugin's
/// declaration.
#[derive(Debug, Clone)]
pub struct DeclaredLaunch {
    /// Canonical workspace root, which relative paths are resolved against.
    pub root: PathBuf,
    /// The kind and the saved field values.
    pub reference: RunTypeReference,
    /// The user's program arguments.
    pub arguments: Vec<String>,
    /// Workspace-relative working directory; `.` is the workspace root.
    pub working_directory: String,
    /// Environment variables added to the plugin's.
    pub environment: BTreeMap<String, String>,
}

/// Reads the saved `build.run-type` entry `configuration_id` of the workspace
/// `root` for a launch, through the same layers the run list resolves.
/// `local_document` replaces the machine-local layer for this request.
pub fn declared_launch(
    root: &str,
    configuration_id: &str,
    local_document: Option<Value>,
) -> Result<DeclaredLaunch, CoreError> {
    let root = existing_root(root)?;
    let layers = read_layers(&root, None, local_document)?;
    let resolved = resolve_layers(&root, &layers, None)?;
    let entry = runnable_configuration(&resolved, configuration_id)?;
    let configuration: RunConfiguration = serde_json::from_value(entry.clone())
        .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid saved run configuration"))?;
    if configuration.provider != RUN_TYPE_PROVIDER || configuration.disabled {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration is not an enabled configuration of a declared type",
        ));
    }
    Ok(DeclaredLaunch {
        reference: RunTypeReference::of(&configuration)?,
        root,
        arguments: configuration.args,
        working_directory: configuration.cwd,
        environment: configuration.env,
    })
}

/// Checks a saved `build.run-type` entry: a valid reference, an execution
/// that is a process, and no keys only other providers use.
pub(crate) fn validate(configuration: &RunConfiguration) -> Result<(), CoreError> {
    RunTypeReference::of(configuration)?;
    if configuration.command.is_some()
        || !configuration.toolchains.is_empty()
        || configuration.debug.is_some()
        || !configuration.members.is_empty()
        || matches!(
            configuration.execution,
            Execution::Service | Execution::Group
        )
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Declared run configurations cannot store commands, tool bindings, debug adapters or members",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind() -> DeclaredRunType {
        DeclaredRunType {
            key: RunTypeKey {
                plugin_id: "dev.lithe.test.tool".into(),
                build_system_id: "tool".into(),
                type_id: "check".into(),
            },
            display_name: "Tool Check".into(),
            description: None,
            icon: None,
            execution: Execution::Task,
            takes_arguments: true,
            fields: vec![
                DeclaredField {
                    id: "scope".into(),
                    kind: DeclaredFieldKind::Choice,
                    title: "Scope".into(),
                    description: None,
                    placeholder: None,
                    choices: vec![
                        DeclaredChoice {
                            value: "package".into(),
                            title: Some("Package".into()),
                        },
                        DeclaredChoice {
                            value: "file".into(),
                            title: None,
                        },
                    ],
                    default: None,
                    required: false,
                    visible_when: None,
                },
                DeclaredField {
                    id: "package".into(),
                    kind: DeclaredFieldKind::Text,
                    title: "Package".into(),
                    description: None,
                    placeholder: Some("./...".into()),
                    choices: Vec::new(),
                    default: Some(json!("./...")),
                    required: true,
                    visible_when: Some(DeclaredCondition {
                        field: "scope".into(),
                        equals: json!("package"),
                    }),
                },
                DeclaredField {
                    id: "file".into(),
                    kind: DeclaredFieldKind::File,
                    title: "File".into(),
                    description: None,
                    placeholder: None,
                    choices: Vec::new(),
                    default: None,
                    required: true,
                    visible_when: Some(DeclaredCondition {
                        field: "scope".into(),
                        equals: json!("file"),
                    }),
                },
                DeclaredField {
                    id: "verbose".into(),
                    kind: DeclaredFieldKind::Checkbox,
                    title: "Verbose".into(),
                    description: None,
                    placeholder: None,
                    choices: Vec::new(),
                    default: None,
                    required: false,
                    visible_when: None,
                },
                DeclaredField {
                    id: "flags".into(),
                    kind: DeclaredFieldKind::Arguments,
                    title: "Tool arguments".into(),
                    description: None,
                    placeholder: None,
                    choices: Vec::new(),
                    default: None,
                    required: false,
                    visible_when: None,
                },
            ],
        }
    }

    /// A fresh project directory, removed when dropped.
    struct Project(PathBuf);

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl Project {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "lithe-declared-run-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    fn editor(values: Value) -> Map<String, Value> {
        values.as_object().unwrap().clone()
    }

    // A new configuration starts with each field's default, its first choice,
    // off, or empty, and its editor names every field after its declared ID.
    #[test]
    fn new_values_are_the_initial_ones() {
        let values = kind().default_values();
        assert_eq!(
            Value::Object(values),
            json!({"field:scope": "package", "field:package": "./...", "field:file": "",
                   "field:verbose": false, "field:flags": ""})
        );
        let fields = kind().editor_fields();
        assert_eq!(fields[0]["options"], json!(["package", "file"]));
        assert_eq!(fields[0]["optionLabels"], json!({"package": "Package"}));
        assert_eq!(
            fields[1]["visibleWhen"],
            json!({"field": "field:scope", "equals": "package"})
        );
        assert_eq!(fields[1]["required"], json!(true));
        assert_eq!(fields[4]["kind"], json!("arguments"));
    }

    // Saving keeps only the fields shown for the chosen values, trims text,
    // splits arguments, stores a file project-relative, and names a missing
    // required value by its editor key.
    #[test]
    fn store_keeps_the_shown_fields() {
        let root = Project::new();
        fs::write(root.path().join("main_test.go"), "").unwrap();
        let (stored, errors) = kind()
            .store(
                root.path(),
                &editor(
                    json!({"field:scope": "package", "field:package": " ./cmd/... ",
                               "field:file": "ignored", "field:verbose": true,
                               "field:flags": "-race -count 1"}),
                ),
            )
            .unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            Value::Object(stored),
            json!({"scope": "package", "package": "./cmd/...", "verbose": true,
                   "flags": ["-race", "-count", "1"]})
        );

        let (stored, errors) = kind()
            .store(
                root.path(),
                &editor(json!({"field:scope": "file", "field:file": "main_test.go"})),
            )
            .unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(stored["file"], json!("main_test.go"));

        let (_, errors) = kind()
            .store(
                root.path(),
                &editor(json!({"field:scope": "file", "field:file": ""})),
            )
            .unwrap();
        assert_eq!(errors, vec![FieldError::new("field:file", "required")]);
        let (_, errors) = kind()
            .store(
                root.path(),
                &editor(json!({"field:scope": "file", "field:file": "missing.go"})),
            )
            .unwrap();
        assert_eq!(errors, vec![FieldError::new("field:file", "notFound")]);
        assert!(kind()
            .store(root.path(), &editor(json!({"field:scope": "module"})))
            .is_err());
        assert!(kind()
            .store(root.path(), &editor(json!({"field:verbose": "yes"})))
            .is_err());
    }

    // A launch uses the shown fields that hold a value, makes files absolute,
    // and refuses a required field that is empty or a choice the kind no
    // longer has.
    #[test]
    fn launch_values_follow_the_shown_fields() {
        let root = Path::new("/workspace");
        let values = editor(
            json!({"scope": "file", "package": "./...", "file": "a_test.go",
                                   "verbose": false, "flags": ["-v"], "gone": 1}),
        );
        assert_eq!(
            kind().launch_values(&values, root).unwrap(),
            vec![
                ("scope".to_string(), DeclaredValue::Text("file".into())),
                (
                    "file".to_string(),
                    DeclaredValue::Text(
                        process::file_path(root, "a_test.go", Platform::HOST)
                            .to_string_lossy()
                            .into_owned()
                    )
                ),
                (
                    "flags".to_string(),
                    DeclaredValue::Arguments(vec!["-v".into()])
                ),
            ]
        );
        let defaults = kind().launch_values(&Map::new(), root).unwrap();
        assert_eq!(
            defaults,
            vec![
                ("scope".to_string(), DeclaredValue::Text("package".into())),
                ("package".to_string(), DeclaredValue::Text("./...".into())),
            ]
        );
        assert_eq!(
            kind()
                .launch_values(&editor(json!({"package": ""})), root)
                .unwrap_err()
                .field,
            "package"
        );
        assert_eq!(
            kind()
                .launch_values(&editor(json!({"scope": "module"})), root)
                .unwrap_err()
                .code,
            "invalidValue"
        );
    }

    // A reference holds identifiers and small values only.
    #[test]
    fn references_are_bounded() {
        let reference = |values: Value| RunTypeReference {
            plugin_id: "dev.lithe.test.tool".into(),
            build_system_id: "tool".into(),
            type_id: "check".into(),
            values: values.as_object().unwrap().clone(),
        };
        assert!(reference(json!({"a": "x", "b": true, "c": ["y"]}))
            .validate()
            .is_ok());
        assert!(reference(json!({"a": 1})).validate().is_err());
        assert!(reference(json!({"a": [1]})).validate().is_err());
        assert!(reference(json!({"a b": "x"})).validate().is_err());
        assert!(reference(json!({"a": "\0"})).validate().is_err());
        let mut many = Map::new();
        for index in 0..=MAX_VALUES {
            many.insert(format!("f{index}"), json!(""));
        }
        assert!(reference(Value::Object(many)).validate().is_err());
    }
}
