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

/// `value` in the shared `/`-separated form, or an error when it does not
/// name a path below the workspace root on this host.
///
/// Requests may still spell a Windows path with `\`; [`slash_separated`]
/// turns only this host's separators into `/`.
fn relative_request_path(value: &str) -> Result<std::borrow::Cow<'_, str>, CoreError> {
    let value = slash_separated(value, Platform::HOST);
    if invalid_relative_path(&value, Platform::HOST) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Path must be relative to the workspace",
        ));
    }
    Ok(value)
}

pub(crate) fn safe_relative_path(root: &Path, value: &str) -> Result<PathBuf, CoreError> {
    let path = root.join(relative_request_path(value)?.as_ref());
    let canonical = fs::canonicalize(path).map_err(CoreError::from)?;
    if !canonical.starts_with(root) {
        return Err(CoreError::new(
            ErrorCode::PermissionDenied,
            "Path is outside the workspace",
        ));
    }
    Ok(canonical)
}

/// Shares unsaved snapshots with internal aliases without copying their contents.
pub(crate) fn canonical_text_overrides<'a>(
    root: &Path,
    overrides: impl IntoIterator<Item = (&'a String, &'a String)>,
    follow_links: bool,
) -> HashMap<PathBuf, &'a str> {
    let mut result = HashMap::new();
    if follow_links {
        let mut paths: Vec<_> = overrides.into_iter().collect();
        paths.sort_by_key(|(path, _)| *path);
        for (path, text) in paths {
            if let Ok(canonical) = safe_relative_path(root, path) {
                result.entry(canonical).or_insert(text.as_str());
            }
        }
    }
    result
}

pub(crate) fn writable_relative_path(root: &Path, value: &str) -> Result<PathBuf, CoreError> {
    let path = root.join(relative_request_path(value)?.as_ref());
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

/// `path` relative to `root` in the shared `/`-separated form. Only this
/// host's separators become `/`, so a Unix name containing `\` stays one
/// component.
pub(crate) fn relative_path(path: &Path, root: &Path) -> String {
    slash_separated_path(path.strip_prefix(root).unwrap_or(path), Platform::HOST)
        .trim_matches('/')
        .to_string()
}

pub(crate) fn safe_relative_path_string(value: &str) -> Result<String, CoreError> {
    Ok(relative_request_path(value)?.trim_matches('/').to_string())
}
