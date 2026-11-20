//! CLI invocation and owned shell registration rules; native adapters perform all I/O.

use crate::{RuleError, RuleErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

/// Parses bounded launch arguments without probing native paths.
pub const PLAN_LAUNCH: &str = "application.cli.planLaunch";
/// Classifies a native path observation into an isolated editor or a project.
pub const OPEN_TARGET: &str = "application.cli.openTarget";
/// Produces an idempotent edit of only Lithe's owned registration.
pub const EDIT_REGISTRATION: &str = "application.cli.editRegistration";
const BEGIN: &str = "# >>> Lithe CLI >>>";
const END: &str = "# <<< Lithe CLI <<<";
const MAX_SOURCE: usize = 1024 * 1024;

fn invalid(reason: &str) -> RuleError {
    RuleError::new(RuleErrorCode::InvalidRequest, reason)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LaunchRequest {
    args: Vec<String>,
}

/// Resolves help, version, a single path and the launcher's internal detach marker.
pub fn plan_launch_json(payload: Value) -> Result<Value, RuleError> {
    let request: LaunchRequest =
        serde_json::from_value(payload).map_err(|_| invalid("Invalid CLI arguments"))?;
    if request.args.len() > 4
        || request
            .args
            .iter()
            .any(|s| s.len() > 32768 || s.contains('\0'))
    {
        return Err(invalid("CLI arguments exceed their bounds"));
    }
    let mut args = request.args.as_slice();
    let detach = args.first().is_some_and(|s| s == "--cli");
    if detach {
        args = &args[1..];
    }
    let literal = args.first().is_some_and(|s| s == "--");
    if literal {
        args = &args[1..];
    }
    let (action, path) = match args {
        [] => ("welcome", ""),
        [arg] if !literal && (arg == "--help" || arg == "-h") => ("help", ""),
        [arg] if !literal && (arg == "--version" || arg == "-v") => ("version", ""),
        [arg] if !arg.is_empty() && (literal || !arg.starts_with('-')) => ("open", arg.as_str()),
        _ => return Err(invalid("Expected one file or folder path")),
    };
    Ok(json!({"action": action, "path": path, "detach": detach}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetRequest {
    /// Native observation; never inferred from the filename or extension.
    kind: String,
}

/// No project services are permitted for a standalone regular file.
pub fn open_target_json(payload: Value) -> Result<Value, RuleError> {
    let request: TargetRequest = serde_json::from_value(payload)
        .map_err(|_| invalid("Invalid native target observation"))?;
    let (mode, plugins) = match request.kind.as_str() {
        "directory" => ("project", true),
        "file" => ("file", false),
        _ => return Err(invalid("Target must be a regular file or directory")),
    };
    // Commands outside this list cannot enter project services from an editor-only session.
    let commands = if plugins {
        Vec::new()
    } else {
        [
            "settings.open",
            "logs.open",
            "theme.system",
            "theme.light",
            "theme.dark",
            "window.fullscreen",
            "language.en",
            "language.zh",
            "editor.vimMode",
            "project.open",
            "document.open",
            "documents.saveAll",
            "tab.close",
            "terminal.new",
            "tool.terminal",
            "editor.find",
            "editor.replace",
            "editor.fold",
            "editor.unfold",
            "editor.foldAll",
            "editor.unfoldAll",
            "editor.addNextOccurrence",
            "editor.unselectOccurrence",
            "editor.selectAllOccurrences",
            "editor.cloneCaretAbove",
            "editor.cloneCaretBelow",
        ]
        .into_iter()
        .collect::<Vec<_>>()
    };
    Ok(json!({"mode": mode, "plugins": plugins, "commands": commands}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationRequest {
    /// zsh, bash, sh, fish or windows; shell discovery belongs to the native adapter.
    shell: String,
    source: String,
    #[serde(default)]
    launcher_source: String,
    directory: String,
    executable: String,
    /// inspect never changes the returned source; install/remove require an explicit click.
    action: String,
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Keeps unrelated configuration byte-for-byte, rejecting malformed owned blocks.
pub fn edit_registration_json(payload: Value) -> Result<Value, RuleError> {
    let request: RegistrationRequest =
        serde_json::from_value(payload).map_err(|_| invalid("Invalid CLI registration request"))?;
    let r = &request;
    if r.source.len() > MAX_SOURCE
        || r.launcher_source.len() > MAX_SOURCE
        || r.directory.is_empty()
        || r.executable.is_empty()
        || [&r.directory, &r.executable]
            .iter()
            .any(|s| s.len() > 32768 || s.contains(['\0', '\r', '\n']))
        || !matches!(r.action.as_str(), "inspect" | "install" | "remove")
    {
        return Err(invalid("Invalid CLI registration values"));
    }
    let windows = r.shell == "windows";
    if !windows && !matches!(r.shell.as_str(), "zsh" | "bash" | "sh" | "fish") {
        return Err(invalid("Unsupported login shell"));
    }
    let platform = if windows {
        lithe_platform::Platform::Windows
    } else {
        lithe_platform::Platform::Unix
    };
    let launcher = lithe_platform::cli::launcher(&r.executable, platform)
        .ok_or_else(|| invalid("Invalid launcher path"))?;
    let owned = lithe_platform::cli::is_managed_launcher(&r.launcher_source, platform);
    if windows {
        if r.directory.contains([';', '"']) {
            return Err(invalid("Invalid Windows PATH entry"));
        }
        let same = |s: &str| lithe_platform::cli::same_path_entry(s, &r.directory, platform);
        let installed = r.source.split(';').any(same);
        let source = match r.action.as_str() {
            "install" if !installed => format!(
                "{}{}{}",
                r.source,
                if r.source.is_empty() || r.source.ends_with(';') {
                    ""
                } else {
                    ";"
                },
                r.directory
            ),
            "remove" => r
                .source
                .split(';')
                .filter(|s| !same(s))
                .collect::<Vec<_>>()
                .join(";"),
            _ => r.source.clone(),
        };
        return Ok(
            json!({"installed": installed, "source": source, "launcher": launcher, "owned": owned}),
        );
    }
    let newline = if r.source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let command = if r.shell == "fish" {
        format!(
            "if not contains -- {} $PATH; set -gx PATH {} $PATH; end",
            fish_quote(&r.directory),
            fish_quote(&r.directory)
        )
    } else {
        format!("export PATH={}:\"$PATH\"", quote(&r.directory))
    };
    let block = format!("{BEGIN}{newline}{command}{newline}{END}{newline}");
    let starts: Vec<_> = r.source.match_indices(BEGIN).collect();
    let ends: Vec<_> = r.source.match_indices(END).collect();
    if starts.len() > 1 || ends.len() > 1 || starts.len() != ends.len() {
        return Err(invalid("Conflicting Lithe CLI registration markers"));
    }
    let range = if let (Some(&(start, _)), Some(&(end, _))) = (starts.first(), ends.first()) {
        if end < start || (start > 0 && !r.source[..start].ends_with('\n')) {
            return Err(invalid("Malformed Lithe CLI registration block"));
        }
        let tail = end + END.len();
        let suffix = &r.source[tail..];
        let length = if suffix.starts_with("\r\n") {
            2
        } else if suffix.starts_with('\n') {
            1
        } else if suffix.is_empty() {
            0
        } else {
            return Err(invalid("Malformed Lithe CLI registration block"));
        };
        Some(start..tail + length)
    } else {
        None
    };
    let installed = range.as_ref().is_some_and(|range| {
        r.source[range.clone()].trim_end_matches(['\r', '\n'])
            == block.trim_end_matches(['\r', '\n'])
    });
    let mut source = r.source.clone();
    match r.action.as_str() {
        "install" => {
            if let Some(range) = range {
                source.replace_range(range, &block);
            } else {
                if !source.is_empty() && !source.ends_with('\n') {
                    source.push_str(newline);
                }
                source.push_str(&block);
            }
        }
        "remove" => {
            if let Some(range) = range {
                source.replace_range(range, "");
            }
        }
        _ => {}
    }
    Ok(json!({"installed": installed, "source": source, "launcher": launcher, "owned": owned}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_rejects_ambiguous_inputs_and_preserves_literal_paths() {
        assert!(plan_launch_json(json!({"args": ["a", "b"]})).is_err());
        assert!(plan_launch_json(json!({"args": ["--unknown"]})).is_err());
        let plan = plan_launch_json(json!({"args": ["--cli", "--", "-file.txt"]})).unwrap();
        assert_eq!(plan["path"], "-file.txt");
        assert_eq!(plan["detach"], true);
        let target = open_target_json(json!({"kind": "file"})).unwrap();
        assert_eq!(target["plugins"], false);
        let commands = target["commands"].as_array().unwrap();
        assert!(commands.contains(&json!("editor.find")));
        assert!(commands.contains(&json!("terminal.new")));
        assert!(commands.contains(&json!("tool.terminal")));
        assert!(!commands.contains(&json!("search.files")));
    }

    #[test]
    fn shell_registration_is_idempotent_and_preserves_unrelated_content() {
        for shell in ["zsh", "bash", "sh", "fish", "windows"] {
            let original = if shell == "windows" {
                "C:\\Tools;%USERPROFILE%\\bin"
            } else {
                "# personal settings\r\nexport EDITOR=vi\r\n"
            };
            let mut request = json!({"shell": shell, "source": original, "directory": "/User Space/bin", "executable": "/Apps/Lithe's.app/lithe", "action": "install"});
            let first = edit_registration_json(request.clone()).unwrap();
            request["source"] = first["source"].clone();
            let second = edit_registration_json(request.clone()).unwrap();
            assert_eq!(first["source"], second["source"]);
            assert_eq!(second["installed"], true);
            request["action"] = json!("remove");
            assert_eq!(edit_registration_json(request).unwrap()["source"], original);
        }
    }

    #[test]
    fn malformed_markers_never_overwrite_shell_configuration() {
        let request = json!({"shell": "zsh", "source": BEGIN, "directory": "/bin", "executable": "/app", "action": "install"});
        assert!(edit_registration_json(request).is_err());
    }
}
