//! Run and test plans: expanding a provider's run and test templates for a project.

use super::*;

/// A run request for one workspace-relative file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPlanRequest {
    relative_file_path: String,
    #[serde(default)]
    arguments: Vec<String>,
}

/// A test discovery request over the workspace's relative file list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestDiscoveryRequest {
    relative_project_file_paths: Vec<String>,
}

/// A test launch request for a workspace, file, or single test case.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestPlanRequest {
    scope: TestScope,
    relative_project_file_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum TestScope {
    Workspace,
    File {
        #[serde(rename = "relativeFilePath")]
        relative_file_path: String,
    },
    TestCase {
        identifier: String,
        #[serde(default, rename = "relativeFilePath")]
        relative_file_path: Option<String>,
    },
}

/// Values substituted into one command template.
struct ExpansionValues<'a> {
    file: Option<&'a str>,
    test_name: Option<&'a str>,
    user_arguments: &'a [String],
}

pub(crate) fn run_plan(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &RunPlanRequest,
) -> Validation<Value> {
    let run = manifest
        .execution_provider(language_id)
        .and_then(|provider| provider.run.as_ref())
        .ok_or_else(|| unavailable(language_id, "run"))?;
    let path = checked_path(&request.relative_file_path)?;
    let arguments = expand(
        &run.arguments,
        &ExpansionValues {
            file: Some(&path),
            test_name: None,
            user_arguments: &request.arguments,
        },
    );
    Ok(json!({
        "executable": run.executable,
        "arguments": arguments,
        "workingDirectory": "."
    }))
}

pub(crate) fn discover_tests(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &TestDiscoveryRequest,
) -> Validation<Value> {
    let test = test_template(manifest, language_id)?;
    let paths = checked_paths(&request.relative_project_file_paths)?;
    if !supports_project(test, &paths) {
        return Ok(json!([]));
    }
    let mut files: Vec<&String> = paths
        .iter()
        .filter(|path| {
            let name = file_name(path).to_lowercase();
            test.test_file_patterns
                .iter()
                .any(|pattern| matches_pattern(pattern, &name))
        })
        .collect();
    files.sort();
    files.dedup();
    let mut items = vec![json!({
        "id": format!("{language_id}:workspace"),
        "label": test.workspace_label,
        "kind": "workspace"
    })];
    items.extend(files.into_iter().map(|path| {
        json!({
            "id": format!("{language_id}:file:{path}"),
            "label": path,
            "kind": "file",
            "relativeFilePath": path
        })
    }));
    Ok(Value::Array(items))
}

pub(crate) fn test_plan(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &TestPlanRequest,
) -> Validation<Value> {
    let test = test_template(manifest, language_id)?;
    let paths = checked_paths(&request.relative_project_file_paths)?;
    if !supports_project(test, &paths) {
        return Err(PluginRejection::new(
            "plugin.test.unsupportedProject",
            format!("The workspace is not a supported {language_id} test project."),
        ));
    }
    let (label, arguments) = match &request.scope {
        TestScope::Workspace => (
            test.workspace_label.clone(),
            expand(
                &test.workspace_arguments,
                &ExpansionValues {
                    file: None,
                    test_name: None,
                    user_arguments: &[],
                },
            ),
        ),
        TestScope::File { relative_file_path } => {
            let path = checked_path(relative_file_path)?;
            let arguments = expand(
                &test.file_arguments,
                &ExpansionValues {
                    file: Some(&path),
                    test_name: None,
                    user_arguments: &[],
                },
            );
            (file_name(&path).to_string(), arguments)
        }
        TestScope::TestCase {
            identifier,
            relative_file_path,
        } => {
            let name = identifier.trim();
            if name.is_empty() || name.contains('\n') || name.contains('\r') {
                return Err(PluginRejection::new(
                    "plugin.test.invalidIdentifier",
                    "The selected test identifier is invalid.",
                ));
            }
            let path = relative_file_path
                .as_deref()
                .map(checked_path)
                .transpose()?;
            let arguments = expand(
                &test.test_case_arguments,
                &ExpansionValues {
                    file: path.as_deref(),
                    test_name: Some(name),
                    user_arguments: &[],
                },
            );
            (name.to_string(), arguments)
        }
    };
    Ok(json!({
        "label": label,
        "frameworkID": test.framework_id,
        "executable": test.executable,
        "arguments": arguments,
        "workingDirectory": "."
    }))
}

fn test_template<'a>(
    manifest: &'a DeclarativePluginManifest,
    language_id: &str,
) -> Validation<&'a TestTemplate> {
    manifest
        .execution_provider(language_id)
        .and_then(|provider| provider.test.as_ref())
        .ok_or_else(|| unavailable(language_id, "test"))
}

fn supports_project(test: &TestTemplate, paths: &[String]) -> bool {
    paths.iter().any(|path| {
        test.project_markers
            .contains(&file_name(path).to_lowercase())
    })
}

fn expand(arguments: &[ArgumentTemplate], values: &ExpansionValues<'_>) -> Vec<String> {
    let mut result = Vec::new();
    for argument in arguments {
        match argument.condition() {
            Some(ArgumentCondition::FileSelected) if values.file.is_none() => continue,
            Some(ArgumentCondition::NoFileSelected) if values.file.is_some() => continue,
            _ => {}
        }
        // Validation guarantees the template parses and that every variable
        // it uses has a value in this context.
        let parts = parse_template(argument.value()).unwrap_or_default();
        if parts == [TemplatePart::Variable(TemplateVariable::UserArguments)] {
            result.extend(values.user_arguments.iter().cloned());
            continue;
        }
        let mut element = String::new();
        for part in parts {
            match part {
                TemplatePart::Text(text) => element.push_str(&text),
                TemplatePart::Variable(TemplateVariable::File) => {
                    element.push_str(&command_path(values.file.unwrap_or_default()))
                }
                TemplatePart::Variable(TemplateVariable::FileDirectory) => {
                    element.push_str(&directory_argument(values.file.unwrap_or_default()))
                }
                TemplatePart::Variable(TemplateVariable::TestName) => {
                    element.push_str(values.test_name.unwrap_or_default())
                }
                TemplatePart::Variable(TemplateVariable::TestNameRegex) => {
                    element.push_str(&regex_literal(values.test_name.unwrap_or_default()))
                }
                TemplatePart::Variable(TemplateVariable::UserArguments) => {}
            }
        }
        result.push(element);
    }
    result
}

/// A file name beginning with `-` would be parsed as an option by most
/// tools, so it is passed as an explicit relative path.
fn command_path(path: &str) -> String {
    if path.starts_with('-') {
        format!("./{path}")
    } else {
        path.to_string()
    }
}

/// The selected file's directory as a `./`-relative argument; `.` for a file
/// at the workspace root.
fn directory_argument(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((directory, _)) if !directory.is_empty() => format!("./{directory}"),
        _ => ".".to_string(),
    }
}

fn matches_pattern(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || !name[first.len()..].ends_with(last) {
        return false;
    }
    let mut remainder = &name[first.len()..name.len() - last.len()];
    for piece in &pieces[1..pieces.len() - 1] {
        match remainder.find(piece) {
            Some(index) => remainder = &remainder[index + piece.len()..],
            None => return false,
        }
    }
    true
}

fn checked_path(value: &str) -> Validation<String> {
    // Paths are not trimmed: surrounding spaces are part of a real file name.
    if is_safe_relative_path(value) {
        Ok(value.to_string())
    } else {
        Err(PluginRejection::new(
            "plugin.path.invalid",
            "A path must stay inside the current workspace.",
        ))
    }
}

fn checked_paths(values: &[String]) -> Validation<Vec<String>> {
    values.iter().map(|value| checked_path(value)).collect()
}

fn unavailable(language_id: &str, operation: &str) -> PluginRejection {
    PluginRejection::new(
        "plugin.execution.unavailable",
        format!("The plugin does not provide {operation} support for {language_id}."),
    )
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
