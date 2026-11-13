//! Shared field checks used by every declaration section.

use super::*;

/// Activation event the host raises before using a build system.
pub fn activation_event(build_system: &BuildSystemDeclaration) -> String {
    format!("onBuildSystem:{}", build_system.id)
}

/// Install-review entries: everything that decides which process runs with
/// which arguments. Display text, detection, and output decoding are left
/// out; changing them does not run anything new. A provider process of its
/// own is reviewed like a language server, with the runtimes it needs.
pub fn programs(build_system: &BuildSystemDeclaration) -> Vec<Value> {
    let mut programs = vec![program(build_system)];
    if let Some(ProviderDeclaration::Process(own)) = &build_system.provider {
        let mut provider = own.process.program(ServerRole::BuildProvider);
        provider["buildSystemID"] = json!(build_system.id);
        provider["executesProjectCode"] = json!(own.executes_project_code);
        provider["mayUseNetwork"] = json!(own.may_use_network);
        programs.push(provider);
        programs.extend(own.process.runtime_programs());
    }
    programs
}

fn program(build_system: &BuildSystemDeclaration) -> Value {
    let options: Vec<Value> = build_system
        .options
        .iter()
        .map(|option| {
            json!({
                "id": option.id,
                "kind": option.kind,
                "default": option.default,
                "choices": option.choices.iter().map(|choice| &choice.value).collect::<Vec<_>>(),
                "choicesFromModel": option.choices_from_model,
                "allowCustom": option.allow_custom,
                "separator": option.separator(),
                "arguments": option.arguments,
                "environment": option.environment,
            })
        })
        .collect();
    let tasks: Vec<Value> = build_system
        .tasks
        .iter()
        .map(|task| {
            let mut review = json!({ "id": task.id, "arguments": task.arguments });
            if task.source == TaskSource::Provider {
                review["source"] = json!("provider");
            }
            // Automatic preparation changes when a task can execute, not merely
            // its display. Keep that behavior in the existing install review.
            if let Some(preparation) = &task.preparation {
                review["preparation"] = json!(preparation);
            }
            review
        })
        .collect();
    let model = match &build_system.model {
        Some(ModelSource::Command { arguments, .. }) => json!({ "arguments": arguments }),
        _ => Value::Null,
    };
    let dependencies = match &build_system.dependencies {
        Some(DependenciesSource::Command { arguments, .. }) => json!({ "arguments": arguments }),
        _ => Value::Null,
    };
    let mut program = json!({
        "kind": "buildSystem",
        "buildSystemID": build_system.id,
        "invocation": build_system.invocation,
        "options": options,
        "tasks": tasks,
        "run": build_system.run,
        "test": build_system.test.as_ref().map(|test| {
            let mut review = json!({ "arguments": test.arguments });
            if test.source == TaskSource::Provider {
                review["source"] = json!("provider");
            }
            if test.discovery { review["discovery"] = json!(true); }
            review
        }),
        "model": model,
        "dependencies": dependencies,
    });
    // Build requests reaching a language server change what that process
    // is asked to do, so the shared form is part of this entry.
    if let Some(ProviderDeclaration::LanguageServer(shared)) = &build_system.provider {
        program["provider"] = json!(shared);
    }
    program
}

/// A build-system or run ID: also a run-configuration provider segment, so
/// it starts with a letter.
pub(crate) fn is_entry_identifier(value: &str) -> bool {
    is_identifier(value)
        && value.len() <= MAX_IDENTIFIER
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
}

pub(crate) fn is_tag(value: &str) -> bool {
    is_identifier(value) && value.len() <= MAX_IDENTIFIER
}

pub(crate) fn is_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase() || first == '_')
        && value.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
}

/// A file name with no directory part.
pub(crate) fn is_file_name(value: &str) -> bool {
    is_safe_relative_path(value) && !value.contains('/') && value != "."
}

/// An option value: no control characters, bounded.
pub(crate) fn is_option_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_OPTION_VALUE && !value.chars().any(char::is_control)
}

pub(crate) fn check_text(value: &str, field: &str, what: &str) -> Validation<()> {
    if value.trim().is_empty() || value.len() > MAX_TEXT || value.chars().any(char::is_control) {
        return Err(invalid(
            field,
            format!("The {what} must be 1-{MAX_TEXT} printable characters."),
        ));
    }
    Ok(())
}

/// Trimmed headings of a task category, or `None` when the path is too deep
/// or a heading is empty, too long, or not printable.
pub(crate) fn category_labels(labels: &[String]) -> Option<Vec<String>> {
    if labels.len() > MAX_CATEGORY_DEPTH {
        return None;
    }
    labels
        .iter()
        .map(|label| {
            let label = label.trim();
            (!label.is_empty() && label.len() <= MAX_TEXT && !label.chars().any(char::is_control))
                .then(|| label.to_string())
        })
        .collect()
}

pub(crate) fn check_paths(values: &mut Vec<String>, field: &str, glob: bool) -> Validation<()> {
    if values.len() > MAX_LIST {
        return Err(invalid(field, "The list is too long."));
    }
    for (index, value) in values.iter().enumerate() {
        let safe = is_safe_relative_path(value)
            && (glob || !value.contains('*'))
            && !value
                .split('/')
                .any(|segment| segment.is_empty() || segment == ".");
        if !safe {
            return Err(PluginRejection::at(
                "plugin.path.invalid",
                format!("{field}[{index}]"),
                "Paths must be relative, use /, and stay inside the project.",
            ));
        }
    }
    *values = sorted_unique(std::mem::take(values));
    Ok(())
}

pub(crate) fn check_regex(value: &str, field: &str, groups: usize) -> Validation<Regex> {
    let compiled = (value.len() <= MAX_REGEX)
        .then(|| Regex::new(value).ok())
        .flatten()
        .ok_or_else(|| invalid(field, "The regular expression is invalid or too long."))?;
    if compiled.captures_len() <= groups {
        return Err(invalid(
            field,
            format!("The regular expression needs at least {groups} capture group(s)."),
        ));
    }
    Ok(compiled)
}

pub(crate) fn check_argument_count(arguments: &[BuildArgument], field: &str) -> Validation<()> {
    if arguments.len() > MAX_ARGUMENTS {
        return Err(invalid(field, "The argument list is too long."));
    }
    Ok(())
}

pub(crate) fn availability(scopes: &[Scope], scope: Scope) -> Availability {
    let workspace = scopes.contains(&Scope::Workspace);
    match scope {
        Scope::Project if scopes.iter().all(|value| *value == Scope::Workspace) => {
            Availability::Never
        }
        Scope::Project if workspace => Availability::Maybe,
        Scope::Project => Availability::Always,
        Scope::Target if !scopes.contains(&Scope::Target) => Availability::Never,
        Scope::Target if scopes.len() == 1 => Availability::Always,
        Scope::Target => Availability::Maybe,
        Scope::Workspace => Availability::Always,
    }
}

pub(crate) fn check_scopes(
    scopes: &mut Vec<Scope>,
    field: &str,
    allowed: &[Scope],
) -> Validation<()> {
    *scopes = std::mem::take(scopes)
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if scopes.is_empty() || scopes.iter().any(|scope| !allowed.contains(scope)) {
        return Err(invalid(
            field,
            format!("Scopes must be a non-empty subset of {allowed:?}."),
        ));
    }
    Ok(())
}
