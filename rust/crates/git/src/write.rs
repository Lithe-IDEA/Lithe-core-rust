//! `git.write`: the staged-index, commit, branch, tag, stash, remote and worktree
//! mutations routed through one request.

use super::*;

// Shared by ordinary staging and reviewed commit selection under its existing lease.
pub(crate) fn stage_paths(root: &str, paths: &[String]) -> Result<GitCommandResponse, CoreError> {
    let paths = validate_paths(paths)?;
    let arguments = vec![
        "add".into(),
        "-A".into(),
        "--pathspec-from-file=-".into(),
        "--pathspec-file-nul".into(),
    ];
    let literal = [("GIT_LITERAL_PATHSPECS".to_string(), "1".to_string())];
    execute_git_with_environment(
        root,
        &arguments,
        Some(nul_pathspec_input(&paths)),
        false,
        &literal,
    )
}

pub(crate) fn unstage_paths(root: &str, paths: &[String]) -> Result<GitCommandResponse, CoreError> {
    let paths = validate_paths(paths)?;
    let literal = [("GIT_LITERAL_PATHSPECS".to_string(), "1".to_string())];
    let input = nul_pathspec_input(&paths);
    let head = execute_git(
        root,
        &["rev-parse".into(), "--verify".into(), "HEAD".into()],
        None,
    )?;
    if head.exit_code != 0 && commit_state::is_unborn(root)? {
        let arguments = vec![
            "rm".into(),
            "--cached".into(),
            "--force".into(),
            "--ignore-unmatch".into(),
            "--pathspec-from-file=-".into(),
            "--pathspec-file-nul".into(),
        ];
        return execute_git_with_environment(root, &arguments, Some(input), false, &literal);
    }
    let restore = vec![
        "restore".into(),
        "--staged".into(),
        "--pathspec-from-file=-".into(),
        "--pathspec-file-nul".into(),
    ];
    let response =
        execute_git_with_environment(root, &restore, Some(input.clone()), false, &literal)?;
    if response.exit_code == 0 {
        return Ok(response);
    }
    let reset = vec![
        "reset".into(),
        "HEAD".into(),
        "--pathspec-from-file=-".into(),
        "--pathspec-file-nul".into(),
    ];
    execute_git_with_environment(root, &reset, Some(input), false, &literal)
}

/// Executes one supported repository mutation without invoking a shell.
pub fn write(request: GitWriteRequest) -> Result<GitCommandResponse, CoreError> {
    let mut response = with_git_invocation_trace(|| write_with_trace(request))?;
    authentication_failure::classify(&mut response);
    Ok(response)
}

fn write_with_trace(request: GitWriteRequest) -> Result<GitCommandResponse, CoreError> {
    if request.fetch_options.is_some() && request.operation != "fetch" {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Fetch options require a fetch operation",
        ));
    }
    if request.check_submodules && request.operation != "push" {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Submodule publication checks require a push operation",
        ));
    }
    if request.expected_branch.is_some() && request.operation != "pull" {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Branch preconditions require a pull operation",
        ));
    }
    if request.operation != "commit"
        && (!request.gitlink_updates.is_empty()
            || (request.expected_commit_state.is_some() && request.operation != "push"))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Commit state preconditions require a commit or push operation",
        ));
    }
    let root = validate_root(&request.root)?;
    // Every typed writer shares the same repository lease, including linked
    // worktrees. Clone has no existing repository whose state it could race.
    let _lease = if request.operation == "clone" {
        None
    } else {
        Some(rewrite::RewriteLease::acquire(&root)?)
    };
    let mut arguments: Vec<String>;

    match request.operation.as_str() {
        "stage" => return stage_paths(&root, &request.paths),
        "unstage" => return unstage_paths(&root, &request.paths),
        "discard" => {
            let paths = validate_paths(&request.paths)?;
            let literal_paths = [("GIT_LITERAL_PATHSPECS".to_string(), "1".to_string())];
            let restore_arguments = ["restore", "--worktree", "--"]
                .into_iter()
                .map(String::from)
                .chain(paths.clone())
                .collect::<Vec<_>>();
            let restore = execute_git_with_environment(
                &root,
                &restore_arguments,
                None,
                false,
                &literal_paths,
            )?;
            if restore.exit_code == 0 {
                return Ok(restore);
            }
            let mut status_arguments = vec![
                "status".to_string(),
                "--porcelain".to_string(),
                "--".to_string(),
            ];
            status_arguments.extend(paths.clone());
            let status =
                execute_git_with_environment(&root, &status_arguments, None, true, &literal_paths)?;
            let is_untracked = status.exit_code == 0
                && status
                    .output
                    .lines()
                    .any(|line| line.starts_with("??") || line.starts_with("!!"));
            if !is_untracked {
                return Ok(restore);
            }
            arguments = ["clean", "-f", "-d", "--"]
                .into_iter()
                .map(String::from)
                .chain(paths)
                .collect();
            return execute_git_with_environment(&root, &arguments, None, false, &literal_paths);
        }
        "discardAll" => {
            let paths = validate_paths(&request.paths)?;
            return discard_all(&root, &paths);
        }
        "stageAll" => arguments = vec!["add".into(), "--all".into()],
        "commit" => {
            let message = required_text(request.message.as_deref(), "commit message")?;
            if request.expected_commit_state.is_some() && !request.paths.is_empty() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Planned commits use the reviewed index",
                ));
            }
            if let Some(expected) = &request.expected_commit_state {
                commit_state::prepare(&root, expected, &request.gitlink_updates)?;
            } else if !request.gitlink_updates.is_empty() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Submodule updates require a reviewed commit state",
                ));
            }
            let identity = commit_identity(request.author.as_deref(), request.sign_off)?;
            if !request.paths.is_empty() {
                let paths = validate_paths(&request.paths)?;
                return commit_selected_paths(&root, paths, message, request.amend, identity);
            }
            arguments = vec!["commit".into()];
            if request.amend {
                arguments.push("--amend".into());
            }
            arguments.extend(identity);
            arguments.extend(["-m".into(), message]);
        }
        "ignore" => {
            return append_git_ignore_patterns(&root, &request.paths, GitIgnoreTarget::Repository)
        }
        "exclude" => {
            return append_git_ignore_patterns(&root, &request.paths, GitIgnoreTarget::LocalExclude)
        }
        "cherryPick" => {
            arguments = vec![
                "cherry-pick".into(),
                validated_revision(request.revision.as_deref())?,
            ];
        }
        "revert" => {
            arguments = vec![
                "revert".into(),
                "--no-edit".into(),
                validated_revision(request.revision.as_deref())?,
            ];
        }
        "reset" => {
            let mode = request.mode.as_deref().unwrap_or("--mixed");
            if !["--soft", "--mixed", "--hard"].contains(&mode) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Unsupported reset mode",
                ));
            }
            arguments = vec![
                mode.into(),
                validated_revision(request.revision.as_deref())?,
            ];
            arguments.insert(0, "reset".into());
        }
        "undoCommit" | "editCommitMessage" | "deleteCommit" | "squashCommits" => {
            return rewrite::execute(&root, &request);
        }
        "createBranch" => {
            let name = validated_branch_name(&root, request.name.as_deref())?;
            let reference = write_request_reference(&root, &request)?;
            // Creating a branch is not "checking out a remote branch": it must not
            // inherit an upstream. With Git's default `branch.autoSetupMerge`, a
            // branch started at `origin/main` would otherwise track `refs/heads/main`,
            // and the next push would carry the explicit `refs/heads/<name>:refs/heads/main`
            // refspec straight over the remote main branch (issue #1272). Freezing the
            // result with `--no-track` keeps it independent of user Git preferences,
            // matching the worktree creation path. Deliberate upstream setup stays
            // available through `setUpstream` and the `checkout` path.
            arguments = if request.checkout {
                vec![
                    "switch".into(),
                    "-c".into(),
                    name,
                    "--no-track".into(),
                    reference,
                ]
            } else {
                vec!["branch".into(), "--no-track".into(), name, reference]
            };
        }
        "publishBranch" => {
            return publish_branch(&root, request.name.as_deref());
        }
        "renameBranch" => {
            let name = validated_branch_name(&root, request.name.as_deref())?;
            let reference = write_request_reference(&root, &request)?;
            let current = current_branch(&root)?;
            let current_reference = format!("refs/heads/{current}");
            arguments = if reference == current || reference == current_reference {
                vec!["branch".into(), "-m".into(), name]
            } else {
                vec!["branch".into(), "-m".into(), reference, name]
            };
        }
        "setUpstream" => {
            let branch = validated_branch_name(&root, request.name.as_deref())?;
            let upstream = request
                .git_reference
                .as_ref()
                .ok_or_else(invalid_git_reference)
                .and_then(|reference| validated_git_reference(&root, reference))?;
            if upstream.kind != "remote" {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "A branch upstream must be a remote Git reference",
                ));
            }
            arguments = vec![
                "branch".into(),
                format!("--set-upstream-to={}", upstream.full_name),
                branch,
            ];
        }
        "unsetUpstream" => {
            let branch = validated_branch_name(&root, request.name.as_deref())?;
            arguments = vec!["branch".into(), "--unset-upstream".into(), branch];
        }
        "deleteBranch" => {
            let reference = write_request_reference(&root, &request)?;
            let branch = local_branch_name(&reference)?;
            if current_branch(&root)?.as_str() == branch {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The current branch cannot be deleted",
                ));
            }
            return delete_branch(&root, &branch);
        }
        "merge" => {
            let reference = write_request_reference(&root, &request)?;
            if is_current_reference(&root, &reference)? {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The current branch cannot be merged into itself",
                ));
            }
            arguments = vec!["merge".into(), "--no-edit".into(), reference];
        }
        "rebase" => {
            let reference = write_request_reference(&root, &request)?;
            if is_current_reference(&root, &reference)? {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The current branch cannot be rebased onto itself",
                ));
            }
            arguments = vec!["rebase".into(), reference];
        }
        "checkoutAndRebase" => return mutations::checkout_and_rebase(&root, request),
        "createWorktree" => return create_worktree(&root, &request),
        "removeWorktree" | "lockWorktree" | "unlockWorktree" => {
            return mutate_worktree(&root, &request)
        }
        "pruneWorktrees" => {
            arguments = vec![
                "worktree".into(),
                "prune".into(),
                "--verbose".into(),
                "--expire=now".into(),
            ]
        }
        "repairWorktrees" => arguments = vec!["worktree".into(), "repair".into()],
        "updateBranch" => return update_local_branch(&root, &request),
        "fetch" => {
            if execution_policy::current().detailed_fetch {
                return fetch_execution::execute(&root, request.fetch_options);
            }
            let options = configuration::fetch_options(&root, request.fetch_options)?;
            arguments = fetch::arguments(&options)?;
            if let Some(remote) = &options.remote {
                if read_git_config_value(&root, &format!("remote.{remote}.url"))?.is_none() {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Fetch remote is not configured",
                    ));
                }
            }
        }
        // Strategy comes from the caller because only the user can decide whether a
        // divergent history should be merged or replayed. Absent a choice we stay on
        // `--ff-only`, which refuses rather than inventing a merge commit.
        "pull" => {
            if let Some(expected) = request.expected_branch.as_deref() {
                if !expected.starts_with("refs/heads/") || expected == "refs/heads/" {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Expected branch must be a complete local reference",
                    ));
                }
                // Repeat the frontend's post-Fetch check under the shared writer
                // lease. A changed checkout must never update its replacement.
                let current = execute_git(
                    &root,
                    &["symbolic-ref".into(), "--quiet".into(), "HEAD".into()],
                    None,
                )?;
                if current.exit_code != 0 || current.output.trim() != expected {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The selected branch changed; review and retry",
                    ));
                }
                if request.auto_stash
                    || request.reference.is_some()
                    || request.git_reference.is_some()
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Guarded background updates use the fetched upstream without auto-stash",
                    ));
                }
                // The host already fetched. Freeze the selected branch's upstream
                // commit and integrate locally; `git pull` would fetch again and
                // reopen a long window for an external client to switch HEAD.
                let upstream = execute_git(
                    &root,
                    &[
                        "rev-parse".into(),
                        "--verify".into(),
                        // Git's upstream selector takes a branch name, while the
                        // checkout guards above/below compare complete refs.
                        format!(
                            "{}@{{upstream}}^{{commit}}",
                            expected.strip_prefix("refs/heads/").unwrap()
                        ),
                    ],
                    None,
                )?;
                if upstream.exit_code != 0 {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The selected branch has no fetched upstream commit",
                    ));
                }
                let revision = upstream.stdout.trim().to_owned();
                arguments = match request.mode.as_deref() {
                    None | Some("ffOnly") => vec!["merge".into(), "--ff-only".into(), revision],
                    Some("merge") => vec!["merge".into(), "--no-edit".into(), revision],
                    Some("rebase") => vec!["rebase".into(), revision],
                    Some(other) => {
                        return Err(CoreError::new(
                            ErrorCode::InvalidRequest,
                            format!("Unknown pull strategy '{other}'"),
                        ))
                    }
                };
                let current = execute_git(
                    &root,
                    &["symbolic-ref".into(), "--quiet".into(), "HEAD".into()],
                    None,
                )?;
                if current.exit_code != 0 || current.output.trim() != expected {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The selected branch changed; review and retry",
                    ));
                }
                return execute_git(&root, &arguments, None);
            }
            arguments = match request.mode.as_deref() {
                None | Some("ffOnly") => vec!["pull".into(), "--ff-only".into()],
                Some("merge") => vec!["pull".into(), "--no-rebase".into(), "--no-edit".into()],
                Some("rebase") => vec!["pull".into(), "--rebase".into()],
                Some(other) => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        format!("Unknown pull strategy '{other}'"),
                    ))
                }
            };
            let explicit_reference = if let Some(reference) = request.git_reference.as_ref() {
                let reference = validated_git_reference(&root, reference)?;
                if reference.kind != "remote" {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Explicit pull requires a remote Git reference",
                    ));
                }
                Some(reference.full_name)
            } else if let Some(reference) = request.reference.as_deref() {
                if request.reference_kind.as_deref() != Some("remote") {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Explicit pull requires a remote Git reference",
                    ));
                }
                Some(validated_reference(Some(reference))?)
            } else {
                None
            };
            if let Some(reference) = explicit_reference {
                let (remote, branch) = mutations::remote_branch_components(&root, &reference)?;
                arguments.extend(["--".into(), remote, branch]);
            }
            if request.auto_stash {
                return pull_with_auto_stash(&root, &arguments);
            }
        }
        "deleteRemoteBranch" => {
            let reference = request
                .git_reference
                .as_ref()
                .ok_or_else(invalid_git_reference)?;
            let reference = validated_git_reference(&root, reference)?;
            if reference.kind != "remote" {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Remote branch deletion requires a remote Git reference",
                ));
            }
            let (remote, branch) =
                mutations::remote_branch_components(&root, &reference.full_name)?;
            arguments = vec![
                "push".into(),
                "--delete".into(),
                "--".into(),
                remote,
                format!("refs/heads/{branch}"),
            ];
        }
        "push" => {
            if let Some(expected) = &request.expected_commit_state {
                if commit_state::inspect_root(&root)? != *expected {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Push state changed; review the commit plan again",
                    ));
                }
            }
            let reference = optional_write_request_reference(&root, &request)?;
            return push(
                &root,
                reference.as_deref(),
                request.force,
                request.push_tags.as_deref(),
                request.expected_push.as_ref(),
                request.check_submodules,
                request.push_target,
            );
        }
        "checkout" => return checkout(&root, request),
        "checkoutRevision" => {
            arguments = vec![
                "switch".into(),
                "--detach".into(),
                validated_revision(request.revision.as_deref())?,
            ];
        }
        "createTag" => {
            let name = validated_tag_name(request.name.as_deref())?;
            let requested_target = validated_revision(request.revision.as_deref())?;
            // Reject an impossible tag form before probing the repository; the
            // typed fields only refine the legacy message-based tag choice.
            validate_create_tag_form(
                request.message.as_deref(),
                request.lightweight,
                request.signed,
            )?;
            // Existence and resolvability probes run before Git so a duplicate
            // or unresolvable target fails with a stable message instead of
            // leaving the caller to parse localized `git tag` stderr.
            if tag_exists(&root, &name)? {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    format!("A tag named '{name}' already exists"),
                ));
            }
            // Git can tag trees and blobs, but the deletion/restore contract
            // promises a commit target, so anything else is rejected here and
            // the tag is created against the resolved commit id.
            let Some(target) = resolved_commit_target(&root, &requested_target)? else {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    format!("Could not resolve tag target '{requested_target}'"),
                ));
            };
            arguments = create_tag_arguments(
                &name,
                &target,
                request.message.as_deref(),
                request.lightweight,
                request.signed,
            );
        }
        "deleteTag" => return delete_tag(&root, request.name.as_deref()),
        "clone" => {
            let remote = required_text(request.remote.as_deref(), "clone source")?;
            let destination = required_text(request.destination.as_deref(), "clone destination")?;
            if destination.starts_with('-') || destination.contains('\0') {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid clone destination",
                ));
            }
            let destination = lithe_workspace::remote_path(Path::new(&destination))
                .to_string_lossy()
                .into_owned();
            arguments = vec!["clone".into(), "--".into(), remote, destination];
        }
        "stashPush" => {
            arguments = vec!["stash".into(), "push".into()];
            if request.include_untracked {
                arguments.push("--include-untracked".into());
            }
            if let Some(message) = request.message.filter(|value| !value.trim().is_empty()) {
                arguments.extend(["-m".into(), message]);
            }
            if !request.paths.is_empty() {
                let paths = validate_paths(&request.paths)?;
                arguments.extend([
                    "--pathspec-from-file=-".into(),
                    "--pathspec-file-nul".into(),
                ]);
                return execute_git(&root, &arguments, Some(nul_pathspec_input(&paths)));
            }
        }
        "operationContinue" | "operationAbort" | "operationSkip" => {
            return resolve_operation(&root, &request.operation)
        }
        "stashApply" | "stashDrop" => {
            let requested = validated_stash_reference(request.reference.as_deref())?;
            // Resolve immutable selections under the repository write lease. Older
            // callers may still supply a positional stash reference.
            let reference = if matches!(requested.len(), 40 | 64)
                && requested.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                stash_reference_for_oid(&root, &requested)?.ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The selected stash no longer exists",
                    )
                })?
            } else {
                requested
            };
            if request.operation == "stashApply" {
                return apply_stash(&root, &reference);
            }
            arguments = vec!["stash".into(), "drop".into(), reference];
        }
        "stashPop" => {
            let requested = validated_stash_reference(request.reference.as_deref())?;
            // Resolve immutable selections under the repository write lease. Older
            // callers may still supply a positional stash reference.
            let reference = if matches!(requested.len(), 40 | 64)
                && requested.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                stash_reference_for_oid(&root, &requested)?.ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The selected stash no longer exists",
                    )
                })?
            } else {
                requested
            };
            return pop_stash(&root, &reference);
        }
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Unsupported Git write operation",
            ))
        }
    }

    execute_git(&root, &arguments, None)
}

/// Discards both index and working-tree content for an explicitly confirmed rollback.
/// The normal `discard` operation deliberately preserves staged content, while
/// this explicit operation is destructive for the whole path.
fn discard_all(root: &str, paths: &[String]) -> Result<GitCommandResponse, CoreError> {
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    let mut tracked_paths = HashSet::new();
    let mut untracked_paths = HashSet::new();
    let literal_paths = [("GIT_LITERAL_PATHSPECS".to_string(), "1".to_string())];
    let mut final_response = successful_git_result();
    // Rollback is often called once per selected file. Never enumerate unrelated
    // untracked trees for every file, and keep path arguments below Windows limits.
    for batch in pathspec_batches(paths, LS_FILES_PATHSPEC_BUDGET) {
        let mut arguments = vec![
            "status".to_string(),
            "--porcelain=v2".to_string(),
            "-z".to_string(),
            "--untracked-files=all".to_string(),
            "--".to_string(),
        ];
        arguments.extend_from_slice(batch);
        let status = execute_git_with_environment(root, &arguments, None, true, &literal_paths)?;
        if status.exit_code != 0 {
            return Ok(status);
        }
        // The shared parser consumes rename source records too; a source name
        // beginning with `?? ` must not be mistaken for an untracked path.
        for change in parse_status(status.stdout.as_bytes(), true) {
            if change.untracked {
                untracked_paths.insert(change.path);
            } else {
                tracked_paths.insert(change.path);
            }
        }
        final_response = status;
    }
    for path in paths {
        // A summarized untracked directory (`dir/`) is reported file by file.
        // A staged deletion can coexist with an untracked disk copy at the
        // same path. Restore HEAD rather than cleaning that retained copy.
        let is_untracked = !tracked_paths.contains(path.as_str())
            && (untracked_paths.contains(path.as_str())
                || (path.ends_with('/')
                    && untracked_paths
                        .iter()
                        .any(|untracked| untracked.starts_with(path.as_str()))));
        if is_untracked {
            untracked.push(path.clone());
        } else {
            tracked.push(path.clone());
        }
    }

    if !tracked.is_empty() {
        let arguments = vec![
            "restore".to_string(),
            "--source=HEAD".to_string(),
            "--staged".to_string(),
            "--worktree".to_string(),
            "--pathspec-from-file=-".to_string(),
            "--pathspec-file-nul".to_string(),
        ];
        // Passing pathspecs over stdin avoids the Windows command-line limit for
        // large source-control selections and preserves unusual path characters.
        let pathspec_input = tracked
            .iter()
            .flat_map(|path| [path.as_str(), "\0"])
            .collect::<String>();
        let restored = execute_git_with_environment(
            root,
            &arguments,
            Some(pathspec_input),
            false,
            &literal_paths,
        )?;
        if restored.exit_code != 0 {
            return Ok(restored);
        }
        final_response = restored;
    }
    for batch in pathspec_batches(&untracked, LS_FILES_PATHSPEC_BUDGET) {
        let mut arguments = vec![
            "clean".to_string(),
            "-f".to_string(),
            "-d".to_string(),
            "--".to_string(),
        ];
        arguments.extend_from_slice(batch);
        let cleaned = execute_git_with_environment(root, &arguments, None, false, &literal_paths)?;
        if cleaned.exit_code != 0 {
            return Ok(cleaned);
        }
        final_response = cleaned;
    }
    Ok(final_response)
}
