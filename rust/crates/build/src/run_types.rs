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

use super::decode;
use super::manifest::{
    check_description, check_text, BuildExecution, BuildSystemDeclaration, RunFieldDeclaration,
    RunFieldKind, RunTypeDeclaration,
};
use super::plans::{self, invalid_selection, unavailable, PlanBase, PlanContext};
use super::template::{self, Values};
use super::BuildManifest;
use lithe_execution::{
    DeclaredChoice, DeclaredCondition, DeclaredField, DeclaredFieldKind, DeclaredLaunch,
    DeclaredRunType, DeclaredValue, Execution, RunTypeKey,
};
use lithe_plugin_model::settings::{parse_localization_file, present, LocalizationRequest};
use lithe_plugin_model::{PluginRejection, Validation};
use lithe_protocol::json_document::UniqueMap;
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// Error code of a translation file Core did not apply.
const INVALID_LOCALIZATION: &str = "plugin.build.invalidLocalization";
/// Error code of a saved configuration whose values the type cannot run.
const INVALID_RUN_CONFIGURATION: &str = "plugin.build.invalidRunConfiguration";
/// Most plugins one editor request may name.
const MAX_PLUGINS: usize = 512;
/// Most build-system instances one plan request may name.
const MAX_INSTANCES: usize = 4_096;

/// The installed plugins whose run configuration types the editor offers.
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

/// The types the plugins declare, in plugin and declaration order, with one
/// translation report per plugin that declares a type when a language is
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
            .filter(|system| system.has_run_configuration_types())
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
        for system in systems {
            for declaration in &system.run_configuration_types {
                let text = localization
                    .types
                    .get(&(system.id.clone(), declaration.id.clone()));
                types.push(declared(manifest.id(), system, declaration, text));
            }
        }
    }
    Ok((types, reports))
}

/// The run type of a validated declaration, with translated text.
fn declared(
    plugin_id: &str,
    system: &BuildSystemDeclaration,
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
        execution: match declaration.execution() {
            BuildExecution::Task => Execution::Task,
            // Validation rejects `service`.
            BuildExecution::Application | BuildExecution::Service => Execution::Application,
        },
        takes_arguments: declaration.takes_arguments(),
        fields,
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
/// `runConfigurationTypes.<buildSystemID>.<typeID>`. Other top-level keys
/// belong to other surfaces.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationFile {
    #[serde(default)]
    run_configuration_types: UniqueMap<UniqueMap<TypeText>>,
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

/// A checked translation of one plugin's types, by build system and type ID.
#[derive(Debug, Default)]
struct Localization {
    types: BTreeMap<(String, String), TypeText>,
}

/// Parses one translation file and checks it against the declarations: it
/// names only declared build systems, types, fields and choices, translates
/// only text the manifest has, and every text fits the manifest's limits.
/// A file that does not fit is refused whole.
fn decode_file(text: &str, systems: &[&BuildSystemDeclaration]) -> Validation<Localization> {
    let file: LocalizationFile = parse_localization_file(text, INVALID_LOCALIZATION)?;
    let mut localization = Localization::default();
    for (system_id, types) in file.run_configuration_types.0 {
        let field = format!("runConfigurationTypes.{system_id}");
        let system = systems
            .iter()
            .find(|system| system.id == system_id)
            .ok_or_else(|| {
                rejection(
                    &field,
                    "The plugin declares no build system with run configuration types under this ID.",
                )
            })?;
        for (type_id, text) in types.0 {
            let field = format!("{field}.{type_id}");
            let declaration = system.run_type(&type_id).ok_or_else(|| {
                rejection(
                    &field,
                    "The build system declares no run configuration type with this ID.",
                )
            })?;
            check_type_text(&text, declaration, &field)?;
            localization
                .types
                .insert((system_id.clone(), type_id), text);
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
            .ok_or_else(|| rejection(&field, "The type declares no field with this ID."))?;
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
    /// The detected instances of the build system, with their stored
    /// options; the plan runs with the instance whose root holds the
    /// working directory.
    pub(crate) instances: Vec<RunTypeInstance>,
    /// Absolute installed plugin directory, for `${pluginDir}`.
    #[serde(default)]
    pub(crate) plugin_directory: Option<String>,
    /// Absolute writable directory of this plugin for this workspace.
    #[serde(default)]
    pub(crate) workspace_state_directory: Option<String>,
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
    let reference = &launch.reference;
    if reference.plugin_id != plugin_id || reference.build_system_id != system.id {
        return Err(invalid_selection(
            "The run configuration belongs to another plugin or build system.",
        ));
    }
    let declaration = system.run_type(&reference.type_id).ok_or_else(|| {
        unavailable(
            system,
            &format!("the run configuration type {}", reference.type_id),
        )
    })?;
    let run_type = declared(plugin_id, system, declaration, None);
    let values = run_type
        .launch_values(&reference.values, workspace_root)
        .map_err(|error| {
            let message = match error.code {
                "required" => "A required field of the run configuration is empty.",
                _ => "A saved value does not fit the run configuration type.",
            };
            PluginRejection::at(
                INVALID_RUN_CONFIGURATION,
                format!("values.{}", error.field),
                message,
            )
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
    let instance = instance_of(&request.instances, &launch.working_directory)?;
    let base = PlanBase {
        model_inputs: instance.model_inputs.clone(),
        root: instance.root.clone(),
        options: instance.options.clone(),
        plugin_directory: request.plugin_directory.clone(),
        workspace_state_directory: request.workspace_state_directory.clone(),
    };
    let context = PlanContext::new(system, &base)?;
    let mut values = context.values(&launch.arguments);
    values.field_arguments = &field_arguments;
    let mut extra = Map::new();
    extra.insert("workingDirectory".into(), json!(launch.working_directory));
    extra.insert("runType".into(), json!(reference.key()));
    let mut plan = context.plan(
        &declaration.arguments,
        &values,
        declaration.execution(),
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
        DeclaredValue::Text(text) if field.arguments.is_empty() => Ok(vec![text.clone()]),
        DeclaredValue::Text(text) => {
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

/// The instance a configuration runs with: the deepest one whose root holds
/// the working directory, or the only one when the working directory is
/// outside every instance, such as an absolute directory outside the
/// workspace.
fn instance_of<'a>(
    instances: &'a [RunTypeInstance],
    working_directory: &str,
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
    let directory = decode::request_path(working_directory, "working directory").ok();
    let holding = directory.and_then(|directory| {
        roots
            .iter()
            .enumerate()
            .filter(|(_, root)| decode::contains(root, &directory))
            .max_by_key(|(_, root)| {
                if *root == "." {
                    0
                } else {
                    root.split('/').count()
                }
            })
            .map(|(index, _)| &instances[index])
    });
    match (holding, instances) {
        (Some(instance), _) | (None, [instance]) => Ok(instance),
        (None, _) => Err(invalid_selection(
            "The working directory is outside every project of this build system; choose one inside a project.",
        )),
    }
}
