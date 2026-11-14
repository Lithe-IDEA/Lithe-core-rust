//! Reading the configuration layers and normalizing their directories.

use super::*;

pub(crate) fn local_layer_document(
    root: &Path,
    provided: Option<Value>,
) -> Result<Value, CoreError> {
    if let Some(mut document) = provided {
        // A host-owned local layer may still carry the v1 shape, mirroring the
        // migration applied to the on-disk document below.
        migrate_document_value(&mut document);
        validate_version_value(&document)?;
        configuration_ids(&document)?;
        return Ok(document);
    }
    Ok(read_document_value(root, "run/local.json")?
        .unwrap_or_else(|| json!({"version": VERSION, "configurations": []})))
}

pub(crate) fn scope_document(scope: &str) -> Result<&'static str, CoreError> {
    match scope {
        "local" => Ok("run/local.json"),
        "project" => Ok("run/configurations.json"),
        _ => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration scope must be local or project",
        )),
    }
}

/// Validates the literal value of a `cwd` override before it is persisted.
///
/// Resolution disables a configuration whose `cwd` is missing and hides it from
/// the run list, so a directory that does not exist must fail here, where the
/// editor can show the error, instead of making the saved entry disappear.
/// Values are not expanded: `${workspaceFolder}` style variables from other
/// editors would otherwise be stored as a literal directory name.
pub(crate) fn normalize_working_directory(root: &Path, value: &str) -> Result<String, CoreError> {
    normalize_project_directory(root, value).map_err(|error| {
        if value.contains('$') && !root.join(value).is_dir() {
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
    let candidate = Path::new(value);
    if (!candidate.is_absolute() && invalid_relative_path(value))
        || candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Project configuration paths must stay inside the project",
        ));
    }
    let canonical_root = fs::canonicalize(root)?;
    let target = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let canonical_target = fs::canonicalize(&target).map_err(|error| {
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
        Ok(relative.to_string_lossy().replace('\\', "/"))
    }
}

pub(crate) fn project_directory_exists(root: &Path, value: &str) -> bool {
    if invalid_relative_path(value) {
        return false;
    }
    let Ok(canonical_root) = fs::canonicalize(root) else {
        return false;
    };
    let Ok(canonical_target) = fs::canonicalize(root.join(value)) else {
        return false;
    };
    canonical_target.is_dir() && canonical_target.starts_with(canonical_root)
}

pub(crate) fn read_document_value(root: &Path, relative: &str) -> Result<Option<Value>, CoreError> {
    let path = root.join(".lithe").join(relative);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|e| {
        CoreError::new(
            ErrorCode::PermissionDenied,
            "Could not read run configuration",
        )
        .with_details(e.to_string())
    })?;
    let mut value: Value = serde_json::from_str(&text).map_err(|e| {
        CoreError::new(
            ErrorCode::ParseFailed,
            format!("Configuration JSON is invalid: .lithe/{relative}"),
        )
        .with_details(e.to_string())
    })?;
    // Only run/* documents carry the run-configuration schema. project.json and
    // the toolchain files share the .lithe root and a version field, but their
    // schema is unrelated -- migrating them would bump a version they never own.
    if relative.starts_with("run/") {
        migrate_document_value(&mut value);
    }
    Ok(Some(value))
}

/// Ids of the configurations this session detected for `root`; a user
/// configuration must not reuse one.
pub(crate) fn detected_ids(root: &Path) -> BTreeSet<String> {
    generated_layer::get(root)
        .map(|layer| {
            layer
                .document
                .configurations
                .iter()
                .map(|configuration| configuration.id.clone())
                .collect()
        })
        .unwrap_or_default()
}
