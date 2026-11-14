//! `options`: declared build options and how their values are emitted.

use super::*;

/// A build dimension the user chooses, stored in its declared scope by the host.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OptionDeclaration {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) kind: OptionKind,
    /// Instance values belong to one detected root, never to its siblings.
    #[serde(default, skip_serializing_if = "OptionScope::is_workspace")]
    pub(crate) scope: OptionScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) choices: Vec<OptionChoice>,
    /// Choices also come from the model's `optionValues` for this option.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) choices_from_model: bool,
    /// Values outside the known choices are accepted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) allow_custom: bool,
    /// Joins multi-select values in `${value}`; `,` by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) separator: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) environment: BTreeMap<String, String>,
    /// Where the host offers the option; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) presentation: Option<OptionPresentation>,
    /// Icon of a `toolbar` toggle; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<UIIcon>,
}

/// Persistence boundary of an option. Existing declarations remain workspace-wide.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OptionScope {
    #[default]
    Workspace,
    Instance,
}

impl OptionScope {
    fn is_workspace(&self) -> bool {
        *self == Self::Workspace
    }
}

/// Where a host offers an option besides its option list. Neither placement
/// changes how the option is resolved or emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OptionPresentation {
    /// A toggle button in the tool window's toolbar; boolean options with an
    /// icon only (offline mode, skipping tests).
    Toolbar,
    /// A checklist node at the top of the tool window's tree; multiSelect
    /// options only (build profiles).
    Tree,
}

impl OptionDeclaration {
    pub(crate) fn separator(&self) -> &str {
        self.separator.as_deref().unwrap_or(DEFAULT_SEPARATOR)
    }

    pub(crate) fn is_select(&self) -> bool {
        matches!(
            self.kind,
            OptionKind::SingleSelect | OptionKind::MultiSelect
        )
    }

    /// Whether a single value is acceptable, given the model's values for a
    /// `choicesFromModel` option (`None` while no model is loaded, in which
    /// case model choices cannot be checked and are accepted).
    pub(crate) fn accepts_value(&self, value: &str, model_values: Option<&[String]>) -> bool {
        if !is_option_value(value) {
            return false;
        }
        if self.kind == OptionKind::MultiSelect && value.contains(self.separator()) {
            return false;
        }
        if !self.is_select() || self.allow_custom {
            return true;
        }
        if self.choices.iter().any(|choice| choice.value == value) {
            return true;
        }
        self.choices_from_model
            && model_values.is_none_or(|values| values.iter().any(|known| known == value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OptionKind {
    Boolean,
    SingleSelect,
    MultiSelect,
    Text,
    File,
    Directory,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OptionChoice {
    pub(crate) value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
}

impl BuildSystemDeclaration {
    pub(crate) fn option(&self, id: &str) -> Option<&OptionDeclaration> {
        self.options.iter().find(|option| option.id == id)
    }

    pub(crate) fn task(&self, id: &str) -> Option<&TaskDeclaration> {
        self.tasks.iter().find(|task| task.id == id)
    }
}

// MARK: Validation

impl BuildSystemDeclaration {
    pub(crate) fn validate_options(&mut self, field: &str) -> Validation<BTreeSet<String>> {
        if self.options.len() > MAX_LIST {
            return Err(invalid(format!("{field}.options"), "Too many options."));
        }
        let mut ids = BTreeSet::new();
        for (index, option) in self.options.iter_mut().enumerate() {
            let field = format!("{field}.options[{index}]");
            option.validate(&field)?;
            if !ids.insert(option.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "An option is declared more than once.",
                ));
            }
        }
        Ok(ids)
    }
}

impl BuildSystemDeclaration {
    /// An option's arguments must be emitted at most once per command line:
    /// either all options in one place, or each option in exactly one of the
    /// global and the command-specific lists, and once within each list.
    pub(crate) fn validate_option_emission(&self, field: &str) -> Validation<()> {
        let twice = |list_field: String| {
            template_error(
                list_field,
                "An option's arguments would be emitted twice; reference each option in one place.",
            )
        };
        let globals = &self.invocation.global_arguments;
        if template::repeats_option_arguments(globals) {
            return Err(twice(format!("{field}.invocation.globalArguments")));
        }
        let (global_all, global_named) = template::option_references(globals);
        let mut lists: Vec<(String, &[BuildArgument])> = Vec::new();
        for (index, task) in self.tasks.iter().enumerate() {
            lists.push((format!("{field}.tasks[{index}].arguments"), &task.arguments));
        }
        for (index, run) in self.run.iter().enumerate() {
            lists.push((format!("{field}.run[{index}].arguments"), &run.arguments));
        }
        if let Some(test) = &self.test {
            lists.push((format!("{field}.test.arguments"), &test.arguments));
        }
        if let Some(ModelSource::Command { arguments, .. }) = &self.model {
            lists.push((format!("{field}.model.arguments"), arguments));
        }
        if let Some(DependenciesSource::Command { arguments, .. }) = &self.dependencies {
            lists.push((format!("{field}.dependencies.arguments"), arguments));
        }
        for (list_field, arguments) in lists {
            let (all, named) = template::option_references(arguments);
            if (global_all && (all || !named.is_empty()))
                || (all && !global_named.is_empty())
                || global_named.intersection(&named).next().is_some()
                || template::repeats_option_arguments(arguments)
            {
                return Err(twice(list_field));
            }
        }
        Ok(())
    }
}

impl OptionDeclaration {
    fn validate(&mut self, field: &str) -> Validation<()> {
        if !is_setting_identifier(&self.id) {
            return Err(invalid(
                format!("{field}.id"),
                "Option IDs start with a lowercase letter and use letters and digits.",
            ));
        }
        check_text(
            &self.display_name,
            &format!("{field}.displayName"),
            "display name",
        )?;
        let select = self.is_select();
        if !select && (!self.choices.is_empty() || self.choices_from_model || self.allow_custom) {
            return Err(invalid(
                field,
                "Choices apply to singleSelect and multiSelect options only.",
            ));
        }
        if select && self.choices.is_empty() && !self.choices_from_model && !self.allow_custom {
            return Err(invalid(
                format!("{field}.choices"),
                "A select option needs choices, choicesFromModel, or allowCustom.",
            ));
        }
        if let Some(separator) = &self.separator {
            if self.kind != OptionKind::MultiSelect
                || separator.is_empty()
                || separator.len() > 4
                || separator.chars().any(char::is_control)
            {
                return Err(invalid(
                    format!("{field}.separator"),
                    "Only multiSelect options take a separator of 1-4 printable characters.",
                ));
            }
        }
        if self.choices.len() > MAX_CHOICES {
            return Err(invalid(format!("{field}.choices"), "Too many choices."));
        }
        let mut values = BTreeSet::new();
        for (index, choice) in self.choices.iter().enumerate() {
            let bad_value = !is_option_value(&choice.value)
                || (self.kind == OptionKind::MultiSelect
                    && choice.value.contains(self.separator()));
            if bad_value || !values.insert(choice.value.clone()) {
                return Err(invalid(
                    format!("{field}.choices[{index}].value"),
                    "Choice values are unique, printable, and free of the separator.",
                ));
            }
            if let Some(title) = &choice.title {
                check_text(title, &format!("{field}.choices[{index}].title"), "title")?;
            }
        }
        self.validate_presentation(field)?;
        self.validate_default(field)?;
        if self.arguments.is_empty() && self.environment.is_empty() {
            return Err(invalid(
                field,
                "An option must map to arguments, environment variables, or both.",
            ));
        }
        if self.arguments.len() > 8 || self.environment.len() > 8 {
            return Err(invalid(
                field,
                "An option maps to at most 8 arguments and 8 variables.",
            ));
        }
        let boolean = self.kind == OptionKind::Boolean;
        let allowed = Allowed {
            value: !boolean,
            ..Allowed::none()
        };
        let mut uses_value = false;
        for (index, argument) in self.arguments.iter().enumerate() {
            template::validate_text(argument, &allowed, &format!("{field}.arguments[{index}]"))?;
            uses_value |= argument.contains("${value}");
        }
        for (name, value) in &self.environment {
            if !is_environment_name(name) {
                return Err(invalid(
                    format!("{field}.environment"),
                    "Environment variable names use uppercase letters, digits, and _.",
                ));
            }
            template::validate_text(value, &allowed, &format!("{field}.environment.{name}"))?;
            uses_value |= value.contains("${value}");
        }
        if !boolean && !uses_value {
            return Err(template_error(
                field,
                "The option's value is never passed; use ${value}.",
            ));
        }
        Ok(())
    }

    fn validate_presentation(&self, field: &str) -> Validation<()> {
        if let Some(icon) = &self.icon {
            icon.validate_at(&format!("{field}.icon"))?;
        }
        match self.presentation {
            Some(OptionPresentation::Toolbar)
                if self.kind != OptionKind::Boolean || self.icon.is_none() =>
            {
                Err(invalid(
                    format!("{field}.presentation"),
                    "A toolbar option is a boolean option with an icon.",
                ))
            }
            Some(OptionPresentation::Tree) if self.kind != OptionKind::MultiSelect => Err(invalid(
                format!("{field}.presentation"),
                "A tree option is a multiSelect option.",
            )),
            None if self.icon.is_some() => Err(invalid(
                format!("{field}.icon"),
                "Only toolbar options have an icon.",
            )),
            _ => Ok(()),
        }
    }

    fn validate_default(&self, field: &str) -> Validation<()> {
        let Some(default) = &self.default else {
            return Ok(());
        };
        let field = format!("{field}.default");
        let valid = match (self.kind, default) {
            (OptionKind::Boolean, Value::Bool(_)) => true,
            (OptionKind::MultiSelect, Value::Array(items)) => items.iter().all(|item| {
                item.as_str()
                    .is_some_and(|value| self.accepts_value(value, Some(&[])))
            }),
            (OptionKind::SingleSelect, Value::String(value)) => {
                self.accepts_value(value, Some(&[]))
            }
            (OptionKind::Text, Value::String(value)) => value.is_empty() || is_option_value(value),
            // Paths are the user's: a plugin cannot know one on this machine.
            (OptionKind::File | OptionKind::Directory, Value::String(value)) => value.is_empty(),
            _ => false,
        };
        if !valid {
            return Err(invalid(
                field,
                "The default does not fit the option's kind and choices; file and directory options have no default.",
            ));
        }
        Ok(())
    }
}
