//! Run configuration types build systems declare, for the run configuration
//! editor and for a launch.
//!
//! The editor commands (`runConfig.configurationTypes`, `editorForm`,
//! `saveConfiguration`) take the installed plugins' manifests with their
//! translation files; [`declared_run_types`] checks them and turns every
//! declared type into the [`DeclaredRunType`] the execution crate draws and
//! stores. `build.runConfigurationPlan` reads a saved configuration of such a
//! type and expands the type's template into an argv plan, like a task plan,
//! so the host starts it as it starts any build command.
//!
//! Before-launch tasks (`beforeLaunchTasks`) are declared the same way and
//! become [`DeclaredRunType`]s of the [`DeclaredRole::BeforeLaunchTask`]
//! role. `build.beforeLaunchTaskPlan` expands a task step that
//! `runConfig.beforeLaunch` returned into the same kind of plan; a task
//! always runs to completion and takes no program arguments.

use super::decode;
use super::manifest::{
    check_description, check_text, BuildExecution, BuildSystemDeclaration, RunFieldDeclaration,
    RunFieldKind, RunTypeDeclaration, RunTypeRole,
};
use super::plans::{self, invalid_selection, unavailable, PlanBase, PlanContext};
use super::template::{self, Values};
use super::BuildManifest;
use lithe_execution::{
    DeclaredChoice, DeclaredCondition, DeclaredField, DeclaredFieldKind, DeclaredLaunch,
    DeclaredRole, DeclaredRunType, DeclaredValue, Execution, RunTypeKey, RunTypeReference,
};
use lithe_plugin_model::settings::{parse_localization_file, present, LocalizationRequest};
use lithe_plugin_model::{PluginRejection, Validation};
use lithe_protocol::json_document::UniqueMap;
use lithe_protocol::{CoreError, ErrorCode};
use lithe_workspace::fs;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// Error code of a translation file Core did not apply.
const INVALID_LOCALIZATION: &str = "plugin.build.invalidLocalization";
/// Error code of a saved configuration whose values the type cannot run.
const INVALID_RUN_CONFIGURATION: &str = "plugin.build.invalidRunConfiguration";
/// Error code of a before-launch task step whose values the task cannot run.
const INVALID_BEFORE_LAUNCH_TASK: &str = "plugin.build.invalidBeforeLaunchTask";
/// Most plugins one editor request may name.
const MAX_PLUGINS: usize = 512;
/// Most build-system instances one plan request may name.
const MAX_INSTANCES: usize = 4_096;
const NO_MATCHING_INSTANCE: &str =
    "The working directory is outside every project of this build system; choose one inside a project.";

/// The installed plugins whose run configuration types and before-launch
/// tasks the editor offers.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunTypePlugins {
    /// Interface language the shown text is translated to; absent keeps the
    /// manifests' wording and reads no translation file.
    #[serde(default)]
    pub language: Option<String>,
    /// Installed plugins, in the order their types are listed.
    #[serde(default)]
    pub plugins: Vec<RunTypePlugin>,
}

/// One installed plugin.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunTypePlugin {
    /// The plugin's `plugin.json`, validated again here.
    pub manifest: Value,
    /// The translation files the native adapter read, by package-relative
    /// path; absent until it has read the candidates a report names.
    #[serde(default)]
    pub localizations: Option<BTreeMap<String, String>>,
}

/// The run configuration types and before-launch tasks the plugins declare,
/// in plugin and declaration order, each with its role, with one
/// translation report per plugin that declares either when a language is
/// named: `{pluginID, language, candidates, maximumBytes, readRequired,
/// applied, rejected}`, the report of `plugins.settings`.
///
/// A manifest that does not validate adds nothing, as on every other surface
/// of an installed plugin; a translation file that does not fit is reported
/// and the manifest's wording is kept.
pub fn declared_run_types<M: BuildManifest>(
    plugins: &RunTypePlugins,
) -> Result<(Vec<DeclaredRunType>, Vec<Value>), CoreError> {
    if plugins.plugins.len() > MAX_PLUGINS {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many plugins in the run configuration type request",
        ));
    }
    let mut types = Vec::new();
    let mut reports = Vec::new();
    for plugin in &plugins.plugins {
        let Ok(manifest) = M::validate(plugin.manifest.clone()) else {
            continue;
        };
        let systems: Vec<&BuildSystemDeclaration> = manifest
            .build_systems()
            .iter()
            .filter(|system| {
                system.has_run_configuration_types() || system.has_before_launch_tasks()
            })
            .collect();
        if systems.is_empty() {
            continue;
        }
        let request =
            LocalizationRequest::new(plugins.language.as_deref(), plugin.localizations.as_ref())
                .map_err(|message| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid run configuration type localization request",
                    )
                    .with_details(message)
                })?;
        let localization = match request {
            Some(request) => {
                let (chosen, mut report) = request.select_with(|text| decode_file(text, &systems));
                report["pluginID"] = json!(manifest.id());
                reports.push(report);
                chosen.unwrap_or_default()
            }
            None => Localization::default(),
        };
        for role in [RunTypeRole::Configuration, RunTypeRole::BeforeLaunchTask] {
            for system in &systems {
                for declaration in system.run_types(role) {
                    let text = localization.text(role, &system.id, &declaration.id);
                    types.push(declared(manifest.id(), system, role, declaration, text));
                }
            }
        }
    }
    Ok((types, reports))
}

/// The run type or task of a validated declaration, with translated text.
fn declared(
    plugin_id: &str,
    system: &BuildSystemDeclaration,
    role: RunTypeRole,
    declaration: &RunTypeDeclaration,
    text: Option<&TypeText>,
) -> DeclaredRunType {
    let fields = declaration
        .fields
        .iter()
        .map(|field| {
            declared_field(
                field,
                text.and_then(|text| text.fields.as_ref())
                    .and_then(|fields| fields.0.get(&field.id)),
            )
        })
        .collect();
    DeclaredRunType {
        key: RunTypeKey {
            plugin_id: plugin_id.to_string(),
            build_system_id: system.id.clone(),
            type_id: declaration.id.clone(),
        },
        role: match role {
            RunTypeRole::Configuration => DeclaredRole::Configuration,
            RunTypeRole::BeforeLaunchTask => DeclaredRole::BeforeLaunchTask,
        },
        display_name: text
            .and_then(|text| text.display_name.clone())
            .unwrap_or_else(|| declaration.display_name.clone()),
        description: text
            .and_then(|text| text.description.clone())
            .or_else(|| declaration.description.clone()),
        icon: declaration
            .icon
            .as_ref()
            .map(|icon| serde_json::to_value(icon).expect("icons encode")),
        execution: match execution(role, declaration) {
            BuildExecution::Task => Execution::Task,
            // Validation rejects `service`.
            BuildExecution::Application | BuildExecution::Service => Execution::Application,
        },
        takes_arguments: declaration.takes_arguments(),
        fields,
    }
}

/// How a started declaration behaves: a task always runs to completion.
fn execution(role: RunTypeRole, declaration: &RunTypeDeclaration) -> BuildExecution {
    match role {
        RunTypeRole::Configuration => declaration.execution(),
        RunTypeRole::BeforeLaunchTask => BuildExecution::Task,
    }
}

fn declared_field(field: &RunFieldDeclaration, text: Option<&FieldText>) -> DeclaredField {
    let options = text.and_then(|text| text.options.as_ref());
    DeclaredField {
        id: field.id.clone(),
        kind: match field.kind {
            RunFieldKind::Text => DeclaredFieldKind::Text,
            RunFieldKind::Choice => DeclaredFieldKind::Choice,
            RunFieldKind::Checkbox => DeclaredFieldKind::Checkbox,
            RunFieldKind::File => DeclaredFieldKind::File,
            RunFieldKind::Directory => DeclaredFieldKind::Directory,
            RunFieldKind::Arguments => DeclaredFieldKind::Arguments,
        },
        title: text
            .and_then(|text| text.title.clone())
            .unwrap_or_else(|| field.title.clone()),
        description: text
            .and_then(|text| text.description.clone())
            .or_else(|| field.description.clone()),
        placeholder: text
            .and_then(|text| text.placeholder.clone())
            .or_else(|| field.placeholder.clone()),
        choices: field
            .choices
            .iter()
            .map(|choice| DeclaredChoice {
                value: choice.value.clone(),
                title: options
                    .and_then(|options| options.0.get(&choice.value).cloned())
                    .or_else(|| choice.title.clone()),
            })
            .collect(),
        default: field.default.clone(),
        required: field.required,
        visible_when: field
            .visible_when
            .as_ref()
            .map(|condition| DeclaredCondition {
                field: condition.field.clone(),
                equals: condition.equals.clone(),
            }),
    }
}

/// The part of a translation file this module reads:
/// `runConfigurationTypes.<buildSystemID>.<typeID>` and
/// `beforeLaunchTasks.<buildSystemID>.<taskID>`. Other top-level keys belong
/// to other surfaces.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationFile {
    #[serde(default)]
    run_configuration_types: UniqueMap<UniqueMap<TypeText>>,
    #[serde(default)]
    before_launch_tasks: UniqueMap<UniqueMap<TypeText>>,
}

/// Translated text of one type, by the manifest field it replaces.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TypeText {
    #[serde(default, deserialize_with = "present")]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<String>,
    /// Field text by field ID.
    #[serde(default, deserialize_with = "present")]
    fields: Option<UniqueMap<FieldText>>,
}

/// Translated text of one field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FieldText {
    #[serde(default, deserialize_with = "present")]
    title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    placeholder: Option<String>,
    /// Choice titles by choice value.
    #[serde(default, deserialize_with = "present")]
    options: Option<UniqueMap<String>>,
}

/// A checked translation of one plugin's types and tasks, by build system
/// and declaration ID.
#[derive(Debug, Default)]
struct Localization {
    types: BTreeMap<(String, String), TypeText>,
    tasks: BTreeMap<(String, String), TypeText>,
}

impl Localization {
    fn entries(&mut self, role: RunTypeRole) -> &mut BTreeMap<(String, String), TypeText> {
        match role {
            RunTypeRole::Configuration => &mut self.types,
            RunTypeRole::BeforeLaunchTask => &mut self.tasks,
        }
    }

    fn text(&self, role: RunTypeRole, system_id: &str, id: &str) -> Option<&TypeText> {
        let entries = match role {
            RunTypeRole::Configuration => &self.types,
            RunTypeRole::BeforeLaunchTask => &self.tasks,
        };
        entries.get(&(system_id.to_string(), id.to_string()))
    }
}

/// Parses one translation file and checks it against the declarations: it
/// names only declared build systems, types, tasks, fields and choices,
/// translates only text the manifest has, and every text fits the
/// manifest's limits. A file that does not fit is refused whole.
fn decode_file(text: &str, systems: &[&BuildSystemDeclaration]) -> Validation<Localization> {
    let file: LocalizationFile = parse_localization_file(text, INVALID_LOCALIZATION)?;
    let mut localization = Localization::default();
    for (role, section) in [
        (RunTypeRole::Configuration, file.run_configuration_types),
        (RunTypeRole::BeforeLaunchTask, file.before_launch_tasks),
    ] {
        let (no_system, no_entry) = match role {
            RunTypeRole::Configuration => (
                "The plugin declares no build system with run configuration types under this ID.",
                "The build system declares no run configuration type with this ID.",
            ),
            RunTypeRole::BeforeLaunchTask => (
                "The plugin declares no build system with before-launch tasks under this ID.",
                "The build system declares no before-launch task with this ID.",
            ),
        };
        for (system_id, entries) in section.0 {
            let field = format!("{}.{system_id}", role.key());
            let system = systems
                .iter()
                .find(|system| system.id == system_id && !system.run_types(role).is_empty())
                .ok_or_else(|| rejection(&field, no_system))?;
            for (id, text) in entries.0 {
                let field = format!("{field}.{id}");
                let declaration = system
                    .run_type(role, &id)
                    .ok_or_else(|| rejection(&field, no_entry))?;
                check_type_text(&text, declaration, &field)?;
                localization
                    .entries(role)
                    .insert((system_id.clone(), id), text);
            }
        }
    }
    Ok(localization)
}

fn check_type_text(
    text: &TypeText,
    declaration: &RunTypeDeclaration,
    field: &str,
) -> Validation<()> {
    if let Some(name) = &text.display_name {
        relabel(check_text(
            name,
            &format!("{field}.displayName"),
            "display name",
        ))?;
    }
    translated(
        text.description.as_deref(),
        declaration.description.is_some(),
        &format!("{field}.description"),
        check_description,
    )?;
    for (id, field_text) in text.fields.iter().flat_map(|fields| &fields.0) {
        let field = format!("{field}.fields.{id}");
        let declared = declaration
            .fields
            .iter()
            .find(|declared| &declared.id == id)
            .ok_or_else(|| rejection(&field, "The declaration has no field with this ID."))?;
        if let Some(title) = &field_text.title {
            relabel(check_text(title, &format!("{field}.title"), "title"))?;
        }
        translated(
            field_text.description.as_deref(),
            declared.description.is_some(),
            &format!("{field}.description"),
            check_description,
        )?;
        translated(
            field_text.placeholder.as_deref(),
            declared.placeholder.is_some(),
            &format!("{field}.placeholder"),
            |value, field| check_text(value, field, "placeholder"),
        )?;
        for (value, title) in field_text.options.iter().flat_map(|options| &options.0) {
            let field = format!("{field}.options.{value}");
            if !declared.choices.iter().any(|choice| &choice.value == value) {
                return Err(rejection(
                    &field,
                    "The field declares no choice with this value.",
                ));
            }
            relabel(check_text(title, &field, "title"))?;
        }
    }
    Ok(())
}

/// Checks a translation of text the manifest may lack: a translation
/// replaces text and never adds it.
fn translated(
    value: Option<&str>,
    declared: bool,
    field: &str,
    check: impl Fn(&str, &str) -> Validation<()>,
) -> Validation<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if !declared {
        return Err(rejection(
            field,
            "The manifest has no text here to translate.",
        ));
    }
    relabel(check(value, field))
}

/// A manifest check's rejection, as a rejection of the translation file.
fn relabel(result: Validation<()>) -> Validation<()> {
    result.map_err(|error| PluginRejection {
        code: INVALID_LOCALIZATION,
        ..error
    })
}

fn rejection(field: impl Into<String>, message: impl Into<String>) -> PluginRejection {
    PluginRejection::at(INVALID_LOCALIZATION, field, message)
}

/// `build.runConfigurationPlan`: the argv plan of a saved configuration of a
/// declared type.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunConfigurationPlanRequest {
    /// The saved `build.run-type` configuration.
    pub(crate) configuration_id: String,
    /// The host's replacement for the machine-local layer, as in
    /// `runConfig.launchPlan`.
    #[serde(default)]
    pub(crate) local_document: Option<Value>,
    /// The detected instances of the build system, with their stored options.
    /// Selection uses the working directory first and can fall back to the
    /// run's file when the workspace root has no instance.
    pub(crate) instances: Vec<RunTypeInstance>,
    /// Absolute installed plugin directory, for `${pluginDir}`.
    #[serde(default)]
    pub(crate) plugin_directory: Option<String>,
    /// Absolute writable directory of this plugin for this workspace.
    #[serde(default)]
    pub(crate) workspace_state_directory: Option<String>,
    /// Whether the user trusts the workspace, as in a task plan.
    #[serde(default)]
    pub(crate) project_code_allowed: bool,
}

/// One detected instance of the build system.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunTypeInstance {
    /// Instance root, workspace-relative.
    root: String,
    /// Stored option values by option ID.
    #[serde(default)]
    options: BTreeMap<String, Value>,
    /// Host-resolved public input snapshot, as in a task plan.
    #[serde(default)]
    model_inputs: Option<Value>,
}

/// `build.beforeLaunchTaskPlan`: the argv plan of one before-launch task
/// step, as `runConfig.beforeLaunch` returned it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BeforeLaunchTaskPlanRequest {
    /// The step's `task`: the plugin, build system, task ID and saved values.
    pub(crate) task: RunTypeReference,
    /// The step's `workingDirectory`, workspace-relative.
    pub(crate) working_directory: String,
    /// The detected instances of the build system, as in
    /// `build.runConfigurationPlan`.
    pub(crate) instances: Vec<RunTypeInstance>,
    /// Absolute installed plugin directory, for `${pluginDir}`.
    #[serde(default)]
    pub(crate) plugin_directory: Option<String>,
    /// Absolute writable directory of this plugin for this workspace.
    #[serde(default)]
    pub(crate) workspace_state_directory: Option<String>,
    /// Whether the user trusts the workspace, as in a task plan.
    #[serde(default)]
    pub(crate) project_code_allowed: bool,
}

/// What a plan takes from its request besides the launch.
struct PlanSources<'a> {
    instances: &'a [RunTypeInstance],
    plugin_directory: Option<String>,
    workspace_state_directory: Option<String>,
    project_code_allowed: bool,
}

/// The wording and identity of one role in a plan.
struct PlanRole {
    role: RunTypeRole,
    other_owner: &'static str,
    entry: &'static str,
    code: &'static str,
    required: &'static str,
    does_not_fit: &'static str,
    /// The plan key that names the kind.
    key: &'static str,
}

const CONFIGURATION_PLAN: PlanRole = PlanRole {
    role: RunTypeRole::Configuration,
    other_owner: "The run configuration belongs to another plugin or build system.",
    entry: "the run configuration type",
    code: INVALID_RUN_CONFIGURATION,
    required: "A required field of the run configuration is empty.",
    does_not_fit: "A saved value does not fit the run configuration type.",
    key: "runType",
};

const TASK_PLAN: PlanRole = PlanRole {
    role: RunTypeRole::BeforeLaunchTask,
    other_owner: "The before-launch task belongs to another plugin or build system.",
    entry: "the before-launch task",
    code: INVALID_BEFORE_LAUNCH_TASK,
    required: "A required field of the before-launch task is empty.",
    does_not_fit: "A saved value does not fit the before-launch task.",
    key: "beforeLaunchTask",
};

/// The plan of `launch`, a saved configuration of a type `system` declares.
///
/// The plan is the task plan shape plus `runType`: the global arguments and
/// the type's arguments with the field values, the user's program arguments
/// and the instance's options, at the configuration's working directory,
/// with the configuration's environment over the options'.
pub(crate) fn run_configuration_plan(
    plugin_id: &str,
    system: &BuildSystemDeclaration,
    launch: &DeclaredLaunch,
    request: &RunConfigurationPlanRequest,
    workspace_root: &Path,
) -> Validation<Value> {
    declared_plan(
        plugin_id,
        system,
        &CONFIGURATION_PLAN,
        launch,
        PlanSources {
            instances: &request.instances,
            plugin_directory: request.plugin_directory.clone(),
            workspace_state_directory: request.workspace_state_directory.clone(),
            project_code_allowed: request.project_code_allowed,
        },
        workspace_root,
    )
}

/// The plan of `launch`, a before-launch task step of a task `system`
/// declares: the task plan shape plus `beforeLaunchTask`, the global
/// arguments and the task's arguments with the field values and the
/// instance's options, at the step's working directory. A task always runs
/// to completion.
pub(crate) fn before_launch_task_plan(
    plugin_id: &str,
    system: &BuildSystemDeclaration,
    launch: &DeclaredLaunch,
    request: &BeforeLaunchTaskPlanRequest,
    workspace_root: &Path,
) -> Validation<Value> {
    declared_plan(
        plugin_id,
        system,
        &TASK_PLAN,
        launch,
        PlanSources {
            instances: &request.instances,
            plugin_directory: request.plugin_directory.clone(),
            workspace_state_directory: request.workspace_state_directory.clone(),
            project_code_allowed: request.project_code_allowed,
        },
        workspace_root,
    )
}

fn declared_plan(
    plugin_id: &str,
    system: &BuildSystemDeclaration,
    role: &PlanRole,
    launch: &DeclaredLaunch,
    sources: PlanSources<'_>,
    workspace_root: &Path,
) -> Validation<Value> {
    let reference = &launch.reference;
    if reference.plugin_id != plugin_id || reference.build_system_id != system.id {
        return Err(invalid_selection(role.other_owner));
    }
    let declaration = system
        .run_type(role.role, &reference.type_id)
        .ok_or_else(|| unavailable(system, &format!("{} {}", role.entry, reference.type_id)))?;
    let run_type = declared(plugin_id, system, role.role, declaration, None);
    let values = run_type
        .launch_values(&reference.values, workspace_root)
        .map_err(|error| {
            let message = match error.code {
                "required" => role.required,
                _ => role.does_not_fit,
            };
            PluginRejection::at(role.code, format!("values.{}", error.field), message)
        })?;
    plans::check_user_arguments(&launch.arguments, &declaration.arguments)?;
    let mut field_arguments = Vec::new();
    for (id, value) in &values {
        let field = declaration
            .fields
            .iter()
            .find(|field| &field.id == id)
            .expect("launch values name declared fields");
        field_arguments.push((id.clone(), field_value_arguments(field, value)?));
    }
    let file_directory = (role.role == RunTypeRole::Configuration)
        .then_some(|| sole_file_directory(workspace_root, declaration, &values));
    let instance = instance_of(sources.instances, &launch.working_directory, file_directory)?;
    let base = PlanBase {
        model_inputs: instance.model_inputs.clone(),
        root: instance.root.clone(),
        options: instance.options.clone(),
        plugin_directory: sources.plugin_directory,
        workspace_state_directory: sources.workspace_state_directory,
        project_code_allowed: sources.project_code_allowed,
    };
    let context = PlanContext::new(system, &base)?;
    let mut values = context.values(&launch.arguments);
    values.field_arguments = &field_arguments;
    let mut extra = Map::new();
    extra.insert("workingDirectory".into(), json!(launch.working_directory));
    extra.insert(role.key.into(), json!(reference.key()));
    let mut plan = context.plan(
        &declaration.arguments,
        &values,
        execution(role.role, declaration),
        extra,
    )?;
    if let Some(environment) = plan["environment"].as_object_mut() {
        environment.extend(
            launch
                .environment
                .iter()
                .map(|(name, value)| (name.clone(), json!(value))),
        );
    }
    Ok(plan)
}

/// The arguments one field value passes: its templates with `${value}`, or
/// the value itself when the field declares none.
fn field_value_arguments(
    field: &RunFieldDeclaration,
    value: &DeclaredValue,
) -> Validation<Vec<String>> {
    match value {
        DeclaredValue::Arguments(arguments) => Ok(arguments.clone()),
        DeclaredValue::Text(text) => {
            // Instance selection uses the document path. The tool interprets
            // the field on its own host, including inside argument templates.
            let host_path = matches!(field.kind, RunFieldKind::File | RunFieldKind::Directory)
                .then(|| lithe_workspace::remote_path(Path::new(text)));
            let text = host_path
                .as_ref()
                .and_then(|path| path.to_str())
                .unwrap_or(text);
            if field.arguments.is_empty() {
                return Ok(vec![text.to_owned()]);
            }
            let values = Values {
                value: Some(text),
                ..Values::default()
            };
            field
                .arguments
                .iter()
                .map(|argument| template::expand_text(argument, &values))
                .collect()
        }
        DeclaredValue::Checked => field
            .arguments
            .iter()
            .map(|argument| template::expand_text(argument, &Values::default()))
            .collect(),
    }
}

/// The workspace-relative parent of the sole active file's resolved target.
/// Resolving both paths keeps aliases inside the workspace usable and prevents
/// a link to an external file from choosing an instance.
fn sole_file_directory(
    workspace_root: &Path,
    declaration: &RunTypeDeclaration,
    values: &[(String, DeclaredValue)],
) -> io::Result<Option<String>> {
    let mut files = values.iter().filter_map(|(id, value)| {
        let DeclaredValue::Text(path) = value else {
            return None;
        };
        declaration
            .fields
            .iter()
            .any(|field| field.id == *id && field.kind == RunFieldKind::File)
            .then_some(path)
    });
    let (Some(file), None) = (files.next(), files.next()) else {
        return Ok(None);
    };
    let workspace_root = fs::canonicalize(workspace_root)?;
    let file = fs::canonicalize(file)?;
    let Some(directory) = file
        .parent()
        .and_then(|parent| parent.strip_prefix(&workspace_root).ok())
    else {
        return Ok(None);
    };
    if !fs::is_file(&file)? {
        return Ok(None);
    }
    let components = directory
        .iter()
        .map(|component| component.to_str())
        .collect::<Option<Vec<_>>>();
    Ok(components.and_then(|parts| decode::request_path(&parts.join("/"), "file directory").ok()))
}

/// Picks the deepest project that contains the working directory. When no
/// root holds it, a run at the workspace root can supply a file's directory
/// to select the instance while keeping the configured working directory.
/// A lone instance can run from outside its project.
fn instance_of<'a>(
    instances: &'a [RunTypeInstance],
    working_directory: &str,
    file_directory: Option<impl FnOnce() -> io::Result<Option<String>>>,
) -> Validation<&'a RunTypeInstance> {
    if instances.is_empty() {
        return Err(invalid_selection(
            "No project of this build system is open.",
        ));
    }
    if instances.len() > MAX_INSTANCES {
        return Err(invalid_selection("Too many build system instances."));
    }
    let roots = instances
        .iter()
        .map(|instance| decode::request_path(&instance.root, "root"))
        .collect::<Validation<Vec<_>>>()?;
    let holding = |directory: &str| {
        roots
            .iter()
            .enumerate()
            .filter(|(_, root)| decode::contains(root, directory))
            .max_by_key(|(_, root)| {
                if *root == "." {
                    0
                } else {
                    root.split('/').count()
                }
            })
            .map(|(index, _)| &instances[index])
    };
    let directory = decode::request_path(working_directory, "working directory").ok();
    match (directory.as_deref().and_then(holding), instances) {
        (Some(instance), _) | (None, [instance]) => return Ok(instance),
        (None, _) => {}
    }
    if let Some(resolve_file_directory) =
        file_directory.filter(|_| directory.as_deref() == Some("."))
    {
        let resolved =
            resolve_file_directory().map_err(|_| invalid_selection(NO_MATCHING_INSTANCE))?;
        if let Some(instance) = resolved.as_deref().and_then(holding) {
            return Ok(instance);
        }
    }
    Err(invalid_selection(NO_MATCHING_INSTANCE))
}
