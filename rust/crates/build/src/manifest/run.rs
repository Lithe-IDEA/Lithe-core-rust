//! `run`: run declarations derived from targets or tasks.

use super::*;

/// Run entries derived from model targets or tasks.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunDeclaration {
    /// A same-package adapter for native executable debug preparation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) debug: Option<RunDebug>,
    pub(crate) id: String,
    /// Provider runs prepare a launch plan; they never become empty argv entries.
    #[serde(default, skip_serializing_if = "TaskSource::is_command")]
    pub(crate) source: TaskSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) from_targets: Option<FromTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) from_tasks: Option<FromTasks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) execution: Option<BuildExecution>,
    /// Entry name; the target or task name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// Argument templates for target entries; task entries reuse the task's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) arguments: Vec<BuildArgument>,
    #[serde(default)]
    pub(crate) working_directory: WorkingDirectory,
    /// Metadata stored under `extensions.<buildSystemID>` of the entry.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) extension: BTreeMap<String, String>,
}

/// Native launch binds a local adapter; connection attach names a public registry type.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunDebug {
    /// Local adapter ID for v5 native executable launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) adapter: Option<String>,
    /// Opaque public registry alias for v11 service connection attach.
    #[serde(
        default,
        rename = "configurationType",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) configuration_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FromTargets {
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FromTasks {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) templates: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) executions: Vec<BuildExecution>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WorkingDirectory {
    #[default]
    Project,
    Root,
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_run(
        &mut self,
        field: &str,
        options: &BTreeSet<String>,
        templates: &BTreeSet<String>,
    ) -> Validation<()> {
        if self.run.is_empty() {
            return Ok(());
        }
        if self.model.is_none() {
            return Err(invalid(
                format!("{field}.run"),
                "Run entries come from the model; declare a model.",
            ));
        }
        if self.run.len() > MAX_LIST {
            return Err(invalid(
                format!("{field}.run"),
                "Too many run declarations.",
            ));
        }
        let mut ids = BTreeSet::new();
        for (index, run) in self.run.iter_mut().enumerate() {
            let field = format!("{field}.run[{index}]");
            if !is_entry_identifier(&run.id) || !ids.insert(run.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "Run IDs are unique identifiers that start with a letter.",
                ));
            }
            if run.debug.is_some() && run.source != TaskSource::Provider {
                return Err(invalid(
                    &field,
                    "Debug preparation requires a provider run.",
                ));
            }
            if run.source == TaskSource::Provider {
                let valid_targets = run.from_targets.as_ref().is_some_and(|from| {
                    !from.tags.is_empty()
                        && from.tags.len() <= MAX_LIST
                        && from.tags.iter().all(|tag| is_tag(tag))
                });
                if !valid_targets
                    || run.from_tasks.is_some()
                    || !run.arguments.is_empty()
                    || !run.extension.is_empty()
                    || run.name.as_ref().is_some_and(|name| {
                        name.trim().is_empty() || name.len() > MAX_TEXT || name.contains("${")
                    })
                {
                    return Err(invalid(
                        &field,
                        "Provider runs require target tags and no host command templates.",
                    ));
                }
                let from = run.from_targets.as_mut().expect("validated target source");
                from.tags = sorted_unique(std::mem::take(&mut from.tags));
                continue;
            }
            let from_tasks = match (&mut run.from_targets, &mut run.from_tasks) {
                (Some(targets), None) => {
                    if targets.tags.is_empty() || targets.tags.iter().any(|tag| !is_tag(tag)) {
                        return Err(invalid(
                            format!("{field}.fromTargets.tags"),
                            "fromTargets needs one or more tag identifiers.",
                        ));
                    }
                    targets.tags = sorted_unique(std::mem::take(&mut targets.tags));
                    if run.arguments.is_empty() {
                        return Err(invalid(
                            format!("{field}.arguments"),
                            "Target run entries need arguments.",
                        ));
                    }
                    false
                }
                (None, Some(tasks)) => {
                    if tasks.templates.is_empty() && tasks.executions.is_empty() {
                        return Err(invalid(
                            format!("{field}.fromTasks"),
                            "fromTasks needs templates, executions, or both.",
                        ));
                    }
                    if let Some(unknown) =
                        tasks.templates.iter().find(|id| !templates.contains(*id))
                    {
                        return Err(invalid(
                            format!("{field}.fromTasks.templates"),
                            format!("{unknown} is not a fromModel task."),
                        ));
                    }
                    if self.tasks.iter().any(|task| {
                        task.source == TaskSource::Provider
                            && task.from_model
                            && (tasks.templates.is_empty() || tasks.templates.contains(&task.id))
                    }) {
                        return Err(invalid(format!("{field}.fromTasks"),
                            "Provider tasks need a typed run capability and cannot become command run entries."));
                    }
                    tasks.templates = sorted_unique(std::mem::take(&mut tasks.templates));
                    tasks.executions.sort();
                    tasks.executions.dedup();
                    if !run.arguments.is_empty() {
                        return Err(invalid(
                            format!("{field}.arguments"),
                            "Task run entries reuse the task's arguments.",
                        ));
                    }
                    true
                }
                _ => {
                    return Err(invalid(
                        field,
                        "A run declaration needs exactly one of fromTargets and fromTasks.",
                    ))
                }
            };
            let allowed = Allowed {
                root: true,
                project: Availability::Always,
                target: if from_tasks {
                    Availability::Never
                } else {
                    Availability::Always
                },
                task: from_tasks,
                user_arguments: true,
                options: Some(options),
                ..Allowed::none()
            };
            check_argument_count(&run.arguments, &format!("{field}.arguments"))?;
            template::validate_arguments(&run.arguments, &allowed, &format!("{field}.arguments"))?;
            let names = Allowed {
                user_arguments: false,
                options: None,
                ..allowed.clone()
            };
            if let Some(name) = &run.name {
                template::validate_text(name, &names, &format!("{field}.name"))?;
            }
            if run.extension.len() > MAX_LIST {
                return Err(invalid(
                    format!("{field}.extension"),
                    "Too many extension values.",
                ));
            }
            for (key, value) in &run.extension {
                if !template::is_property_key(key) {
                    return Err(invalid(
                        format!("{field}.extension"),
                        "Extension keys use letters, digits, ., _, and -.",
                    ));
                }
                template::validate_text(value, &names, &format!("{field}.extension.{key}"))?;
            }
        }
        Ok(())
    }
}
