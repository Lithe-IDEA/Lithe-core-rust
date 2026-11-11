//! Plans Java launches whose command line exceeds the operating system limit.
//!
//! Oversized path-list options (`-cp`, `-classpath`, `--class-path`, `-p`,
//! `--module-path`) move into a JDK argument file. The JVM options before the
//! application target and every program argument stay on the command line.

use crate::protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};

/// Windows refuses a CreateProcess command line of 32,767 UTF-16 code units.
/// The 2048-unit headroom covers the terminating null and host quoting.
const DEFAULT_COMMAND_LINE_LIMIT: usize = 32767 - 2048;

/// The JDK reads `@argfile` references from JDK 9 onward.
const ARGUMENT_FILE_MIN_FEATURE_VERSION: u32 = 9;

/// Options whose value is a path list; they carry nearly all of an oversized command line.
const PATH_OPTIONS: [&str; 5] = ["-cp", "-classpath", "--class-path", "-p", "--module-path"];

/// Launcher options whose following argument is a value, not the application target.
const VALUE_OPTIONS: [&str; 7] = [
    "--add-modules",
    "--add-exports",
    "--add-opens",
    "--add-reads",
    "--patch-module",
    "--limit-modules",
    "--upgrade-module-path",
];

/// A Java launch request. The executable is the launcher path; the JDK feature version
/// is either explicit or derived from host-read `release` contents, nearest first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchCommandRequest {
    /// Launcher path as the host will spawn it.
    pub executable: String,
    /// Complete argument list, excluding the executable.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Known JDK feature version. Takes precedence over `java_release_contents`.
    pub java_feature_version: Option<u32>,
    /// Host-read `release` file contents, ordered nearest first. Core reads the first
    /// entry that contains `JAVA_VERSION`; an unrecognized value is unknown.
    pub java_release_contents: Option<Vec<String>>,
    /// Command-line byte budget. Defaults to the Windows-safe budget.
    pub command_line_limit: Option<usize>,
}

/// The launch the host spawns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchCommandPlan {
    /// Arguments to spawn. When `argument_file` is set, the element at
    /// `argument_file.argument_index` is an empty placeholder that the host replaces
    /// with `@<argument file path>`.
    pub arguments: Vec<String>,
    /// Argument-file text to write, or `None` when the launch is unchanged.
    pub argument_file: Option<LaunchArgumentFile>,
}

/// Argument-file text and the index of the placeholder that references it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchArgumentFile {
    /// Unicode text in the JDK argument-file quoting format.
    pub text: String,
    /// Index into `LaunchCommandPlan::arguments` of the placeholder.
    pub argument_index: usize,
}

/// Plans a launch. Only a Java launcher of feature version 9 or later whose command line
/// exceeds the limit is changed; every other launch is returned unchanged.
pub fn plan_launch_command(request: &LaunchCommandRequest) -> Result<LaunchCommandPlan, CoreError> {
    if request.executable.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Launch command has no executable.",
        ));
    }
    let unchanged = || LaunchCommandPlan {
        arguments: request.arguments.clone(),
        argument_file: None,
    };
    let limit = request
        .command_line_limit
        .unwrap_or(DEFAULT_COMMAND_LINE_LIMIT);
    if !is_java_launcher(&request.executable)
        || command_line_size(&request.executable, &request.arguments) <= limit
    {
        return Ok(unchanged());
    }
    let version = request.java_feature_version.or_else(|| {
        request
            .java_release_contents
            .as_deref()
            .and_then(feature_version_from_nearest)
    });
    match version {
        Some(version) if version >= ARGUMENT_FILE_MIN_FEATURE_VERSION => {}
        _ => return Ok(unchanged()),
    }

    let mut arguments = Vec::with_capacity(request.arguments.len());
    let mut body = String::new();
    let mut placeholder = None;
    let mut index = 0;
    while index < request.arguments.len() {
        let argument = request.arguments[index].as_str();
        // The application target (main class, -jar, -m/--module) ends the JVM options;
        // it and everything after it are program arguments and stay verbatim.
        if is_application_target(argument) || !argument.starts_with('-') {
            arguments.extend(request.arguments[index..].iter().cloned());
            break;
        }
        if PATH_OPTIONS.contains(&argument) && index + 1 < request.arguments.len() {
            placeholder.get_or_insert_with(|| {
                arguments.push(String::new());
                arguments.len() - 1
            });
            push_argument_file_pair(&mut body, argument, &request.arguments[index + 1]);
            index += 2;
            continue;
        }
        // The --class-path=value spelling is one argument and moves the same way.
        if let Some((name, value)) = argument.split_once('=') {
            if PATH_OPTIONS.contains(&name) {
                placeholder.get_or_insert_with(|| {
                    arguments.push(String::new());
                    arguments.len() - 1
                });
                push_argument_file_pair(&mut body, name, value);
                index += 1;
                continue;
            }
        }
        if VALUE_OPTIONS.contains(&argument) && index + 1 < request.arguments.len() {
            arguments.push(argument.to_owned());
            arguments.push(request.arguments[index + 1].clone());
            index += 2;
            continue;
        }
        arguments.push(argument.to_owned());
        index += 1;
    }

    match placeholder {
        Some(argument_index) => Ok(LaunchCommandPlan {
            arguments,
            argument_file: Some(LaunchArgumentFile {
                text: body,
                argument_index,
            }),
        }),
        None => Ok(unchanged()),
    }
}

/// Reads the JDK feature version from the text of one `release` file. Legacy `1.x`
/// versions report the second number, so `1.8.0_392` is 8 and `17.0.2` is 17.
pub fn java_feature_version_from_release(contents: &str) -> Option<u32> {
    parse_feature_version(release_java_version(contents)?)
}

/// Returns the raw value of the first `JAVA_VERSION=` line, without surrounding spaces.
fn release_java_version(contents: &str) -> Option<&str> {
    contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("JAVA_VERSION="))
}

/// Uses the nearest release file that names a `JAVA_VERSION`. A farther file never
/// answers for an unrecognized nearer one.
fn feature_version_from_nearest(contents: &[String]) -> Option<u32> {
    let nearest = contents
        .iter()
        .find(|text| release_java_version(text).is_some())?;
    java_feature_version_from_release(nearest)
}

fn parse_feature_version(value: &str) -> Option<u32> {
    let parts: Vec<&str> = value
        .trim_matches('"')
        .split(|c: char| matches!(c, '.' | '_' | '-' | '+'))
        .filter(|part| !part.is_empty())
        .collect();
    let first: u32 = parts.first()?.parse().ok()?;
    if first == 1 && parts.len() > 1 {
        parts[1].parse().ok()
    } else {
        Some(first)
    }
}

fn is_application_target(argument: &str) -> bool {
    matches!(argument, "-jar" | "-m" | "--module") || argument.starts_with("--module=")
}

/// Accepts the launcher names of every platform, with or without the `.exe` suffix.
fn is_java_launcher(executable: &str) -> bool {
    let name = executable
        .rsplit(|c: char| c == '/' || c == '\\')
        .next()
        .unwrap_or(executable)
        .to_ascii_lowercase();
    matches!(name.as_str(), "java" | "java.exe" | "javaw" | "javaw.exe")
}

/// Estimates the command line: each argument carries a quote and a separator.
fn command_line_size(executable: &str, arguments: &[String]) -> usize {
    arguments
        .iter()
        .fold(executable.len() + 3, |size, argument| {
            size + argument.len() + 3
        })
}

fn push_argument_file_pair(body: &mut String, option: &str, value: &str) {
    push_argument_file_token(body, option);
    push_argument_file_token(body, value);
}

/// Quotes one token the way the JDK argument-file reader expects: double quotes, with
/// backslashes and double quotes escaped.
fn push_argument_file_token(body: &mut String, token: &str) {
    body.push('"');
    for character in token.chars() {
        if character == '\\' || character == '"' {
            body.push('\\');
        }
        body.push(character);
    }
    body.push_str("\"\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(executable: &str, arguments: &[&str]) -> LaunchCommandRequest {
        LaunchCommandRequest {
            executable: executable.to_owned(),
            arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
            java_feature_version: Some(17),
            java_release_contents: None,
            command_line_limit: Some(40),
        }
    }

    fn unchanged(request: &LaunchCommandRequest) -> LaunchCommandPlan {
        LaunchCommandPlan {
            arguments: request.arguments.clone(),
            argument_file: None,
        }
    }

    #[test]
    fn launch_plan_keeps_a_command_line_within_the_limit() {
        let mut request = request("/jdk/bin/java", &["-cp", "a.jar", "Main"]);
        request.command_line_limit = None;
        assert_eq!(plan_launch_command(&request).unwrap(), unchanged(&request));
    }

    #[test]
    fn launch_plan_moves_an_oversized_classpath_into_an_argument_file() {
        let request = request(
            "/jdk/bin/java",
            &[
                "-Xmx1g",
                "-cp",
                "lib/one.jar:lib/two.jar",
                "com.example.Main",
                "--flag",
            ],
        );
        let plan = plan_launch_command(&request).unwrap();
        assert_eq!(plan.arguments, ["-Xmx1g", "", "com.example.Main", "--flag"]);
        assert_eq!(
            plan.argument_file,
            Some(LaunchArgumentFile {
                text: "\"-cp\"\n\"lib/one.jar:lib/two.jar\"\n".to_owned(),
                argument_index: 1,
            })
        );
    }

    #[test]
    fn launch_plan_escapes_spaces_quotes_and_backslashes_in_values() {
        let request = request(
            "C:\\jdk\\bin\\java.exe",
            &["-classpath", "C:\\my libs\\\"x\".jar", "Main"],
        );
        let plan = plan_launch_command(&request).unwrap();
        let file = plan.argument_file.unwrap();
        assert_eq!(
            file.text,
            "\"-classpath\"\n\"C:\\\\my libs\\\\\\\"x\\\".jar\"\n"
        );
    }

    #[test]
    fn launch_plan_moves_module_path_in_both_spellings() {
        let request = request(
            "/jdk/bin/java",
            &["-p", "mods/a", "--module-path=mods/b", "com.example.Main"],
        );
        let plan = plan_launch_command(&request).unwrap();
        assert_eq!(plan.arguments, ["", "com.example.Main"]);
        assert_eq!(
            plan.argument_file.unwrap().text,
            "\"-p\"\n\"mods/a\"\n\"--module-path\"\n\"mods/b\"\n"
        );
    }

    #[test]
    fn launch_plan_leaves_java_older_than_nine_unchanged() {
        let mut request = request("/jdk/bin/java", &["-cp", "a.jar", "Main"]);
        request.java_feature_version = Some(8);
        assert_eq!(plan_launch_command(&request).unwrap(), unchanged(&request));
    }

    #[test]
    fn launch_plan_leaves_an_unknown_jdk_version_unchanged() {
        let mut request = request("/jdk/bin/java", &["-cp", "a.jar", "Main"]);
        request.java_feature_version = None;
        request.java_release_contents = None;
        assert_eq!(plan_launch_command(&request).unwrap(), unchanged(&request));
    }

    #[test]
    fn launch_plan_leaves_non_java_launches_unchanged() {
        let request = request("/usr/bin/node", &["-cp", "a.jar", "Main"]);
        assert_eq!(plan_launch_command(&request).unwrap(), unchanged(&request));
    }

    #[test]
    fn launch_plan_preserves_program_arguments_after_the_main_class() {
        let request = request(
            "/jdk/bin/java",
            &["-cp", "a.jar", "com.example.Main", "-cp", "user-value"],
        );
        let plan = plan_launch_command(&request).unwrap();
        assert_eq!(
            plan.arguments,
            ["", "com.example.Main", "-cp", "user-value"]
        );
        assert_eq!(plan.argument_file.unwrap().text, "\"-cp\"\n\"a.jar\"\n");
    }

    #[test]
    fn launch_plan_stops_at_jar_and_module_targets() {
        let jar = request("/jdk/bin/java", &["-jar", "app.jar", "-cp", "x"]);
        assert_eq!(plan_launch_command(&jar).unwrap(), unchanged(&jar));

        let module = request(
            "/jdk/bin/java",
            &["--module", "com.example/com.example.Main", "-p", "mods"],
        );
        assert_eq!(plan_launch_command(&module).unwrap(), unchanged(&module));
    }

    #[test]
    fn launch_plan_keeps_value_options_intact() {
        let request = request(
            "/jdk/bin/java",
            &["--add-modules", "-cp", "-cp", "a.jar", "Main"],
        );
        let plan = plan_launch_command(&request).unwrap();
        assert_eq!(plan.arguments, ["--add-modules", "-cp", "", "Main"]);
    }

    #[test]
    fn launch_plan_reads_the_release_version_forms() {
        assert_eq!(
            java_feature_version_from_release("JAVA_VERSION=\"1.8.0_392\"\n"),
            Some(8)
        );
        assert_eq!(
            java_feature_version_from_release("IMPLEMENTOR=\"x\"\nJAVA_VERSION=\"17.0.2\"\n"),
            Some(17)
        );
        assert_eq!(
            java_feature_version_from_release("JAVA_VERSION=\"21\""),
            Some(21)
        );
        assert_eq!(java_feature_version_from_release("IMPLEMENTOR=\"x\""), None);
        assert_eq!(
            java_feature_version_from_release("JAVA_VERSION=\"weird\""),
            None
        );
    }

    #[test]
    fn launch_plan_takes_the_nearest_release_and_does_not_fall_back() {
        let mut request = request("/jdk/bin/java", &["-cp", "a.jar", "Main"]);
        request.java_feature_version = None;
        request.java_release_contents = Some(vec![
            "JAVA_VERSION=\"weird\"".to_owned(),
            "JAVA_VERSION=\"17\"".to_owned(),
        ]);
        assert_eq!(plan_launch_command(&request).unwrap(), unchanged(&request));

        request.command_line_limit = Some(10);
        request.java_release_contents = Some(vec![
            "IMPLEMENTOR=\"x\"".to_owned(),
            "JAVA_VERSION=\"21\"".to_owned(),
        ]);
        assert!(plan_launch_command(&request)
            .unwrap()
            .argument_file
            .is_some());
    }

    #[test]
    fn launch_plan_prefers_an_explicit_feature_version() {
        let mut request = request("/jdk/bin/java", &["-cp", "a.jar", "Main"]);
        request.java_feature_version = Some(8);
        request.java_release_contents = Some(vec!["JAVA_VERSION=\"17\"".to_owned()]);
        assert_eq!(plan_launch_command(&request).unwrap(), unchanged(&request));
    }

    #[test]
    fn launch_plan_command_dispatches_through_the_protocol() {
        let response: serde_json::Value = serde_json::from_str(&crate::execute_json(
            &serde_json::json!({
                "id": "plan",
                "command": "execution.planLaunchCommand",
                "payload": {
                    "executable": "/jdk/bin/java",
                    "arguments": ["-cp", "a.jar", "Main"],
                    "javaFeatureVersion": 17,
                    "commandLineLimit": 10
                }
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["data"]["argumentFile"]["argumentIndex"], 0);
        assert_eq!(
            response["data"]["arguments"],
            serde_json::json!(["", "Main"])
        );
    }

    #[test]
    fn launch_plan_rejects_an_empty_executable() {
        let request = request("", &[]);
        assert!(plan_launch_command(&request).is_err());
    }
}
