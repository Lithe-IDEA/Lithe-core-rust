//! Stash listing, apply and pop, and pulls that stash local changes around the update.

use super::*;

/// Lists stashes with stable references and parsed metadata.
pub fn stashes(request: GitStashesRequest) -> Result<GitStashesResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let response = readonly_command(GitCommandRequest {
        root,
        arguments: vec![
            "stash".to_string(),
            "list".to_string(),
            // `--date` would also turn `%gd` into `stash@{<date>}`, so the date is asked for in
            // its own ISO form and the reference stays the positional `stash@{N}`.
            "--pretty=format:%gd%x1f%gs%x1f%ai%x1f%H".to_string(),
        ],
        input: None,
    })?;
    if response.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git stash list failed")
                .with_details(response.output),
        );
    }
    Ok(GitStashesResponse {
        stashes: response.output.lines().filter_map(parse_stash).collect(),
    })
}

pub(crate) fn pop_stash(root: &str, reference: &str) -> Result<GitCommandResponse, CoreError> {
    let mut result = execute_git(
        root,
        &["stash".into(), "pop".into(), reference.to_string()],
        None,
    )?;
    let conflicted_paths = conflicted_paths(root)?;
    let entry_was_kept = stash_reference_exists(root, reference)?;
    if entry_was_kept && (!conflicted_paths.is_empty() || result.exit_code == 0) {
        result.stash_restore = Some(GitStashRestoreResponse {
            stash_reference: reference.to_string(),
            conflicted_paths,
        });
    }
    Ok(result)
}

pub(crate) fn apply_stash(root: &str, reference: &str) -> Result<GitCommandResponse, CoreError> {
    let mut result = execute_git(
        root,
        &["stash".into(), "apply".into(), reference.to_string()],
        None,
    )?;
    let conflicted_paths = conflicted_paths(root)?;
    if !conflicted_paths.is_empty() {
        result.exit_code = 1;
        result.stash_restore = Some(GitStashRestoreResponse {
            stash_reference: reference.to_string(),
            conflicted_paths,
        });
    }
    Ok(result)
}

pub(crate) fn pull_with_auto_stash(
    root: &str,
    pull_arguments: &[String],
) -> Result<GitCommandResponse, CoreError> {
    let status = execute_git_readonly(
        root,
        &[
            "status".into(),
            "--porcelain=v1".into(),
            "-z".into(),
            "--untracked-files=all".into(),
        ],
        None,
    )?;
    if status.exit_code != 0 {
        return Ok(status);
    }
    if status.stdout.is_empty() {
        return execute_git(root, pull_arguments, None);
    }

    let stash_marker = format!(
        "lithe: auto-stash before pull:{}:{}",
        std::process::id(),
        AUTO_STASH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let stashed = execute_git(
        root,
        &[
            "stash".into(),
            "push".into(),
            "--include-untracked".into(),
            "--message".into(),
            stash_marker.clone(),
        ],
        None,
    )?;
    if stashed.exit_code != 0 {
        return Ok(stashed);
    }
    let stash_oid = find_stash_oid_by_message(root, &stash_marker)?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::ProcessFailed,
            "Pull created a temporary stash but could not identify it",
        )
    })?;

    let pulled = execute_git(root, pull_arguments, None)?;
    if pulled.exit_code != 0 {
        return Ok(pulled);
    }

    let mut restored = execute_git(
        root,
        &["stash".into(), "apply".into(), stash_oid.clone()],
        None,
    )?;
    let conflicted_paths = conflicted_paths(root)?;
    let stash_reference = stash_reference_for_oid(root, &stash_oid)?;
    if restored.exit_code != 0 || !conflicted_paths.is_empty() {
        restored.exit_code = 1;
        restored.stash_restore = Some(GitStashRestoreResponse {
            stash_reference: stash_reference.unwrap_or(stash_oid),
            conflicted_paths,
        });
        return Ok(restored);
    }

    let Some(stash_reference) = stash_reference else {
        restored.warnings.push(GitOperationWarning::new(
            "git_stash_drop_failed",
            "The pull and stash restore succeeded, but the saved stash could not be located for removal",
            None,
        ));
        return Ok(restored);
    };
    let dropped = execute_git(
        root,
        &["stash".into(), "drop".into(), stash_reference],
        None,
    )?;
    if dropped.exit_code != 0 {
        restored.warnings.push(GitOperationWarning::new(
            "git_stash_drop_failed",
            "The pull and stash restore succeeded, but the saved stash could not be removed",
            Some(dropped.output),
        ));
    }
    Ok(restored)
}

fn find_stash_oid_by_message(root: &str, message: &str) -> Result<Option<String>, CoreError> {
    let list = execute_git_readonly(
        root,
        &["stash".into(), "list".into(), "--format=%H%x09%gs".into()],
        None,
    )?;
    if list.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not inspect Git stashes")
                .with_details(list.output),
        );
    }
    Ok(list.stdout.lines().find_map(|line| {
        let (oid, subject) = line.split_once('\t')?;
        subject.contains(message).then(|| oid.to_string())
    }))
}

pub(crate) fn stash_reference_for_oid(root: &str, oid: &str) -> Result<Option<String>, CoreError> {
    let list = execute_git_readonly(
        root,
        &["stash".into(), "list".into(), "--format=%gd%x09%H".into()],
        None,
    )?;
    if list.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not inspect Git stashes")
                .with_details(list.output),
        );
    }
    Ok(list.stdout.lines().find_map(|line| {
        let (reference, commit) = line.split_once('\t')?;
        (commit == oid).then(|| reference.to_string())
    }))
}

pub(crate) fn conflicted_paths(root: &str) -> Result<Vec<String>, CoreError> {
    let response = execute_git(
        root,
        &[
            "diff".into(),
            "--name-only".into(),
            "--diff-filter=U".into(),
            "--".into(),
        ],
        None,
    )?;
    if response.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git conflict status failed")
                .with_details(response.output),
        );
    }
    let mut paths = response
        .output
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(String::from)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(crate) fn stash_reference_exists(root: &str, reference: &str) -> Result<bool, CoreError> {
    let list = execute_git(
        root,
        &["stash".into(), "list".into(), "--format=%gd".into()],
        None,
    )?;
    if list.exit_code != 0 {
        return Ok(false);
    }
    Ok(list.output.lines().any(|line| line.trim() == reference))
}

pub(crate) fn find_stash_reference(root: &str, message: &str) -> Result<Option<String>, CoreError> {
    let list = execute_git(
        root,
        &["stash".into(), "list".into(), "--format=%gd%x09%gs".into()],
        None,
    )?;
    if list.exit_code != 0 {
        return Ok(None);
    }
    Ok(list.output.lines().find_map(|line| {
        let (reference, subject) = line.split_once('\t')?;
        subject
            .contains(message)
            .then(|| reference.trim().to_string())
    }))
}

/// Marker on stashes created by smart checkout, so the restore step can tell whether
/// `git stash pop` consumed its entry or kept it after a conflict.
pub(crate) const AUTO_STASH_MESSAGE: &str = "lithe: auto-stash before checkout";
