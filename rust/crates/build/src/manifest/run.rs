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
    /// `buildProviders` 1: the provider names entrypoints in source files
    /// (`lithe/build/runEntrypoints`), and the run starts one of them in a
    /// project. It applies to no target and is never listed per target.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) entrypoints: bool,
    /// `runConfigurationTypes` 1: ID of a `runConfigurationTypes` entry in
    /// this build system. Each matching target materializes a generated
    /// configuration of that declared kind instead of a raw command or
    /// provider run, sharing the kind's icon, group and editor form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) configuration_type: Option<String>,
    /// `runConfigurationTypes` 1: initial field values for `configurationType`,
    /// keyed by declared field ID. String values and `arguments` list items
    /// may reference target and project template variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) values: BTreeMap<String, Value>,
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
            if !run.values.is_empty() && run.configuration_type.is_none() {
                return Err(invalid(
                    format!("{field}.values"),
                    "Run field values require configurationType.",
                ));
            }
            if let Some(type_id) = run.configuration_type.as_deref() {
                if run.source == TaskSource::Provider
                    || run.debug.is_some()
                    || run.entrypoints
                    || run.from_tasks.is_some()
                    || !run.arguments.is_empty()
                    || !run.extension.is_empty()
                    || run.execution.is_some()
                {
                    return Err(invalid(
                        &field,
                        "A run entry with configurationType uses target tags, field values, and the declared type's command line.",
                    ));
                }
                let Some(targets) = &mut run.from_targets else {
                    return Err(invalid(
                        format!("{field}.fromTargets"),
                        "A run entry with configurationType requires fromTargets.",
                    ));
                };
                if targets.tags.is_empty()
                    || targets.tags.len() > MAX_LIST
                    || targets.tags.iter().any(|tag| !is_tag(tag))
                {
                    return Err(invalid(
                        format!("{field}.fromTargets.tags"),
                        "fromTargets needs one or more tag identifiers.",
                    ));
                }
                targets.tags = sorted_unique(std::mem::take(&mut targets.tags));
                let Some(run_type) = self
                    .run_configuration_types
                    .iter()
                    .find(|declared| declared.id == type_id)
                else {
                    return Err(invalid(
                        format!("{field}.configurationType"),
                        format!(
                            "configurationType {type_id} is not declared in runConfigurationTypes."
                        ),
                    ));
                };
                let allowed = Allowed {
                    root: true,
                    project: Availability::Always,
                    target: Availability::Always,
                    ..Allowed::none()
                };
                if let Some(name) = &run.name {
                    template::validate_text(name, &allowed, &format!("{field}.name"))?;
                }
                validate_run_type_values(
                    run_type,
                    &run.values,
                    &allowed,
                    &format!("{field}.values"),
                )?;
                continue;
            }
            if run.debug.is_some() && run.source != TaskSource::Provider {
                return Err(invalid(
                    &field,
                    "Debug preparation requires a provider run.",
                ));
            }
            if run.entrypoints {
                if run.source != TaskSource::Provider
                    || run.from_targets.is_some()
                    || run.from_tasks.is_some()
                    || !run.arguments.is_empty()
                    || !run.extension.is_empty()
                    || run.execution.unwrap_or(BuildExecution::Application)
                        != BuildExecution::Application
                    || run
                        .debug
                        .as_ref()
                        .is_some_and(|debug| debug.adapter.is_some())
                    || run.name.as_ref().is_some_and(|name| {
                        name.trim().is_empty() || name.len() > MAX_TEXT || name.contains("${")
                    })
                {
                    return Err(invalid(
                        &field,
                        "Entrypoint runs are provider applications without targets, templates or native debug preparation.",
                    ));
                }
                continue;
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

/// Checks `values` of a `buildSystems[].run[]` entry that targets `run_type`.
fn validate_run_type_values(
    run_type: &RunTypeDeclaration,
    values: &BTreeMap<String, Value>,
    allowed: &Allowed<'_>,
    field: &str,
) -> Validation<()> {
    for (id, value) in values {
        let value_field = format!("{field}.{id}");
        let Some(declared) = run_type.fields.iter().find(|entry| entry.id == *id) else {
            return Err(invalid(
                value_field,
                format!(
                    "{id} is not a field of runConfigurationType {}.",
                    run_type.id
                ),
            ));
        };
        match (declared.kind, value) {
            (
                RunFieldKind::Text
                | RunFieldKind::Choice
                | RunFieldKind::File
                | RunFieldKind::Directory,
                Value::String(text),
            ) => {
                if text.len() > MAX_TEXT || text.contains('\0') {
                    return Err(invalid(value_field, "Field value is invalid."));
                }
                template::validate_text(text, allowed, &value_field)?;
                if declared.kind == RunFieldKind::Choice
                    && !text.contains("${")
                    && !declared.choices.iter().any(|choice| choice.value == *text)
                {
                    return Err(invalid(
                        value_field,
                        "Choice field value must match one of the declared choices.",
                    ));
                }
            }
            (RunFieldKind::Checkbox, Value::Bool(_)) => {}
            (RunFieldKind::Arguments, Value::Array(items)) => {
                if items.len() > 64 {
                    return Err(invalid(value_field, "Too many arguments in field value."));
                }
                for (item_index, item) in items.iter().enumerate() {
                    let Some(text) = item.as_str() else {
                        return Err(invalid(
                            format!("{value_field}[{item_index}]"),
                            "Arguments field values must be strings.",
                        ));
                    };
                    if text.len() > MAX_TEXT || text.contains('\0') {
                        return Err(invalid(
                            format!("{value_field}[{item_index}]"),
                            "Field value is invalid.",
                        ));
                    }
                    template::validate_text(
                        text,
                        allowed,
                        &format!("{value_field}[{item_index}]"),
                    )?;
                }
            }
            _ => {
                return Err(invalid(
                    value_field,
                    "Field value does not match the declared field kind.",
                ));
            }
        }
    }

    // Ensure every visible required field without a default is supplied.
    let mut effective: BTreeMap<&str, Value> = BTreeMap::new();
    for declared in &run_type.fields {
        let visible = declared.visible_when.as_ref().is_none_or(|condition| {
            effective
                .get(condition.field.as_str())
                .is_some_and(|actual| *actual == condition.equals)
        });
        if !visible {
            continue;
        }
        let chosen = values
            .get(&declared.id)
            .cloned()
            .or_else(|| declared.default.clone())
            .or_else(|| {
                (declared.kind == RunFieldKind::Choice)
                    .then(|| {
                        declared
                            .choices
                            .first()
                            .map(|choice| Value::String(choice.value.clone()))
                    })
                    .flatten()
            })
            .or_else(|| (declared.kind == RunFieldKind::Checkbox).then_some(Value::Bool(false)));
        let empty = match &chosen {
            None => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            Some(Value::Array(items)) => items.is_empty(),
            Some(_) => false,
        };
        if declared.required && empty {
            return Err(invalid(
                format!("{field}.{}", declared.id),
                format!(
                    "Required field {} of runConfigurationType {} must have a value or default.",
                    declared.id, run_type.id
                ),
            ));
        }
        if let Some(chosen) = chosen {
            effective.insert(&declared.id, chosen);
        }
    }
    Ok(())
}
