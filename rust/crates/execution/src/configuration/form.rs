//! The editor of built-in process runs, described as data.
//!
//! Core decides which fields a configuration kind has, their current and
//! automatic values, and how saved values are checked and stored; the host
//! only draws the fields and applies the returned [`FileChange`]s. A host
//! draws fields in the order given, skips a field whose `kind` it does not
//! know, and sends back value keys it does not know unchanged, so a later
//! Core can add fields without a host change.

use super::before_launch::{check_edit, steps_of, Resolved};
use super::process::{self, FieldError, Kind, ProcessOptions};
use super::*;

/// Version of the editor form description; a host refuses a newer one.
const FORM_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request for the configuration kinds a user can create on this platform.
pub struct ConfigurationTypesRequest {}

/// The kinds of configuration a user can create here, in display order.
/// Batch scripts are listed only on Windows.
pub fn configuration_types(_request: ConfigurationTypesRequest) -> Result<Value, CoreError> {
    let types = Kind::ALL
        .into_iter()
        .filter(|kind| kind.supported_on(Platform::HOST))
        .map(|kind| json!({ "provider": kind.provider() }))
        .collect::<Vec<_>>();
    Ok(json!({ "types": types }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request for the editor of a new or saved process run.
pub struct EditorFormRequest {
    /// Canonical workspace root.
    pub root: String,
    /// Provider of a new configuration, such as `process.shell`.
    #[serde(default)]
    pub provider: Option<String>,
    /// Saved configuration to edit; its provider wins over `provider`.
    #[serde(default)]
    pub configuration_id: Option<String>,
    /// Unsaved values of the open editor by field id. They replace the saved
    /// values, so automatic values follow the edit.
    #[serde(default)]
    pub values: Option<serde_json::Map<String, Value>>,
}

/// The fields, values and automatic values of a process run editor.
pub fn editor_form(request: EditorFormRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layers = read_layers(&root, None, None)?;
    let (kind, mut values) = match request.configuration_id.as_deref() {
        Some(id) => {
            let saved = SavedEntry::find(&layers, id)?;
            let kind = saved.kind()?;
            if request
                .provider
                .as_deref()
                .is_some_and(|provider| provider != kind.provider())
            {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Run configuration has a different provider",
                ));
            }
            (kind, saved.values(kind)?)
        }
        None => {
            let kind = creatable_kind(request.provider.as_deref().unwrap_or_default())?;
            (kind, default_values(kind))
        }
    };
    values.extend(request.values.unwrap_or_default());
    let draft = Draft::read(kind, &values)?;
    let candidates =
        Resolved::read(&root, &layers)?.candidates(request.configuration_id.as_deref());
    Ok(json!({
        "formVersion": FORM_VERSION,
        "provider": kind.provider(),
        "configurationId": request.configuration_id,
        "values": values,
        "fields": fields(kind, &root, &draft, candidates),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to save the values of a process run editor.
pub struct SaveConfigurationRequest {
    /// Canonical workspace root.
    pub root: String,
    /// Provider of the configuration, such as `process.shell`.
    pub provider: String,
    /// Saved configuration the values replace; absent creates one.
    #[serde(default)]
    pub configuration_id: Option<String>,
    /// Editor values by field id, as [`editor_form`] returned them.
    pub values: serde_json::Map<String, Value>,
    /// The installed plugins, whose identity a written plugin document records.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
}

/// Checks the editor's values and returns the changes that save them.
///
/// Replies `{status: "invalid", errors: [{field, code}]}` when a value is
/// wrong, else `{status: "saved", id, changes}`. The entry is saved whole in
/// the project layer when `storeAsProjectFile` is set and in the local layer
/// otherwise, and removed from the other layer, so it lives in one file.
/// Keys and extension namespaces the editor does not show are kept.
pub fn save_configuration(request: SaveConfigurationRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let kind = creatable_kind(&request.provider)?;
    let layers = read_layers(&root, None, None)?;
    let saved = match request.configuration_id.as_deref() {
        Some(id) => {
            let saved = SavedEntry::find(&layers, id)?;
            if saved.kind()? != kind {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Run configuration has a different provider",
                ));
            }
            Some(saved)
        }
        None => None,
    };
    let draft = Draft::read(kind, &request.values)?;
    let (stored, mut errors) = draft.store(&root);
    let id = match &saved {
        Some(saved) => saved.id.clone(),
        None => unique_user_configuration_id(&stored.name, &existing_ids(&root, &layers)),
    };
    if let Some(code) = check_edit(&root, &layers, &id, &draft.before_launch)? {
        errors.push(FieldError::new("beforeLaunch", code));
    }
    if !errors.is_empty() {
        return Ok(json!({ "status": "invalid", "errors": errors }));
    }
    let mut entry = saved
        .as_ref()
        .map(|saved| saved.merged.clone())
        .unwrap_or_else(|| json!({}));
    let object = entry.as_object_mut().ok_or_else(|| {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Run configuration must be an object",
        )
    })?;
    for key in ["source", "plugin", "availability"] {
        object.remove(key);
    }
    object.insert("id".into(), json!(id));
    object.insert("name".into(), json!(stored.name));
    object.insert("provider".into(), json!(kind.provider()));
    object.insert("confidence".into(), json!("declared"));
    set_or_remove(
        object,
        "args",
        (!stored.arguments.is_empty()).then(|| json!(stored.arguments)),
    );
    set_or_remove(
        object,
        "cwd",
        (stored.working_directory != ".").then(|| json!(stored.working_directory)),
    );
    set_or_remove(
        object,
        "env",
        (!stored.environment.is_empty()).then(|| json!(stored.environment)),
    );
    set_or_remove(
        object,
        "beforeLaunch",
        (!draft.before_launch.is_empty()).then(|| json!(steps_of(&draft.before_launch))),
    );
    let extensions = object
        .entry("extensions")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "Run configuration extensions must be an object",
            )
        })?;
    let mut options = stored.options;
    if let Some(previous) = extensions.get("process") {
        // A later release's keys survive an edit made with this one.
        options.other = serde_json::from_value::<ProcessOptions>(previous.clone())
            .map(|previous| previous.other)
            .unwrap_or_default();
    }
    extensions.insert(
        "process".into(),
        serde_json::to_value(options).expect("process options encode"),
    );
    let typed: RunConfiguration = serde_json::from_value(entry.clone())
        .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid saved run configuration"))?;
    validate_configuration(&typed)?;
    let target = if stored.project {
        Scope::Project
    } else {
        Scope::Local
    };
    let other = if stored.project {
        Scope::Local
    } else {
        Scope::Project
    };
    let writing = write_context(&root, request.plugins.as_deref());
    let mut document = layers.layer(target).document.clone();
    remove_configuration(&mut document, &id)?;
    let items = document["configurations"]
        .as_array_mut()
        .expect("validated layers hold a configurations array");
    items.push(entry);
    items.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    let mut changes = layers
        .layer(target)
        .changes(&document, &writing.context())?;
    let mut other_document = layers.layer(other).document.clone();
    if remove_configuration(&mut other_document, &id)? {
        changes.extend(
            layers
                .layer(other)
                .changes(&other_document, &writing.context())?,
        );
    }
    Ok(json!({ "status": "saved", "id": id, "changes": changes }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to delete a user-authored configuration.
pub struct DeleteConfigurationRequest {
    /// Canonical workspace root.
    pub root: String,
    /// The `user:` configuration to delete.
    pub configuration_id: String,
    /// The installed plugins, whose identity a written plugin document records.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
}

/// The changes that remove a user-authored configuration from both saved
/// layers. Generated configurations come back on the next generation, so
/// they cannot be deleted; a user disables them instead.
pub fn delete_configuration(request: DeleteConfigurationRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    if !request.configuration_id.starts_with("user:") {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Only user configurations can be deleted",
        ));
    }
    let layers = read_layers(&root, None, None)?;
    let writing = write_context(&root, request.plugins.as_deref());
    let mut changes = Vec::new();
    let mut found = false;
    for scope in [Scope::Project, Scope::Local] {
        let mut document = layers.layer(scope).document.clone();
        if remove_configuration(&mut document, &request.configuration_id)? {
            found = true;
            changes.extend(layers.layer(scope).changes(&document, &writing.context())?);
        }
    }
    if !found {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration was not found",
        ));
    }
    Ok(json!({ "changes": changes }))
}

/// The kind of `provider` when this platform can create it.
fn creatable_kind(provider: &str) -> Result<Kind, CoreError> {
    let kind = Kind::from_provider(provider).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotSupported,
            "Only built-in process runs have an editor form",
        )
        .with_details(provider)
    })?;
    if !kind.supported_on(Platform::HOST) {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "Batch scripts run only on Windows",
        ));
    }
    Ok(kind)
}

/// Removes configuration `id` from a layer document; whether it was there.
fn remove_configuration(document: &mut Value, id: &str) -> Result<bool, CoreError> {
    let items = document["configurations"]
        .as_array_mut()
        .ok_or_else(|| CoreError::new(ErrorCode::ParseFailed, "configurations must be an array"))?;
    let before = items.len();
    items.retain(|item| item["id"] != id);
    Ok(items.len() != before)
}

fn set_or_remove(object: &mut serde_json::Map<String, Value>, key: &str, value: Option<Value>) {
    match value {
        Some(value) => {
            object.insert(key.to_string(), value);
        }
        None => {
            object.remove(key);
        }
    }
}

/// A saved configuration as both layers hold it.
struct SavedEntry {
    id: String,
    /// The project entry with the local entry merged over it.
    merged: Value,
    /// Whether the project layer holds it.
    in_project: bool,
}

impl SavedEntry {
    fn find(layers: &Layers, id: &str) -> Result<Self, CoreError> {
        let item = |document: &Value| {
            document["configurations"]
                .as_array()
                .and_then(|items| items.iter().find(|item| item["id"] == id))
                .cloned()
        };
        let project = item(&layers.project.document);
        let local = item(&layers.local.document);
        let merged = match (project.clone(), local) {
            (Some(mut merged), Some(local)) => {
                merge_configuration_value(&mut merged, &local)?;
                merged
            }
            (Some(entry), None) | (None, Some(entry)) => entry,
            (None, None) => {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Run configuration was not found",
                ))
            }
        };
        Ok(Self {
            id: id.to_string(),
            merged,
            in_project: project.is_some(),
        })
    }

    fn configuration(&self) -> Result<RunConfiguration, CoreError> {
        serde_json::from_value(self.merged.clone())
            .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid saved run configuration"))
    }

    fn kind(&self) -> Result<Kind, CoreError> {
        let configuration = self.configuration()?;
        creatable_kind(&configuration.provider)
    }

    /// The editor values of the saved entry.
    fn values(&self, kind: Kind) -> Result<serde_json::Map<String, Value>, CoreError> {
        let configuration = self.configuration()?;
        let options = ProcessOptions::of(&configuration)?;
        let mut values = default_values(kind);
        values.insert("name".into(), json!(configuration.name));
        values.insert("storeAsProjectFile".into(), json!(self.in_project));
        values.insert(
            kind.arguments_field().into(),
            json!(argument_text(&configuration.args)),
        );
        values.insert("workingDirectory".into(), json!(configuration.cwd));
        values.insert(
            "environment".into(),
            json!(configuration
                .env
                .iter()
                .map(|(name, value)| json!({ "name": name, "value": value }))
                .collect::<Vec<_>>()),
        );
        values.insert(
            "beforeLaunch".into(),
            json!(configuration
                .before_launch
                .iter()
                .map(|step| step.configuration.clone())
                .collect::<Vec<_>>()),
        );
        if kind.runs_script() {
            let text = options.script().is_none() && options.text.is_some();
            values.insert("source".into(), json!(if text { "text" } else { "file" }));
            values.insert("script".into(), json!(options.script.unwrap_or_default()));
            values.insert("text".into(), json!(options.text.unwrap_or_default()));
            values.insert(
                "interpreter".into(),
                json!(options.interpreter.unwrap_or_default()),
            );
            values.insert(
                "interpreterArguments".into(),
                json!(options
                    .interpreter_arguments
                    .as_deref()
                    .map(argument_text)
                    .unwrap_or_default()),
            );
        } else {
            values.insert("program".into(), json!(options.program.unwrap_or_default()));
        }
        if kind == Kind::PowerShell {
            values.insert(
                "executionPolicyBypass".into(),
                json!(options.execution_policy_bypass),
            );
        }
        Ok(values)
    }
}

/// Values of a new configuration of `kind`.
fn default_values(kind: Kind) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    values.insert("name".into(), json!(""));
    values.insert("storeAsProjectFile".into(), json!(false));
    if kind.runs_script() {
        values.insert("source".into(), json!("file"));
        values.insert("script".into(), json!(""));
        values.insert("text".into(), json!(""));
    } else {
        values.insert("program".into(), json!(""));
    }
    values.insert(kind.arguments_field().into(), json!(""));
    values.insert("workingDirectory".into(), json!("."));
    values.insert("environment".into(), json!([]));
    values.insert("beforeLaunch".into(), json!([]));
    if kind.runs_script() {
        values.insert("interpreter".into(), json!(""));
        values.insert("interpreterArguments".into(), json!(""));
    }
    if kind == Kind::PowerShell {
        values.insert("executionPolicyBypass".into(), json!(true));
    }
    values
}

fn argument_text(arguments: &[String]) -> String {
    crate::format_argument_text(crate::ArgumentTextRequest {
        argument_lists: vec![arguments.to_vec()],
    })
    .remove(0)
}

/// The fields of `kind`'s editor, with automatic values for `draft`.
fn fields(kind: Kind, root: &Path, draft: &Draft, candidates: Vec<Value>) -> Vec<Value> {
    let platform = Platform::HOST;
    let options = draft.options();
    let mut fields = vec![
        field("name", "text")
            .required()
            .header()
            .automatic(process::default_name(&options)),
        field("storeAsProjectFile", "checkbox").header(),
    ];
    if kind.runs_script() {
        let shebang = match draft.source {
            Source::File => options
                .script()
                .filter(|script| process::text_error(script, 4096).is_none())
                .and_then(|script| {
                    process::Shebang::of_file(&process::file_path(root, script.trim(), platform))
                }),
            Source::Text => options
                .text()
                .and_then(|text| process::Shebang::parse(text.as_bytes())),
        };
        let automatic = process::automatic_interpreter(kind, shebang.as_ref(), platform);
        let mut without_arguments = options.clone();
        without_arguments.interpreter_arguments = None;
        let effective = process::effective_interpreter(
            kind,
            &without_arguments,
            shebang.as_ref(),
            root,
            platform,
        );
        fields.extend([
            field("source", "choice").options(&["file", "text"]),
            field("script", "file")
                .required()
                .visible_when("source", "file"),
            field("text", "multilineText")
                .required()
                .visible_when("source", "text"),
            field(kind.arguments_field(), "arguments"),
            field("workingDirectory", "directory"),
            field("environment", "environment"),
            field("interpreter", "executable")
                .automatic(Some(automatic.executable.text().to_string())),
            field("interpreterArguments", "arguments")
                .automatic(Some(argument_text(&effective.arguments))),
        ]);
        if kind == Kind::PowerShell {
            fields.push(field("executionPolicyBypass", "checkbox").note("windowsOnly"));
        }
    } else {
        fields.extend([
            field("program", "executable").required(),
            field(kind.arguments_field(), "arguments"),
            field("workingDirectory", "directory"),
            field("environment", "environment"),
        ]);
    }
    fields.push(field("beforeLaunch", "beforeLaunch").candidates(candidates));
    fields.into_iter().map(|field| field.0).collect()
}

/// One field description being built.
struct Field(Value);

fn field(id: &str, kind: &str) -> Field {
    Field(json!({ "id": id, "kind": kind }))
}

impl Field {
    fn required(mut self) -> Self {
        self.0["required"] = json!(true);
        self
    }

    /// Drawn in the editor's top row, beside the name.
    fn header(mut self) -> Self {
        self.0["placement"] = json!("header");
        self
    }

    /// The value used while the field is empty; shown as its placeholder.
    fn automatic(mut self, value: Option<String>) -> Self {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            self.0["automatic"] = json!(value);
        }
        self
    }

    fn options(mut self, options: &[&str]) -> Self {
        self.0["options"] = json!(options);
        self
    }

    fn visible_when(mut self, field: &str, equals: &str) -> Self {
        self.0["visibleWhen"] = json!({ "field": field, "equals": equals });
        self
    }

    /// The run configurations a `beforeLaunch` field may name, `{id, name}`.
    fn candidates(mut self, candidates: Vec<Value>) -> Self {
        self.0["candidates"] = json!(candidates);
        self
    }

    /// A short explanation key, such as `windowsOnly`.
    fn note(mut self, note: &str) -> Self {
        self.0["note"] = json!(note);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    File,
    Text,
}

/// Editor values read into their types, before they are checked.
struct Draft {
    kind: Kind,
    name: String,
    project: bool,
    source: Source,
    script: String,
    text: String,
    program: String,
    arguments: String,
    working_directory: String,
    environment: Vec<(String, String)>,
    interpreter: String,
    interpreter_arguments: String,
    execution_policy_bypass: bool,
    /// The configurations started before this one, by ID.
    before_launch: Vec<String>,
}

/// Checked values, in the form they are saved.
struct Stored {
    name: String,
    project: bool,
    arguments: Vec<String>,
    working_directory: String,
    environment: BTreeMap<String, String>,
    options: ProcessOptions,
}

impl Draft {
    fn read(kind: Kind, values: &serde_json::Map<String, Value>) -> Result<Self, CoreError> {
        let text = |key: &str| -> Result<String, CoreError> {
            match values.get(key) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(value)) => Ok(value.clone()),
                Some(_) => Err(invalid_value(key)),
            }
        };
        let flag = |key: &str, default: bool| -> Result<bool, CoreError> {
            match values.get(key) {
                None | Some(Value::Null) => Ok(default),
                Some(Value::Bool(value)) => Ok(*value),
                Some(_) => Err(invalid_value(key)),
            }
        };
        let source = match text("source")?.as_str() {
            "" | "file" => Source::File,
            "text" => Source::Text,
            _ => return Err(invalid_value("source")),
        };
        let environment = match values.get("environment") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => serde_json::from_value::<Vec<EnvironmentRow>>(value.clone())
                .map_err(|_| invalid_value("environment"))?
                .into_iter()
                .map(|row| (row.name, row.value))
                .collect(),
        };
        Ok(Self {
            kind,
            name: text("name")?,
            project: flag("storeAsProjectFile", false)?,
            source,
            script: text("script")?,
            text: text("text")?,
            program: text("program")?,
            arguments: text(kind.arguments_field())?,
            working_directory: text("workingDirectory")?,
            environment,
            interpreter: text("interpreter")?,
            interpreter_arguments: text("interpreterArguments")?,
            execution_policy_bypass: flag("executionPolicyBypass", true)?,
            before_launch: match values.get("beforeLaunch") {
                None | Some(Value::Null) => Vec::new(),
                Some(value) => serde_json::from_value::<Vec<String>>(value.clone())
                    .map_err(|_| invalid_value("beforeLaunch"))?,
            },
        })
    }

    /// The options as typed, without checking or normalizing paths.
    fn options(&self) -> ProcessOptions {
        let some = |value: &str| (!value.trim().is_empty()).then(|| value.trim().to_string());
        let mut options = ProcessOptions::default();
        if self.kind.runs_script() {
            match self.source {
                Source::File => options.script = some(&self.script),
                Source::Text => {
                    options.text = (!self.text.trim().is_empty()).then(|| self.text.clone())
                }
            }
            options.interpreter = some(&self.interpreter);
            options.interpreter_arguments =
                some(&self.interpreter_arguments).map(|text| super::launch::split_arguments(&text));
        } else {
            options.program = some(&self.program);
        }
        if self.kind == Kind::PowerShell {
            options.execution_policy_bypass = self.execution_policy_bypass;
        }
        options
    }

    /// The values in their saved form, and every value that is wrong.
    fn store(&self, root: &Path) -> (Stored, Vec<FieldError>) {
        let mut errors = Vec::new();
        let mut options = self.options();
        if self.kind.runs_script() {
            match self.source {
                Source::File => match options.script.as_deref() {
                    None => errors.push(FieldError::new("script", "required")),
                    Some(script) => match stored_file(root, script, false) {
                        Ok(stored) => options.script = Some(stored),
                        Err(code) => errors.push(FieldError::new("script", code)),
                    },
                },
                Source::Text if options.text.is_none() => {
                    errors.push(FieldError::new("text", "required"))
                }
                Source::Text => {}
            }
            if let Some(interpreter) = options.interpreter.as_deref() {
                match stored_program(root, interpreter) {
                    Ok(stored) => options.interpreter = Some(stored),
                    Err(code) => errors.push(FieldError::new("interpreter", code)),
                }
            }
        } else {
            match options.program.as_deref() {
                None => errors.push(FieldError::new("program", "required")),
                Some(program) => match stored_program(root, program) {
                    Ok(stored) => options.program = Some(stored),
                    Err(code) => errors.push(FieldError::new("program", code)),
                },
            }
        }
        let working_directory = match self.working_directory.trim() {
            "" | "." => ".".to_string(),
            value => normalize_working_directory(root, value).unwrap_or_else(|_| {
                let target = process::file_path(root, value, Platform::HOST);
                let code = if target.is_dir() {
                    "outsideProject"
                } else {
                    "notFound"
                };
                errors.push(FieldError::new("workingDirectory", code));
                ".".to_string()
            }),
        };
        let mut environment = BTreeMap::new();
        for (name, value) in &self.environment {
            let name = name.trim();
            if name.is_empty() && value.is_empty() {
                continue;
            }
            if environment
                .insert(name.to_string(), value.clone())
                .is_some()
            {
                errors.push(FieldError::new("environment", "duplicate"));
            }
        }
        let name = match self.name.trim() {
            "" => process::default_name(&options).unwrap_or_default(),
            name => name.to_string(),
        };
        if name.is_empty() {
            errors.push(FieldError::new("name", "required"));
        }
        let stored = Stored {
            name,
            project: self.project,
            arguments: super::launch::split_arguments(&self.arguments),
            working_directory,
            environment,
            options,
        };
        // The saved entry must pass the same checks a launch makes; report
        // the ones the steps above do not, such as lengths and characters.
        // A missing value is left to the steps above, which name the field
        // of the chosen source: an empty text is not also a missing script.
        let configuration = RunConfiguration {
            id: "user:draft".into(),
            name: stored.name.clone(),
            provider: self.kind.provider().into(),
            execution: Execution::default(),
            category: Default::default(),
            command: None,
            args: stored.arguments.clone(),
            cwd: stored.working_directory.clone(),
            env: stored.environment.clone(),
            confidence: Confidence::Declared,
            toolchains: BTreeMap::new(),
            debug: None,
            members: Vec::new(),
            extensions: BTreeMap::from([(
                "process".to_string(),
                serde_json::to_value(&stored.options).expect("process options encode"),
            )]),
            before_launch: Vec::new(),
            disabled: false,
            source: None,
            plugin: None,
            availability: None,
        };
        if let Ok(found) = process::field_errors(&configuration, Platform::HOST) {
            for error in found {
                if error.code != "required"
                    && !errors.iter().any(|known| known.field == error.field)
                {
                    errors.push(error);
                }
            }
        }
        (stored, errors)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvironmentRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
}

fn invalid_value(field: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Invalid editor value").with_details(field)
}

/// A script file as saved: project-relative with `/` when it is inside the
/// project, else absolute.
fn stored_file(root: &Path, value: &str, keep_dot: bool) -> Result<String, &'static str> {
    if let Some(code) = process::text_error(value, 4096) {
        return Err(code);
    }
    let platform = Platform::HOST;
    let absolute = lithe_platform::paths::is_absolute_path(value, platform);
    let path = if absolute {
        PathBuf::from(value)
    } else {
        root.join(value)
    };
    if !path.is_file() {
        return Err("notFound");
    }
    let canonical = fs::canonicalize(&path).map_err(|_| "notFound")?;
    let canonical_root = fs::canonicalize(root).map_err(|_| "notFound")?;
    if let Ok(relative) = canonical.strip_prefix(&canonical_root) {
        let relative = slash_separated_path(relative, platform);
        // `./tool` keeps a project program from reading as a `PATH` name.
        return Ok(if keep_dot && !relative.contains('/') {
            format!("./{relative}")
        } else {
            relative
        });
    }
    if absolute {
        return Ok(value.to_string());
    }
    let text = canonical.to_string_lossy();
    Ok(text.strip_prefix(r"\\?\").unwrap_or(&text).to_string())
}

/// A program as saved: a bare name stays a `PATH` name, and a path is saved
/// like [`stored_file`].
fn stored_program(root: &Path, value: &str) -> Result<String, &'static str> {
    if !process::is_path(value) {
        return process::text_error(value, 4096).map_or(Ok(value.to_string()), Err);
    }
    stored_file(root, value, true)
}
