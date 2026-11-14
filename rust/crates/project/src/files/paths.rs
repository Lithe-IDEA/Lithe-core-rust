//! Resolving and checking workspace-relative paths.

use super::*;

pub(crate) fn existing_root(value: &str) -> Result<PathBuf, CoreError> {
    let path = PathBuf::from(value);
    let metadata = fs::metadata(&path)
        .map_err(|_| CoreError::new(ErrorCode::WorkspaceNotFound, "Workspace does not exist"))?;
    if !metadata.is_dir() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Workspace root must be a directory",
        ));
    }
    fs::canonicalize(path).map_err(CoreError::from)
}

pub(crate) fn safe_relative_path(root: &Path, value: &str) -> Result<PathBuf, CoreError> {
    let relative = Path::new(value);
    if relative.is_absolute() || invalid_relative_path(value) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Path must be relative to the workspace",
        ));
    }
    let path = root.join(relative);
    let canonical = fs::canonicalize(path).map_err(CoreError::from)?;
    if !canonical.starts_with(root) {
        return Err(CoreError::new(
            ErrorCode::PermissionDenied,
            "Path is outside the workspace",
        ));
    }
    Ok(canonical)
}

pub(crate) fn writable_relative_path(root: &Path, value: &str) -> Result<PathBuf, CoreError> {
    let relative = Path::new(value);
    if relative.is_absolute() || invalid_relative_path(value) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Path must be relative to the workspace",
        ));
    }
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        let mut existing_parent = parent;
        while !fs::try_exists(existing_parent)? {
            existing_parent = existing_parent.parent().ok_or_else(|| {
                CoreError::new(ErrorCode::PermissionDenied, "Path is outside the workspace")
            })?;
        }
        let canonical_parent = fs::canonicalize(existing_parent).map_err(CoreError::from)?;
        if !canonical_parent.starts_with(root) {
            return Err(CoreError::new(
                ErrorCode::PermissionDenied,
                "Path is outside the workspace",
            ));
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() {
            return Err(CoreError::new(
                ErrorCode::PermissionDenied,
                "Path is outside the workspace",
            ));
        }
        let canonical = fs::canonicalize(&path).map_err(CoreError::from)?;
        if !canonical.starts_with(root) {
            return Err(CoreError::new(
                ErrorCode::PermissionDenied,
                "Path is outside the workspace",
            ));
        }
    }
    Ok(path)
}

pub(crate) fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
        .trim_matches('/')
        .to_string()
}

pub(crate) fn safe_relative_path_string(value: &str) -> Result<String, CoreError> {
    let path = Path::new(value);
    if path.is_absolute() || invalid_relative_path(value) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Path must be relative to the workspace",
        ));
    }
    Ok(value.replace('\\', "/").trim_matches('/').to_string())
}
