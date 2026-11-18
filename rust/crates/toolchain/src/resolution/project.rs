//! Project-level toolchain choices and resolution preparation.

use super::*;

/// Project sources are merged before attempting a candidate; distance wins over kind.
pub(crate) struct ProjectChoice {
    pub(crate) anchor: String,
    pub(crate) category: u8,
    pub(crate) native: bool,
    pub(crate) value: Value,
    pub(crate) candidate: bool,
}

pub(crate) fn ancestors(directory: &str) -> Vec<&str> {
    let mut result = vec![directory];
    let mut current = directory;
    while current != "." && result.len() < 64 {
        current = current.rsplit_once('/').map_or(".", |(parent, _)| parent);
        result.push(current);
    }
    result
}

/// Collects `projectFile` candidates present along `ancestors(directory)` without
/// probing or granting execution authority.
pub(crate) fn collect_project_file_choices(
    snapshot: &Snapshot<'_>,
    candidates: &[&Value],
    root: &str,
    directory: &str,
) -> Validation<Vec<ProjectChoice>> {
    let mut choices = Vec::new();
    for source in candidates {
        if source["kind"] != "projectFile" {
            continue;
        }
        for ancestor in ancestors(directory) {
            let anchor = if ancestor == "." {
                root.to_owned()
            } else {
                paths::join(root, ancestor, snapshot.platform).ok_or_else(|| reject("directory"))?
            };
            let native = paths::join(
                &anchor,
                source["path"].as_str().unwrap_or(""),
                snapshot.platform,
            )
            .ok_or_else(|| reject("projectFile"))?;
            if snapshot.fact(&native, "file").is_err() {
                continue;
            }
            let markers = source["requires"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|marker| {
                    marker
                        .as_str()
                        .and_then(|marker| paths::join(&anchor, marker, snapshot.platform))
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
                    paths::join(&anchor, home, snapshot.platform)
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
    Ok(choices)
}

/// Validates authorization, directory containment, marker containment, executable
/// evidence and `pathPrepend` safety for one `projectFile` candidate before any probe.
pub(crate) fn validate_project_candidate<'a>(
    snapshot: &Snapshot<'_>,
    choice: &'a ProjectChoice,
) -> Result<(&'a str, Option<&'a str>), &'static str> {
    if snapshot.input["projectCodeAllowed"] != true {
        return Err("projectCodeRequiresAction");
    }
    let path = choice.value["nativePath"].as_str().ok_or("unsafePath")?;
    let containing_root = snapshot
        .roots
        .iter()
        .find(|root| {
            paths::within(path, root, snapshot.platform, snapshot.case_sensitive)
                && paths::within(
                    &choice.anchor,
                    root,
                    snapshot.platform,
                    snapshot.case_sensitive,
                )
        })
        .ok_or("unsafePath")?;
    let anchor_fact = snapshot
        .fact(&choice.anchor, "directory")
        .map_err(|_| "unsafePath")?;
    let canonical_anchor = anchor_fact["canonicalPath"].as_str().ok_or("unsafePath")?;
    if !paths::within(
        canonical_anchor,
        containing_root,
        snapshot.platform,
        snapshot.case_sensitive,
    ) {
        return Err("unsafePath");
    }
    let rel_path = choice.value["path"].as_str().ok_or("unsafePath")?;
    let home = choice.value["nativeHome"].as_str();
    if let Some(home) = home {
        if !paths::within(path, home, snapshot.platform, snapshot.case_sensitive) {
            return Err("unsafePath");
        }
        let fact = snapshot.fact(home, "directory").map_err(|_| "unsafePath")?;
        let canonical_home = fact["canonicalPath"].as_str().ok_or("unsafePath")?;
        if snapshot.key(canonical_home).map_err(|_| "unsafePath")?
            != snapshot.key(home).map_err(|_| "unsafePath")?
            || !paths::within(
                canonical_home,
                containing_root,
                snapshot.platform,
                snapshot.case_sensitive,
            )
        {
            return Err("unsafePath");
        }
        // Every intermediate parent directory between `home` and the leaf executable
        // (such as `.venv/bin` or `.venv/Scripts`) must be an observed directory whose
        // canonical target remains inside `home` and the workspace root.
        let mut prefix = String::new();
        let segments: Vec<&str> = rel_path.split('/').collect();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            let dir =
                paths::join(&choice.anchor, &prefix, snapshot.platform).ok_or("unsafePath")?;
            if paths::within(&dir, home, snapshot.platform, snapshot.case_sensitive) {
                let dir_fact = snapshot.fact(&dir, "directory").map_err(|_| "unsafePath")?;
                let canonical_dir = dir_fact["canonicalPath"].as_str().ok_or("unsafePath")?;
                if !paths::within(
                    canonical_dir,
                    canonical_home,
                    snapshot.platform,
                    snapshot.case_sensitive,
                ) || !paths::within(
                    canonical_dir,
                    containing_root,
                    snapshot.platform,
                    snapshot.case_sensitive,
                ) {
                    return Err("unsafePath");
                }
            }
        }
    } else {
        // Any observed parent directory above a wrapper must remain within the workspace.
        let mut prefix = String::new();
        let segments: Vec<&str> = rel_path.split('/').collect();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            let dir =
                paths::join(&choice.anchor, &prefix, snapshot.platform).ok_or("unsafePath")?;
            if let Ok(dir_fact) = snapshot.fact(&dir, "directory") {
                let canonical_dir = dir_fact["canonicalPath"].as_str().ok_or("unsafePath")?;
                if !paths::within(
                    canonical_dir,
                    containing_root,
                    snapshot.platform,
                    snapshot.case_sensitive,
                ) {
                    return Err("unsafePath");
                }
            }
        }
        if snapshot.definition["launch"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|token| token == "${home}")
        {
            return Err("homeUnavailable");
        }
    }
    for marker in choice.value["requires"].as_array().into_iter().flatten() {
        let marker_rel = marker.as_str().ok_or("unsafePath")?;
        let marker_path =
            paths::join(&choice.anchor, marker_rel, snapshot.platform).ok_or("unsafePath")?;
        let marker_fact = snapshot
            .fact(&marker_path, "file")
            .map_err(|_| "notFound")?;
        let canonical_marker = marker_fact["canonicalPath"].as_str().ok_or("unsafePath")?;
        if !paths::within(
            canonical_marker,
            containing_root,
            snapshot.platform,
            snapshot.case_sensitive,
        ) {
            return Err("unsafePath");
        }
        if let Some(home) = home {
            if paths::within(
                &marker_path,
                home,
                snapshot.platform,
                snapshot.case_sensitive,
            ) && !paths::within(
                canonical_marker,
                home,
                snapshot.platform,
                snapshot.case_sensitive,
            ) {
                return Err("unsafePath");
            }
        }
    }
    let file = snapshot.fact(path, "file").map_err(|_| "notFound")?;
    if file["executable"] != true {
        return Err("notExecutable");
    }
    let canonical = file["canonicalPath"].as_str().ok_or("unsafePath")?;
    let target = snapshot.fact(canonical, "file").map_err(|_| "unsafePath")?;
    if target["executable"] != true {
        return Err("notExecutable");
    }
    let target_canonical = target["canonicalPath"].as_str().ok_or("unsafePath")?;
    if snapshot.key(target_canonical).map_err(|_| "unsafePath")?
        != snapshot.key(canonical).map_err(|_| "unsafePath")?
    {
        return Err("unsafePath");
    }
    // Wrappers (no `home`) must resolve inside the workspace root; project environments
    // (`home` present, such as `.venv`) may link their leaf interpreter to an external
    // base runtime while keeping their environment root and parent directories contained.
    if home.is_none()
        && !paths::within(
            canonical,
            containing_root,
            snapshot.platform,
            snapshot.case_sensitive,
        )
    {
        return Err("unsafePath");
    }
    // Verify `exports` and `pathPrepend` before probing so broken environments never run.
    snapshot.environment_selection(home, path, None, "host:project", "project", true)?;
    Ok((path, home))
}

pub(crate) fn project_selection(
    snapshot: &Snapshot<'_>,
    choice: &ProjectChoice,
) -> Result<Value, &'static str> {
    if !choice.candidate {
        let home = choice.value["installation"]
            .as_str()
            .ok_or("requestedNotInstalled")?;
        return snapshot.selection(
            home,
            choice.value["provider"].as_str().unwrap_or(""),
            "project",
        );
    }
    let (path, home) = validate_project_candidate(snapshot, choice)?;
    // Project environments require a probe on the lexical interpreter path; wrappers only
    // require a probe when the caller requested a `minimumVersion`.
    let version = if home.is_some() {
        Some(snapshot.version(path)?)
    } else {
        match snapshot.version(path) {
            Ok(version) => Some(version),
            Err(code) if snapshot.minimum.is_some() => return Err(code),
            Err(_) => None,
        }
    };
    // Preserve the lexical interpreter path so virtual environments resolve `pyvenv.cfg`
    // and `sys.prefix` relative to the project environment rather than the base runtime.
    snapshot.environment_selection(
        home,
        path,
        version.as_deref(),
        "host:project",
        "project",
        true,
    )
}

/// Formats one discovered `projectFile` candidate for `candidates.project` and
/// `resolve.projectCandidates`, emitting `executable` only after authorization and
/// containment checks pass.
pub(crate) fn project_candidate_row(snapshot: &Snapshot<'_>, choice: &ProjectChoice) -> Value {
    let id = choice.value["id"].as_str().unwrap_or("projectFile");
    let project_kind = choice
        .value
        .get("projectKind")
        .and_then(Value::as_str)
        .unwrap_or("wrapper");
    let mut row = json!({
        "candidateID": id,
        "projectKind": project_kind,
        "path": choice.value["nativePath"],
    });
    if let Some(home) = choice.value["nativeHome"].as_str() {
        row["home"] = json!(home);
    }
    match validate_project_candidate(snapshot, choice) {
        Ok((executable, _)) => {
            row["executable"] = json!(executable);
            match project_selection(snapshot, choice) {
                Ok(selection) => {
                    if let Some(version) = selection["version"].as_str() {
                        row["version"] = json!(version);
                    }
                }
                Err(code) => {
                    row["reason"] = json!(code);
                }
            }
        }
        Err(code) => {
            row["reason"] = json!(code);
        }
    }
    row
}

/// Validates the native evidence shared by resolution and candidate listing: platform
/// and case policy, workspace roots, filesystem facts and bounded probe outputs.
pub(crate) fn prepare<'a>(
    input: &'a Value,
    definition: &'a Value,
) -> Validation<(Snapshot<'a>, String)> {
    let platform = input["platform"]
        .as_str()
        .filter(|platform| matches!(*platform, "macos" | "windows" | "linux"))
        .ok_or_else(|| reject("platform"))?;
    let case_sensitive = input
        .get("filesystemCaseSensitive")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| reject("filesystemCaseSensitive"))
        })
        .transpose()?
        .unwrap_or(platform != "windows");
    let global = input
        .get("global")
        .map(|value| value.as_bool().ok_or_else(|| reject("global")))
        .transpose()?
        .unwrap_or(false);
    let root = if global {
        if input["workspaceRoot"] != "" {
            return Err(reject("workspaceRoot"));
        }
        String::new()
    } else {
        input["workspaceRoot"]
            .as_str()
            .and_then(|root| paths::normalize(root, platform, true))
            .ok_or_else(|| reject("workspaceRoot"))?
    };
    let mut roots = if global {
        Vec::new()
    } else {
        vec![root.clone()]
    };
    for value in array(input, "workspaceRoots", 64)? {
        roots.push(
            value
                .as_str()
                .ok_or_else(|| reject("workspaceRoots"))?
                .to_owned(),
        );
    }
    let facts = array(input, "filesystem", 16384)?;
    let minimum = input
        .get("minimumVersion")
        .map(|value| {
            value
                .as_str()
                .filter(|value| components(value).is_some())
                .ok_or_else(|| reject("minimumVersion"))
        })
        .transpose()?;
    let mut probes = BTreeMap::new();
    for value in array(input, "probes", 1024)? {
        let probe: Probe = serde_json::from_value(value.clone()).map_err(|_| reject("probes"))?;
        if probe.output.len() > 128 * 1024 {
            return Err(reject("probes.output"));
        }
        let key = paths::normalize(&probe.executable, platform, case_sensitive)
            .ok_or_else(|| reject("probes.executable"))?;
        if probes.insert(key, probe).is_some() {
            return Err(reject("probes"));
        }
    }
    let mut snapshot = Snapshot {
        input,
        definition,
        platform,
        case_sensitive,
        roots,
        facts,
        targets: Vec::new(),
        probes,
        minimum,
    };
    snapshot.validate_facts()?;
    snapshot.targets = query::implied_targets(facts, platform, case_sensitive)
        .map_err(|_| reject("filesystem"))?;
    if !global
        && snapshot.key(
            snapshot.fact(&root, "directory")?["canonicalPath"]
                .as_str()
                .ok_or_else(|| reject("workspaceRoot"))?,
        )? != snapshot.key(&root)?
    {
        return Err(reject("workspaceRoot"));
    }
    Ok((snapshot, root))
}
