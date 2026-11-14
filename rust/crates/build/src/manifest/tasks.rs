//! `execution` and `tasks`: task groups, scopes, preparation and sources.

use super::*;

/// How a task's process behaves; the same values as run configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BuildExecution {
    Application,
    Service,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TaskGroup {
    Lifecycle,
    Build,
    Test,
    Run,
    Verification,
    Documentation,
    Other,
}

/// What a task, test, or dependency command acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Scope {
    Workspace,
    Project,
    Target,
}

/// A task in the build system's catalog. `fromModel` tasks are templates
/// for the tasks the model lists.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskDeclaration {
    pub(crate) id: String,
    /// Optional automatic preparation selected by purpose and model source role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preparation: Option<TaskPreparation>,
    pub(crate) display_name: String,
    pub(crate) group: TaskGroup,
    #[serde(default = "task_execution")]
    pub(crate) execution: BuildExecution,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) scopes: Vec<Scope>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) from_model: bool,
    /// For target-scoped tasks: tags of the targets it applies to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) target_tags: Vec<String>,
    /// The host must ask for arguments before running it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) requires_user_arguments: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// Headings the host files the task under in its project, outermost
    /// first (`["Lifecycle"]`); also the default of the model tasks this
    /// template runs. Display only; empty files the task by `group`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) category: Vec<String>,
    #[serde(default, skip_serializing_if = "TaskSource::is_command")]
    pub(crate) source: TaskSource,
    pub(crate) arguments: Vec<BuildArgument>,
}

/// Tool-independent association between a project task and a language workflow.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskPreparation {
    /// Opaque workflow purpose, such as `run`; never a task ID or tool name.
    pub(crate) purpose: String,
    /// Canonical content-root roles for which this task prepares its project.
    pub(crate) source_roles: Vec<String>,
}

/// Whether Core expands a command or a workspace provider executes the task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TaskSource {
    #[default]
    #[serde(skip_deserializing)]
    Command,
    Provider,
}

impl TaskSource {
    pub(crate) fn is_command(&self) -> bool {
        *self == Self::Command
    }
}

fn task_execution() -> BuildExecution {
    BuildExecution::Task
}

impl BuildSystemDeclaration {
    /// Validates the catalog and returns the IDs of `fromModel` templates.
    pub(crate) fn validate_tasks(
        &mut self,
        field: &str,
        options: &BTreeSet<String>,
    ) -> Validation<BTreeSet<String>> {
        if self.tasks.len() > MAX_TASKS {
            return Err(invalid(format!("{field}.tasks"), "Too many tasks."));
        }
        let mut ids = BTreeSet::new();
        let mut templates = BTreeSet::new();
        let mut preparations = BTreeSet::new();
        for (index, task) in self.tasks.iter_mut().enumerate() {
            let field = format!("{field}.tasks[{index}]");
            if let Some(preparation) = &mut task.preparation {
                if !is_entry_identifier(&preparation.purpose)
                    || preparation.source_roles.is_empty()
                    || preparation.source_roles.len() > 6
                    || task.from_model
                    || task.execution != BuildExecution::Task
                    || task.requires_user_arguments
                    || !task.scopes.contains(&Scope::Project)
                    || preparation.source_roles.iter().any(|role| {
                        !matches!(
                            role.as_str(),
                            "source"
                                | "generatedSource"
                                | "resource"
                                | "testSource"
                                | "generatedTestSource"
                                | "testResource"
                        ) || !preparations.insert((preparation.purpose.clone(), role.clone()))
                    })
                {
                    return Err(invalid(format!("{field}.preparation"),
                        "Preparation requires a project task without prompts and unique purpose/source-role bindings."));
                }
                preparation.source_roles.sort();
            }
            if !is_entry_identifier(&task.id) || !ids.insert(task.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "Task IDs are unique identifiers that start with a letter.",
                ));
            }
            check_text(
                &task.display_name,
                &format!("{field}.displayName"),
                "display name",
            )?;
            if let Some(description) = &task.description {
                if description.len() > 1024 {
                    return Err(invalid(
                        format!("{field}.description"),
                        "The description is too long.",
                    ));
                }
            }
            if !task.category.is_empty() && category_labels(&task.category).is_none() {
                return Err(invalid(
                    format!("{field}.category"),
                    format!(
                        "A category is 1-{MAX_CATEGORY_DEPTH} headings of 1-{MAX_TEXT} printable characters."
                    ),
                ));
            }
            let allowed = if task.from_model {
                if !task.scopes.is_empty() || !task.target_tags.is_empty() {
                    return Err(invalid(
                        field,
                        "A fromModel task acts on its model task; it has no scopes or targetTags.",
                    ));
                }
                templates.insert(task.id.clone());
                Allowed {
                    root: true,
                    project: Availability::Always,
                    task: true,
                    user_arguments: true,
                    conditions: true,
                    options: Some(options),
                    ..Allowed::none()
                }
            } else {
                check_scopes(
                    &mut task.scopes,
                    &format!("{field}.scopes"),
                    &[Scope::Workspace, Scope::Project, Scope::Target],
                )?;
                check_target_tags(&mut task.target_tags, &task.scopes, &field)?;
                Allowed {
                    root: true,
                    project: availability(&task.scopes, Scope::Project),
                    target: availability(&task.scopes, Scope::Target),
                    user_arguments: true,
                    conditions: true,
                    options: Some(options),
                    ..Allowed::none()
                }
            };
            if task.source == TaskSource::Provider {
                if !task.arguments.is_empty() || task.execution != BuildExecution::Task {
                    return Err(invalid(
                        &field,
                        "A provider task has empty arguments and task execution.",
                    ));
                }
                continue;
            }
            check_argument_count(&task.arguments, &format!("{field}.arguments"))?;
            template::validate_arguments(&task.arguments, &allowed, &format!("{field}.arguments"))?;
            if task.requires_user_arguments
                && !template::uses(&task.arguments, &Variable::UserArguments)
            {
                return Err(invalid(
                    format!("{field}.requiresUserArguments"),
                    "A task that asks for arguments must pass ${userArguments}.",
                ));
            }
        }
        if !templates.is_empty() && self.model.is_none() {
            return Err(invalid(
                format!("{field}.model"),
                "fromModel tasks need a model.",
            ));
        }
        Ok(templates)
    }
}
