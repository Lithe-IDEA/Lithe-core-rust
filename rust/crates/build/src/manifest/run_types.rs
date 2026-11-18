//! `runConfigurationTypes`: kinds of run configuration a build system adds to
//! the Run/Debug Configurations dialog, and `beforeLaunchTasks`: kinds of step
//! a configuration runs before it starts.
//!
//! A type is data only: the fields its editor shows and the argv template
//! those fields fill. The host draws the fields Core describes, Core checks
//! and stores the values, and a run expands the template with them, so a
//! plugin adds a configuration kind such as `Go Build` without a backend.
//! A before-launch task is declared the same way; it runs to completion in
//! the owning configuration's working directory and takes no program
//! arguments, so it has neither an `execution` nor `${userArguments}`.

use super::*;

const MAX_RUN_TYPES: usize = 16;
const MAX_RUN_FIELDS: usize = 16;
const MAX_FIELD_ARGUMENTS: usize = 8;
/// Longest description of a type or a field.
const MAX_DESCRIPTION: usize = 512;

/// One kind of run configuration the user can create, or one kind of
/// before-launch task.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunTypeDeclaration {
    /// Unique within the build system; stored in every configuration of the type.
    pub(crate) id: String,
    pub(crate) display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// Icon of the type in the dialog's `+` menu and list; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<UIIcon>,
    /// How the started process behaves; `application` by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) execution: Option<BuildExecution>,
    /// The fields of the editor, in display order, between the name and the
    /// program arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) fields: Vec<RunFieldDeclaration>,
    /// The command line after the global arguments. It names every field
    /// that passes a value as `${field:<id>}`, and the program arguments as
    /// `${userArguments}` when the type takes them; the editor shows the
    /// program arguments field only then.
    pub(crate) arguments: Vec<BuildArgument>,
}

/// One field of a run configuration type's editor.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunFieldDeclaration {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) kind: RunFieldKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) placeholder: Option<String>,
    /// The values of a `choice` field, in display order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) choices: Vec<OptionChoice>,
    /// The value of a new configuration: a string for `text` and `choice`,
    /// a boolean for `checkbox`, a list of strings for `arguments`. File and
    /// directory fields have none; a plugin cannot know a path of this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default: Option<Value>,
    /// A visible field may not be left empty.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) required: bool,
    /// The field is shown, checked and passed only while an earlier `choice`
    /// or `checkbox` field holds `equals`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) visible_when: Option<RunFieldCondition>,
    /// Templates that pass the value, `${value}` standing for it, such as
    /// `["-o", "${value}"]`. Without them a value is passed as one argument;
    /// a checkbox passes them while it is checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) arguments: Vec<String>,
}

/// What a run configuration field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RunFieldKind {
    /// One line of text, passed as is.
    Text,
    /// One of the declared choices.
    Choice,
    /// On or off.
    Checkbox,
    /// A file, passed as an absolute path.
    File,
    /// A directory, passed as an absolute path.
    Directory,
    /// Arguments the user types, split like the program arguments.
    Arguments,
}

/// Which list of a build system a declaration belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunTypeRole {
    /// `runConfigurationTypes`: a configuration the user creates and runs.
    Configuration,
    /// `beforeLaunchTasks`: a step of another configuration's Before Launch list.
    BeforeLaunchTask,
}

impl RunTypeRole {
    /// The manifest key of the list.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Configuration => "runConfigurationTypes",
            Self::BeforeLaunchTask => "beforeLaunchTasks",
        }
    }
}

impl RunFieldKind {
    /// Whether another field can be shown depending on this one's value.
    fn controls_visibility(self) -> bool {
        matches!(self, Self::Choice | Self::Checkbox)
    }
}

/// The value an earlier field must hold for a field to be shown.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunFieldCondition {
    pub(crate) field: String,
    pub(crate) equals: Value,
}

impl BuildSystemDeclaration {
    /// Whether the build system adds run configuration types, which
    /// `runConfigurationTypes` introduced.
    pub fn has_run_configuration_types(&self) -> bool {
        !self.run_configuration_types.is_empty()
    }

    /// Whether the build system adds before-launch tasks, which
    /// `beforeLaunchTasks` introduced.
    pub fn has_before_launch_tasks(&self) -> bool {
        !self.before_launch_tasks.is_empty()
    }

    /// The declarations of one list.
    pub(crate) fn run_types(&self, role: RunTypeRole) -> &[RunTypeDeclaration] {
        match role {
            RunTypeRole::Configuration => &self.run_configuration_types,
            RunTypeRole::BeforeLaunchTask => &self.before_launch_tasks,
        }
    }

    /// The declaration `id` of one list.
    pub(crate) fn run_type(&self, role: RunTypeRole, id: &str) -> Option<&RunTypeDeclaration> {
        self.run_types(role)
            .iter()
            .find(|run_type| run_type.id == id)
    }

    pub(crate) fn validate_run_types(
        &mut self,
        field: &str,
        options: &BTreeSet<String>,
    ) -> Validation<()> {
        for role in [RunTypeRole::Configuration, RunTypeRole::BeforeLaunchTask] {
            let list = format!("{field}.{}", role.key());
            let declarations = self.run_types(role);
            if declarations.len() > MAX_RUN_TYPES {
                return Err(invalid(
                    list,
                    match role {
                        RunTypeRole::Configuration => "Too many run configuration types.",
                        RunTypeRole::BeforeLaunchTask => "Too many before-launch tasks.",
                    },
                ));
            }
            let mut ids = BTreeSet::new();
            for (index, run_type) in declarations.iter().enumerate() {
                let field = format!("{list}[{index}]");
                if !is_entry_identifier(&run_type.id) || !ids.insert(run_type.id.clone()) {
                    return Err(invalid(
                        format!("{field}.id"),
                        match role {
                            RunTypeRole::Configuration => "Run configuration type IDs are unique identifiers that start with a letter.",
                            RunTypeRole::BeforeLaunchTask => "Before-launch task IDs are unique identifiers that start with a letter.",
                        },
                    ));
                }
                run_type.validate(&field, options, role)?;
            }
        }
        Ok(())
    }
}

impl RunTypeDeclaration {
    /// How a configuration of this type runs.
    pub(crate) fn execution(&self) -> BuildExecution {
        self.execution.unwrap_or(BuildExecution::Application)
    }

    /// Whether the command line takes the user's program arguments.
    pub(crate) fn takes_arguments(&self) -> bool {
        template::user_argument_references(&self.arguments) == 1
    }

    fn validate(
        &self,
        field: &str,
        options: &BTreeSet<String>,
        role: RunTypeRole,
    ) -> Validation<()> {
        check_text(
            &self.display_name,
            &format!("{field}.displayName"),
            "display name",
        )?;
        if let Some(description) = &self.description {
            check_description(description, &format!("{field}.description"))?;
        }
        if let Some(icon) = &self.icon {
            icon.validate_at(&format!("{field}.icon"))?;
        }
        match role {
            RunTypeRole::Configuration if self.execution == Some(BuildExecution::Service) => {
                return Err(invalid(
                    format!("{field}.execution"),
                    "A run configuration type runs an application or a task.",
                ));
            }
            RunTypeRole::BeforeLaunchTask if self.execution.is_some() => {
                return Err(invalid(
                    format!("{field}.execution"),
                    "A before-launch task always runs to completion; it declares no execution.",
                ));
            }
            _ => {}
        }
        if self.fields.len() > MAX_RUN_FIELDS {
            return Err(invalid(format!("{field}.fields"), "Too many fields."));
        }
        let mut ids = BTreeSet::new();
        for (index, declared) in self.fields.iter().enumerate() {
            let field = format!("{field}.fields[{index}]");
            if !is_setting_identifier(&declared.id) || !ids.insert(declared.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "Field IDs are unique, start with a lowercase letter, and use letters and digits.",
                ));
            }
            declared.validate(&field, &self.fields[..index])?;
        }
        let arguments_field = format!("{field}.arguments");
        check_argument_count(&self.arguments, &arguments_field)?;
        // A before-launch task has no program arguments field, so its
        // command line cannot name them.
        let allowed = Allowed {
            root: true,
            user_arguments: role == RunTypeRole::Configuration,
            options: Some(options),
            fields: Some(&ids),
            ..Allowed::none()
        };
        template::validate_arguments(&self.arguments, &allowed, &arguments_field)?;
        let references = template::field_references(&self.arguments);
        if let Some(twice) = references.iter().find(|(_, count)| **count > 1) {
            return Err(template_error(
                arguments_field,
                format!("${{field:{}}} is named more than once.", twice.0),
            ));
        }
        if template::user_argument_references(&self.arguments) > 1 {
            return Err(template_error(
                arguments_field,
                "The arguments name ${userArguments} at most once, where the program arguments go.",
            ));
        }
        for (index, declared) in self.fields.iter().enumerate() {
            if references.contains_key(&declared.id) {
                continue;
            }
            // A field the command line does not name only shows or hides
            // other fields.
            let controls = self.fields.iter().any(|other| {
                other
                    .visible_when
                    .as_ref()
                    .is_some_and(|condition| condition.field == declared.id)
            });
            if !controls || !declared.arguments.is_empty() {
                return Err(template_error(
                    format!("{field}.fields[{index}]"),
                    format!(
                        "The field is never passed; name it as ${{field:{}}} in the arguments.",
                        declared.id
                    ),
                ));
            }
        }
        Ok(())
    }
}

impl RunFieldDeclaration {
    /// Checks the field against the fields declared before it.
    fn validate(&self, field: &str, earlier: &[RunFieldDeclaration]) -> Validation<()> {
        check_text(&self.title, &format!("{field}.title"), "title")?;
        if let Some(description) = &self.description {
            check_description(description, &format!("{field}.description"))?;
        }
        if let Some(placeholder) = &self.placeholder {
            check_text(placeholder, &format!("{field}.placeholder"), "placeholder")?;
        }
        if (self.kind == RunFieldKind::Choice) == self.choices.is_empty() {
            return Err(invalid(
                format!("{field}.choices"),
                "A choice field needs choices, and only a choice field has them.",
            ));
        }
        if self.choices.len() > MAX_CHOICES {
            return Err(invalid(format!("{field}.choices"), "Too many choices."));
        }
        let mut values = BTreeSet::new();
        for (index, choice) in self.choices.iter().enumerate() {
            if !is_option_value(&choice.value) || !values.insert(choice.value.as_str()) {
                return Err(invalid(
                    format!("{field}.choices[{index}].value"),
                    "Choice values are unique and printable.",
                ));
            }
            if let Some(title) = &choice.title {
                check_text(title, &format!("{field}.choices[{index}].title"), "title")?;
            }
        }
        if self.required && matches!(self.kind, RunFieldKind::Checkbox | RunFieldKind::Choice) {
            return Err(invalid(
                format!("{field}.required"),
                "A checkbox or choice field always holds a value.",
            ));
        }
        if let Some(default) = &self.default {
            if !self.fits(default)
                || matches!(self.kind, RunFieldKind::File | RunFieldKind::Directory)
            {
                return Err(invalid(
                    format!("{field}.default"),
                    "The default does not fit the field's kind and choices; file and directory fields have no default.",
                ));
            }
        }
        if let Some(condition) = &self.visible_when {
            let controller = earlier
                .iter()
                .find(|other| other.id == condition.field)
                .filter(|other| other.kind.controls_visibility());
            if !controller.is_some_and(|controller| controller.fits(&condition.equals)) {
                return Err(invalid(
                    format!("{field}.visibleWhen"),
                    "visibleWhen names an earlier choice or checkbox field and one of its values.",
                ));
            }
        }
        self.validate_arguments(field)
    }

    fn validate_arguments(&self, field: &str) -> Validation<()> {
        let field = format!("{field}.arguments");
        if self.arguments.len() > MAX_FIELD_ARGUMENTS {
            return Err(invalid(
                field,
                format!("A field maps to at most {MAX_FIELD_ARGUMENTS} arguments."),
            ));
        }
        match self.kind {
            RunFieldKind::Arguments if !self.arguments.is_empty() => {
                return Err(invalid(
                    field,
                    "An arguments field passes the arguments the user typed.",
                ))
            }
            RunFieldKind::Checkbox if self.arguments.is_empty() => {
                return Err(invalid(
                    field,
                    "A checkbox passes its arguments while it is checked; declare them.",
                ))
            }
            _ => {}
        }
        let checkbox = self.kind == RunFieldKind::Checkbox;
        let allowed = Allowed {
            value: !checkbox,
            ..Allowed::none()
        };
        for (index, argument) in self.arguments.iter().enumerate() {
            template::validate_text(argument, &allowed, &format!("{field}[{index}]"))?;
        }
        if !checkbox
            && !self.arguments.is_empty()
            && !self
                .arguments
                .iter()
                .any(|argument| argument.contains("${value}"))
        {
            return Err(template_error(
                field,
                "The field's value is never passed; use ${value}.",
            ));
        }
        Ok(())
    }

    /// Whether `value` is a value of this field.
    pub(crate) fn fits(&self, value: &Value) -> bool {
        match (self.kind, value) {
            (RunFieldKind::Checkbox, Value::Bool(_)) => true,
            (RunFieldKind::Choice, Value::String(value)) => {
                self.choices.iter().any(|choice| &choice.value == value)
            }
            (
                RunFieldKind::Text | RunFieldKind::File | RunFieldKind::Directory,
                Value::String(value),
            ) => value.is_empty() || is_option_value(value),
            (RunFieldKind::Arguments, Value::Array(items)) => {
                items.len() <= MAX_ARGUMENTS
                    && items
                        .iter()
                        .all(|item| item.as_str().is_some_and(is_option_value))
            }
            _ => false,
        }
    }
}

pub(crate) fn check_description(value: &str, field: &str) -> Validation<()> {
    if value.trim().is_empty()
        || value.len() > MAX_DESCRIPTION
        || value
            .chars()
            .any(|character| character.is_control() && character != '\n')
    {
        return Err(invalid(
            field,
            format!("The description must be 1-{MAX_DESCRIPTION} printable characters."),
        ));
    }
    Ok(())
}
