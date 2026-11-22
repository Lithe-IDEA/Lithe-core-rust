//! Reading the sidecar documents and normalizing configuration directories.

use super::*;

/// Validates the literal value of a `cwd` override before it is persisted.
///
/// Resolution disables a configuration whose `cwd` is missing and hides it from
/// the run list, so a directory that does not exist must fail here, where the
/// editor can show the error, instead of making the saved entry disappear.
/// Values are not expanded: `${workspaceFolder}` style variables from other
/// editors would otherwise be stored as a literal directory name.
pub(crate) fn normalize_working_directory(root: &Path, value: &str) -> Result<String, CoreError> {
    normalize_project_directory(root, value).map_err(|error| {
        if value.contains('$') && !workspace_fs::is_dir(root.join(value)).unwrap_or(false) {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Working directory variables are not supported; use a path relative to the project",
            )
            .with_details(value)
        } else {
            error
        }
    })
}

pub(crate) fn normalize_project_directory(root: &Path, value: &str) -> Result<String, CoreError> {
    let platform = run_platform();
    let absolute = lithe_platform::paths::is_absolute_path(value, platform);
    let candidate = Path::new(value);
    if (!absolute && invalid_native_relative_path(value, platform))
        || candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Project configuration paths must stay inside the project",
        ));
    }
    let canonical_root = workspace_fs::canonicalize(root)?;
    let target = if absolute {
        process::file_path(root, value, platform)
    } else {
        root.join(candidate)
    };
    let canonical_target = workspace_fs::canonicalize(&target).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Project configuration directory does not exist",
        )
        .with_details(format!("{}: {error}", target.display()))
    })?;
    let relative = canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|_| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Project configuration paths must stay inside the project",
            )
        })?;
    if relative.as_os_str().is_empty() {
        Ok(".".to_string())
    } else {
        Ok(slash_separated_path(relative, Platform::HOST))
    }
}

pub(crate) fn project_directory_exists(root: &Path, value: &str) -> bool {
    if invalid_native_relative_path(value, run_platform()) {
        return false;
    }
    let Ok(canonical_root) = workspace_fs::canonicalize(root) else {
        return false;
    };
    let Ok(canonical_target) = workspace_fs::canonicalize(root.join(value)) else {
        return false;
    };
    canonical_target.starts_with(canonical_root)
        && workspace_fs::is_dir(&canonical_target).unwrap_or(false)
}

/// Reads a sidecar document below `.lithe`, such as `project.json`, with the
/// hash of its bytes. Sidecars keep their own schema and are never migrated.
pub(crate) fn read_sidecar(
    root: &Path,
    relative: &str,
) -> Result<Option<(Value, String)>, CoreError> {
    let Some((text, sha256)) = super::storage::read_file(root, relative)? else {
        return Ok(None);
    };
    let value = serde_json::from_str(&text).map_err(|e| {
        CoreError::new(
            ErrorCode::ParseFailed,
            format!("Configuration JSON is invalid: .lithe/{relative}"),
        )
        .with_details(e.to_string())
    })?;
    Ok(Some((value, sha256)))
}

/// Ids of the configurations this session generated for `root`; a user
/// configuration must not reuse one.
pub(crate) fn generated_ids(root: &Path) -> BTreeSet<String> {
    generated_layer::get(root)
        .map(|document| {
            document
                .configurations
                .iter()
                .map(|configuration| configuration.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn existing_root(value: &str) -> Result<PathBuf, CoreError> {
    let path = PathBuf::from(value);
    if let Some(host) = lithe_workspace::current() {
        if host.local_workspace_path("").as_deref() != Some(path.as_path()) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Run configuration root does not match the remote workspace",
            ));
        }
    }
    match workspace_fs::is_dir(&path) {
        Ok(true) => {}
        Err(error) if workspace_fs::is_transport_failure(&error) => {
            return Err(CoreError::from(error))
        }
        Ok(false) | Err(_) => {
            return Err(CoreError::new(
                ErrorCode::WorkspaceNotFound,
                "Project root does not exist",
            ));
        }
    }
    Ok(path)
}

pub(crate) fn read_local_toolchains(root: &Path) -> Result<Option<Value>, CoreError> {
    Ok(read_local_toolchains_file(root)?.map(|(document, _)| document))
}

/// The local toolchains document with the hash of its bytes, validated.
pub(crate) fn read_local_toolchains_file(
    root: &Path,
) -> Result<Option<(Value, String)>, CoreError> {
    let Some((document, sha256)) = read_sidecar(root, "toolchains/local.json")? else {
        return Ok(None);
    };
    validate_sidecar_version_value(&document)?;
    if document
        .get("toolchains")
        .and_then(Value::as_object)
        .is_none()
    {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Local toolchains must contain a toolchains object",
        ));
    }
    Ok(Some((document, sha256)))
}
