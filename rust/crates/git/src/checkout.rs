//! Checkout and switch of local and remote references, with an optional auto-stash.

use super::*;

/// Checks out `request.reference`, honouring the conflict-resolution strategy the user
/// picked in the checkout dialog.
///
/// Three strategies, mirroring IntelliJ IDEA:
/// - default: plain switch. Git refuses when local changes would be overwritten.
/// - `force`: `--discard-changes`, throwing the local edits away.
/// - `auto_stash`: stash, switch, then restore the stash ("smart checkout").
pub(crate) fn checkout(
    root: &str,
    request: GitWriteRequest,
) -> Result<GitCommandResponse, CoreError> {
    if request.reference_kind.as_deref() == Some("local") {
        if let Some(reference) = request
            .reference
            .as_deref()
            .filter(|value| !value.starts_with('-') && !value.chars().any(char::is_whitespace))
        {
            if is_current_reference(root, reference)? {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The current branch is already checked out",
                ));
            }
        }
    }
    if request.auto_stash {
        // Reject local-only commits before creating a stash. Ordinary remote
        // checkout aligns the branch; checkoutAndRebase retains its own policy.
        let is_remote = request.git_reference.as_ref().map_or(
            request.reference_kind.as_deref() == Some("remote"),
            |reference| reference.kind == "remote",
        );
        if is_remote {
            let reference = checkout_request_reference(root, &request)?;
            let (_, local_name) = mutations::remote_branch_components(root, &reference.full_name)?;
            let local_ref = format!("refs/heads/{local_name}");
            if resolve_ref_object(root, &local_ref)?.is_some() {
                remote_checkout_revisions(root, &local_ref, &reference.full_name)?;
            }
        }
        return checkout_with_auto_stash(root, request);
    }
    switch_reference(root, &request)
}

/// Checks out a local or remote branch, then rebases it onto the branch that
/// was current before the switch. A dirty tree is rejected before checkout so
/// the composite operation cannot leave the repository half-switched.
/// Stash, switch, restore. A failed switch leaves the stash untouched so the caller can
/// recover it, and a conflicting restore is reported as a failure rather than silently
/// leaving the entry behind.
fn checkout_with_auto_stash(
    root: &str,
    request: GitWriteRequest,
) -> Result<GitCommandResponse, CoreError> {
    let stash = execute_git(
        root,
        &[
            "stash".into(),
            "push".into(),
            "--include-untracked".into(),
            "--message".into(),
            AUTO_STASH_MESSAGE.into(),
        ],
        None,
    )?;
    if stash.exit_code != 0 {
        return Ok(stash);
    }

    let switched = switch_reference(root, &request)?;
    if switched.exit_code != 0 {
        // Leave the stash in place; the working tree is still on the original branch.
        return Ok(switched);
    }

    let Some(stash_reference) = find_stash_reference(root, AUTO_STASH_MESSAGE)? else {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Smart Checkout created a stash but could not locate it for restore.",
        ));
    };
    let mut restored = execute_git(
        root,
        &["stash".into(), "pop".into(), stash_reference.clone()],
        None,
    )?;
    let conflicted_paths = conflicted_paths(root)?;
    let entry_was_kept = stash_reference_exists(root, &stash_reference)?;
    if entry_was_kept && (restored.exit_code == 0 || !conflicted_paths.is_empty()) {
        restored.exit_code = 1;
        restored.stash_restore = Some(GitStashRestoreResponse {
            stash_reference,
            conflicted_paths,
        });
    }
    Ok(restored)
}

pub(crate) fn switch_reference(
    root: &str,
    request: &GitWriteRequest,
) -> Result<GitCommandResponse, CoreError> {
    let reference = checkout_request_reference(root, request)?;
    switch_validated_reference(
        root,
        &reference,
        request.force,
        request.operation == "checkout",
    )
}

fn switch_validated_reference(
    root: &str,
    reference: &ValidatedGitReference,
    force: bool,
    update_remote: bool,
) -> Result<GitCommandResponse, CoreError> {
    let mut base: Vec<String> = vec!["switch".into()];
    if force {
        base.push("--discard-changes".into());
    }
    match reference.kind.as_str() {
        "local" => {
            if current_branch(root)? == reference.short_name {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The current branch is already checked out",
                ));
            }
            // `git switch` rejects fully qualified refs ("refs/heads/foo"), so pass the
            // short branch name. Tags still need the full ref for the --detach form.
            base.push(reference.short_name.clone());
            execute_git(root, &base, None)
        }
        "tag" => {
            base.push("--detach".into());
            base.push(reference.full_name.clone());
            execute_git(root, &base, None)
        }
        "remote" => {
            let (_, local_name) = mutations::remote_branch_components(root, &reference.full_name)?;
            let local_ref = format!("refs/heads/{local_name}");
            let existing = execute_git(
                root,
                &[
                    "show-ref".into(),
                    "--verify".into(),
                    "--quiet".into(),
                    local_ref.clone(),
                ],
                None,
            )?;
            if existing.exit_code == 0 {
                let upstream = execute_git_readonly(
                    root,
                    &[
                        "for-each-ref".into(),
                        "--format=%(upstream)".into(),
                        "--count=1".into(),
                        local_ref.clone(),
                    ],
                    None,
                )?;
                if upstream.exit_code != 0 {
                    return Err(CoreError::new(
                        ErrorCode::ProcessFailed,
                        "Could not inspect the local branch upstream",
                    )
                    .with_details(upstream.output));
                }
                if upstream.stdout.trim() != reference.full_name {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "A same-named local branch tracks a different Git reference",
                    ));
                }
                if update_remote {
                    return checkout_updated_remote_branch(
                        root,
                        &local_name,
                        &local_ref,
                        &reference.full_name,
                        force,
                    );
                }
                if current_branch(root)? == local_name {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The current branch is already checked out",
                    ));
                }
                base.push(local_name);
            } else {
                base.push("--track".into());
                base.push("-c".into());
                base.push(local_name);
                base.push(reference.full_name.clone());
            }
            execute_git(root, &base, None)
        }
        _ => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git reference kind",
        )),
    }
}

/// Pins the local and fetched remote commits and rejects any local-only history.
fn remote_checkout_revisions(
    root: &str,
    local_ref: &str,
    remote_ref: &str,
) -> Result<(String, String), CoreError> {
    let local_revision = resolve_commit_revision(root, local_ref)?;
    let remote_revision = resolve_commit_revision(root, remote_ref)?;
    let ancestor = execute_git_readonly(
        root,
        &[
            "merge-base".into(),
            "--is-ancestor".into(),
            local_revision.clone(),
            remote_revision.clone(),
        ],
        None,
    )?;
    match ancestor.exit_code {
        0 => Ok((local_revision, remote_revision)),
        1 => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The local branch has commits not included in the selected remote branch. Reconcile those commits before checking out the remote branch.",
        )),
        _ => Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not compare the local and remote branch commits",
        )
        .with_details(ancestor.output)),
    }
}

/// Switches to an existing tracking branch and fast-forwards to a pinned remote
/// commit, including when that local branch is already current.
fn checkout_updated_remote_branch(
    root: &str,
    local_name: &str,
    local_ref: &str,
    remote_ref: &str,
    force: bool,
) -> Result<GitCommandResponse, CoreError> {
    let (local_revision, remote_revision) = remote_checkout_revisions(root, local_ref, remote_ref)?;
    if !force {
        // Check against the final remote tree before switching to the old local
        // tree, so a blocked update cannot leave the user on another branch.
        let preflight = checkout_preflight(GitCheckoutPreflightRequest {
            root: root.to_string(),
            reference: Some(remote_revision.clone()),
            git_reference: None,
        })?;
        if !preflight.blocking_paths.is_empty() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Local changes would be overwritten by checking out the remote branch",
            )
            .with_details(preflight.blocking_paths.join("\n")));
        }
    }
    if force || current_branch(root)? != local_name {
        let mut arguments = vec!["switch".into()];
        if force {
            arguments.push("--discard-changes".into());
        }
        arguments.push(local_name.to_string());
        let switched = execute_git(root, &arguments, None)?;
        if switched.exit_code != 0 {
            return Ok(switched);
        }
    }
    // Post-checkout hooks and external clients may change HEAD. Verify the
    // branch and its tip again; --ff-only also prevents losing local commits.
    if current_branch(root)? != local_name
        || resolve_commit_revision(root, "HEAD")? != local_revision
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The selected branch changed; review and retry",
        ));
    }
    // `branch.<name>.mergeOptions=--squash` would make the merge exit 0 while
    // only staging the remote diff. Override it for this call without touching
    // the user's configuration, then confirm HEAD reached the pinned commit.
    let merged = execute_git(
        root,
        &[
            "merge".into(),
            "--ff-only".into(),
            "--no-squash".into(),
            remote_revision.clone(),
        ],
        None,
    )?;
    if merged.exit_code == 0
        && (current_branch(root)? != local_name
            || resolve_commit_revision(root, "HEAD")? != remote_revision)
    {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Git did not move the branch to the selected remote commit",
        )
        .with_details(merged.output));
    }
    Ok(merged)
}
