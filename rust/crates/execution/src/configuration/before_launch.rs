//! Steps run before a configuration starts (IDEA's "Before launch").
//!
//! A configuration may name, in order, steps to run first, each of which must
//! exit with 0 before the next one, and before the configuration itself,
//! starts. A step is one of:
//!
//! - a built-in process run to start first: a shell script that compiles a
//!   module, a program that generates sources. The step reuses everything the
//!   run already has (interpreter, arguments, working directory, environment
//!   and launch plan), so it is a reference to the run, not a second command
//!   format. A referenced run that names steps of its own runs them first, as
//!   starting it on its own would.
//! - a task an installed plugin declares (`buildSystems[].beforeLaunchTasks`),
//!   such as running a Go command. The step keeps the task's reference with
//!   the values of its fields, like a saved configuration of a declared kind,
//!   and runs in the working directory of the configuration whose list names
//!   it.
//!
//! Core checks the steps, expands them into one ordered list, and says
//! whether the list needs the user's confirmation: steps a project file names
//! run code from the repository the user did not choose on this machine, so
//! the host asks before running a list it has not seen. The host runs a
//! process run through `runConfig.createLaunchPlan` and a task through
//! `build.beforeLaunchTaskPlan`.

use super::declared::{declared_task, DeclaredRunType, RunTypeKey, RunTypeReference};
use super::process::{self, FieldError, ProcessOptions};
use super::*;
use serde_json::Map;
use sha2::{Digest, Sha256};

/// Most steps one configuration names.
pub(crate) const MAXIMUM_STEPS: usize = 16;
/// Most steps one start runs once referenced runs are expanded.
const MAXIMUM_EXPANDED_STEPS: usize = 32;
/// Longest configuration ID a step names.
const MAXIMUM_ID_BYTES: usize = 256;

/// One step of `beforeLaunch`: `{configuration}` or `{task}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BeforeLaunchStep {
    /// A run configuration to start first.
    Configuration(ConfigurationStep),
    /// A task a plugin declares.
    Task(TaskStep),
}

/// A step that starts a run configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationStep {
    /// ID of a built-in process run of the same project.
    pub configuration: String,
}

/// A step that runs a task a plugin declares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStep {
    /// The plugin, build system and task ID, with the saved field values.
    pub task: RunTypeReference,
}

impl BeforeLaunchStep {
    /// A step that starts the run configuration `id`.
    pub fn configuration(id: impl Into<String>) -> Self {
        Self::Configuration(ConfigurationStep {
            configuration: id.into(),
        })
    }
}

/// Checks the shape of a configuration's own steps: their number, IDs that
/// are not empty, not too long and not the configuration itself, and task
/// references of a valid shape. Whether a step names a runnable process run,
/// or a task an installed plugin declares, is checked when it starts, so a
/// removed step target never hides the configuration from the list.
pub(crate) fn validate_shape(configuration: &RunConfiguration) -> Result<(), CoreError> {
    let steps = &configuration.before_launch;
    if steps.len() > MAXIMUM_STEPS {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "A run configuration names at most 16 steps before launch",
        ));
    }
    for step in steps {
        match step {
            BeforeLaunchStep::Configuration(step) => {
                let id = step.configuration.as_str();
                if id.trim().is_empty() || id.len() > MAXIMUM_ID_BYTES || id == configuration.id {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "A step before launch names another run configuration",
                    )
                    .with_details(id.to_string()));
                }
            }
            BeforeLaunchStep::Task(step) => step.task.validate().map_err(|_| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "A step before launch names an invalid task",
                )
            })?,
        }
    }
    Ok(())
}

/// Why a list of steps cannot run, for the field or the step it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StepRefusal {
    /// `notFound`, `notAllowed`, `unavailable`, `cycle`, `tooLong`,
    /// `taskNotDeclared` or `invalidTaskValues`.
    pub(crate) code: &'static str,
    /// The configuration the refusal is about; for a task, the one whose
    /// list names it.
    pub(crate) configuration: String,
    /// The task the refusal is about.
    pub(crate) task: Option<RunTypeKey>,
}

/// The resolved configurations by ID.
pub(crate) struct Resolved {
    root: PathBuf,
    by_id: BTreeMap<String, RunConfiguration>,
}

impl Resolved {
    pub(crate) fn read(root: &Path, layers: &Layers) -> Result<Self, CoreError> {
        let resolved = resolve_layers(root, layers, None)?;
        let configurations: Vec<RunConfiguration> =
            serde_json::from_value(resolved["configurations"].clone()).map_err(|error| {
                CoreError::new(ErrorCode::ParseFailed, "Invalid resolved run configuration")
                    .with_details(error.to_string())
            })?;
        Ok(Self {
            root: root.to_path_buf(),
            by_id: configurations
                .into_iter()
                .map(|configuration| (configuration.id.clone(), configuration))
                .collect(),
        })
    }

    /// The configurations a step of `owner` may name: every runnable built-in
    /// process run but `owner`, in name order.
    pub(crate) fn candidates(&self, owner: Option<&str>) -> Vec<Value> {
        let mut candidates = self
            .by_id
            .values()
            .filter(|configuration| {
                Some(configuration.id.as_str()) != owner
                    && process::is_process_provider(&configuration.provider)
                    && configuration.availability.is_none()
            })
            .map(|configuration| json!({ "id": configuration.id, "name": configuration.name }))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            (left["name"].as_str(), left["id"].as_str())
                .cmp(&(right["name"].as_str(), right["id"].as_str()))
        });
        candidates
    }

    /// Expands `steps`, the steps `owner` would have, into the runs and tasks
    /// they start, nested steps first. `owner` may be a configuration being
    /// created, which nothing names yet; its tasks then run at the workspace
    /// root. `tasks` are the kinds installed plugins declare.
    pub(crate) fn expand(
        &self,
        owner: &str,
        steps: &[BeforeLaunchStep],
        tasks: &[DeclaredRunType],
    ) -> Result<Expansion, StepRefusal> {
        let mut expansion = Expansion::default();
        let mut stack = vec![owner.to_string()];
        let working_directory = self
            .by_id
            .get(owner)
            .map_or(".", |configuration| configuration.cwd.as_str());
        self.expand_into(steps, working_directory, tasks, &mut stack, &mut expansion)?;
        expansion.owners.insert(0, owner.to_string());
        Ok(expansion)
    }

    fn expand_into(
        &self,
        steps: &[BeforeLaunchStep],
        working_directory: &str,
        tasks: &[DeclaredRunType],
        stack: &mut Vec<String>,
        expansion: &mut Expansion,
    ) -> Result<(), StepRefusal> {
        let owner = stack.last().cloned().unwrap_or_default();
        if steps.len() > MAXIMUM_STEPS {
            return Err(StepRefusal {
                code: "tooLong",
                configuration: owner,
                task: None,
            });
        }
        for step in steps {
            let expanded = match step {
                BeforeLaunchStep::Configuration(step) => {
                    let id = &step.configuration;
                    let refuse = |code| StepRefusal {
                        code,
                        configuration: id.clone(),
                        task: None,
                    };
                    if stack.contains(id) {
                        return Err(refuse("cycle"));
                    }
                    let target = self.by_id.get(id).ok_or_else(|| refuse("notFound"))?;
                    if !process::is_process_provider(&target.provider) {
                        return Err(refuse("notAllowed"));
                    }
                    if target.availability.is_some() {
                        return Err(refuse("unavailable"));
                    }
                    if !target.before_launch.is_empty() {
                        stack.push(id.clone());
                        expansion.owners.push(id.clone());
                        self.expand_into(
                            &target.before_launch,
                            &target.cwd,
                            tasks,
                            stack,
                            expansion,
                        )?;
                        stack.pop();
                    }
                    ExpandedStep::Run(Box::new(target.clone()))
                }
                BeforeLaunchStep::Task(step) => {
                    let reference = &step.task;
                    let refuse = |code| StepRefusal {
                        code,
                        configuration: owner.clone(),
                        task: Some(reference.key()),
                    };
                    let task = declared_task(tasks, &reference.key())
                        .map_err(|_| refuse("taskNotDeclared"))?;
                    task.launch_values(&reference.values, &self.root)
                        .map_err(|_| refuse("invalidTaskValues"))?;
                    ExpandedStep::Task(Box::new(ExpandedTask {
                        reference: reference.clone(),
                        name: task.display_name.clone(),
                        summary: task.summary(&reference.values),
                        working_directory: working_directory.to_string(),
                    }))
                }
            };
            if expansion.steps.len() >= MAXIMUM_EXPANDED_STEPS {
                return Err(StepRefusal {
                    code: "tooLong",
                    configuration: owner,
                    task: None,
                });
            }
            expansion.steps.push(expanded);
        }
        Ok(())
    }
}

/// Steps expanded for one start.
#[derive(Default)]
pub(crate) struct Expansion {
    /// What to run, in order.
    pub(crate) steps: Vec<ExpandedStep>,
    /// The configurations whose step lists were read: the one that starts,
    /// then each referenced run that names steps of its own.
    pub(crate) owners: Vec<String>,
}

/// One step of an expanded list. Both variants are boxed because they differ
/// widely in size.
pub(crate) enum ExpandedStep {
    /// A process run to start.
    Run(Box<RunConfiguration>),
    /// A plugin task to run.
    Task(Box<ExpandedTask>),
}

/// A plugin task of an expanded list.
pub(crate) struct ExpandedTask {
    reference: RunTypeReference,
    name: String,
    summary: String,
    /// The working directory of the configuration whose list names it.
    working_directory: String,
}

/// A step that refused to expand, in the shape the host shows.
fn refusal_value(refusal: &StepRefusal) -> Value {
    let mut value = json!({ "code": refusal.code, "configurationId": refusal.configuration });
    if let Some(task) = &refusal.task {
        value["task"] = json!(task);
    }
    value
}

/// The steps of a `beforeLaunch` editor value: a configuration ID string, or
/// a task step as [`editor_items`] and `runConfig.checkBeforeLaunchTask`
/// return it, of which only `task` is read. `None` when an item has another
/// shape.
pub(crate) fn steps_from_editor(items: &[Value]) -> Option<Vec<BeforeLaunchStep>> {
    items
        .iter()
        .map(|item| match item {
            Value::String(id) => Some(BeforeLaunchStep::configuration(id.trim())),
            Value::Object(object) => {
                let task = serde_json::from_value(object.get("task")?.clone()).ok()?;
                Some(BeforeLaunchStep::Task(TaskStep { task }))
            }
            _ => None,
        })
        .collect()
}

/// The `beforeLaunch` editor value of saved steps: a configuration ID for a
/// run, `{task, name, summary, values}` for a task an installed plugin
/// declares, and `{task, name, summary, missing: true}` for one no installed
/// plugin declares, so the editor still shows and keeps it.
pub(crate) fn editor_items(steps: &[BeforeLaunchStep], tasks: &[DeclaredRunType]) -> Vec<Value> {
    steps
        .iter()
        .map(|step| match step {
            BeforeLaunchStep::Configuration(step) => json!(step.configuration),
            BeforeLaunchStep::Task(step) => match declared_task(tasks, &step.task.key()) {
                Ok(task) => task.task_step_item(&step.task),
                Err(_) => json!({
                    "task": step.task,
                    "name": step.task.type_id,
                    "summary": "",
                    "missing": true,
                }),
            },
        })
        .collect()
}

/// What a `beforeLaunch` editor field offers: the configurations a step may
/// start and the kinds of task installed plugins declare.
pub(crate) struct StepChoices {
    /// `{id, name}` of each configuration, as [`Resolved::candidates`].
    pub(crate) candidates: Vec<Value>,
    /// Each declared task kind, as `DeclaredRunType::task_kind_item`.
    pub(crate) tasks: Vec<Value>,
}

impl StepChoices {
    pub(crate) fn new(resolved: &Resolved, owner: Option<&str>, kinds: &[DeclaredRunType]) -> Self {
        Self {
            candidates: resolved.candidates(owner),
            tasks: kinds
                .iter()
                .filter(|kind| kind.role == super::declared::DeclaredRole::BeforeLaunchTask)
                .map(DeclaredRunType::task_kind_item)
                .collect(),
        }
    }
}

/// The layer a configuration's own step list was read from: the local layer
/// when it names `beforeLaunch` there, else the project layer.
fn list_from_project(layers: &Layers, id: &str) -> bool {
    let names_steps = |document: &Value| {
        document["configurations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|item| item["id"] == id && item.get("beforeLaunch").is_some())
    };
    if names_steps(&layers.local.document) {
        return false;
    }
    names_steps(&layers.project.document)
}

/// A one-line description of what a process run starts, for the
/// confirmation that lists the steps.
fn summary(configuration: &RunConfiguration) -> String {
    let options = ProcessOptions::of(configuration).unwrap_or_default();
    let mut words = Vec::new();
    if let Some(program) = options.program() {
        words.push(program.to_string());
    } else if let Some(script) = options.script() {
        if let Some(interpreter) = options.interpreter() {
            words.push(interpreter.to_string());
        }
        words.push(script.to_string());
    } else if let Some(text) = options.text() {
        let first = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let more = text.lines().filter(|line| !line.trim().is_empty()).count() > 1;
        words.push(if more {
            format!("{} …", first.trim())
        } else {
            first.trim().to_string()
        });
    }
    if !configuration.args.is_empty() {
        words.push(
            crate::format_argument_text(crate::ArgumentTextRequest {
                argument_lists: vec![configuration.args.clone()],
            })
            .remove(0),
        );
    }
    words.join(" ")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request for the steps a configuration runs before it starts.
pub struct BeforeLaunchRequest {
    /// Canonical workspace root.
    pub root: String,
    /// The configuration that starts.
    pub configuration_id: String,
    /// Host-owned replacement for the machine-local layer.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// The kinds installed plugins declare, set by the dispatcher from
    /// `runTypePlugins`.
    #[serde(skip)]
    pub run_types: Vec<DeclaredRunType>,
}

/// `runConfig.beforeLaunch`: the runs and tasks to start, in order, before
/// configuration `configurationId` starts.
///
/// Replies `{status: "ok", steps, confirmation}` with each run step's
/// `{configurationId, name, summary}`, each task step's `{task, name,
/// summary, workingDirectory}` and, when a project file names any of the step
/// lists, `confirmation: {fingerprint}`. The fingerprint covers the lists and
/// what they start; the host asks once per fingerprint before running them. A
/// list that cannot run replies `{status: "rejected", refusal: {code,
/// configurationId, task}}` with `notFound`, `notAllowed` (not a built-in
/// process run), `unavailable`, `cycle`, `tooLong`, `taskNotDeclared` (no
/// installed plugin declares the task) or `invalidTaskValues` (its values no
/// longer fit the task); `task` is set for the last two.
pub fn before_launch(request: BeforeLaunchRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layers = read_layers(&root, None, request.local_document)?;
    let resolved = Resolved::read(&root, &layers)?;
    let configuration = resolved
        .by_id
        .get(&request.configuration_id)
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Run configuration was not found")
        })?;
    let expansion = match resolved.expand(
        &configuration.id,
        &configuration.before_launch,
        &request.run_types,
    ) {
        Ok(expansion) => expansion,
        Err(refusal) => {
            return Ok(json!({ "status": "rejected", "refusal": refusal_value(&refusal) }))
        }
    };
    let steps = expansion
        .steps
        .iter()
        .map(|step| match step {
            ExpandedStep::Run(step) => {
                json!({ "configurationId": step.id, "name": step.name, "summary": summary(step) })
            }
            ExpandedStep::Task(task) => json!({
                "task": task.reference,
                "name": task.name,
                "summary": task.summary,
                "workingDirectory": task.working_directory,
            }),
        })
        .collect::<Vec<_>>();
    let from_project = expansion
        .owners
        .iter()
        .any(|owner| list_from_project(&layers, owner));
    let confirmation = (from_project && !steps.is_empty())
        .then(|| json!({ "fingerprint": fingerprint(&resolved, &expansion) }));
    Ok(json!({ "status": "ok", "steps": steps, "confirmation": confirmation }))
}

/// Identity of what the steps start: each list, each run's definition, and
/// each task with its values and working directory. Changing a list or what
/// it starts asks the user again.
fn fingerprint(resolved: &Resolved, expansion: &Expansion) -> String {
    let lists = expansion
        .owners
        .iter()
        .map(|owner| {
            let steps = resolved
                .by_id
                .get(owner)
                .map(|configuration| configuration.before_launch.clone())
                .unwrap_or_default();
            json!({ "owner": owner, "steps": steps })
        })
        .collect::<Vec<_>>();
    let runs = expansion
        .steps
        .iter()
        .map(|step| match step {
            ExpandedStep::Run(step) => json!({
                "id": step.id,
                "provider": step.provider,
                "args": step.args,
                "cwd": step.cwd,
                "env": step.env,
                "process": step.extensions.get("process"),
            }),
            ExpandedStep::Task(task) => json!({
                "task": task.reference,
                "workingDirectory": task.working_directory,
            }),
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&json!({ "lists": lists, "runs": runs }))
        .expect("fingerprint input encodes");
    format!("{:x}", Sha256::digest(bytes))
}

/// Checks the steps an editor names for configuration `owner` against the
/// saved configurations and the installed plugins' tasks; the code of the
/// first that is wrong.
pub(crate) fn check_edit(
    root: &Path,
    layers: &Layers,
    owner: &str,
    steps: &[BeforeLaunchStep],
    tasks: &[DeclaredRunType],
) -> Result<Option<&'static str>, CoreError> {
    if steps.len() > MAXIMUM_STEPS {
        return Ok(Some("tooLong"));
    }
    for step in steps {
        match step {
            BeforeLaunchStep::Configuration(step) => {
                let id = step.configuration.trim();
                if id.is_empty() || id == owner {
                    return Ok(Some("notAllowed"));
                }
            }
            BeforeLaunchStep::Task(step) => {
                if step.task.validate().is_err() {
                    return Ok(Some("invalidTaskValues"));
                }
            }
        }
    }
    let resolved = Resolved::read(root, layers)?;
    Ok(resolved
        .expand(owner, steps, tasks)
        .err()
        .map(|refusal| match refusal.code {
            "cycle" => "conflict",
            code => code,
        }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to check the values of a task step being added or edited.
pub struct CheckBeforeLaunchTaskRequest {
    /// Canonical workspace root, which file and directory values are
    /// resolved against.
    pub root: String,
    /// The task kind, as in the `tasks` of a `beforeLaunch` field.
    pub run_type: RunTypeKey,
    /// The values of the task's form by editor key, as `values` of the task
    /// kind or step item gave them.
    #[serde(default)]
    pub values: Map<String, Value>,
    /// The kinds installed plugins declare, set by the dispatcher from
    /// `runTypePlugins`.
    #[serde(skip)]
    pub run_types: Vec<DeclaredRunType>,
}

/// `runConfig.checkBeforeLaunchTask`: checks the form of a task step and
/// returns the step to put in the `beforeLaunch` editor value.
///
/// Replies `{status: "ok", step: {task, name, summary, values}}`, or
/// `{status: "invalid", errors: [{field, code}]}` with the editor keys of the
/// wrong fields. A task no installed plugin declares fails the call with
/// `NotSupported`. Nothing is written; the step is saved with the
/// configuration.
pub fn check_before_launch_task(request: CheckBeforeLaunchTaskRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let task = declared_task(&request.run_types, &request.run_type)?;
    let (values, errors): (_, Vec<FieldError>) = task.store(&root, &request.values)?;
    if !errors.is_empty() {
        return Ok(json!({ "status": "invalid", "errors": errors }));
    }
    let reference = RunTypeReference {
        plugin_id: task.key.plugin_id.clone(),
        build_system_id: task.key.build_system_id.clone(),
        type_id: task.key.type_id.clone(),
        values,
    };
    reference.validate()?;
    Ok(json!({ "status": "ok", "step": task.task_step_item(&reference) }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request for the Before Launch list of a saved configuration, for an editor
/// that does not use `runConfig.editorForm`.
pub struct BeforeLaunchFieldRequest {
    /// Canonical workspace root.
    pub root: String,
    /// The saved configuration whose list is edited.
    pub configuration_id: String,
    /// Host-owned replacement for the machine-local layer.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// The kinds installed plugins declare, set by the dispatcher from
    /// `runTypePlugins`.
    #[serde(skip)]
    pub run_types: Vec<DeclaredRunType>,
}

/// `runConfig.beforeLaunchField`: the Before Launch list of configuration
/// `configurationId` as the `beforeLaunch` field of `runConfig.editorForm`
/// gives it.
///
/// Replies `{candidates, tasks, value}`: the configurations a step may start,
/// the task kinds installed plugins declare, and the effective list as editor
/// items. The host sends the edited items back through
/// `runConfig.updateOptions`. An unknown ID fails with `InvalidRequest`.
pub fn before_launch_field(request: BeforeLaunchFieldRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layers = read_layers(&root, None, request.local_document)?;
    let resolved = Resolved::read(&root, &layers)?;
    let configuration = resolved
        .by_id
        .get(&request.configuration_id)
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Run configuration was not found")
        })?;
    let choices = StepChoices::new(&resolved, Some(&configuration.id), &request.run_types);
    Ok(json!({
        "candidates": choices.candidates,
        "tasks": choices.tasks,
        "value": editor_items(&configuration.before_launch, &request.run_types),
    }))
}
