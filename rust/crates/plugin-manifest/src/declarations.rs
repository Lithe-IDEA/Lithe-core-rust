//! Normalization of language, language-server and execution-provider declarations,
//! with their executable and argument checks.

use super::*;

impl LanguageDeclaration {
    pub(crate) fn normalize(&mut self, index: usize) -> Validation<()> {
        let field = format!("languages[{index}]");
        if let Some(grammar) = &self.grammar {
            grammar.validate(&format!("{field}.grammar"))?;
        }
        if let Some(configuration) = &self.configuration {
            configuration.validate(&format!("{field}.configuration"))?;
        }
        let id = self.id.trim().to_lowercase();
        if id.is_empty()
            || id != self.id
            || !id.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || "+-_".contains(character)
            })
        {
            return Err(invalid(
                format!("{field}.id"),
                "The language ID must be lowercase.",
            ));
        }
        if self.display_name.trim().is_empty() {
            return Err(invalid(
                format!("{field}.displayName"),
                "The language display name is empty.",
            ));
        }
        self.file_extensions = normalized_extensions(&self.file_extensions);
        self.file_names = normalized_names(&self.file_names);
        self.project_file_names = normalized_names(&self.project_file_names);
        if self.file_extensions.is_empty()
            && self.file_names.is_empty()
            && self.project_file_names.is_empty()
        {
            return Err(invalid(
                field,
                "The language declares no file extensions or file names.",
            ));
        }
        if self
            .file_extensions
            .iter()
            .chain(&self.file_names)
            .chain(&self.project_file_names)
            .any(|name| name.contains('/') || name.contains('\\'))
        {
            return Err(invalid(
                field,
                "Language file names must not contain path separators.",
            ));
        }
        if let Some(dependencies) = &mut self.dependencies {
            dependencies.management_file_names =
                normalized_names(&dependencies.management_file_names);
            dependencies.project_dependency_paths =
                sorted_unique(dependencies.project_dependency_paths.clone());
            dependencies.project_binary_paths =
                sorted_unique(dependencies.project_binary_paths.clone());
            if dependencies
                .management_file_names
                .iter()
                .chain(&dependencies.project_dependency_paths)
                .chain(&dependencies.project_binary_paths)
                .any(|path| !is_safe_relative_path(path))
            {
                return Err(invalid(
                    format!("{field}.dependencies"),
                    "Dependency paths must stay inside the project.",
                ));
            }
        }
        Ok(())
    }
}

impl LanguageServerDeclaration {
    pub(crate) fn validate(
        &self,
        field: &str,
        declared_settings: &DeclaredSettings,
        runtime_tools: RuntimeTools<'_>,
    ) -> Validation<()> {
        self.process.validate(
            field,
            declared_settings,
            ServerRole::LanguageServer,
            runtime_tools,
        )?;
        self.session.validate(field)?;
        if self.process.is_backend_launch() && self.host_integration.is_some() {
            return Err(invalid(
                format!("{field}.hostIntegration"),
                "A language server started through the plugin backend has no host integration.",
            ));
        }
        if let Some(configuration) = &self.workspace_configuration {
            let field = format!("{field}.workspaceConfiguration");
            if !configuration.is_object() {
                return Err(invalid(field, "workspaceConfiguration is a JSON object."));
            }
            validate_launch_value(
                configuration,
                &declared_settings.for_configuration(),
                &field,
            )?;
            // The host re-expands it on every change without a launch, so
            // only setting values may vary.
            if mentions_launch_directory(configuration) {
                return Err(invalid_template(
                    field,
                    "workspaceConfiguration may only use ${setting:<id>} variables.",
                ));
            }
        }
        if let Some(identifier) = &self.language_identifier {
            if identifier.trim().is_empty() {
                return Err(invalid(
                    format!("{field}.languageIdentifier"),
                    "The LSP language identifier is empty.",
                ));
            }
        }
        if let Some(integration) = &self.host_integration {
            let field = format!("{field}.hostIntegration.id");
            let message = match integration.get("id").and_then(Value::as_str) {
                Some(id) => format!("The host has no integration named {id}."),
                None => "The host has no language-server integration.".to_owned(),
            };
            return Err(PluginRejection::at(
                "plugin.integration.unsupported",
                field,
                message,
            ));
        }
        Ok(())
    }
}

impl ExecutionProviderDeclaration {
    pub(crate) fn normalize(&mut self, field: &str) -> Validation<()> {
        if self.run.is_none() && self.test.is_none() {
            return Err(invalid(
                field.to_string(),
                "The execution provider declares neither run nor test.",
            ));
        }
        if let Some(run) = &self.run {
            validate_executable(&run.executable, &format!("{field}.run.executable"))?;
            validate_arguments(
                &run.arguments,
                TemplateContext::Run,
                &format!("{field}.run.arguments"),
            )?;
        }
        if let Some(test) = &mut self.test {
            let test_field = format!("{field}.test");
            if !is_identifier(&test.framework_id) {
                return Err(invalid(
                    format!("{test_field}.frameworkID"),
                    "The test framework ID is invalid.",
                ));
            }
            if test.workspace_label.trim().is_empty() {
                return Err(invalid(
                    format!("{test_field}.workspaceLabel"),
                    "The workspace test label is empty.",
                ));
            }
            test.project_markers = normalized_names(&test.project_markers);
            test.test_file_patterns = normalized_names(&test.test_file_patterns);
            if test.project_markers.is_empty()
                || test.project_markers.iter().any(|name| name.contains('/'))
            {
                return Err(invalid(
                    format!("{test_field}.projectMarkers"),
                    "Project markers must be file names.",
                ));
            }
            if test.test_file_patterns.is_empty()
                || test.test_file_patterns.iter().any(|pattern| {
                    pattern.contains('/') || pattern.contains('?') || pattern.contains('[')
                })
            {
                return Err(invalid(
                    format!("{test_field}.testFilePatterns"),
                    "Test file patterns must be file names using only *.",
                ));
            }
            validate_executable(&test.executable, &format!("{test_field}.executable"))?;
            validate_arguments(
                &test.workspace_arguments,
                TemplateContext::TestWorkspace,
                &format!("{test_field}.workspaceArguments"),
            )?;
            validate_arguments(
                &test.file_arguments,
                TemplateContext::TestFile,
                &format!("{test_field}.fileArguments"),
            )?;
            validate_arguments(
                &test.test_case_arguments,
                TemplateContext::TestCase,
                &format!("{test_field}.testCaseArguments"),
            )?;
            let names_test = test.test_case_arguments.iter().any(|argument| {
                parse_template(argument.value()).is_some_and(|parts| {
                    parts.iter().any(|part| {
                        matches!(
                            part,
                            TemplatePart::Variable(
                                TemplateVariable::TestName | TemplateVariable::TestNameRegex
                            )
                        )
                    })
                })
            });
            if !names_test {
                return Err(invalid_template(
                    format!("{test_field}.testCaseArguments"),
                    "The test case command must reference the test name.",
                ));
            }
        }
        Ok(())
    }
}

fn validate_executable(executable: &ExecutableDeclaration, field: &str) -> Validation<()> {
    match (&executable.command, &executable.toolchain) {
        (Some(command), None) if is_bare_command(command) => Ok(()),
        (None, Some(toolchain)) if is_identifier(toolchain) => Ok(()),
        _ => Err(invalid(
            field.to_string(),
            "The executable must be exactly one bare command or toolchain ID.",
        )),
    }
}

fn validate_arguments(
    arguments: &[ArgumentTemplate],
    context: TemplateContext,
    field: &str,
) -> Validation<()> {
    for (index, argument) in arguments.iter().enumerate() {
        let element = format!("{field}[{index}]");
        let value = argument.value();
        if value.contains('\0') {
            return Err(invalid_template(element, "Arguments must not contain NUL."));
        }
        let Some(parts) = parse_template(value) else {
            return Err(invalid_template(
                element,
                "The argument uses an unknown or unterminated template variable.",
            ));
        };
        let condition = argument.condition();
        if condition.is_some() && context != TemplateContext::TestCase {
            return Err(invalid_template(
                element,
                "Conditional arguments are only allowed in test case commands.",
            ));
        }
        for part in &parts {
            let TemplatePart::Variable(variable) = part else {
                continue;
            };
            if !context.allows(*variable) {
                return Err(invalid_template(
                    element,
                    "The argument uses a variable that is not available in this command.",
                ));
            }
            if *variable == TemplateVariable::UserArguments && parts.len() != 1 {
                return Err(invalid_template(
                    element,
                    "${userArguments} must be a whole argument.",
                ));
            }
            // A test case may run without a selected file, so file variables
            // must be guarded instead of expanding to an empty argument.
            if context == TemplateContext::TestCase
                && variable.needs_file()
                && condition != Some(ArgumentCondition::FileSelected)
            {
                return Err(invalid_template(
                    element,
                    "File variables in a test case command require \"when\": \"fileSelected\".",
                ));
            }
        }
    }
    Ok(())
}

// MARK: Plans

/// Whether any string in a JSON value names a variable other than a setting.
fn mentions_launch_directory(value: &Value) -> bool {
    match value {
        Value::String(text) => parse_template_with(text, LaunchVariable::parse)
            .unwrap_or_default()
            .iter()
            .any(|part| {
                matches!(part, TemplatePart::Variable(variable) if !matches!(variable, LaunchVariable::Setting(_)))
            }),
        Value::Array(items) => items.iter().any(mentions_launch_directory),
        Value::Object(members) => members.values().any(mentions_launch_directory),
        _ => false,
    }
}

/// Trims and lowercases complete file names and markers. A leading `.` is
/// part of the name (`.toolrc`, `.env.example`) and is kept.
fn normalized_names(values: &[String]) -> Vec<String> {
    sorted_unique(
        values
            .iter()
            .filter_map(|value| {
                let name = value.trim().to_lowercase();
                (!name.is_empty()).then_some(name)
            })
            .collect(),
    )
}

/// Normalizes file extensions, where a leading `.` is optional notation:
/// `.php` and `php` both declare the `php` extension.
fn normalized_extensions(values: &[String]) -> Vec<String> {
    let names = normalized_names(values);
    sorted_unique(
        names
            .iter()
            .filter_map(|name| {
                let extension = name.strip_prefix('.').unwrap_or(name);
                (!extension.is_empty()).then(|| extension.to_string())
            })
            .collect(),
    )
}

// MARK: Command entry points
