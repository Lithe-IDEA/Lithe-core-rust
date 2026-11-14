//! Linked worktrees: listing, creation, removal and unclean-removal classification.

use super::*;

/// Returns one metadata-only snapshot for every registered worktree.
///
/// Each entry carries the state its row shows and the writes it allows. With `countChanges`
/// Core also runs status in every checkout that is there and not bare; a checkout whose
/// status fails keeps `changes: null` and is ranked as if it had none.
pub fn worktrees(request: GitWorktreesRequest) -> Result<GitWorktreesResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let mut worktrees = list_worktrees(&root)?;
    for worktree in &mut worktrees {
        let facts = worktree_rules::WorktreeFacts {
            is_primary: worktree.is_primary,
            is_current: worktree.is_current,
            is_locked: worktree.is_locked,
            is_prunable: worktree.is_prunable,
        };
        if request.count_changes && !worktree.is_prunable && !worktree.is_bare {
            worktree.changes = status(GitStatusRequest {
                root: worktree.path.clone(),
                include_index_only_changes: true,
                repository_roots: Vec::new(),
                summarize_untracked_directories: true,
            })
            .ok()
            .map(|status| status.changes.len());
        }
        worktree.status = worktree_rules::status(facts, worktree.changes);
        worktree.actions = worktree_rules::actions(facts);
    }
    Ok(GitWorktreesResponse { worktrees })
}

fn list_worktrees(root: &str) -> Result<Vec<GitWorktreeResponse>, CoreError> {
    let arguments = vec![
        "worktree".to_string(),
        "list".to_string(),
        "--porcelain".to_string(),
        "-z".to_string(),
    ];
    let response = execute_git_readonly(root, &arguments, None)?;
    if response.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git worktree listing failed")
                .with_details(response.output),
        );
    }
    let current_root = canonicalize_or_original(&repository_root(root)?).map_err(git_path_error)?;
    let mut records = Vec::new();
    let mut fields = Vec::new();
    for field in response.stdout.split('\0') {
        if field.is_empty() {
            if !fields.is_empty() {
                records.push(parse_worktree_record(
                    &fields,
                    records.is_empty(),
                    &current_root,
                )?);
                fields.clear();
            }
        } else {
            fields.push(field);
        }
    }
    if !fields.is_empty() {
        records.push(parse_worktree_record(
            &fields,
            records.is_empty(),
            &current_root,
        )?);
    }
    // Git currently emits the primary worktree first, but sorting here makes
    // that display contract explicit and stable across Git versions.
    records.sort_by(|left, right| {
        right
            .is_primary
            .cmp(&left.is_primary)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(records)
}

fn worktree_paths_match(left: &str, right: &str) -> bool {
    let left_path = PathBuf::from(left);
    let right_path = PathBuf::from(right);
    if left_path == right_path {
        return true;
    }
    match (fs::canonicalize(&left_path), fs::canonicalize(&right_path)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn parse_worktree_record(
    fields: &[&str],
    is_primary: bool,
    current_root: &Path,
) -> Result<GitWorktreeResponse, CoreError> {
    let path = fields
        .iter()
        .find_map(|field| field.strip_prefix("worktree "))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::ProcessFailed,
                "Git returned an invalid worktree record",
            )
        })?;
    let head = fields
        .iter()
        .find_map(|field| field.strip_prefix("HEAD "))
        .unwrap_or_default()
        .to_string();
    let branch = fields
        .iter()
        .find_map(|field| field.strip_prefix("branch "))
        .map(str::to_string);
    let value_after_marker = |marker: &str| {
        fields.iter().find_map(|field| {
            if *field == marker {
                Some(None)
            } else {
                field
                    .strip_prefix(&format!("{marker} "))
                    .map(|value| Some(value.to_string()))
            }
        })
    };
    let lock = value_after_marker("locked");
    let prunable = value_after_marker("prunable");
    let reported_path = PathBuf::from(path);
    let normalized_path = canonicalize_or_original(&reported_path).map_err(git_path_error)?;
    Ok(GitWorktreeResponse {
        path: normalized_path.to_string_lossy().to_string(),
        head,
        branch,
        is_current: normalized_path == current_root,
        is_primary,
        is_bare: fields.contains(&"bare"),
        is_detached: fields.contains(&"detached"),
        is_locked: lock.is_some(),
        lock_reason: lock.flatten(),
        is_prunable: prunable.is_some(),
        prune_reason: prunable.flatten(),
        status: worktree_rules::GitWorktreeStatus::Available,
        changes: None,
        actions: worktree_rules::actions(worktree_rules::WorktreeFacts::default()),
    })
}

pub(crate) fn create_worktree(
    root: &str,
    request: &GitWriteRequest,
) -> Result<GitCommandResponse, CoreError> {
    let destination = required_text(request.destination.as_deref(), "worktree destination")?;
    if destination.starts_with('-') || destination.contains(['\0', '\n', '\r']) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git worktree destination",
        ));
    }
    let mut arguments = vec!["worktree".into(), "add".into()];
    if request.no_checkout {
        arguments.push("--no-checkout".into());
    }
    let mode = request.worktree_mode.as_deref().unwrap_or("newBranch");
    let source = match mode {
        "newBranch" => {
            let branch = validated_branch_name(root, request.name.as_deref())?;
            let reference = request
                .git_reference
                .as_ref()
                .ok_or_else(invalid_git_reference)
                .and_then(|reference| validated_git_reference(root, reference))?;
            if reference.kind == "remote" && request.revision.is_none() {
                // Branch creation and upstream setup remain one Git mutation.
                arguments.push("--track".into());
            } else {
                // An explicit commit has no tracking identity. Freeze the
                // result independently of branch.autoSetupMerge preferences.
                arguments.push("--no-track".into());
            }
            arguments.extend(["-b".into(), branch]);
            if let Some(revision) = request.revision.as_deref() {
                resolve_commit_revision(root, &validated_revision(Some(revision))?)?
            } else {
                reference.full_name
            }
        }
        "existingBranch" => {
            if request.name.is_some() || request.revision.is_some() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Existing-branch worktrees do not accept a new branch name or revision",
                ));
            }
            let reference = request
                .git_reference
                .as_ref()
                .ok_or_else(invalid_git_reference)
                .and_then(|reference| validated_git_reference(root, reference))?;
            if reference.kind != "local" {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "An existing-branch worktree requires a local Git reference",
                ));
            }
            let branch = validated_branch_name(root, Some(&reference.short_name))?;
            resolve_commit_revision(root, &reference.full_name)?;
            // Git recognizes an existing branch from its short name here;
            // supplying refs/heads/... would create a detached checkout instead.
            // The complete identity was validated above; Git enforces occupancy.
            branch
        }
        "detached" => {
            if request.name.is_some() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "A detached worktree does not accept a branch name",
                ));
            }
            arguments.push("--detach".into());
            let reference = request
                .git_reference
                .as_ref()
                .map(|reference| validated_git_reference(root, reference))
                .transpose()?;
            let revision = if let Some(revision) = request.revision.as_deref() {
                validated_revision(Some(revision))?
            } else {
                reference.ok_or_else(invalid_git_reference)?.full_name
            };
            // A fixed OID prevents a concurrently moved ref from changing the
            // detached commit after this request has selected its starting point.
            resolve_commit_revision(root, &revision)?
        }
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Unsupported Git worktree creation mode",
            ))
        }
    };
    let destination = lithe_workspace::remote_path(Path::new(&destination))
        .to_string_lossy()
        .into_owned();
    arguments.extend(["--".into(), destination, source]);
    execute_git(root, &arguments, None)
}

pub(crate) fn mutate_worktree(
    root: &str,
    request: &GitWriteRequest,
) -> Result<GitCommandResponse, CoreError> {
    let destination = required_text(request.destination.as_deref(), "worktree destination")?;
    if destination.starts_with('-') || destination.contains(['\0', '\n', '\r']) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git worktree destination",
        ));
    }
    let entries = list_worktrees(root)?;
    let target = entries
        .iter()
        .find(|entry| worktree_paths_match(&entry.path, &destination))
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::GitWorktreeNotRegistered,
                "The selected path is not a registered Git worktree",
            )
        })?;
    let removing = request.operation == "removeWorktree";
    // Each refusal has its own code so hosts can explain it without parsing
    // messages. The primary worktree is checked first: it can never be removed,
    // locked or unlocked, whichever worktree the request was made from.
    if target.is_primary {
        return Err(CoreError::new(
            ErrorCode::GitWorktreePrimary,
            "The primary Git worktree cannot be removed, locked or unlocked",
        ));
    }
    if removing && target.is_current {
        return Err(CoreError::new(
            ErrorCode::GitWorktreeCurrent,
            "The current Git worktree cannot be removed",
        ));
    }
    if removing && target.is_locked {
        return Err(CoreError::new(
            ErrorCode::GitWorktreeLocked,
            "Unlock the Git worktree before removing it",
        ));
    }

    let mut arguments = vec!["worktree".to_string()];
    match request.operation.as_str() {
        "removeWorktree" => {
            arguments.push("remove".into());
            if request.force {
                arguments.push("--force".into());
            }
        }
        "lockWorktree" => arguments.push("lock".into()),
        "unlockWorktree" => arguments.push("unlock".into()),
        _ => unreachable!("caller restricts worktree mutations"),
    }
    let destination = lithe_workspace::remote_path(Path::new(&destination))
        .to_string_lossy()
        .into_owned();
    arguments.extend(["--".into(), destination]);
    let mut response = execute_git(root, &arguments, None)?;
    if removing && !request.force {
        classify_unclean_worktree_removal(&mut response);
    }
    Ok(response)
}

/// Git's C-locale refusal of `git worktree remove` without `--force` for a
/// checkout with modified or untracked files (`check_clean_worktree` in Git's
/// `builtin/worktree.c`). Every Git child runs with `LC_ALL=C`, so the text is
/// fixed whatever the user's system language is.
const UNCLEAN_WORKTREE_REMOVAL_MARKER: &str = "contains modified or untracked files";

/// Gives a failed, unforced worktree removal that Git refused for local
/// changes the `git_worktree_has_changes` code, so hosts can offer a forced
/// removal without matching stderr. The message is the first stderr line and
/// the details the combined output. Any other failure is left as it is.
pub(crate) fn classify_unclean_worktree_removal(response: &mut GitCommandResponse) {
    if response.exit_code == 0
        || response.operation_error.is_some()
        || !response.stderr.contains(UNCLEAN_WORKTREE_REMOVAL_MARKER)
    {
        return;
    }
    let message = response
        .stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("The Git worktree has modified or untracked files")
        .to_string();
    response.operation_error = Some(
        CoreError::new(ErrorCode::GitWorktreeHasChanges, message)
            .with_details(response.output.clone()),
    );
}
