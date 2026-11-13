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
    if snapshot.input["projectCodeAllowed"] != true {
        return Err("projectCodeRequiresAction");
    }
    let path = choice.value["nativePath"].as_str().ok_or("unsafePath")?;
    let file = snapshot.fact(path, "file").map_err(|_| "notFound")?;
    if file["executable"] != true {
        return Err("notExecutable");
    }
    let canonical = file["canonicalPath"].as_str().ok_or("unsafePath")?;
    if snapshot.key(canonical).map_err(|_| "unsafePath")?
        != snapshot.key(path).map_err(|_| "unsafePath")?
    {
        // Native link-chain identities are not available in this Core-only slice.
        return Err("unsafePath");
    }
    let home = choice.value["nativeHome"].as_str();
    if let Some(home) = home {
        let fact = snapshot.fact(home, "directory").map_err(|_| "unsafePath")?;
        let canonical_home = fact["canonicalPath"].as_str().ok_or("unsafePath")?;
        if snapshot.key(canonical_home).map_err(|_| "unsafePath")?
            != snapshot.key(home).map_err(|_| "unsafePath")?
            || !snapshot.roots.iter().any(|root| {
                paths::within(
                    canonical_home,
                    root,
                    snapshot.platform,
                    snapshot.case_sensitive,
                )
            })
        {
            return Err("unsafePath");
        }
    }
    // A lexical project path must never turn into a provider-owned installation.
    if !snapshot
        .roots
        .iter()
        .any(|root| paths::within(path, root, snapshot.platform, snapshot.case_sensitive))
    {
        return Err("unsafePath");
    }
    if home.is_none()
        && snapshot.definition["launch"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|token| token == "${home}")
    {
        return Err("homeUnavailable");
    }
    // Preserve the lexical interpreter path for project environments (including venv).
    let version = match snapshot.version(path) {
        Ok(version) => Some(version),
        Err(code) if snapshot.minimum.is_some() => return Err(code),
        Err(_) => None,
    };
    snapshot.environment_selection(
        home,
        path,
        version.as_deref(),
        "host:project",
        "project",
        true,
    )
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
    let root = input["workspaceRoot"]
        .as_str()
        .and_then(|root| paths::normalize(root, platform, true))
        .ok_or_else(|| reject("workspaceRoot"))?;
    let mut roots = vec![root.clone()];
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
    if snapshot.key(
        snapshot.fact(&root, "directory")?["canonicalPath"]
            .as_str()
            .ok_or_else(|| reject("workspaceRoot"))?,
    )? != snapshot.key(&root)?
    {
        return Err(reject("workspaceRoot"));
    }
    Ok((snapshot, root))
}
