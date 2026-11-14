//! Push previews, push target resolution and expectation checks, and publishing a branch.

use super::*;

#[derive(Clone)]
struct PushTarget {
    local_branch: String,
    remote: String,
    /// Remote branch the push updates. For a divergent upstream this follows the
    /// confirmed choice, so previews, expectation checks, leases and refspecs all
    /// describe the same branch.
    remote_branch: String,
    /// Relationship between the local branch and its upstream; independent of the
    /// confirmed choice.
    target_kind: PushTargetKind,
    /// Whether the destination branch does not exist on the remote yet.
    creates_remote_branch: bool,
    upstream: Option<String>,
    /// Branch name of a divergent upstream on `remote`; `None` for every other kind.
    divergent_upstream_branch: Option<String>,
    comparison_reference: Option<String>,
}

/// Resolves the remote destination and commits that a subsequent push will use.
pub fn push_preview(request: GitPushPreviewRequest) -> Result<GitPushPreviewResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let reference = if let Some(reference) = request.git_reference.as_ref() {
        let reference = validated_git_reference(&root, reference)?;
        if reference.kind != "local" {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Only local branches can be pushed",
            ));
        }
        Some(reference.full_name)
    } else {
        request
            .reference
            .as_deref()
            .map(|reference| validated_reference(Some(reference)))
            .transpose()?
    };
    let target = resolve_push_target(&root, reference.as_deref(), request.push_target)?;
    let limit = request.limit.clamp(1, 5_000);
    let local_reference = format!("refs/heads/{}", target.local_branch);
    let local_head = resolve_commit_revision(&root, &local_reference)?;
    let tags = resolve_push_tags(
        &root,
        request.push_tags.as_deref().unwrap_or("none"),
        &local_head,
    )?;
    let remote_tracking_oid = target
        .comparison_reference
        .as_deref()
        .map(|reference| resolve_commit_revision(&root, reference))
        .transpose()?;
    let selectors = if let Some(comparison_reference) = target.comparison_reference.as_ref() {
        vec![format!("{comparison_reference}..{local_reference}")]
    } else {
        // A branch without an upstream may still be based on another remote branch.
        // Excluding every commit reachable from the selected remote keeps the preview
        // focused on commits that publication would introduce.
        vec![
            local_reference,
            "--not".to_string(),
            format!("--remotes={}", target.remote),
        ]
    };
    let (commits, has_more) = read_commit_log(&root, selectors, limit, "Git push preview failed")?;

    Ok(GitPushPreviewResponse {
        local_branch: target.local_branch,
        local_head,
        remote: target.remote,
        remote_branch: target.remote_branch,
        target_kind: target.target_kind.as_contract(),
        creates_remote_branch: target.creates_remote_branch,
        remote_tracking_oid,
        upstream: target.upstream,
        tags,
        commits,
        has_more,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Destination the user explicitly confirmed for a push.
///
/// Only a divergent upstream requires this: the usual same-named destination stays
/// implicit so routine pushes keep their current interaction cost.
pub enum PushTargetChoiceRequest {
    /// Publish the local branch to the same-named remote branch.
    Publish,
    /// Keep using the existing upstream branch, even though its name differs.
    Upstream,
}

/// Rejection for a divergent upstream that no caller confirmed. The message names
/// the command that unblocks a client without a destination picker, because the
/// native macOS push does not offer one yet.
pub(crate) fn push_destination_needs_confirmation(
    local_branch: &str,
    remote: &str,
    upstream: &str,
) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        format!(
            "Push destination needs confirmation: '{local_branch}' tracks '{upstream}', which has a different name. \
             Run `git push -u {remote} {local_branch}` to publish it under its own name, \
             or confirm the destination in a client that offers the choice."
        ),
    )
}

pub(crate) fn push(
    root: &str,
    reference: Option<&str>,
    force: bool,
    push_tags: Option<&str>,
    expected_push: Option<&GitPushExpectationRequest>,
    check_submodules: bool,
    push_target: Option<PushTargetChoiceRequest>,
) -> Result<GitCommandResponse, CoreError> {
    let tag_scope = push_tags.unwrap_or("none");
    let tag_argument = match tag_scope {
        "none" => None,
        "all" => Some("--tags"),
        "reachable" => Some("--follow-tags"),
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Unsupported Git push tag scope",
            ))
        }
    };
    // Issue #1272: the effective destination is resolved once, with the caller's
    // confirmed choice, by the same function the preview uses. A reviewed push is
    // therefore compared against exactly the branch the user was shown.
    let target = resolve_push_target(root, reference, push_target)?;
    if target.target_kind == PushTargetKind::Divergent && push_target.is_none() {
        return Err(push_destination_needs_confirmation(
            &target.local_branch,
            &target.remote,
            target.upstream.as_deref().unwrap_or_default(),
        ));
    }
    if let Some(expected_push) = expected_push {
        validate_push_expectation(root, &target, tag_scope, expected_push)?;
    }
    // Only an upstream that was missing or deliberately replaced is repointed after
    // a successful push. Confirming "upstream" keeps the foreign branch name and
    // leaves the user's tracking configuration untouched.
    let should_set_upstream = match target.target_kind {
        PushTargetKind::Tracking => false,
        PushTargetKind::Publish => target.upstream.is_none(),
        PushTargetKind::Divergent => push_target == Some(PushTargetChoiceRequest::Publish),
    };
    let mut arguments = vec!["push".to_string()];
    if check_submodules {
        arguments.push("--recurse-submodules=check".into());
    }
    if force {
        // Bind reviewed pushes to the observed remote OID so an unseen remote update is rejected.
        arguments.push(match expected_push {
            Some(expected) => format!(
                "--force-with-lease=refs/heads/{}:{}",
                target.remote_branch,
                expected.remote_tracking_oid.as_deref().unwrap_or_default()
            ),
            None => "--force-with-lease".into(),
        });
    }
    if expected_push.is_none() {
        if let Some(tag_argument) = tag_argument {
            arguments.push(tag_argument.into());
        }
    }
    let source = expected_push
        .map(|expected| expected.local_head.clone())
        .unwrap_or_else(|| format!("refs/heads/{}", target.local_branch));
    let remote = target.remote;
    let remote_branch = target.remote_branch;
    let local_branch = target.local_branch;
    arguments.extend([
        remote.clone(),
        format!("{source}:refs/heads/{remote_branch}"),
    ]);
    if let Some(expected) = expected_push {
        arguments.extend(
            expected
                .tags
                .iter()
                .map(|tag| format!("{}:{}", tag.object_id, tag.full_name)),
        );
    }
    let pushed = execute_git(root, &arguments, None)?;
    if pushed.exit_code != 0 || !should_set_upstream {
        return Ok(pushed);
    }

    if let Some(configuration) =
        configure_branch_upstream(root, &local_branch, &remote, &remote_branch)?
    {
        return Ok(push_with_upstream_warning(pushed, configuration));
    }
    Ok(pushed)
}

fn push_with_upstream_warning(
    mut pushed: GitCommandResponse,
    configuration: GitCommandResponse,
) -> GitCommandResponse {
    pushed.warnings.push(GitOperationWarning::new(
        "git_upstream_configuration_failed",
        "The push succeeded, but the branch upstream could not be configured",
        Some(configuration.output),
    ));
    pushed
}

fn validate_push_expectation(
    root: &str,
    target: &PushTarget,
    tag_scope: &str,
    expected: &GitPushExpectationRequest,
) -> Result<(), CoreError> {
    let local_reference = format!("refs/heads/{}", target.local_branch);
    let local_head = resolve_commit_revision(root, &local_reference)?;
    let remote_tracking_oid = target
        .comparison_reference
        .as_deref()
        .map(|reference| resolve_commit_revision(root, reference))
        .transpose()?;
    let tags = resolve_push_tags(root, tag_scope, &local_head)?;
    let expected_tags = expected
        .tags
        .iter()
        .map(|tag| (&tag.full_name, &tag.object_id))
        .collect::<Vec<_>>();
    let actual_tags = tags
        .iter()
        .map(|tag| (&tag.full_name, &tag.object_id))
        .collect::<Vec<_>>();
    // A reviewed push records the destination relationship too, so a branch that
    // started tracking `main` after the preview cannot slip through the check.
    // `targetKind` is compared only when the caller supplied it: a caller that
    // predates target kinds keeps its previous behaviour instead of being rejected
    // as stale.
    let target_kind_changed = expected
        .target_kind
        .as_ref()
        .is_some_and(|kind| *kind != target.target_kind.as_contract());
    if expected.local_branch != target.local_branch
        || expected.local_head != local_head
        || expected.remote != target.remote
        || expected.remote_branch != target.remote_branch
        || expected.remote_tracking_oid != remote_tracking_oid
        || target_kind_changed
        || expected_tags != actual_tags
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git push preview is stale; refresh and try again.",
        ));
    }
    Ok(())
}

fn resolve_push_tags(
    root: &str,
    scope: &str,
    local_head: &str,
) -> Result<Vec<GitPushTagResponse>, CoreError> {
    if scope == "none" {
        return Ok(Vec::new());
    }
    if !matches!(scope, "all" | "reachable") {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Unsupported Git push tag scope",
        ));
    }
    let listed = execute_git_readonly(
        root,
        &[
            "for-each-ref".into(),
            "--sort=refname".into(),
            "--format=%(refname)%09%(objectname)%09%(objecttype)%09%(*objectname)".into(),
            "refs/tags".into(),
        ],
        None,
    )?;
    if listed.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not inspect Git tags")
                .with_details(listed.output),
        );
    }

    let mut tags = Vec::new();
    for line in listed.stdout.lines() {
        let columns = line.split('\t').collect::<Vec<_>>();
        if columns.len() != 4 || !columns[0].starts_with("refs/tags/") || columns[1].is_empty() {
            return Err(CoreError::new(
                ErrorCode::ParseFailed,
                "Could not decode Git tag metadata",
            ));
        }
        if scope == "reachable" {
            if columns[2] != "tag" || columns[3].is_empty() {
                continue;
            }
            let reachable = execute_git_readonly(
                root,
                &[
                    "merge-base".into(),
                    "--is-ancestor".into(),
                    columns[3].into(),
                    local_head.into(),
                ],
                None,
            )?;
            if reachable.exit_code == 1 {
                continue;
            }
            if reachable.exit_code != 0 {
                return Err(CoreError::new(
                    ErrorCode::ProcessFailed,
                    "Could not inspect Git tag reachability",
                )
                .with_details(reachable.output));
            }
        }
        tags.push(GitPushTagResponse {
            full_name: columns[0].to_string(),
            object_id: columns[1].to_string(),
        });
    }
    Ok(tags)
}

/// Resolves the single destination used by both `pushPreview` and `push`.
///
/// `choice` only matters for a divergent upstream. Keeping the preview and the push
/// on this one function is what lets a reviewed push compare the exact remote
/// branch, tracking OID and lease the user was shown.
fn resolve_push_target(
    root: &str,
    reference: Option<&str>,
    choice: Option<PushTargetChoiceRequest>,
) -> Result<PushTarget, CoreError> {
    let local_branch = match reference {
        Some(reference) => local_branch_name(&validated_reference(Some(reference))?)?,
        None => current_branch(root)?,
    };
    let upstream_lookup = execute_git_readonly(
        root,
        &[
            "rev-parse".into(),
            "--abbrev-ref".into(),
            format!("{local_branch}@{{upstream}}"),
        ],
        None,
    )?;
    let (upstream, upstream_components) = if upstream_lookup.exit_code == 0 {
        let upstream = upstream_lookup.output.trim().to_string();
        let components =
            mutations::remote_branch_components(root, &format!("refs/remotes/{upstream}"))?;
        (Some(upstream), Some(components))
    } else {
        (None, None)
    };

    let branch_push_remote =
        read_git_config_value(root, &format!("branch.{local_branch}.pushRemote"))?;
    let default_push_remote = read_git_config_value(root, "remote.pushDefault")?;
    let branch_remote = read_git_config_value(root, &format!("branch.{local_branch}.remote"))?
        .filter(|remote| remote != ".");
    let remote = branch_push_remote
        .or(default_push_remote)
        .or_else(|| {
            upstream_components
                .as_ref()
                .map(|(remote, _)| remote.clone())
        })
        .or(branch_remote)
        .map(Ok)
        .unwrap_or_else(|| default_push_remote_name(root))?;
    validate_push_component(&remote)?;

    // The upstream only identifies a destination when it lives on the remote this
    // push actually targets. A `pushRemote`/`remote.pushDefault` override therefore
    // falls back to the same-named branch on the selected remote.
    let upstream_branch = upstream_components
        .as_ref()
        .filter(|(upstream_remote, _)| upstream_remote == &remote)
        .map(|(_, branch)| branch.clone());
    let (target_kind, remote_branch, divergent_upstream_branch) = match upstream_branch {
        // Issue #1272: a divergent upstream means pushing would update a different
        // branch, so it is never the implicit destination. The same-named branch is
        // reported while the choice is pending only so a client can describe it;
        // `push` refuses to use it until a choice is confirmed.
        Some(branch) if branch != local_branch => {
            let remote_branch = match choice {
                Some(PushTargetChoiceRequest::Upstream) => branch.clone(),
                Some(PushTargetChoiceRequest::Publish) | None => local_branch.clone(),
            };
            (PushTargetKind::Divergent, remote_branch, Some(branch))
        }
        Some(branch) => (PushTargetKind::Tracking, branch, None),
        None => (PushTargetKind::Publish, local_branch.clone(), None),
    };
    let target_reference = format!("refs/remotes/{remote}/{remote_branch}");
    let comparison_reference = if reference_exists(root, &target_reference)? {
        Some(target_reference)
    } else {
        None
    };

    Ok(PushTarget {
        local_branch,
        remote,
        remote_branch,
        target_kind,
        creates_remote_branch: comparison_reference.is_none(),
        upstream,
        divergent_upstream_branch,
        comparison_reference,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// A branch whose upstream has a different name and therefore needs the user's
/// choice before anything is pushed.
pub struct DivergentPushDestination {
    /// Short name of the local branch being pushed.
    pub local_branch: String,
    /// Remote that both choices target; the upstream lives on it.
    pub remote: String,
    /// Upstream short name such as `origin/release`.
    pub upstream: String,
    /// Branch name of the upstream on `remote`, which may itself contain slashes.
    pub upstream_branch: String,
}

impl DivergentPushDestination {
    /// Remote branch that a confirmed `choice` updates.
    pub(crate) fn remote_branch_for(&self, choice: PushTargetChoiceRequest) -> &str {
        match choice {
            PushTargetChoiceRequest::Publish => &self.local_branch,
            PushTargetChoiceRequest::Upstream => &self.upstream_branch,
        }
    }
}

/// Reports whether pushing `reference` needs a destination choice, using the same
/// resolution as `push` so a plan can ask before it commits anything.
pub(crate) fn divergent_push_destination(
    root: &str,
    reference: &str,
) -> Result<Option<DivergentPushDestination>, CoreError> {
    let target = resolve_push_target(root, Some(reference), None)?;
    Ok(
        match (
            target.target_kind,
            target.upstream,
            target.divergent_upstream_branch,
        ) {
            (PushTargetKind::Divergent, Some(upstream), Some(upstream_branch)) => {
                Some(DivergentPushDestination {
                    local_branch: target.local_branch,
                    remote: target.remote,
                    upstream,
                    upstream_branch,
                })
            }
            _ => None,
        },
    )
}

/// Push destination relationship, mirroring [`GitPushTargetKind`] inside Core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PushTargetKind {
    /// No upstream on the selected remote; publish to the same-named branch.
    Publish,
    /// Upstream and local branch share a name.
    Tracking,
    /// Upstream branch name differs from the local branch name.
    Divergent,
}

impl PushTargetKind {
    /// Contract representation returned to callers.
    fn as_contract(self) -> GitPushTargetKind {
        match self {
            Self::Publish => GitPushTargetKind::Publish,
            Self::Tracking => GitPushTargetKind::Tracking,
            Self::Divergent => GitPushTargetKind::Divergent,
        }
    }
}

fn reference_exists(root: &str, reference: &str) -> Result<bool, CoreError> {
    let result = execute_git_readonly(
        root,
        &[
            "show-ref".into(),
            "--verify".into(),
            "--quiet".into(),
            reference.to_string(),
        ],
        None,
    )?;
    if result.exit_code > 1 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not inspect Git push destination",
        )
        .with_details(result.output));
    }
    Ok(result.exit_code == 0)
}

fn default_push_remote_name(root: &str) -> Result<String, CoreError> {
    let remotes = execute_git_readonly(root, &["remote".into()], None)?;
    if remotes.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not list Git remotes")
                .with_details(remotes.output),
        );
    }
    let remote = remotes
        .output
        .lines()
        .map(str::trim)
        .find(|remote| *remote == "origin")
        .or_else(|| {
            remotes
                .output
                .lines()
                .map(str::trim)
                .find(|remote| !remote.is_empty())
        })
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "No Git remote is configured"))?;
    validate_push_component(remote)?;
    Ok(remote.to_string())
}

pub(crate) fn validate_push_component(value: &str) -> Result<(), CoreError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains(['\0', '\n', '\r'])
        || value.chars().any(char::is_whitespace)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git push destination",
        ));
    }
    Ok(())
}

pub(crate) fn publish_branch(
    root: &str,
    name: Option<&str>,
) -> Result<GitCommandResponse, CoreError> {
    let name = validated_branch_name(root, name)?;
    match optional_current_branch(root)? {
        Some(current) if current != name => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Publish the currently checked out branch",
            ));
        }
        Some(_) => {}
        None => {
            let created = execute_git(
                root,
                &["switch".into(), "-c".into(), name.clone(), "HEAD".into()],
                None,
            )?;
            if created.exit_code != 0 {
                return Ok(created);
            }
        }
    }
    execute_git(
        root,
        &[
            "push".into(),
            "--set-upstream".into(),
            "origin".into(),
            name,
        ],
        None,
    )
}
