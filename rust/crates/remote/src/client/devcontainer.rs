//! Dev Containers: a local folder's `devcontainer.json` built and started
//! by the reference Dev Container CLI, then entered as a running container.
//!
//! Lithe does not interpret the configuration. [`discover`] finds the
//! configuration files the specification places in a folder and reads only
//! their `name` for the picker; [`up_command`] asks the `devcontainer` CLI
//! (`@devcontainers/cli`) to build or reuse the container exactly as the
//! configuration says, including its mounts, features and lifecycle
//! commands; [`parse_up_result`] reads the container, user and workspace
//! folder the CLI reports. The connection then enters that container through
//! [`ContainerConnection`](crate::client::ContainerConnection), like any
//! running container (S32), and opens the reported workspace folder.
//!
//! Building runs the folder's code on this machine (`initializeCommand`,
//! Dockerfiles, feature scripts), so the caller starts it only for a folder
//! the user trusts.
//!
//! Note: .agents/notes/proposed/architecture/2026-10-24-remote-development-workspace-host.md

use crate::client::transport::{is_container_name, is_container_user, ContainerEngine};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The Dev Container CLI's executable, which `PATH` resolves. npm installs a
/// command script on Windows.
#[cfg(not(windows))]
pub const CLI: &str = "devcontainer";
/// The Dev Container CLI's executable, which `PATH` resolves. npm installs a
/// command script on Windows.
#[cfg(windows)]
pub const CLI: &str = "devcontainer.cmd";

/// Bytes of one `devcontainer.json` read for its name; larger files are
/// listed without one.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Subfolders of `.devcontainer` searched for configurations, in name order.
const MAX_SUBFOLDERS: usize = 64;

/// Longest configuration name kept, in bytes.
const MAX_NAME_BYTES: usize = 128;

/// Longest workspace folder accepted from the CLI, in bytes.
const MAX_FOLDER_BYTES: usize = 4096;

/// Longest error text kept from the CLI's result, in bytes.
const MAX_MESSAGE_BYTES: usize = 2048;

/// One configuration in a folder.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevcontainerConfig {
    /// Path relative to the folder, with `/` separators, for example
    /// `.devcontainer/devcontainer.json`.
    pub path: String,
    /// The configuration's `name`, when it has a readable one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The configurations of `folder` in the places the Dev Container
/// specification looks: `.devcontainer/devcontainer.json`,
/// `.devcontainer.json`, then `.devcontainer/<name>/devcontainer.json` for
/// each subfolder in name order. Only regular files are listed; a name is
/// read only from a file inside `folder` that is at most
/// [`MAX_CONFIG_BYTES`] long.
pub fn discover(folder: &Path) -> Vec<DevcontainerConfig> {
    let mut candidates = vec![
        ".devcontainer/devcontainer.json".to_string(),
        ".devcontainer.json".to_string(),
    ];
    if let Ok(entries) = std::fs::read_dir(folder.join(".devcontainer")) {
        let mut subfolders: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect();
        subfolders.sort();
        subfolders.truncate(MAX_SUBFOLDERS);
        candidates.extend(
            subfolders
                .into_iter()
                .map(|name| format!(".devcontainer/{name}/devcontainer.json")),
        );
    }
    let root = std::fs::canonicalize(folder).ok();
    candidates
        .into_iter()
        .filter_map(|path| {
            let file = folder.join(&path);
            if !std::fs::metadata(&file).is_ok_and(|metadata| metadata.is_file()) {
                return None;
            }
            let name = read_name(&file, root.as_deref());
            Some(DevcontainerConfig { path, name })
        })
        .collect()
}

/// The `name` of the configuration at `file` when it resolves inside `root`.
fn read_name(file: &Path, root: Option<&Path>) -> Option<String> {
    let resolved = std::fs::canonicalize(file).ok()?;
    if !root.is_some_and(|root| resolved.starts_with(root)) {
        return None;
    }
    if std::fs::metadata(&resolved).ok()?.len() > MAX_CONFIG_BYTES {
        return None;
    }
    config_name(&std::fs::read_to_string(&resolved).ok()?)
}

/// The `name` of a `devcontainer.json` text, which is JSON with comments and
/// trailing commas. Control characters are removed and the name is cut to
/// 128 bytes; an empty or missing name is `None`.
pub fn config_name(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(&strip_jsonc(text)).ok()?;
    let name = value.get("name")?.as_str()?;
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cut(cleaned.trim(), MAX_NAME_BYTES);
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

/// `text` without `//` and `/* */` comments and without commas that only
/// precede a closing `}` or `]`, outside strings.
fn strip_jsonc(text: &str) -> String {
    strip_trailing_commas(&strip_comments(text))
}

/// Calls `outside` for each character outside a JSON string and copies the
/// characters of strings, escapes included, unchanged.
fn rewrite_outside_strings(
    text: &str,
    mut outside: impl FnMut(char, &mut std::iter::Peekable<std::str::Chars>, &mut String),
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else {
            outside(c, &mut chars, &mut out);
        }
    }
    out
}

fn strip_comments(text: &str) -> String {
    rewrite_outside_strings(text, |c, chars, out| match c {
        '/' if chars.peek() == Some(&'/') => {
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
        }
        '/' if chars.peek() == Some(&'*') => {
            chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            out.push(' ');
        }
        _ => out.push(c),
    })
}

fn strip_trailing_commas(text: &str) -> String {
    rewrite_outside_strings(text, |c, chars, out| {
        let trailing = c == ','
            && matches!(
                chars.clone().find(|next| !next.is_whitespace()),
                Some('}') | Some(']')
            );
        if !trailing {
            out.push(c);
        }
    })
}

/// Command line, starting with [`CLI`], that builds or reuses the dev
/// container of `folder` and starts it. `config` is the configuration file's
/// absolute path; the CLI's default search when absent. Podman is passed as
/// the CLI's Docker program, after `up`, which is where the CLI reads it.
pub fn up_command(engine: ContainerEngine, folder: &str, config: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        CLI.to_string(),
        "up".into(),
        "--workspace-folder".into(),
        folder.into(),
    ];
    if let Some(config) = config {
        argv.push("--config".into());
        argv.push(config.into());
    }
    if engine == ContainerEngine::Podman {
        argv.push("--docker-path".into());
        argv.push(engine.name().into());
    }
    argv
}

/// The container `devcontainer up` started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedDevcontainer {
    /// Full container id.
    pub container_id: String,
    /// User the configuration connects as (`remoteUser`), which the
    /// connection passes to `exec`; the image's default user when absent.
    pub remote_user: Option<String>,
    /// Absolute folder inside the container that holds the workspace.
    pub remote_workspace_folder: String,
}

/// Why [`parse_up_result`] found no started container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpFailure {
    /// The CLI reported an error: its message and description, joined.
    Reported(String),
    /// The CLI printed no result line.
    Missing,
    /// The CLI reported success with a container id, user or folder Lithe
    /// cannot use.
    Unusable,
}

/// The result `devcontainer up` printed: the last line of `stdout` that is a
/// JSON object with an `outcome`.
pub fn parse_up_result(stdout: &[u8]) -> Result<StartedDevcontainer, UpFailure> {
    let text = String::from_utf8_lossy(stdout);
    let result = text
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| {
            value
                .get("outcome")
                .is_some_and(|outcome| outcome.is_string())
        })
        .ok_or(UpFailure::Missing)?;
    let field = |name: &str| result.get(name).and_then(|value| value.as_str());
    if field("outcome") != Some("success") {
        let reported: Vec<&str> = [field("message"), field("description")]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect();
        return Err(UpFailure::Reported(
            cut(&reported.join("\n"), MAX_MESSAGE_BYTES).to_string(),
        ));
    }
    let container_id = field("containerId")
        .map(str::trim)
        .filter(|id| is_container_name(id))
        .ok_or(UpFailure::Unusable)?;
    let remote_user = match field("remoteUser").map(str::trim) {
        None | Some("") => None,
        Some(user) if is_container_user(user) => Some(user.to_string()),
        Some(_) => return Err(UpFailure::Unusable),
    };
    let folder = field("remoteWorkspaceFolder").ok_or(UpFailure::Unusable)?;
    if !folder.starts_with('/')
        || folder.len() > MAX_FOLDER_BYTES
        || folder.chars().any(char::is_control)
    {
        return Err(UpFailure::Unusable);
    }
    Ok(StartedDevcontainer {
        container_id: container_id.to_string(),
        remote_user,
        remote_workspace_folder: folder.to_string(),
    })
}

/// Command line, starting with the engine CLI's name, that prints the name
/// of container `id`.
pub fn name_command(engine: ContainerEngine, id: &str) -> Vec<String> {
    vec![
        engine.name().into(),
        "inspect".into(),
        "--format".into(),
        "{{.Name}}".into(),
        id.into(),
    ]
}

/// The container name [`name_command`] printed, without Docker's leading
/// `/`; `None` when it is not a name `exec` can be given.
pub fn parse_name(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let name = text.lines().next()?.trim();
    let name = name.strip_prefix('/').unwrap_or(name);
    is_container_name(name).then(|| name.to_string())
}

/// `text` cut to at most `limit` bytes at a character boundary.
fn cut(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Folder(PathBuf);

    impl Folder {
        fn new(label: &str) -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "lithe-devcontainer-{label}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn write(&self, path: &str, text: &str) {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn config(path: &str, name: Option<&str>) -> DevcontainerConfig {
        DevcontainerConfig {
            path: path.into(),
            name: name.map(Into::into),
        }
    }

    #[test]
    fn discovery_lists_the_specification_places_in_order() {
        let folder = Folder::new("places");
        folder.write(".devcontainer.json", r#"{"name": "Root"}"#);
        folder.write(".devcontainer/devcontainer.json", r#"{"name": "Main"}"#);
        folder.write(".devcontainer/zeta/devcontainer.json", "{}");
        folder.write(
            ".devcontainer/alpha/devcontainer.json",
            r#"{"name": " Alpha "}"#,
        );
        folder.write(".devcontainer/empty/other.json", "{}");
        folder.write(".devcontainer/.hidden/devcontainer.json", "{}");
        folder.write(".devcontainer/deep/one/devcontainer.json", "{}");
        assert_eq!(
            discover(&folder.0),
            vec![
                config(".devcontainer/devcontainer.json", Some("Main")),
                config(".devcontainer.json", Some("Root")),
                config(".devcontainer/alpha/devcontainer.json", Some("Alpha")),
                config(".devcontainer/zeta/devcontainer.json", None),
            ]
        );
        assert!(discover(&folder.0.join("absent")).is_empty());
    }

    #[test]
    fn discovery_skips_folders_named_like_configurations() {
        let folder = Folder::new("folders");
        std::fs::create_dir_all(folder.0.join(".devcontainer.json")).unwrap();
        assert!(discover(&folder.0).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn names_are_read_only_inside_the_folder() {
        let folder = Folder::new("inside");
        let outside = Folder::new("outside");
        outside.write("devcontainer.json", r#"{"name": "Outside"}"#);
        std::fs::create_dir_all(folder.0.join(".devcontainer")).unwrap();
        std::os::unix::fs::symlink(
            outside.0.join("devcontainer.json"),
            folder.0.join(".devcontainer/devcontainer.json"),
        )
        .unwrap();
        assert_eq!(
            discover(&folder.0),
            vec![config(".devcontainer/devcontainer.json", None)]
        );
    }

    #[test]
    fn names_are_read_from_json_with_comments_and_trailing_commas() {
        let text = r#"// Dev container for "the app"
{
    /* the picker shows this */
    "name": "App // not a comment", // trailing
    "image": "mcr.microsoft.com/devcontainers/base:ubuntu",
    "forwardPorts": [3000, 5432,],
    "features": {
        "ghcr.io/devcontainers/features/node:1": {},
    },
}"#;
        assert_eq!(config_name(text).as_deref(), Some("App // not a comment"));
        assert_eq!(
            config_name(r#"{"name": "a\"b, }"}"#).as_deref(),
            Some("a\"b, }")
        );
        assert_eq!(
            config_name("{\"name\": \"n\", // last\n}").as_deref(),
            Some("n")
        );
        assert_eq!(
            config_name("{\"name\": \"n\", /* x */ }").as_deref(),
            Some("n")
        );
        assert_eq!(config_name(r#"{"name": "  "}"#), None);
        assert_eq!(config_name(r#"{"name": 3}"#), None);
        assert_eq!(config_name("{"), None);
        assert_eq!(
            config_name(r#"{"name": "x\u0007y"}"#).as_deref(),
            Some("xy")
        );
        let long = format!(r#"{{"name": "{}"}}"#, "é".repeat(100));
        assert_eq!(config_name(&long).unwrap().len(), 128);
    }

    #[test]
    fn up_passes_the_folder_the_configuration_and_podman_after_up() {
        assert_eq!(
            up_command(ContainerEngine::Docker, "/work/app", None),
            [CLI, "up", "--workspace-folder", "/work/app"]
        );
        assert_eq!(
            up_command(
                ContainerEngine::Podman,
                "/work/app",
                Some("/work/app/.devcontainer/python/devcontainer.json")
            ),
            [
                CLI,
                "up",
                "--workspace-folder",
                "/work/app",
                "--config",
                "/work/app/.devcontainer/python/devcontainer.json",
                "--docker-path",
                "podman",
            ]
        );
    }

    #[test]
    fn results_name_the_container_user_and_folder() {
        let stdout = b"[2 ms] Start: Run\n{\"outcome\":\"success\",\"containerId\":\"0123abcd\",\"remoteUser\":\"vscode\",\"remoteWorkspaceFolder\":\"/workspaces/app\"}\n";
        assert_eq!(
            parse_up_result(stdout),
            Ok(StartedDevcontainer {
                container_id: "0123abcd".into(),
                remote_user: Some("vscode".into()),
                remote_workspace_folder: "/workspaces/app".into(),
            })
        );
        let without_user =
            br#"{"outcome":"success","containerId":"c1","remoteWorkspaceFolder":"/w"}"#;
        assert_eq!(parse_up_result(without_user).unwrap().remote_user, None);
    }

    #[test]
    fn failed_or_unusable_results_say_why() {
        let reported = br#"{"outcome":"error","message":"Command failed: docker build","description":"An error occurred building the image."}"#;
        assert_eq!(
            parse_up_result(reported),
            Err(UpFailure::Reported(
                "Command failed: docker build\nAn error occurred building the image.".into()
            ))
        );
        assert_eq!(
            parse_up_result(b"npm ERR! not found\n"),
            Err(UpFailure::Missing)
        );
        assert_eq!(parse_up_result(b"{\"other\":1}\n"), Err(UpFailure::Missing));
        for unusable in [
            r#"{"outcome":"success","containerId":"--privileged","remoteWorkspaceFolder":"/w"}"#,
            r#"{"outcome":"success","containerId":"c","remoteUser":"a b","remoteWorkspaceFolder":"/w"}"#,
            r#"{"outcome":"success","containerId":"c","remoteWorkspaceFolder":"w"}"#,
            r#"{"outcome":"success","containerId":"c"}"#,
            r#"{"outcome":"success","remoteWorkspaceFolder":"/w"}"#,
        ] {
            assert_eq!(
                parse_up_result(unusable.as_bytes()),
                Err(UpFailure::Unusable)
            );
        }
    }

    #[test]
    fn names_come_from_inspect_without_the_leading_slash() {
        assert_eq!(
            name_command(ContainerEngine::Podman, "c1"),
            ["podman", "inspect", "--format", "{{.Name}}", "c1"]
        );
        assert_eq!(
            parse_name(b"/vigilant_bassi\n").as_deref(),
            Some("vigilant_bassi")
        );
        assert_eq!(
            parse_name(b"app_devcontainer-app-1\n").as_deref(),
            Some("app_devcontainer-app-1")
        );
        assert_eq!(parse_name(b"\n"), None);
        assert_eq!(parse_name(b"-x\n"), None);
    }
}
