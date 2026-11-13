//! Resolves captured toolchain candidates without native I/O or manager semantics.

use super::{definitions, fixed::FixedToolchainSelection, paths, pins, query};
use crate::ManifestValidator;
use lithe_plugin_model::{PluginRejection, Validation};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

mod automatic;
mod discovery;
mod installations;
mod launch;
mod probe;
mod project;
mod snapshot;

pub(crate) use automatic::*;
pub(crate) use discovery::*;
pub(crate) use installations::*;
pub(crate) use launch::*;
pub(crate) use probe::*;
pub(crate) use project::*;
pub(crate) use snapshot::*;

/// Resolves definitions, provider replies, pins and trusted native observations once.
pub(crate) fn resolve(input: &Value, validate_manifest: ManifestValidator) -> Validation<Value> {
    fields(
        input,
        &[
            "registry",
            "tool",
            "directory",
            "operationID",
            "local",
            "platform",
            "filesystemCaseSensitive",
            "workspaceRoot",
            "workspaceRoots",
            "filesystem",
            "minimumVersion",
            "probes",
            "queries",
            "systemInstallations",
            "globalDefault",
            "environment",
            "commandHomes",
            "projectCodeAllowed",
            "providerChoices",
            "generation",
            "context",
            "fixed",
        ],
    )?;
    let registry = definitions::registry(&input["registry"], validate_manifest)?;
    let tool = input["tool"]
        .as_str()
        .filter(|tool| lithe_plugin_model::is_tool_id(tool))
        .ok_or_else(|| reject("tool"))?;
    let directory = input["directory"]
        .as_str()
        .ok_or_else(|| reject("directory"))?;
    input["operationID"]
        .as_str()
        .filter(|value| bounded(value, 128))
        .ok_or_else(|| reject("operationID"))?;
    let document = input
        .get("local")
        .cloned()
        .unwrap_or(json!({"version":1,"toolchains":{}}));
    let pin = pins::resolve(&json!({"document":document,"directory":directory,"tool":tool}))?;
    let fixed = input
        .get("fixed")
        .map(FixedToolchainSelection::parse)
        .transpose()?;
    let Some(definition) = enabled(&registry, tool) else {
        return Ok(output(
            input,
            json!({"kind":"skip","reason":reason("definitionUnavailable",tool)}),
            vec![],
            vec![],
            vec![],
        ));
    };
    let generation = input["generation"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| reject("generation"))?;
    let (snapshot, root) = prepare(input, definition)?;
    let platform = snapshot.platform;
    let case_sensitive = snapshot.case_sensitive;
    if input
        .get("projectCodeAllowed")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(reject("projectCodeAllowed"));
    }
    string_maps(input, &["environment", "commandHomes", "providerChoices"])?;
    if let Some(choices) = input.get("providerChoices") {
        fields(choices, &["project", "global", "automatic"])?;
    }
    if let Some(fixed) = fixed {
        return Ok(match snapshot.fixed_selection(&fixed) {
            Ok(selection) => output(
                input,
                json!({"kind":"resolved","selection":selection}),
                vec![json!({"candidateID":"consumerFixed","layer":"pinned","state":"resolved"})],
                vec![],
                vec![],
            ),
            Err(code) => output(
                input,
                json!({"kind":"terminalFailure","reason":reason(code,tool)}),
                vec![
                    json!({"candidateID":"consumerFixed","layer":"pinned","state":"terminalFailure","reason":reason(code,tool)}),
                ],
                vec![],
                vec![],
            ),
        });
    }
    // A failed optional provider must never revoke a usable explicit fixed value.
    if let Some(home) = pin["pin"]["home"].as_str() {
        let result = snapshot.selection(
            home,
            pin["pin"]["provider"].as_str().unwrap_or(""),
            "pinned",
        );
        return Ok(match result {
            Ok(selection) => output(
                input,
                json!({"kind":"resolved","selection":selection}),
                vec![json!({"candidateID":"pinned","layer":"pinned","state":"resolved"})],
                vec![],
                vec![],
            ),
            Err(code) => output(
                input,
                json!({"kind":"terminalFailure","reason":reason(code,tool)}),
                vec![
                    json!({"candidateID":"pinned","layer":"pinned","state":"terminalFailure","reason":reason(code,tool)}),
                ],
                vec![],
                vec![],
            ),
        });
    }
    let mut installations = Vec::new();
    let mut statements = Vec::new();
    let mut unmet = Vec::new();
    let mut query_providers = BTreeSet::new();
    for value in array(input, "queries", 256)? {
        if value["registry"] != input["registry"] {
            return Err(reject("queries.registry"));
        }
        if value["platform"] != platform {
            return Err(reject("queries.platform"));
        }
        if value["request"]["params"]["workspace"]["root"]
            .as_str()
            .and_then(|path| snapshot.key(path).ok())
            != Some(snapshot.key(&root)?)
        {
            return Err(reject("queries.workspace"));
        }
        let requested_directory = value["request"]["params"]["workspace"]["directory"]
            .as_str()
            .unwrap_or("");
        if requested_directory != directory {
            return Err(reject("queries.directory"));
        }
        if value["filesystem"] != input["filesystem"] {
            return Err(reject("queries.filesystem"));
        }
        let query_case = value
            .get("filesystemCaseSensitive")
            .and_then(Value::as_bool)
            .unwrap_or(platform != "windows");
        if query_case != case_sensitive
            || value.get("workspaceRoots") != input.get("workspaceRoots")
        {
            return Err(reject("queries.filesystemPolicy"));
        }
        if value["currentGeneration"].as_u64() != Some(generation) {
            return Err(reject("queries.generation"));
        }
        if value["request"]["params"]["context"] != input["context"] {
            return Err(reject("queries.context"));
        }
        if !query_providers.insert((
            value["pluginID"].to_string(),
            value["request"]["params"]["providerID"].to_string(),
        )) {
            return Err(reject("queries.providerID"));
        }
        let decoded = match query::decode(value, validate_manifest) {
            Ok(decoded) => decoded,
            Err(error)
                if matches!(
                    error.code,
                    "plugin.toolchain.timeout" | "plugin.toolchain.providerUnavailable"
                ) =>
            {
                // Only a provider asked about this tool leaves it without an answer; `cause`
                // keeps a missed deadline apart from a backend that could not answer at all.
                if value["request"]["params"]["tools"]
                    .as_array()
                    .is_some_and(|tools| tools.iter().any(|asked| asked == tool))
                {
                    unmet.push(json!({"code":"providerUnavailable","tool":tool,
                        "provider":format!("{}:{}",value["pluginID"].as_str().unwrap_or(""),value["request"]["params"]["providerID"].as_str().unwrap_or("")),
                        "cause":error.code}));
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        installations.extend(
            decoded["installations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|value| value["tool"] == tool)
                .cloned(),
        );
        statements.extend(
            decoded["statements"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|value| value["tool"] == tool)
                .cloned(),
        );
        unmet.extend(
            decoded["diagnostics"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|value| value["tool"] == tool && value["code"] != "requestedNotInstalled")
                .cloned(),
        );
    }
    installations.extend(system_installations(input, tool)?);
    let candidates: Vec<_> = definition["executables"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|value| applicable(value, platform))
        .collect();
    let mut trace_rows = Vec::new();
    let mut conflicts = Vec::new();
    let mut outcome = None;
    let mut project_done = false;
    if let Some(default) = input.get("globalDefault").filter(|value| !value.is_null()) {
        fields(default, &["home"])?;
        default["home"]
            .as_str()
            .filter(|value| bounded(value, 4096))
            .ok_or_else(|| reject("globalDefault.home"))?;
    }
    // Explicit application defaults obey the same terminal policy without a plugin slot.
    let default_preflight = input
        .get("globalDefault")
        .filter(|value| !value.is_null())
        .is_some()
        && !candidates
            .iter()
            .any(|value| value["kind"] == "setting" && value["layer"] == "global");
    let mut default_checked = !default_preflight;
    for candidate in &candidates {
        let kind = candidate["kind"].as_str().unwrap_or("");
        let id = candidate["id"].as_str().unwrap_or(kind);
        let layer = match kind {
            "setting" if candidate["layer"] == "project" => "pinned",
            "projectFile" => "project",
            "toolchain" if candidate["scope"] == "project" => "project",
            "toolchain" if candidate["scope"] == "installed" => "automatic",
            _ => "global",
        };
        if !default_checked && matches!(layer, "global" | "automatic") && outcome.is_none() {
            default_checked = true;
            let home = input["globalDefault"]["home"]
                .as_str()
                .ok_or_else(|| reject("globalDefault"))?;
            let result = snapshot.selection(home, "host:default", "global");
            outcome = Some(match result {
                Ok(selection) => json!({"kind":"resolved","selection":selection}),
                Err(code) => json!({"kind":"terminalFailure","reason":reason(code,tool)}),
            });
        }
        if outcome.is_some() {
            trace(&mut trace_rows, id, layer, "shadowed", None);
            continue;
        }
        let mut selected = None;
        let mut failure = None;
        let mut terminal = false;
        match (kind, layer) {
            ("setting", "pinned") => {
                if let Some(home) = pin["pin"]["home"].as_str() {
                    match snapshot.selection(
                        home,
                        pin["pin"]["provider"].as_str().unwrap_or(""),
                        layer,
                    ) {
                        Ok(value) => selected = Some(value),
                        Err(code) => {
                            failure = Some(reason(code, tool));
                            terminal = true;
                        }
                    }
                }
            }
            ("setting", _) => {
                if let Some(value) = input.get("globalDefault").filter(|value| !value.is_null()) {
                    let home = value["home"]
                        .as_str()
                        .ok_or_else(|| reject("globalDefault"))?;
                    match snapshot.selection(home, "host:default", layer) {
                        Ok(value) => selected = Some(value),
                        Err(code) => {
                            failure = Some(reason(code, tool));
                            terminal = true;
                        }
                    }
                }
            }
            (_, "project") if !project_done => {
                project_done = true;
                let mut choices = Vec::new();
                for source in &candidates {
                    if source["kind"] != "projectFile" {
                        continue;
                    }
                    for ancestor in ancestors(directory) {
                        let anchor = if ancestor == "." {
                            root.clone()
                        } else {
                            paths::join(&root, ancestor, platform)
                                .ok_or_else(|| reject("directory"))?
                        };
                        let native =
                            paths::join(&anchor, source["path"].as_str().unwrap_or(""), platform)
                                .ok_or_else(|| reject("projectFile"))?;
                        if snapshot.fact(&native, "file").is_err() {
                            continue;
                        }
                        let markers =
                            source["requires"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .all(|marker| {
                                    marker
                                        .as_str()
                                        .and_then(|marker| paths::join(&anchor, marker, platform))
                                        .is_some_and(|path| snapshot.fact(&path, "file").is_ok())
                                });
                        if !markers {
                            continue;
                        }
                        let mut value = (*source).clone();
                        value["nativePath"] = json!(native);
                        if let Some(home) = source["home"].as_str() {
                            value["nativeHome"] = json!(if home == "." {
                                anchor.clone()
                            } else {
                                paths::join(&anchor, home, platform)
                                    .ok_or_else(|| reject("projectFile.home"))?
                            });
                        }
                        choices.push(ProjectChoice {
                            anchor,
                            category: if source["projectKind"] == "environment" {
                                1
                            } else {
                                0
                            },
                            native: true,
                            value,
                            candidate: true,
                        });
                    }
                }
                if candidates
                    .iter()
                    .any(|value| value["kind"] == "toolchain" && value["scope"] == "project")
                {
                    for statement in statements
                        .iter()
                        .filter(|value| value["scope"] == "project")
                    {
                        let anchor = statement["sourceDirectory"]
                            .as_str()
                            .ok_or_else(|| reject("statements.sourceDirectory"))?;
                        choices.push(ProjectChoice {
                            anchor: anchor.to_owned(),
                            category: if statement["kind"] == "environment" {
                                1
                            } else {
                                2
                            },
                            native: statement["reader"] == "native",
                            value: statement.clone(),
                            candidate: false,
                        });
                    }
                }
                choices.sort_by_key(|value| {
                    (
                        std::cmp::Reverse(value.anchor.split('/').count()),
                        value.category,
                    )
                });
                // The nearest declaration masks inherited declarations even when unmet.
                if let Some(nearest) = choices.first().map(|value| value.anchor.split('/').count())
                {
                    choices.retain(|value| value.anchor.split('/').count() == nearest);
                }
                while let Some(first) = choices.first() {
                    let depth = first.anchor.split('/').count();
                    let category = first.category;
                    let count = choices
                        .iter()
                        .take_while(|value| {
                            value.anchor.split('/').count() == depth && value.category == category
                        })
                        .count();
                    let mut group: Vec<_> = choices.drain(..count).collect();
                    let native_sources: BTreeSet<_> = group
                        .iter()
                        .filter(|value| value.native && !value.candidate)
                        .map(|value| value.value["source"].clone().to_string())
                        .collect();
                    group.retain(|value| {
                        value.native || !native_sources.contains(&value.value["source"].to_string())
                    });
                    let homes: BTreeSet<_> = group
                        .iter()
                        .filter_map(|value| value.value["installation"].as_str().map(str::to_owned))
                        .collect();
                    let requests: BTreeSet<_> = group
                        .iter()
                        .filter(|value| !value.candidate)
                        .map(|value| {
                            (
                                value.value["request"].to_string(),
                                value.value["installation"].to_string(),
                            )
                        })
                        .collect();
                    let choice = input["providerChoices"]["project"].as_str();
                    if let Some(chosen) = choice {
                        group.retain(|value| value.candidate || value.value["provider"] == chosen);
                    }
                    let retained: BTreeSet<_> = group
                        .iter()
                        .filter(|value| !value.candidate)
                        .map(|value| {
                            (
                                value.value["request"].to_string(),
                                value.value["installation"].to_string(),
                            )
                        })
                        .collect();
                    if (choice.is_none() && (homes.len() > 1 || requests.len() > 1))
                        || retained.len() > 1
                    {
                        let providers: BTreeSet<_> = group
                            .iter()
                            .filter_map(|value| value.value["provider"].as_str())
                            .collect();
                        conflicts.push(json!({"scope":"project","providers":providers,"action":"chooseProvider"}));
                        failure = Some(reason("providerConflict", tool));
                        break;
                    }
                    for choice in &group {
                        match project_selection(&snapshot, choice) {
                            Ok(value) => {
                                selected = Some(value);
                                break;
                            }
                            Err(code) => {
                                let mut why = reason(code, tool);
                                for key in ["provider", "source", "request"] {
                                    if let Some(value) = choice.value.get(key) {
                                        why[key] = value.clone();
                                    }
                                }
                                if code == "requestedNotInstalled" && !unmet.contains(&why) {
                                    unmet.push(why.clone());
                                }
                                failure = Some(why);
                            }
                        }
                    }
                    if selected.is_some() {
                        break;
                    }
                }
            }
            (_, "project") => continue,
            ("toolchain", "global") => {
                let globals: Vec<_> = statements
                    .iter()
                    .filter(|value| value["scope"] == "global")
                    .collect();
                let chosen = input["providerChoices"]["global"].as_str();
                let globals: Vec<_> = globals
                    .into_iter()
                    .filter(|value| chosen.is_none_or(|chosen| value["provider"] == chosen))
                    .collect();
                let homes: BTreeSet<_> = globals
                    .iter()
                    .filter_map(|value| value["installation"].as_str())
                    .collect();
                let requests: BTreeSet<_> = globals
                    .iter()
                    .map(|value| {
                        (
                            value["request"].to_string(),
                            value["installation"].to_string(),
                        )
                    })
                    .collect();
                if homes.len() > 1 || requests.len() > 1 {
                    let providers: BTreeSet<_> = globals
                        .iter()
                        .filter_map(|value| value["provider"].as_str())
                        .collect();
                    conflicts.push(
                        json!({"scope":"global","providers":providers,"action":"chooseProvider"}),
                    );
                    failure = Some(reason("providerConflict", tool));
                } else if let Some(value) = globals.first() {
                    if let Some(home) = value["installation"].as_str() {
                        match snapshot.selection(
                            home,
                            value["provider"].as_str().unwrap_or(""),
                            layer,
                        ) {
                            Ok(value) => selected = Some(value),
                            Err(code) => failure = Some(reason(code, tool)),
                        }
                    } else {
                        let why = json!({"code":"requestedNotInstalled","tool":tool,"provider":value["provider"],"source":value["source"],"request":value["request"]});
                        unmet.push(why.clone());
                        failure = Some(why);
                    }
                }
            }
            ("toolchain", "automatic") => {
                let set = automatic_set(&snapshot, &installations)?;
                failure = set.failure.map(|code| reason(code, tool));
                let (choice, conflict) = automatic_choice(
                    &snapshot,
                    set.eligible,
                    input["providerChoices"]["automatic"].as_str(),
                )?;
                selected = choice;
                if let Some(conflict) = conflict {
                    conflicts.push(conflict);
                    failure = Some(reason("ambiguousInstallation", tool));
                }
            }
            ("environment", _) => {
                if let Some(home) = candidate["variable"]
                    .as_str()
                    .and_then(|name| input["environment"][name].as_str())
                    .filter(|value| !value.is_empty())
                {
                    match snapshot.layout_selection(
                        home,
                        candidate["path"].as_str(),
                        "host:shell",
                        layer,
                    ) {
                        Ok(value) => selected = Some(value),
                        Err(code) => failure = Some(reason(code, tool)),
                    }
                }
            }
            ("command", _) => {
                if let Some(home) = candidate["command"]
                    .as_str()
                    .and_then(|name| input["commandHomes"][name].as_str())
                {
                    match snapshot.selection(home, "host:shell", layer) {
                        Ok(value) => selected = Some(value),
                        Err(code) => failure = Some(reason(code, tool)),
                    }
                }
            }
            _ => return Err(reject("executables.kind")),
        }
        if let Some(selection) = selected {
            trace(&mut trace_rows, id, layer, "resolved", None);
            outcome = Some(json!({"kind":"resolved","selection":selection}));
        } else if let Some(why) = failure {
            trace(
                &mut trace_rows,
                id,
                layer,
                if terminal { "terminalFailure" } else { "skip" },
                Some(why.clone()),
            );
            if terminal {
                outcome = Some(json!({"kind":"terminalFailure","reason":why}));
            }
        } else {
            trace(&mut trace_rows, id, layer, "empty", None);
        }
    }
    if outcome.is_none() && !default_checked {
        let home = input["globalDefault"]["home"]
            .as_str()
            .ok_or_else(|| reject("globalDefault"))?;
        outcome = Some(match snapshot.selection(home, "host:default", "global") {
            Ok(selection) => json!({"kind":"resolved","selection":selection}),
            Err(code) => json!({"kind":"terminalFailure","reason":reason(code,tool)}),
        });
    }
    Ok(output(
        input,
        outcome.unwrap_or_else(|| json!({"kind":"skip","reason":reason("notFound",tool)})),
        trace_rows,
        unmet,
        conflicts,
    ))
}
