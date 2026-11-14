//! Appending patterns to `.gitignore` and `.git/info/exclude`.

use super::*;

/// Chooses whether ignore patterns belong to the shared repository file or the
/// current checkout's local Git metadata.
pub(crate) enum GitIgnoreTarget {
    Repository,
    LocalExclude,
}

pub(crate) fn append_git_ignore_patterns(
    root: &str,
    paths: &[String],
    target: GitIgnoreTarget,
) -> Result<GitCommandResponse, CoreError> {
    let patterns = git_ignore_patterns(paths)?;
    let target_path = match target {
        GitIgnoreTarget::Repository => repository_root(root)?.join(".gitignore"),
        GitIgnoreTarget::LocalExclude => git_path(root, "info/exclude")?,
    };
    let existing = read_git_ignore_bytes(&target_path)?;
    let existing_text = String::from_utf8_lossy(&existing);
    let additions = patterns
        .into_iter()
        .filter(|pattern| !existing_text.lines().any(|line| line == pattern))
        .collect::<Vec<_>>();
    if additions.is_empty() {
        return Ok(successful_git_result());
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|error| git_ignore_io_error("create", error))?;
    }
    let mut appended = String::new();
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        appended.push('\n');
    }
    for pattern in additions {
        appended.push_str(&pattern);
        appended.push('\n');
    }
    fs::append(&target_path, appended.as_bytes())
        .map_err(|error| git_ignore_io_error("write", error))?;
    Ok(successful_git_result())
}

fn read_git_ignore_bytes(path: &Path) -> Result<Vec<u8>, CoreError> {
    match fs::read(path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(git_ignore_io_error("read", error)),
    }
}

fn git_ignore_patterns(paths: &[String]) -> Result<Vec<String>, CoreError> {
    let mut patterns = Vec::with_capacity(paths.len());
    for path in paths {
        if path.contains(['\0', '\n', '\r']) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Git ignore operation contains an invalid path",
            ));
        }
        let normalized = path.replace('\\', "/");
        let is_directory = normalized.ends_with('/');
        let normalized = normalized.trim_end_matches('/');
        if !is_safe_pathspec(normalized) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Git ignore operation contains an invalid path",
            ));
        }

        let mut escaped = String::with_capacity(normalized.len() + 2);
        escaped.push('/');
        for character in normalized.chars() {
            if matches!(character, '*' | '?' | '[' | ']' | '#' | '!' | ' ') {
                escaped.push('\\');
            }
            escaped.push(character);
        }
        if is_directory {
            escaped.push('/');
        }
        patterns.push(escaped);
    }
    patterns.sort();
    patterns.dedup();
    if patterns.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git ignore operation contains an invalid path",
        ));
    }
    Ok(patterns)
}

fn git_ignore_io_error(action: &str, error: std::io::Error) -> CoreError {
    let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::Unknown
    };
    CoreError::new(code, format!("Could not {action} Git ignore file"))
        .with_details(error.to_string())
}
