//! The editor of user-authored runs, described as data.
//!
//! Core decides which fields a configuration kind has, their current and
//! automatic values, and how saved values are checked and stored; the host
//! only draws the fields and applies the returned [`FileChange`]s. A host
//! draws fields in the order given, skips a field whose `kind` it does not
//! know, and sends back value keys it does not know unchanged, so a later
//! Core can add fields without a host change.
//!
//! The kinds are the built-in process runs and the kinds installed plugins
//! declare (`build.run-type`). The host passes the declared kinds with each
//! request, already checked and localized; a configuration whose plugin is
//! gone cannot be edited until the plugin is back.

use super::before_launch::{
    check_edit, editor_items, steps_from_editor, BeforeLaunchStep, Resolved, StepChoices,
};
use super::declared::{
    declared, DeclaredRole, DeclaredRunType, RunTypeKey, RunTypeReference, RUN_TYPE_PROVIDER,
};
use super::process::{self, FieldError, Kind, ProcessOptions};
use super::*;

/// Version of the editor form description; a host refuses a newer one.
const FORM_VERSION: u32 = 1;

/// The editor field of the program arguments of a declared kind.
const DECLARED_ARGUMENTS_FIELD: &str = "programArguments";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request for the configuration kinds a user can create on this platform.
pub struct ConfigurationTypesRequest {
    /// The kinds installed plugins declare, set by the dispatcher.
    #[serde(skip)]
    pub run_types: Vec<DeclaredRunType>,
}

/// The kinds of configuration a user can create, in display order: the
/// built-in process runs, then the kinds installed plugins declare. Every
/// platform creates and edits every built-in kind, so a project shared with
/// Windows keeps its batch scripts editable elsewhere; a kind this platform
/// cannot run carries the `availability` its configurations resolve with.
pub fn configuration_types(request: ConfigurationTypesRequest) -> Result<Value, CoreError> {
    let mut types = Kind::ALL
        .into_iter()
        .map(|kind| {
            let mut item = json!({ "provider": kind.provider() });
            if let Some(availability) = process::platform_availability(kind, run_platform()) {
                item["availability"] = json!(availability);
            }
            item
        })
        .collect::<Vec<_>>();
    types.extend(
        request
            .run_types
            .iter()
            .filter(|run_type| run_type.role == DeclaredRole::Configuration)
            .map(DeclaredRunType::type_item),
    );
    Ok(json!({ "types": types }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request for the editor of a new or saved user-authored run.
pub struct EditorFormRequest {
    /// Canonical workspace root.
    pub root: String,
    /// Provider of a new configuration, such as `process.shell`.
    #[serde(default)]
    pub provider: Option<String>,
    /// The declared kind of a new `build.run-type` configuration.
    #[serde(default)]
    pub run_type: Option<RunTypeKey>,
    /// Saved configuration to edit; its provider wins over `provider`.
    #[serde(default)]
    pub configuration_id: Option<String>,
    /// Unsaved values of the open editor by field id. They replace the saved
    /// values, so automatic values follow the edit.
    #[serde(default)]
    pub values: Option<serde_json::Map<String, Value>>,
    /// The kinds installed plugins declare, set by the dispatcher.
    #[serde(skip)]
    pub run_types: Vec<DeclaredRunType>,
}

/// The fields, values and automatic values of a run editor.
pub fn editor_form(request: EditorFormRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layers = read_layers(&root, None, None)?;
    let run_types = &request.run_types;
    let (editor, mut values) = match request.configuration_id.as_deref() {
        Some(id) => {
            let saved = SavedEntry::find(&layers, id)?;
            let editor = saved.editor(run_types)?;
            if request.provider.is_some()
                && !Editor::new(
                    request.provider.as_deref().unwrap_or_default(),
                    request.run_type.as_ref(),
                    run_types,
                )?
                .same(editor)
            {
                return Err(different_provider());
            }
            (editor, saved.values(editor, run_types)?)
        }
        None => {
            let editor = Editor::new(
                request.provider.as_deref().unwrap_or_default(),
                request.run_type.as_ref(),
                run_types,
            )?;
            (editor, editor.default_values())
        }
    };
    values.extend(request.values.unwrap_or_default());
    let choices = StepChoices::new(
        &Resolved::read(&root, &layers)?,
        request.configuration_id.as_deref(),
        run_types,
    );
    let fields = match editor {
        Editor::Process(kind) => fields(kind, &root, &Draft::read(kind, &values)?, choices),
        Editor::Declared(run_type) => {
            Common::read(&values, DECLARED_ARGUMENTS_FIELD)?;
            declared_fields(run_type, choices)
        }
    };
    let mut form = json!({
        "formVersion": FORM_VERSION,
        "provider": editor.provider(),
        "configurationId": request.configuration_id,
        "values": values,
        "fields": fields,
    });
    match editor {
        Editor::Process(kind) => {
            if let Some(availability) = process::platform_availability(kind, run_platform()) {
                form["availability"] = json!(availability);
            }
        }
        Editor::Declared(run_type) => form["runType"] = json!(run_type.key),
    }
    Ok(form)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to save the values of a run editor.
pub struct SaveConfigurationRequest {
    /// Canonical workspace root.
    pub root: String,
    /// Provider of the configuration, such as `process.shell`.
    pub provider: String,
    /// The declared kind of a `build.run-type` configuration.
    #[serde(default)]
    pub run_type: Option<RunTypeKey>,
    /// Saved configuration the values replace; absent creates one.
    #[serde(default)]
    pub configuration_id: Option<String>,
    /// Editor values by field id, as [`editor_form`] returned them.
    pub values: serde_json::Map<String, Value>,
    /// The installed plugins, whose identity a written plugin document records.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
    /// The kinds installed plugins declare, set by the dispatcher.
    #[serde(skip)]
    pub run_types: Vec<DeclaredRunType>,
}

/// Checks the editor's values and returns the changes that save them.
///
/// Replies `{status: "invalid", errors: [{field, code}]}` when a value is
/// wrong, else `{status: "saved", id, changes}`. The entry is saved whole in
/// the project layer when `storeAsProjectFile` is set and in the local layer
/// otherwise, and removed from the other layer, so it lives in one file; an
/// entry of a declared kind goes to its plugin's document of that layer.
/// Keys and extension namespaces the editor does not show are kept.
pub fn save_configuration(request: SaveConfigurationRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let run_types = &request.run_types;
    let editor = Editor::new(&request.provider, request.run_type.as_ref(), run_types)?;
    let layers = read_layers(&root, None, None)?;
    let saved = match request.configuration_id.as_deref() {
        Some(id) => {
            let saved = SavedEntry::find(&layers, id)?;
            if !saved.editor(run_types)?.same(editor) {
                return Err(different_provider());
            }
            Some(saved)
        }
        None => None,
    };
    let common = Common::read(&request.values, editor.arguments_field())?;
    let (stored, extension, mut errors) = match editor {
        Editor::Process(kind) => {
            let (stored, errors) = Draft::read(kind, &request.values)?.store(&root);
            (stored.common, Extension::Process(stored.options), errors)
        }
        Editor::Declared(run_type) => {
            let (values, mut errors) = run_type.store(&root, &request.values)?;
            let mut stored = common.store(&root, Some(run_type.display_name.clone()), &mut errors);
            if !run_type.takes_arguments {
                // A kind without program arguments keeps none, whatever an
                // older editor sent.
                stored.arguments.clear();
            }
            let reference = RunTypeReference {
                plugin_id: run_type.key.plugin_id.clone(),
                build_system_id: run_type.key.build_system_id.clone(),
                type_id: run_type.key.type_id.clone(),
                values,
            };
            let draft = stored.configuration(RUN_TYPE_PROVIDER, "build", json!(reference));
            for error in process::name_error(&stored.name).into_iter().chain(
                process::arguments_and_environment_errors(&draft, DECLARED_ARGUMENTS_FIELD),
            ) {
                if !errors.iter().any(|known| known.field == error.field) {
                    errors.push(error);
                }
            }
            (stored, Extension::Declared(reference), errors)
        }
    };
    let id = match &saved {
        Some(saved) => saved.id.clone(),
        None => unique_user_configuration_id(&stored.name, &existing_ids(&root, &layers)),
    };
    if let Some(code) = check_edit(&root, &layers, &id, &common.before_launch, run_types)? {
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
    object.insert("provider".into(), json!(editor.provider()));
    object.insert("confidence".into(), json!("declared"));
    if let Editor::Declared(run_type) = editor {
        set_or_remove(
            object,
            "execution",
            (run_type.execution != Execution::default()).then(|| json!(run_type.execution)),
        );
    }
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
        (!common.before_launch.is_empty()).then(|| json!(common.before_launch)),
    );
    set_or_remove(
        object,
        "allowMultipleInstances",
        stored.multiple.then(|| json!(true)),
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
    match extension {
        Extension::Process(mut options) => {
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
        }
        Extension::Declared(reference) => {
            extensions.insert(
                "build".into(),
                serde_json::to_value(reference).expect("run type references encode"),
            );
        }
    }
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

/// The provider-owned part of a saved entry.
enum Extension {
    Process(ProcessOptions),
    Declared(RunTypeReference),
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

/// What an editor edits: a built-in process kind, or a kind an installed
/// plugin declares.
#[derive(Clone, Copy)]
enum Editor<'a> {
    Process(Kind),
    Declared(&'a DeclaredRunType),
}

impl<'a> Editor<'a> {
    /// The kind of `provider`, and for `build.run-type` the declared kind
    /// `run_type`. Every platform creates and edits every built-in kind,
    /// also one it cannot run.
    fn new(
        provider: &str,
        run_type: Option<&RunTypeKey>,
        run_types: &'a [DeclaredRunType],
    ) -> Result<Self, CoreError> {
        match (provider, run_type) {
            (RUN_TYPE_PROVIDER, Some(key)) => Ok(Self::Declared(declared(run_types, key)?)),
            (RUN_TYPE_PROVIDER, None) | (_, Some(_)) => Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "A run configuration type is named with the build.run-type provider only",
            )),
            (provider, None) => Kind::from_provider(provider)
                .map(Self::Process)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::NotSupported,
                        "Only built-in process runs and declared run configuration types have an editor form",
                    )
                    .with_details(provider)
                }),
        }
    }

    fn provider(self) -> &'static str {
        match self {
            Self::Process(kind) => kind.provider(),
            Self::Declared(_) => RUN_TYPE_PROVIDER,
        }
    }

    fn arguments_field(self) -> &'static str {
        match self {
            Self::Process(kind) => kind.arguments_field(),
            Self::Declared(_) => DECLARED_ARGUMENTS_FIELD,
        }
    }

    fn same(self, other: Editor<'_>) -> bool {
        match (self, other) {
            (Self::Process(left), Editor::Process(right)) => left == right,
            (Self::Declared(left), Editor::Declared(right)) => left.key == right.key,
            _ => false,
        }
    }

    /// Values of a new configuration.
    fn default_values(self) -> serde_json::Map<String, Value> {
        let mut values = common_default_values(self.arguments_field());
        match self {
            Self::Process(kind) => values.extend(process_default_values(kind)),
            Self::Declared(run_type) => values.extend(run_type.default_values()),
        }
        values
    }
}

fn different_provider() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Run configuration has a different provider",
    )
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

    /// The editor of the saved entry; a declared kind no installed plugin
    /// declares has none.
    fn editor<'a>(&self, run_types: &'a [DeclaredRunType]) -> Result<Editor<'a>, CoreError> {
        let configuration = self.configuration()?;
        if configuration.provider == RUN_TYPE_PROVIDER {
            let reference = RunTypeReference::of(&configuration)?;
            return Editor::new(RUN_TYPE_PROVIDER, Some(&reference.key()), run_types);
        }
        Editor::new(&configuration.provider, None, run_types)
    }

    /// The editor values of the saved entry; `run_types` describe its task
    /// steps.
    fn values(
        &self,
        editor: Editor<'_>,
        run_types: &[DeclaredRunType],
    ) -> Result<serde_json::Map<String, Value>, CoreError> {
        let configuration = self.configuration()?;
        let mut values = editor.default_values();
        values.insert("name".into(), json!(configuration.name));
        values.insert("storeAsProjectFile".into(), json!(self.in_project));
        values.insert(
            "allowMultipleInstances".into(),
            json!(configuration.allow_multiple_instances),
        );
        values.insert(
            editor.arguments_field().into(),
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
            json!(editor_items(&configuration.before_launch, run_types)),
        );
        let kind = match editor {
            Editor::Process(kind) => kind,
            Editor::Declared(run_type) => {
                values.extend(run_type.saved_values(&RunTypeReference::of(&configuration)?));
                return Ok(values);
            }
        };
        let options = ProcessOptions::of(&configuration)?;
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

/// Values of the fields every editable run has, for a new configuration.
fn common_default_values(arguments_field: &str) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    values.insert("name".into(), json!(""));
    values.insert("storeAsProjectFile".into(), json!(false));
    values.insert("allowMultipleInstances".into(), json!(false));
    values.insert(arguments_field.into(), json!(""));
    values.insert("workingDirectory".into(), json!("."));
    values.insert("environment".into(), json!([]));
    values.insert("beforeLaunch".into(), json!([]));
    values
}

/// Values of the fields of a new process run of `kind` beyond the common ones.
fn process_default_values(kind: Kind) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    if kind.runs_script() {
        values.insert("source".into(), json!("file"));
        values.insert("script".into(), json!(""));
        values.insert("text".into(), json!(""));
        values.insert("interpreter".into(), json!(""));
        values.insert("interpreterArguments".into(), json!(""));
    } else {
        values.insert("program".into(), json!(""));
    }
    if kind == Kind::PowerShell {
        values.insert("executionPolicyBypass".into(), json!(true));
    }
    values
}

pub(super) fn argument_text(arguments: &[String]) -> String {
    crate::format_argument_text(crate::ArgumentTextRequest {
        argument_lists: vec![arguments.to_vec()],
    })
    .remove(0)
}

/// The fields at the top of every editor.
fn header_fields(automatic_name: Option<String>) -> Vec<Field> {
    vec![
        field("name", "text")
            .required()
            .header()
            .automatic(automatic_name),
        field("storeAsProjectFile", "checkbox").header(),
        field("allowMultipleInstances", "checkbox").header(),
    ]
}

/// The fields of `kind`'s editor, with automatic values for `draft`.
fn fields(kind: Kind, root: &Path, draft: &Draft, choices: StepChoices) -> Vec<Value> {
    let platform = run_platform();
    let options = draft.options();
    let mut fields = header_fields(process::default_name(&options));
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
                .automatic(Some(automatic.executable.text().to_string()))
                .shell_family(kind.shell_family()),
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
    fields.push(field("beforeLaunch", "beforeLaunch").steps(choices));
    fields.into_iter().map(|field| field.0).collect()
}

/// The fields of a declared kind's editor: the header, the kind's own
/// fields, then the fields every run has.
fn declared_fields(run_type: &DeclaredRunType, choices: StepChoices) -> Vec<Value> {
    let mut fields = header_fields(Some(run_type.display_name.clone()))
        .into_iter()
        .map(|field| field.0)
        .collect::<Vec<_>>();
    fields.extend(run_type.editor_fields());
    if run_type.takes_arguments {
        fields.push(field(DECLARED_ARGUMENTS_FIELD, "arguments").0);
    }
    fields.extend(
        [
            field("workingDirectory", "directory"),
            field("environment", "environment"),
            field("beforeLaunch", "beforeLaunch").steps(choices),
        ]
        .map(|field| field.0),
    );
    fields
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

    /// The family of shell an `executable` field takes, such as `posix`, so
    /// a host can offer the installed shells of that family as choices.
    fn shell_family(mut self, family: Option<&str>) -> Self {
        if let Some(family) = family {
            self.0["shellFamily"] = json!(family);
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

    /// What a `beforeLaunch` field offers: `candidates`, the run
    /// configurations a step may start as `{id, name}`, and, when installed
    /// plugins declare any, `tasks`, the kinds of task a step may run.
    fn steps(mut self, choices: StepChoices) -> Self {
        self.0["candidates"] = json!(choices.candidates);
        if !choices.tasks.is_empty() {
            self.0["tasks"] = json!(choices.tasks);
        }
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

/// A text editor value; absent is empty.
fn text_value(values: &serde_json::Map<String, Value>, key: &str) -> Result<String, CoreError> {
    match values.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(invalid_value(key)),
    }
}

/// A checkbox editor value; absent is `default`.
fn flag_value(
    values: &serde_json::Map<String, Value>,
    key: &str,
    default: bool,
) -> Result<bool, CoreError> {
    match values.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(invalid_value(key)),
    }
}

/// Editor values of the fields every editable run has, before they are checked.
struct Common {
    name: String,
    project: bool,
    multiple: bool,
    arguments: String,
    working_directory: String,
    environment: Vec<(String, String)>,
    /// The steps run before this one.
    before_launch: Vec<BeforeLaunchStep>,
}

/// The fields every editable run has, in the form they are saved.
struct StoredCommon {
    name: String,
    project: bool,
    multiple: bool,
    arguments: Vec<String>,
    working_directory: String,
    environment: BTreeMap<String, String>,
}

impl Common {
    fn read(
        values: &serde_json::Map<String, Value>,
        arguments_field: &str,
    ) -> Result<Self, CoreError> {
        let environment = match values.get("environment") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => serde_json::from_value::<Vec<EnvironmentRow>>(value.clone())
                .map_err(|_| invalid_value("environment"))?
                .into_iter()
                .map(|row| (row.name, row.value))
                .collect(),
        };
        Ok(Self {
            name: text_value(values, "name")?,
            project: flag_value(values, "storeAsProjectFile", false)?,
            multiple: flag_value(values, "allowMultipleInstances", false)?,
            arguments: text_value(values, arguments_field)?,
            working_directory: text_value(values, "workingDirectory")?,
            environment,
            before_launch: match values.get("beforeLaunch") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => {
                    steps_from_editor(items).ok_or_else(|| invalid_value("beforeLaunch"))?
                }
                Some(_) => return Err(invalid_value("beforeLaunch")),
            },
        })
    }

    /// The values in their saved form; a wrong value is added to `errors`.
    /// An empty name takes `automatic_name`.
    fn store(
        &self,
        root: &Path,
        automatic_name: Option<String>,
        errors: &mut Vec<FieldError>,
    ) -> StoredCommon {
        let working_directory = match self.working_directory.trim() {
            "" | "." => ".".to_string(),
            value => normalize_working_directory(root, value).unwrap_or_else(|_| {
                let target = process::file_path(root, value, run_platform());
                let code = if workspace_fs::is_dir(&target).unwrap_or(false) {
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
            "" => automatic_name.unwrap_or_default(),
            name => name.to_string(),
        };
        if name.is_empty() {
            errors.push(FieldError::new("name", "required"));
        }
        StoredCommon {
            name,
            project: self.project,
            multiple: self.multiple,
            arguments: super::launch::split_arguments(&self.arguments),
            working_directory,
            environment,
        }
    }
}

impl StoredCommon {
    /// A configuration of these values with one extension, to run the
    /// checks a launch makes before it is saved.
    fn configuration(&self, provider: &str, namespace: &str, extension: Value) -> RunConfiguration {
        RunConfiguration {
            id: "user:draft".into(),
            name: self.name.clone(),
            folder: None,
            provider: provider.into(),
            execution: Execution::default(),
            category: Default::default(),
            command: None,
            args: self.arguments.clone(),
            cwd: self.working_directory.clone(),
            env: self.environment.clone(),
            confidence: Confidence::Declared,
            toolchains: BTreeMap::new(),
            debug: None,
            members: Vec::new(),
            extensions: BTreeMap::from([(namespace.to_string(), extension)]),
            before_launch: Vec::new(),
            disabled: false,
            allow_multiple_instances: self.multiple,
            source: None,
            plugin: None,
            availability: None,
        }
    }
}

/// Editor values of a process run read into their types, before they are checked.
struct Draft {
    kind: Kind,
    common: Common,
    source: Source,
    script: String,
    text: String,
    program: String,
    interpreter: String,
    interpreter_arguments: String,
    execution_policy_bypass: bool,
}

/// Checked values of a process run, in the form they are saved.
struct Stored {
    common: StoredCommon,
    options: ProcessOptions,
}

impl Draft {
    fn read(kind: Kind, values: &serde_json::Map<String, Value>) -> Result<Self, CoreError> {
        let source = match text_value(values, "source")?.as_str() {
            "" | "file" => Source::File,
            "text" => Source::Text,
            _ => return Err(invalid_value("source")),
        };
        Ok(Self {
            kind,
            common: Common::read(values, kind.arguments_field())?,
            source,
            script: text_value(values, "script")?,
            text: text_value(values, "text")?,
            program: text_value(values, "program")?,
            interpreter: text_value(values, "interpreter")?,
            interpreter_arguments: text_value(values, "interpreterArguments")?,
            execution_policy_bypass: flag_value(values, "executionPolicyBypass", true)?,
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
        let common = self
            .common
            .store(root, process::default_name(&options), &mut errors);
        // The saved entry must pass the same checks a launch makes; report
        // the ones the steps above do not, such as lengths and characters.
        // A missing value is left to the steps above, which name the field
        // of the chosen source: an empty text is not also a missing script.
        let configuration = common.configuration(
            self.kind.provider(),
            "process",
            serde_json::to_value(&options).expect("process options encode"),
        );
        if let Ok(found) = process::field_errors(&configuration, run_platform()) {
            for error in found {
                if error.code != "required"
                    && !errors.iter().any(|known| known.field == error.field)
                {
                    errors.push(error);
                }
            }
        }
        (Stored { common, options }, errors)
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

pub(super) fn invalid_value(field: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Invalid editor value").with_details(field)
}

/// A script file as saved: project-relative with `/` when it is inside the
/// project, else absolute.
pub(super) fn stored_file(
    root: &Path,
    value: &str,
    keep_dot: bool,
) -> Result<String, &'static str> {
    stored_path(root, value, keep_dot, false)
}

/// A directory as saved, like [`stored_file`]; the project itself is `.`.
pub(super) fn stored_directory(root: &Path, value: &str) -> Result<String, &'static str> {
    stored_path(root, value, false, true)
}

fn stored_path(
    root: &Path,
    value: &str,
    keep_dot: bool,
    directory: bool,
) -> Result<String, &'static str> {
    if let Some(code) = process::text_error(value, 4096) {
        return Err(code);
    }
    let platform = run_platform();
    let absolute = lithe_platform::paths::is_absolute_path(value, platform);
    let path = if absolute {
        PathBuf::from(value)
    } else {
        root.join(value)
    };
    if (directory && !workspace_fs::is_dir(&path).unwrap_or(false))
        || (!directory && !workspace_fs::is_file(&path).unwrap_or(false))
    {
        return Err("notFound");
    }
    let canonical = workspace_fs::canonicalize(&path).map_err(|_| "notFound")?;
    let canonical_root = workspace_fs::canonicalize(root).map_err(|_| "notFound")?;
    if let Ok(relative) = canonical.strip_prefix(&canonical_root) {
        let relative = slash_separated_path(relative, platform);
        if relative.is_empty() {
            return Ok(".".to_string());
        }
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
