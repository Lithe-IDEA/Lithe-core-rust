//! Observes remote login toolchains and submits their evidence to the shared resolver.

use crate::protocol::{CoreError, ErrorCode};
use lithe_plugin_manifest::validated_manifest_json;
use lithe_remote::protocol::methods::{self, FileKind, OperatingSystem, ResolveExecutableParams};
use lithe_toolchain::paths;
use lithe_workspace::Host;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    registry: Value,
    tool: String,
    directory: String,
    minimum_version: Option<String>,
    #[serde(default)]
    project_code_allowed: bool,
}

pub(super) fn resolve(input: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(input).map_err(|_| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid remote toolchain request",
        )
    })?;
    let host = lithe_workspace::current().ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotSupported,
            "A connected remote workspace is required",
        )
    })?;
    if !request.project_code_allowed {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "Remote toolchain probes require a trusted workspace",
        ));
    }
    let platform = match host.connection.session.hello().host.os {
        OperatingSystem::Linux => "linux",
        OperatingSystem::Macos => "macos",
    };
    let registered =
        lithe_toolchain::registry_command(request.registry.clone(), validated_manifest_json)?;
    if registered["status"] != "ok" {
        return Ok(registered);
    }
    let mut snapshot = json!({
        "registry": request.registry,
        "tool": request.tool,
        "directory": request.directory,
        "operationID": "remote-toolchain",
        "generation": 1,
        "platform": platform,
        "workspaceRoot": host.remote_root(),
        "filesystem": [],
    });
    if let Some(minimum) = request.minimum_version {
        snapshot["minimumVersion"] = json!(minimum);
    }
    let Some(definition) = list(&registered["registry"]["definitions"])
        .map(|entry| &entry["definition"])
        .find(|definition| definition["id"] == snapshot["tool"])
    else {
        return lithe_toolchain::resolve_command(snapshot, validated_manifest_json);
    };
    let mut facts = Observations {
        host: &host,
        platform,
        paths: BTreeMap::new(),
    };
    facts.observe(host.remote_root())?;
    let mut environment = BTreeMap::new();
    let mut commands = BTreeMap::new();
    let mut homes = BTreeSet::new();
    for candidate in list(&definition["executables"]).filter(|value| applies(value, platform)) {
        match candidate["kind"].as_str() {
            Some("environment") => {
                let variable = candidate["variable"].as_str().expect("validated variable");
                let layout = candidate["path"].as_str().expect("validated layout");
                if let Some(executable) = lookup(
                    &host,
                    ResolveExecutableParams::Environment {
                        variable: variable.to_owned(),
                        path: layout.to_owned(),
                    },
                )? {
                    if let Some(home) = installation_home(&executable, layout, platform) {
                        homes.insert(home.clone());
                        environment.insert(variable.to_owned(), home);
                    }
                }
            }
            Some("command") => {
                let command = candidate["command"].as_str().expect("validated command");
                if let Some(executable) = lookup(
                    &host,
                    ResolveExecutableParams::Program {
                        value: command.to_owned(),
                        search_path: None,
                    },
                )? {
                    let canonical = facts.observe(&executable)?;
                    let home = [canonical.as_str(), executable.as_str()]
                        .into_iter()
                        .find_map(|path| {
                            list(&definition["executableLayouts"])
                                .filter(|value| applies(value, platform))
                                .filter_map(|layout| layout["path"].as_str())
                                .find_map(|layout| installation_home(path, layout, platform))
                        });
                    if let Some(home) = home {
                        homes.insert(home.clone());
                        commands.insert(command.to_owned(), home);
                    }
                }
            }
            _ => {}
        }
    }
    for home in homes {
        facts.installation(&home, definition)?;
    }
    snapshot["filesystem"] = json!(facts.paths.into_values().collect::<Vec<_>>());
    snapshot["environment"] = json!(environment);
    snapshot["commandHomes"] = json!(commands);
    let candidates = lithe_toolchain::candidates_command(
        json!({
            "registry": snapshot["registry"],
            "tool": snapshot["tool"],
            "platform": platform,
            "workspaceRoot": host.remote_root(),
            "filesystem": snapshot["filesystem"],
            "environment": snapshot["environment"],
            "commandHomes": snapshot["commandHomes"],
        }),
        validated_manifest_json,
    )?;
    if candidates["status"] != "ok" {
        return Ok(candidates);
    }
    let executables: BTreeSet<_> = list(&candidates["candidates"]["shell"])
        .filter_map(|candidate| candidate["executable"].as_str())
        .collect();
    let mut probes = Vec::with_capacity(executables.len());
    for executable in executables {
        let argv = std::iter::once(executable)
            .chain(list(&definition["version"]["arguments"]).filter_map(Value::as_str))
            .map(str::to_owned)
            .collect();
        let capture = host.capture_tool_version(argv, request.directory.clone())?;
        let mut output = methods::decode_bytes(capture["stdout"].as_str().unwrap_or_default())
            .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid tool probe output"))?;
        output.push(b'\n');
        output.extend(
            methods::decode_bytes(capture["stderr"].as_str().unwrap_or_default())
                .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid tool probe output"))?,
        );
        probes.push(json!({
            "executable": executable,
            "output": String::from_utf8_lossy(&output),
            "exitCode": capture["exitCode"].as_i64().unwrap_or(-1),
        }));
    }
    snapshot["probes"] = json!(probes);
    lithe_toolchain::resolve_command(snapshot, validated_manifest_json)
}

fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn applies(value: &Value, platform: &str) -> bool {
    value
        .get("platforms")
        .is_none_or(|platforms| list(platforms).any(|value| value == platform))
}

fn lookup(host: &Host, candidate: ResolveExecutableParams) -> Result<Option<String>, CoreError> {
    host.request(
        methods::PROCESS_RESOLVE_EXECUTABLE,
        serde_json::to_value(candidate).expect("Executable candidate always encodes"),
    )
}

fn installation_home(executable: &str, layout: &str, platform: &str) -> Option<String> {
    let home = executable.strip_suffix(layout)?.strip_suffix('/')?;
    paths::normalize(if home.is_empty() { "/" } else { home }, platform, true)
}

struct Observations<'a> {
    host: &'a Host,
    platform: &'a str,
    paths: BTreeMap<String, Value>,
}

impl Observations<'_> {
    fn install_path(&mut self, home: &str, relative: &str) -> Result<(), CoreError> {
        if let Some(path) = paths::join(home, relative, self.platform) {
            self.observe(&path)?;
        }
        Ok(())
    }

    fn installation(&mut self, home: &str, definition: &Value) -> Result<(), CoreError> {
        let canonical = self.observe(home)?;
        for home in [home, &canonical] {
            for layout in
                list(&definition["executableLayouts"]).filter(|value| applies(value, self.platform))
            {
                self.install_path(home, layout["path"].as_str().expect("validated layout"))?;
            }
            for suffix in list(&definition["pathPrepend"])
                .filter_map(Value::as_str)
                .filter_map(|value| value.strip_prefix("${home}/"))
            {
                self.install_path(home, suffix)?;
            }
        }
        Ok(())
    }

    fn observe(&mut self, path: &str) -> Result<String, CoreError> {
        if let Some(fact) = self.paths.get(path) {
            return Ok(fact["canonicalPath"]
                .as_str()
                .expect("recorded path")
                .to_owned());
        }
        if self.paths.len() >= 512 {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Remote toolchain evidence exceeds its limit",
            ));
        }
        let (canonical, kind, executable) = match self
            .host
            .request::<methods::FileMetadata>(methods::FS_STAT, json!({"path": path}))
        {
            Ok(metadata) => {
                let mode = metadata.symlink_target_mode.unwrap_or(metadata.mode);
                let kind = match metadata.symlink_target_kind.unwrap_or(metadata.kind) {
                    FileKind::File => "file",
                    FileKind::Directory => "directory",
                    _ => "missing",
                };
                let canonical = if kind == "missing" {
                    path.to_owned()
                } else {
                    let resolved: methods::RealPathResult = self
                        .host
                        .request(methods::FS_REAL_PATH, json!({"path": path}))?;
                    resolved.path
                };
                (canonical, kind, kind == "file" && mode & 0o111 != 0)
            }
            Err(error) if matches!(error.code, ErrorCode::WorkspaceNotFound) => {
                (path.to_owned(), "missing", false)
            }
            Err(error) => return Err(error),
        };
        self.paths.insert(path.to_owned(), json!({"path": path, "canonicalPath": canonical, "kind": kind, "executable": executable}));
        Ok(canonical)
    }
}
