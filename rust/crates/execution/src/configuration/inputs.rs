//! Project inputs that fingerprint generated configurations.

use super::*;

/// Collects generation inputs; every input is hashed by content.
pub(crate) struct ProjectInputs {
    pub(crate) inputs: BTreeMap<String, String>,
    pub(crate) unreadable_paths: BTreeSet<String>,
}

// Retention is independent of filesystem permissions so both hosts preserve the
// same last-known configuration when an optional subtree becomes unreadable.
pub(crate) fn retain_unknown_inputs(
    configurations: &mut Vec<RunConfiguration>,
    snapshot: &mut ProjectInputs,
    previous: RunConfigurationDocument,
) {
    for configuration in previous.configurations {
        let source = configuration.source.as_deref();
        if source.is_some_and(|path| under_unknown(path, &snapshot.unreadable_paths))
            && !configurations
                .iter()
                .any(|current| current.id == configuration.id)
        {
            configurations.push(configuration);
        }
    }
    if let Some(metadata) = previous.generator {
        for (path, hash) in metadata.inputs {
            if under_unknown(&path, &snapshot.unreadable_paths) {
                snapshot.inputs.entry(path).or_insert(hash);
            }
        }
    }
}

pub(crate) fn under_unknown(path: &str, unknown: &BTreeSet<String>) -> bool {
    unknown
        .iter()
        .any(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")))
}

pub(crate) fn incomplete_scan_diagnostic(paths: &BTreeSet<String>) -> Value {
    json!({"code": "incompleteProjectScan", "message": format!(
        "Project scan is incomplete; freshness and removals could not be verified. Unreadable inputs: {}",
        paths.iter().cloned().collect::<Vec<_>>().join(", "))})
}

pub(crate) fn project_inputs(root: &Path) -> Result<ProjectInputs, CoreError> {
    let scan = crate::workspace_scan::scan_run_workspace(root, None)?;
    let mut result = ProjectInputs {
        inputs: BTreeMap::new(),
        unreadable_paths: scan.unreadable_paths,
    };
    for directory in scan.directories {
        for name in directory.files {
            let path = directory.path.join(name);
            if !fingerprint_input(&path) {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("scan stays within root")
                .to_string_lossy()
                .replace('\\', "/");
            lithe_protocol::cancellation::check()?;
            // The scan lists files through the workspace adapter, so content reads
            // must use the same adapter or remote projects look permanently unreadable.
            match lithe_workspace::fs::read(&path) {
                Ok(bytes) => {
                    result
                        .inputs
                        .insert(relative, format!("sha256:{:x}", Sha256::digest(bytes)));
                }
                Err(_) => {
                    result.unreadable_paths.insert(relative);
                }
            }
        }
    }
    Ok(result)
}

pub(crate) fn fingerprint_from_inputs(inputs: &BTreeMap<String, String>) -> String {
    let mut digest = Sha256::new();
    // Detection changes invalidate persisted output even when project files are
    // unchanged, so an application upgrade cannot keep launching a stale plan.
    digest.update(GENERATOR_REVISION.as_bytes());
    digest.update([0]);
    for (relative, content_hash) in inputs {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(content_hash.as_bytes());
        digest.update([0]);
    }
    format!("sha256:{:x}", digest.finalize())
}

pub(crate) fn input_change_summary(
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> String {
    let added = current
        .keys()
        .filter(|path| !previous.contains_key(*path))
        .count();
    let removed = previous
        .keys()
        .filter(|path| !current.contains_key(*path))
        .count();
    let changed = current
        .iter()
        .filter(|(path, hash)| previous.get(*path).is_some_and(|old| old != *hash))
        .count();
    format!("Project inputs changed: {added} added, {removed} removed, {changed} modified")
}

fn fingerprint_input(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(
            "mise.toml"
                | "package.json"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "bun.lock"
                | "bun.lockb"
                | "docker-compose.yml"
                | "docker-compose.yaml"
                | "compose.yml"
                | "compose.yaml"
                | "pyproject.toml"
                | "manage.py"
                | "Cargo.toml"
                | "rust-toolchain"
                | "rust-toolchain.toml"
                | "go.mod"
                | "Procfile"
                | "Procfile.dev"
                | "Procfile.local"
                | "Makefile"
                | "makefile"
                | "GNUmakefile"
                | "justfile"
                | "Justfile"
                | ".justfile"
        )
    )
}

pub(crate) fn declared_node_version(root: &Path) -> Option<String> {
    highest_version(
        project_manifest_paths(root, &["package.json"])
            .into_iter()
            .filter_map(|path| lithe_workspace::fs::read_to_string(path).ok())
            .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
            .filter_map(|document| {
                document
                    .get("engines")?
                    .get("node")?
                    .as_str()
                    .and_then(first_numeric_version)
            })
            .collect(),
    )
}

fn first_numeric_version(value: &str) -> Option<String> {
    regex::Regex::new(r"[0-9]+(?:\.[0-9]+)+")
        .ok()?
        .find(value)
        .map(|value| value.as_str().to_string())
}

fn highest_version(versions: Vec<String>) -> Option<String> {
    versions
        .into_iter()
        .max_by(|left, right| version_parts(left).cmp(&version_parts(right)))
}

fn project_manifest_paths(root: &Path, names: &[&str]) -> Vec<PathBuf> {
    // This heuristic only supplements declared toolchain requirements. Fingerprint
    // scanning separately reports unknown subtrees to both product hosts.
    let Ok(scan) = crate::workspace_scan::scan_run_workspace(root, None) else {
        return Vec::new();
    };
    scan.directories
        .into_iter()
        .flat_map(|directory| {
            directory
                .files
                .into_iter()
                .filter(|name| names.contains(&name.as_str()))
                .map(move |name| directory.path.join(name))
        })
        .collect()
}

pub(crate) fn existing_root(value: &str) -> Result<PathBuf, CoreError> {
    let path = PathBuf::from(value);
    if !path.is_dir() {
        return Err(CoreError::new(
            ErrorCode::WorkspaceNotFound,
            "Project root does not exist",
        ));
    }
    Ok(path)
}
