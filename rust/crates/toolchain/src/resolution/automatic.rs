//! Automatic selection among candidates.

use super::*;

/// The automatic layer's candidate set, keyed by canonical home.
pub(crate) struct Automatic {
    pub(crate) eligible: BTreeMap<String, Value>,
    /// Reported installations that failed a check, with the executable to probe when
    /// the layout itself was valid.
    skipped: BTreeMap<String, Value>,
    pub(crate) failure: Option<&'static str>,
}

pub(crate) fn automatic_set(
    snapshot: &Snapshot<'_>,
    installations: &[Value],
) -> Validation<Automatic> {
    let mut set = Automatic {
        eligible: BTreeMap::new(),
        skipped: BTreeMap::new(),
        failure: None,
    };
    for installation in installations {
        let Some(home) = installation["home"].as_str() else {
            continue;
        };
        let provider = installation["provider"].as_str().unwrap_or("");
        match snapshot.selection(home, provider, "automatic") {
            Ok(value) => {
                let key = snapshot.key(value["home"].as_str().unwrap_or(""))?;
                if set
                    .eligible
                    .get(&key)
                    .is_none_or(|kept: &Value| attribution(&value) < attribution(kept))
                {
                    set.eligible.insert(key, value);
                }
            }
            Err(code) => {
                set.failure = Some(code);
                let checked = snapshot.checked(home, None).ok();
                let canonical = checked
                    .as_ref()
                    .and_then(|value| value["home"].as_str())
                    .unwrap_or(home);
                let mut row = json!({"home":canonical,"provider":provider,"reason":code});
                if let Some(executable) = checked.as_ref().map(|value| &value["executable"]) {
                    row["executable"] = executable.clone();
                }
                let key = snapshot
                    .key(canonical)
                    .unwrap_or_else(|_| canonical.to_owned());
                if set
                    .skipped
                    .get(&key)
                    .is_none_or(|kept: &Value| attribution(&row) < attribution(kept))
                {
                    set.skipped.insert(key, row);
                }
            }
        }
    }
    set.skipped.retain(|key, _| !set.eligible.contains_key(key));
    Ok(set)
}

/// Picks the highest version; equal highest versions in different homes need `chosen`,
/// otherwise they become an `automatic` conflict naming every route.
pub(crate) fn automatic_choice(
    snapshot: &Snapshot<'_>,
    eligible: BTreeMap<String, Value>,
    chosen: Option<&str>,
) -> Validation<(Option<Value>, Option<Value>)> {
    let newest = eligible
        .values()
        .filter_map(|value| components(value["version"].as_str()?))
        .max();
    let mut best: Vec<_> = eligible
        .into_values()
        .filter(|value| value["version"].as_str().and_then(components) == newest)
        .collect();
    if best.len() <= 1 {
        return Ok((best.pop(), None));
    }
    if let Some(chosen) = chosen {
        let identity = snapshot.key(chosen)?;
        let selected = best
            .iter()
            .find(|value| {
                value["home"]
                    .as_str()
                    .and_then(|home| snapshot.key(home).ok())
                    .as_ref()
                    == Some(&identity)
            })
            .cloned();
        if selected.is_some() {
            return Ok((selected, None));
        }
    }
    let providers: BTreeSet<_> = best
        .iter()
        .filter_map(|value| value["provider"].as_str())
        .collect();
    let installations: Vec<_> = best.iter().map(|value| value["home"].clone()).collect();
    Ok((
        None,
        Some(
            json!({"scope":"automatic","providers":providers,"installations":installations,
            "action":"chooseInstallation"}),
        ),
    ))
}

/// Lists what a settings page shows for one tool's global and automatic layers without
/// resolving a directory: login-shell homes and the discovered installations with their
/// checked executables, versions or skip reasons, and Core's automatic choice. Hosts
/// probe only the executables returned here, then call again with `probes`.
///
/// `shell` reports each declared `environment` variable and `command` the host found
/// (`environment`, `commandHomes`). A shell home that checks out is also an entry of
/// `installations` under the `host:shell` provider, once per canonical home: it is a
/// home the resolver would run, so a picker must be able to offer it even where no
/// discovery scan covers it. `selected` stays the automatic layer's own choice.
pub(crate) fn candidates(input: &Value, validate_manifest: ManifestValidator) -> Validation<Value> {
    fields(
        input,
        &[
            "registry",
            "tool",
            "platform",
            "filesystemCaseSensitive",
            "workspaceRoot",
            "global",
            "workspaceRoots",
            "filesystem",
            "minimumVersion",
            "probes",
            "systemInstallations",
            "providerInstallations",
            "fixed",
            "projectCodeAllowed",
            "environment",
            "commandHomes",
        ],
    )?;
    let registry = definitions::registry(&input["registry"], validate_manifest)?;
    let tool = input["tool"]
        .as_str()
        .filter(|tool| lithe_plugin_model::is_tool_id(tool))
        .ok_or_else(|| reject("tool"))?;
    let definition = enabled(&registry, tool).ok_or_else(|| reject("tool"))?;
    let (snapshot, root) = prepare(input, definition)?;
    if input
        .get("projectCodeAllowed")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(reject("projectCodeAllowed"));
    }
    string_maps(input, &["environment", "commandHomes"])?;
    let applicable_candidates: Vec<_> = definition["executables"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|value| applicable(value, snapshot.platform))
        .collect();
    let project = if root.is_empty() {
        Vec::new()
    } else {
        collect_project_file_choices(&snapshot, &applicable_candidates, &root, ".")?
            .iter()
            .map(|choice| project_candidate_row(&snapshot, choice))
            .collect()
    };
    let mut shell = Vec::new();
    let mut shell_selections = Vec::new();
    for candidate in &applicable_candidates {
        // The same lookups the resolver's environment and command layers make.
        let (source, home, layout) = match candidate["kind"].as_str() {
            Some("environment") => {
                let Some(variable) = candidate["variable"].as_str() else {
                    continue;
                };
                let home = input["environment"][variable].as_str();
                (
                    json!({"variable":variable}),
                    home,
                    candidate["path"].as_str(),
                )
            }
            Some("command") => {
                let Some(command) = candidate["command"].as_str() else {
                    continue;
                };
                let home = input["commandHomes"][command].as_str();
                (json!({"command":command}), home, None)
            }
            _ => continue,
        };
        let Some(home) = home.filter(|value| !value.is_empty()) else {
            continue;
        };
        let mut row = json!({"candidateID":candidate["id"],"home":home});
        for (key, value) in source.as_object().into_iter().flatten() {
            row[key] = value.clone();
        }
        if let Ok(checked) = snapshot.checked(home, layout) {
            row["home"] = checked["home"].clone();
            row["executable"] = checked["executable"].clone();
        }
        match snapshot.layout_selection(home, layout, "host:shell", "global") {
            Ok(value) => {
                row["version"] = value["version"].clone();
                shell_selections.push(value);
            }
            Err(code) => row["reason"] = json!(code),
        }
        shell.push(row);
    }
    let mut reported = provider_installations(input, tool, &registry)?;
    reported.extend(system_installations(input, tool)?);
    let set = automatic_set(&snapshot, &reported)?;
    let mut listed = set.eligible.clone();
    for selection in shell_selections {
        let key = snapshot.key(selection["home"].as_str().unwrap_or(""))?;
        listed.entry(key).or_insert(selection);
    }
    let mut installations: Vec<_> = listed
        .values()
        .map(|value| {
            json!({"home":value["home"],"executable":value["executable"],
                "version":value["version"],"provider":value["provider"]})
        })
        .collect();
    installations.sort_by(|left, right| {
        let version = |value: &Value| value["version"].as_str().and_then(components);
        version(right)
            .cmp(&version(left))
            .then_with(|| left["home"].as_str().cmp(&right["home"].as_str()))
    });
    let mut skipped: Vec<_> = set.skipped.into_values().collect();
    if let Some(value) = input.get("fixed") {
        let fixed = FixedToolchainSelection::parse(value)?;
        let executable = match &fixed {
            FixedToolchainSelection::Installation { home } => snapshot
                .checked(home, None)
                .ok()
                .and_then(|value| value["executable"].as_str().map(str::to_owned)),
            FixedToolchainSelection::Executable { executable } => snapshot
                .fact(executable, "file")
                .ok()
                .filter(|fact| fact["executable"] == true)
                .map(|_| executable.clone()),
        };
        if let Err(code) = snapshot.fixed_selection(&fixed) {
            let mut row = json!({"reason":code});
            // A project program is not a probe target without the current action.
            if code == "probeFailed" {
                if let Some(executable) = executable {
                    row["executable"] = json!(executable);
                }
            }
            skipped.push(row);
        }
    }
    let (selected, conflict) = automatic_choice(&snapshot, set.eligible, None)?;
    Ok(
        json!({"tool":tool,"project":project,"shell":shell,"installations":installations,"skipped":skipped,
        "selected":selected.map(|value| value["home"].clone()),
        "conflicts":conflict.into_iter().collect::<Vec<_>>()}),
    )
}
