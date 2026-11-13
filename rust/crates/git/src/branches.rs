//! Branch and tag creation, deletion and upstream configuration.

use super::*;

/// Reports whether `refs/tags/<name>` already resolves, using `--verify` so
/// the probe matches the exact ref instead of any revision expression.
pub(crate) fn tag_exists(root: &str, name: &str) -> Result<bool, CoreError> {
    let probe = execute_git(
        root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            format!("refs/tags/{name}"),
        ],
        None,
    )?;
    Ok(probe.exit_code == 0)
}

/// Resolves a tag target revision to a commit and returns its object id.
/// Git allows tagging trees and blobs; the tag contract only promises commit
/// targets, so `<revision>^{commit}` both validates and yields the id the
/// mutation should point at.
pub(crate) fn resolved_commit_target(
    root: &str,
    target: &str,
) -> Result<Option<String>, CoreError> {
    let probe = execute_git(
        root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            format!("{target}^{{commit}}"),
        ],
        None,
    )?;
    Ok((probe.exit_code == 0).then(|| probe.stdout.trim().to_string()))
}

/// Rejects tag-form combinations Git cannot represent.
///
/// An explicitly lightweight tag cannot also be annotated or signed; the
/// message rules match the previous Windows host behavior, including allowing a
/// whitespace-only message that the lightweight form ignores.
pub(crate) fn validate_create_tag_form(
    message: Option<&str>,
    lightweight: Option<bool>,
    signed: Option<bool>,
) -> Result<(), CoreError> {
    if lightweight == Some(true)
        && (signed == Some(true) || message.is_some_and(|message| !message.trim().is_empty()))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Lightweight tags cannot include a message or signature",
        ));
    }
    Ok(())
}

/// Builds the `git tag` argument vector for an already validated form and a
/// target resolved to a commit id.
///
/// `lightweight: true` always adds `--no-sign`, so an explicit lightweight tag
/// stays independent of `tag.gpgSign`. `signed: true` adds `-s`; with a message
/// this becomes a signed annotated tag. Omitted fields keep the legacy
/// message-selects-annotated behavior.
pub(crate) fn create_tag_arguments(
    name: &str,
    target: &str,
    message: Option<&str>,
    lightweight: Option<bool>,
    signed: Option<bool>,
) -> Vec<String> {
    if lightweight == Some(true) {
        return vec![
            "tag".to_string(),
            "--no-sign".to_string(),
            name.to_string(),
            target.to_string(),
        ];
    }
    let mut arguments = vec!["tag".to_string()];
    if signed == Some(true) {
        arguments.push("-s".to_string());
    }
    match message {
        // An explicit message field (even an empty one) selects the annotated
        // form. Verbatim cleanup keeps restored CRLF and trailing blank lines
        // intact; UIs trim newly entered messages before send.
        Some(message) => arguments.extend([
            "-a".to_string(),
            "--cleanup=verbatim".to_string(),
            name.to_string(),
            "-m".to_string(),
            message.to_string(),
            target.to_string(),
        ]),
        None => arguments.extend([name.to_string(), target.to_string()]),
    }
    arguments
}

/// Deletes one tag and returns a structured deletion record so the host can
/// offer a restore. The probes run before the deletion because `git tag -d`
/// diagnostics are localized prose that cannot be mapped to stable errors.
pub(crate) fn delete_tag(root: &str, value: Option<&str>) -> Result<GitCommandResponse, CoreError> {
    let name = validated_tag_name(value)?;
    let reference = format!("refs/tags/{name}");
    let expected_object = resolve_ref_object(root, &reference)?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            format!("The tag '{name}' does not exist"),
        )
    })?;
    let object_type = execute_git(
        root,
        &["cat-file".into(), "-t".into(), expected_object.clone()],
        None,
    )?;
    if object_type.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("The tag '{name}' does not exist"),
        ));
    }
    let is_annotated = object_type.stdout.trim() == "tag";
    let mut message = None;
    if is_annotated {
        let tag_object = execute_git(
            root,
            &["cat-file".into(), "tag".into(), expected_object.clone()],
            None,
        )?;
        if tag_object.exit_code == 0 {
            message = annotation_message_from_tag_object(&tag_object.stdout);
        }
    }
    // Peel the ref to a commit so pre-existing tree/blob tags cannot produce
    // a recovery record that violates the restore contract.
    let peeled = execute_git(
        root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            format!("{expected_object}^{{commit}}"),
        ],
        None,
    )?;
    if peeled.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            format!("Could not resolve tag target '{name}'"),
        )
        .with_details(peeled.output));
    }
    let mut result = delete_ref_if_unchanged(root, &reference, &expected_object)?;
    if result.exit_code == 0 {
        result.tag_deletion = Some(GitTagDeletionResponse {
            name,
            deleted_target: peeled.stdout.trim().to_string(),
            kind: if is_annotated {
                "annotated"
            } else {
                "lightweight"
            }
            .to_string(),
            message,
        });
    }
    Ok(result)
}

/// Deletes one local branch and returns a structured deletion record so the
/// host can offer a restore. The commit is resolved before the deletion
/// because `git branch -d` diagnostics are localized prose.
pub(crate) fn delete_branch(root: &str, branch: &str) -> Result<GitCommandResponse, CoreError> {
    let reference = format!("refs/heads/{branch}");
    let target = resolve_ref_object(root, &reference)?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            format!("The branch '{branch}' does not exist"),
        )
    })?;
    ensure_branch_is_safely_deletable(root, branch, &reference, &target)?;
    let mut result = delete_ref_if_unchanged(root, &reference, &target)?;
    if result.exit_code == 0 {
        result.branch_deletion = Some(GitBranchDeletionResponse {
            name: branch.to_string(),
            deleted_target: target,
        });
        if let Err(error) = remove_branch_config(root, branch) {
            // The ref mutation already committed. Preserve recovery data and
            // report configuration cleanup as a diagnosable partial success.
            result.warnings.push(GitOperationWarning::new(
                "branch_config_cleanup_failed",
                &error.message,
                error.details,
            ));
        }
    }
    Ok(result)
}

/// Resolves an exact refname to its current unpeeled object id.
pub(crate) fn resolve_ref_object(root: &str, reference: &str) -> Result<Option<String>, CoreError> {
    let probe = execute_git(
        root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            reference.to_string(),
        ],
        None,
    )?;
    Ok((probe.exit_code == 0).then(|| probe.stdout.trim().to_string()))
}

/// Deletes a ref only when it still points at the object observed by the
/// caller. `update-ref` performs the comparison and mutation under the same
/// ref lock, closing the probe-then-mutate race.
fn delete_ref_if_unchanged(
    root: &str,
    reference: &str,
    expected_object: &str,
) -> Result<GitCommandResponse, CoreError> {
    let mut result = execute_git(
        root,
        &[
            "update-ref".into(),
            "-d".into(),
            reference.to_string(),
            expected_object.to_string(),
        ],
        None,
    )?;
    if result.exit_code != 0 {
        result.operation_error = Some(
            CoreError::new(
                ErrorCode::InvalidRequest,
                format!("The Git reference '{reference}' changed before it could be deleted"),
            )
            .with_details(result.output.clone()),
        );
    }
    Ok(result)
}

/// Preserves `git branch -d` safety before the atomic ref mutation: a branch
/// must not be checked out in any worktree and must be merged into its valid
/// upstream, or into HEAD when it has no usable upstream.
fn ensure_branch_is_safely_deletable(
    root: &str,
    branch: &str,
    reference: &str,
    target: &str,
) -> Result<(), CoreError> {
    let worktrees = execute_git(
        root,
        &["worktree".into(), "list".into(), "--porcelain".into()],
        None,
    )?;
    if worktrees.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not inspect Git worktrees")
                .with_details(worktrees.output),
        );
    }
    if worktrees
        .stdout
        .lines()
        .any(|line| line == format!("branch {reference}"))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("The branch '{branch}' is checked out in a worktree"),
        ));
    }

    let upstream = execute_git(
        root,
        &[
            "for-each-ref".into(),
            "--format=%(upstream)".into(),
            reference.to_string(),
        ],
        None,
    )?;
    if upstream.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not inspect the branch upstream",
        )
        .with_details(upstream.output));
    }
    let upstream = upstream.stdout.trim();
    let merge_target = if !upstream.is_empty() && resolved_commit_target(root, upstream)?.is_some()
    {
        upstream
    } else {
        "HEAD"
    };
    let merged = execute_git(
        root,
        &[
            "merge-base".into(),
            "--is-ancestor".into(),
            target.to_string(),
            merge_target.to_string(),
        ],
        None,
    )?;
    match merged.exit_code {
        0 => Ok(()),
        1 => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("The branch '{branch}' is not fully merged"),
        )),
        _ => Err(CoreError::new(
            ErrorCode::ProcessFailed,
            format!("Could not verify whether branch '{branch}' is merged"),
        )
        .with_details(merged.output)),
    }
}

/// Removes branch-local configuration after the ref has been deleted, matching
/// the metadata cleanup performed by `git branch -d`.
fn remove_branch_config(root: &str, branch: &str) -> Result<(), CoreError> {
    let listing = capture_git_with_options(
        root,
        &[
            "config".into(),
            "--name-only".into(),
            "--get-regexp".into(),
            "^branch\\.".into(),
        ],
        None,
        false,
    )?;
    if listing.exit_code == 1 {
        return Ok(());
    }
    if listing.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not inspect branch configuration",
        )
        .with_details(String::from_utf8_lossy(&listing.stderr)));
    }
    let prefix = format!("branch.{branch}.");
    if !String::from_utf8_lossy(&listing.stdout)
        .lines()
        .any(|key| key.starts_with(&prefix))
    {
        return Ok(());
    }
    let cleanup = capture_git_with_options(
        root,
        &[
            "config".into(),
            "--remove-section".into(),
            format!("branch.{branch}"),
        ],
        None,
        false,
    )?;
    if cleanup.exit_code == 0 {
        return Ok(());
    }
    Err(CoreError::new(
        ErrorCode::ProcessFailed,
        format!("Could not remove configuration for deleted branch '{branch}'"),
    )
    .with_details(String::from_utf8_lossy(&cleanup.stderr)))
}

/// Extracts the annotation message from a raw tag object byte-for-byte, so a
/// restored tag keeps the original message including CRLF line endings and
/// trailing newlines. Only the signature block is cut, by locating its first
/// line in the raw content; the signature belongs to the previous tagger and
/// a restored tag would be signed separately.
pub(crate) fn annotation_message_from_tag_object(raw: &str) -> Option<String> {
    let (_, message) = raw.split_once("\n\n")?;
    let mut offset = 0;
    for line in message.split_inclusive('\n') {
        let without_eol = line.trim_end_matches(['\r', '\n']);
        if without_eol.starts_with("-----BEGIN ") && without_eol.ends_with("SIGNATURE-----") {
            return Some(message[..offset].to_string());
        }
        offset += line.len();
    }
    Some(message.to_string())
}

pub(crate) fn local_branch_name(reference: &str) -> Result<String, CoreError> {
    let branch = reference
        .strip_prefix("refs/heads/")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Only local branches support this Git operation",
            )
        })?;
    if !is_safe_pathspec(branch) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git branch name",
        ));
    }
    Ok(branch.to_string())
}

pub(crate) fn current_branch(root: &str) -> Result<String, CoreError> {
    let response = execute_git(root, &["branch".into(), "--show-current".into()], None)?;
    if response.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not determine current branch",
        )
        .with_details(response.output));
    }
    let branch = response.output.trim();
    if branch.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git operation requires a checked out branch",
        ));
    }
    Ok(branch.to_string())
}

pub(crate) fn optional_current_branch(root: &str) -> Result<Option<String>, CoreError> {
    let response = execute_git_readonly(root, &["branch".into(), "--show-current".into()], None)?;
    if response.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not determine current branch",
        )
        .with_details(response.output));
    }
    Ok(match response.output.trim() {
        "" => None,
        branch => Some(branch.to_string()),
    })
}

pub(crate) fn is_current_reference(root: &str, reference: &str) -> Result<bool, CoreError> {
    let current = current_branch(root)?;
    Ok(reference == current || reference == format!("refs/heads/{current}"))
}

pub(crate) fn update_local_branch(
    root: &str,
    request: &GitWriteRequest,
) -> Result<GitCommandResponse, CoreError> {
    let reference = request
        .git_reference
        .as_ref()
        .ok_or_else(invalid_git_reference)
        .and_then(|reference| validated_git_reference(root, reference))?;
    if reference.kind != "local" {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Branch update requires a local Git reference",
        ));
    }
    if optional_current_branch(root)?.is_some_and(|current| {
        reference.full_name == current || reference.full_name == format!("refs/heads/{current}")
    }) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The current branch must be updated through pull",
        ));
    }

    let upstream = execute_git_readonly(
        root,
        &[
            "for-each-ref".into(),
            "--format=%(upstream)".into(),
            reference.full_name.clone(),
        ],
        None,
    )?;
    if upstream.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not inspect the branch upstream",
        )
        .with_details(upstream.output));
    }
    let upstream_reference = upstream.stdout.trim();
    if upstream_reference.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The selected branch has no upstream",
        ));
    }
    let (remote, remote_branch) = mutations::remote_branch_components(root, upstream_reference)?;

    // One atomic fetch refreshes the tracking ref and fast-forwards the selected
    // local branch without changing HEAD. Git rejects non-fast-forward updates
    // and branches checked out by any worktree.
    execute_git(
        root,
        &[
            "fetch".into(),
            "--atomic".into(),
            "--no-tags".into(),
            "--".into(),
            remote,
            format!("+refs/heads/{remote_branch}:{upstream_reference}"),
            format!("refs/heads/{remote_branch}:{}", reference.full_name),
        ],
        None,
    )
}

pub(crate) fn configure_branch_upstream(
    root: &str,
    local_branch: &str,
    remote: &str,
    remote_branch: &str,
) -> Result<Option<GitCommandResponse>, CoreError> {
    let remote_config = execute_git(
        root,
        &[
            "config".to_string(),
            "--local".to_string(),
            "--replace-all".to_string(),
            format!("branch.{local_branch}.remote"),
            remote.to_string(),
        ],
        None,
    )?;
    if remote_config.exit_code != 0 {
        return Ok(Some(remote_config));
    }
    let merge_config = execute_git(
        root,
        &[
            "config".to_string(),
            "--local".to_string(),
            "--replace-all".to_string(),
            format!("branch.{local_branch}.merge"),
            format!("refs/heads/{remote_branch}"),
        ],
        None,
    )?;
    Ok((merge_config.exit_code != 0).then_some(merge_config))
}
