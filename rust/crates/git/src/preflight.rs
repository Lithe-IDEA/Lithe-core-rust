//! Read-only checks before checkout, merge, rebase and pull, and the repository's
//! in-progress operation state.

use super::*;

/// Reports which files would block a checkout of `reference`.
///
/// A file blocks the switch when it has uncommitted changes *and* its content
/// differs between HEAD and the target ref. Files dirty in only one of those two
/// senses are carried across by Git without complaint.
pub fn checkout_preflight(
    request: GitCheckoutPreflightRequest,
) -> Result<GitCheckoutPreflightResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let reference = request_reference(
        &root,
        request.git_reference.as_ref(),
        request.reference.as_deref(),
    )?;

    let dirty = readonly_command(GitCommandRequest {
        root: root.clone(),
        arguments: vec![
            "diff".to_string(),
            "HEAD".to_string(),
            "--name-only".to_string(),
        ],
        input: None,
    })?;
    if dirty.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git diff failed").with_details(dirty.output)
        );
    }
    let dirty_paths = dirty
        .output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<std::collections::HashSet<_>>();

    // Untracked files block a checkout too, whenever the target branch tracks the same
    // path: git refuses rather than overwrite them. These never appear in `diff HEAD`.
    let untracked = readonly_command(GitCommandRequest {
        root: root.clone(),
        arguments: vec![
            "ls-files".to_string(),
            "--others".to_string(),
            "--exclude-standard".to_string(),
        ],
        input: None,
    })?;
    if untracked.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git ls-files failed")
                .with_details(untracked.output),
        );
    }
    let untracked_paths = untracked
        .output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<std::collections::HashSet<_>>();

    if dirty_paths.is_empty() && untracked_paths.is_empty() {
        return Ok(GitCheckoutPreflightResponse {
            blocking_paths: Vec::new(),
        });
    }

    let mut blocking_paths = Vec::new();
    if !untracked_paths.is_empty() {
        let tracked_on_target = readonly_command(GitCommandRequest {
            root: root.clone(),
            arguments: vec![
                "ls-tree".to_string(),
                "-r".to_string(),
                "--name-only".to_string(),
                reference.clone(),
            ],
            input: None,
        })?;
        if tracked_on_target.exit_code != 0 {
            return Err(
                CoreError::new(ErrorCode::ProcessFailed, "Git ls-tree failed")
                    .with_details(tracked_on_target.output),
            );
        }
        blocking_paths.extend(
            tracked_on_target
                .output
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && untracked_paths.contains(line))
                .map(str::to_string),
        );
    }

    if dirty_paths.is_empty() {
        blocking_paths.sort();
        blocking_paths.dedup();
        return Ok(GitCheckoutPreflightResponse { blocking_paths });
    }

    let divergent = readonly_command(GitCommandRequest {
        root,
        arguments: vec![
            "diff".to_string(),
            "HEAD".to_string(),
            reference,
            "--name-only".to_string(),
        ],
        input: None,
    })?;
    if divergent.exit_code != 0 {
        return Err(CoreError::new(ErrorCode::ProcessFailed, "Git diff failed")
            .with_details(divergent.output));
    }

    blocking_paths.extend(
        divergent
            .output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && dirty_paths.contains(line))
            .map(str::to_string),
    );
    blocking_paths.sort();
    blocking_paths.dedup();
    Ok(GitCheckoutPreflightResponse { blocking_paths })
}

/// Lists staged files that still contain conflict markers.
///
/// Only `<<<<<<< ` and friends with their trailing space are matched: a bare
/// `=======` is also a Markdown heading underline, and matching it flags ordinary
/// documentation. `|||||||` covers the diff3 conflict style. Git skips binary
/// files itself, so a blob containing those bytes cannot trip this.
pub fn conflict_marker_paths(
    request: GitConflictMarkerRequest,
) -> Result<GitConflictMarkerResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let paths = staged_conflict_marker_paths(&root)?;
    Ok(GitConflictMarkerResponse { paths })
}

pub(crate) fn staged_conflict_marker_paths(root: &str) -> Result<Vec<String>, CoreError> {
    staged_conflict_marker_paths_with_environment(root, &[], false)
}

pub(crate) fn staged_conflict_marker_paths_with_environment(
    root: &str,
    environment: &[(String, String)],
    changed_only: bool,
) -> Result<Vec<String>, CoreError> {
    // The alternate index starts at HEAD, so its staged diff is the authoritative
    // expansion of directory and glob pathspecs without repeating a long path list.
    let changed_paths = if changed_only {
        let changed = execute_git_with_environment(
            root,
            &[
                "diff".to_string(),
                "--cached".to_string(),
                "--no-ext-diff".to_string(),
                "--name-only".to_string(),
                "-z".to_string(),
            ],
            None,
            true,
            environment,
        )?;
        if changed.exit_code != 0 {
            return Err(
                CoreError::new(ErrorCode::ProcessFailed, "Git staged diff failed")
                    .with_details(changed.output),
            );
        }
        Some(
            changed
                .stdout
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<HashSet<_>>(),
        )
    } else {
        None
    };
    let arguments = vec![
        "grep".to_string(),
        "--cached".to_string(),
        "-l".to_string(),
        "-z".to_string(),
        "-E".to_string(),
        r"^(<<<<<<<|>>>>>>>|\|\|\|\|\|\|\|) ".to_string(),
    ];
    let found = execute_git_with_environment(root, &arguments, None, true, environment)?;
    // `git grep` exits 1 when nothing matches, which is not a failure here.
    if found.exit_code > 1 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git grep failed").with_details(found.output)
        );
    }

    let mut paths: Vec<String> = found
        .stdout
        .split('\0')
        .filter(|path| {
            !path.is_empty()
                && changed_paths
                    .as_ref()
                    .is_none_or(|changed| changed.contains(*path))
        })
        .map(str::to_string)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// How an operation decides whether a dirty working tree is in its way.
enum IntegrationShape {
    /// Refuses only over files differing between the merge base and the target.
    MergeBase,
    /// Refuses over any uncommitted change, however unrelated.
    AnyDirty,
    /// Refuses only over files the single replayed commit touches.
    SingleCommit,
}

/// Reports what would stop a merge, rebase, cherry-pick, or revert from starting.
///
/// The rules differ and were each checked against Git directly: merge, cherry-pick
/// and revert refuse only when a dirty file overlaps what they would write, while a
/// rebase refuses on any uncommitted change, staged or not, however unrelated.
/// Matching Git's stderr is not an option since it is localized.
pub fn integration_preflight(
    request: GitIntegrationPreflightRequest,
) -> Result<GitIntegrationPreflightResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let reference = request_reference(
        &root,
        request.git_reference.as_ref(),
        request.reference.as_deref(),
    )?;
    let shape = match request.operation.as_str() {
        "merge" => IntegrationShape::MergeBase,
        "rebase" => IntegrationShape::AnyDirty,
        // Verified against Git: both refuse only when a dirty file overlaps what the
        // commit touches, exactly like a merge. The overlap set differs though, since
        // they replay one commit rather than joining two branches.
        "cherryPick" | "revert" => IntegrationShape::SingleCommit,
        other => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!("Unknown integration operation '{other}'"),
            ))
        }
    };

    // Tracked files differing from HEAD, staged or not: `diff HEAD` covers both.
    let dirty = readonly_command(GitCommandRequest {
        root: root.clone(),
        arguments: vec![
            "diff".to_string(),
            "HEAD".to_string(),
            "--name-only".to_string(),
        ],
        input: None,
    })?;
    if dirty.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git diff failed").with_details(dirty.output)
        );
    }
    let dirty_paths: Vec<String> = dirty
        .output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();

    if dirty_paths.is_empty() {
        return Ok(GitIntegrationPreflightResponse {
            blocking_paths: Vec::new(),
            blocks_entirely: false,
        });
    }

    // A rebase stops for any uncommitted change, so every dirty path is blocking
    // and there is no overlap to compute.
    if matches!(shape, IntegrationShape::AnyDirty) {
        let mut blocking_paths = dirty_paths;
        blocking_paths.sort();
        blocking_paths.dedup();
        return Ok(GitIntegrationPreflightResponse {
            blocking_paths,
            blocks_entirely: true,
        });
    }

    // Everything else refuses only over files it would write. For a merge those are
    // the files differing since the merge base; for a single replayed commit they
    // are the files that commit changed against its own parent.
    let written = match shape {
        IntegrationShape::MergeBase => {
            let base = readonly_command(GitCommandRequest {
                root: root.clone(),
                arguments: vec![
                    "merge-base".to_string(),
                    "HEAD".to_string(),
                    reference.clone(),
                ],
                input: None,
            })?;
            if base.exit_code != 0 {
                return Err(
                    CoreError::new(ErrorCode::ProcessFailed, "Git merge-base failed")
                        .with_details(base.output),
                );
            }
            readonly_command(GitCommandRequest {
                root,
                arguments: vec![
                    "diff".to_string(),
                    base.output.trim().to_string(),
                    reference,
                    "--name-only".to_string(),
                ],
                input: None,
            })?
        }
        // `diff-tree` against the commit itself; the extra flags make a merge commit
        // and the root commit behave rather than print nothing.
        IntegrationShape::SingleCommit => readonly_command(GitCommandRequest {
            root,
            arguments: vec![
                "diff-tree".to_string(),
                "-r".to_string(),
                "-m".to_string(),
                "--root".to_string(),
                "--name-only".to_string(),
                "--no-commit-id".to_string(),
                reference,
            ],
            input: None,
        })?,
        IntegrationShape::AnyDirty => unreachable!("handled above"),
    };
    if written.exit_code != 0 {
        return Err(CoreError::new(ErrorCode::ProcessFailed, "Git diff failed")
            .with_details(written.output));
    }
    let written_paths = written
        .output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<std::collections::HashSet<_>>();

    let mut blocking_paths: Vec<String> = dirty_paths
        .into_iter()
        .filter(|path| written_paths.contains(path.as_str()))
        .collect();
    blocking_paths.sort();
    blocking_paths.dedup();
    Ok(GitIntegrationPreflightResponse {
        blocking_paths,
        blocks_entirely: false,
    })
}

/// Reports whether a pull can fast-forward, so the UI can ask before it fails.
///
/// Git's own refusal for a divergent pull is a multi-line hint block that is
/// localized, so the counts are computed here instead. Fetching first would make
/// the numbers fresher, but this stays read-only: the caller decides when to hit
/// the network.
pub fn pull_preflight(
    request: GitPullPreflightRequest,
) -> Result<GitPullPreflightResponse, CoreError> {
    let root = validate_root(&request.root)?;

    let upstream = readonly_command(GitCommandRequest {
        root: root.clone(),
        arguments: vec![
            "rev-parse".to_string(),
            "--abbrev-ref".to_string(),
            "--symbolic-full-name".to_string(),
            "@{upstream}".to_string(),
        ],
        input: None,
    })?;
    // A non-zero exit here means "no upstream configured", not a failure.
    if upstream.exit_code != 0 {
        return Ok(GitPullPreflightResponse {
            upstream: None,
            ahead: 0,
            behind: 0,
            diverged: false,
            has_local_changes: false,
            configured_strategy: None,
        });
    }
    let upstream = upstream.output.trim().to_string();

    let counts = readonly_command(GitCommandRequest {
        root: root.clone(),
        arguments: vec![
            "rev-list".to_string(),
            "--left-right".to_string(),
            "--count".to_string(),
            format!("{upstream}...HEAD"),
        ],
        input: None,
    })?;
    if counts.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git rev-list failed")
                .with_details(counts.output),
        );
    }
    // `--left-right --count` prints "<behind>\t<ahead>" for `upstream...HEAD`.
    let mut fields = counts.output.split_whitespace();
    let behind = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let ahead = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);

    let dirty = readonly_command(GitCommandRequest {
        root: root.clone(),
        arguments: vec![
            "status".to_string(),
            "--porcelain".to_string(),
            "--untracked-files=no".to_string(),
        ],
        input: None,
    })?;
    if dirty.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git status failed")
                .with_details(dirty.output),
        );
    }

    Ok(GitPullPreflightResponse {
        upstream: Some(upstream),
        ahead,
        behind,
        diverged: ahead > 0 && behind > 0,
        has_local_changes: !dirty.output.trim().is_empty(),
        configured_strategy: configured_pull_strategy(&root)?,
    })
}

/// The pull strategy Git is explicitly configured for, or `None` when neither
/// `pull.rebase` nor `pull.ff` says anything. `git.updatePlan` falls back to
/// fast-forward only in that case.
fn configured_pull_strategy(root: &str) -> Result<Option<&'static str>, CoreError> {
    let rebase = read_git_config_value(root, "pull.rebase")?;
    let fast_forward = read_git_config_value(root, "pull.ff")?;
    let enabled = |value: &Option<String>, accepted: &[&str]| {
        value
            .as_deref()
            .is_some_and(|value| accepted.contains(&value.to_ascii_lowercase().as_str()))
    };
    Ok(
        if enabled(
            &rebase,
            &["true", "yes", "on", "1", "interactive", "merges"],
        ) {
            Some("rebase")
        } else if enabled(&fast_forward, &["only"]) {
            Some("ffOnly")
        } else if enabled(&fast_forward, &["false", "no", "off", "0"])
            || enabled(&rebase, &["false", "no", "off", "0"])
        {
            Some("merge")
        } else {
            None
        },
    )
}

/// Resolves the Git directory once, honoring worktrees and submodules where
/// `.git` is a file pointing elsewhere rather than a directory.
pub(crate) fn git_directory(root: &str) -> Result<Option<PathBuf>, CoreError> {
    let response = execute_git_readonly(
        root,
        &["rev-parse".to_string(), "--absolute-git-dir".to_string()],
        None,
    )?;
    if response.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git rev-parse failed")
                .with_details(response.output),
        );
    }
    let raw = response.output.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(raw)))
}

/// Reads a single-line numeric counter written by an in-progress rebase.
fn rebase_counter(directory: &std::path::Path, name: &str) -> Option<usize> {
    fs::read_to_string(directory.join(name))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Reports whichever sequential operation Git has left half-finished.
///
/// Detection reads the marker files Git itself writes, so an operation stopped by
/// a conflict is reported the same way whether the user started it in Lithe or on
/// the command line.
pub fn operation_state(
    request: GitOperationStateRequest,
) -> Result<GitOperationStateResponse, CoreError> {
    let root = validate_root(&request.root)?;

    let mut kind = String::new();
    let mut reference = None;
    let mut step = None;
    let mut total = None;

    // Order matters: a conflicted rebase can carry a REVERT_HEAD from the commit
    // it is replaying, so the more specific rebase directories are checked first.
    if let Some(git_directory) = git_directory(&root)? {
        let rebase_merge = git_directory.join("rebase-merge");
        let rebase_apply = git_directory.join("rebase-apply");
        let operation_directory = if fs::try_exists(&rebase_merge)? {
            Some(rebase_merge)
        } else if fs::try_exists(&rebase_apply)? {
            Some(rebase_apply)
        } else {
            None
        };
        if let Some(directory) = operation_directory {
            kind = "rebase".to_string();
            step = rebase_counter(&directory, "msgnum");
            total = rebase_counter(&directory, "end");
            reference = fs::read_to_string(directory.join("onto"))
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
        } else if fs::try_exists(git_directory.join("MERGE_HEAD"))? {
            kind = "merge".to_string();
        } else if fs::try_exists(git_directory.join("CHERRY_PICK_HEAD"))? {
            kind = "cherryPick".to_string();
        } else if fs::try_exists(git_directory.join("REVERT_HEAD"))? {
            kind = "revert".to_string();
        }
    }

    let status = execute_git_readonly(
        &root,
        &["status".to_string(), "--porcelain".to_string()],
        None,
    )?;
    if status.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git status failed")
                .with_details(status.output),
        );
    }

    let mut conflicted_paths = status
        .output
        .lines()
        .filter(|line| line.len() > 3 && is_conflicted_status(&line[..2]))
        .map(|line| line[3..].trim().to_string())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    conflicted_paths.sort();
    conflicted_paths.dedup();

    Ok(GitOperationStateResponse {
        kind,
        reference,
        step,
        total,
        conflicted_paths,
    })
}

/// Continues, aborts, or skips whichever operation is currently in progress.
///
/// The subcommand depends on what Git left behind, so the state is read first
/// rather than trusted from the caller: the UI's view of it may be a refresh
/// behind, and issuing `rebase --continue` during a merge would just fail.
pub(crate) fn resolve_operation(
    root: &str,
    operation: &str,
) -> Result<GitCommandResponse, CoreError> {
    if let Some(response) = rebase_session::resolve_owned_operation(root, operation)? {
        return Ok(response);
    }
    let state = operation_state(GitOperationStateRequest {
        root: root.to_string(),
    })?;
    if state.kind.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "No Git operation is in progress",
        ));
    }

    let subcommand = match state.kind.as_str() {
        "rebase" => "rebase",
        "merge" => "merge",
        "cherryPick" => "cherry-pick",
        "revert" => "revert",
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Unsupported Git operation state",
            ))
        }
    };

    let action = match operation {
        "operationContinue" => "--continue",
        "operationAbort" => "--abort",
        _ => "--skip",
    };

    // Only a rebase can skip a step; merge and cherry-pick have no equivalent.
    if action == "--skip" && subcommand != "rebase" {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Only a rebase can skip a step",
        ));
    }

    if action == "--continue" && !state.conflicted_paths.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Resolve the conflicted files before continuing",
        )
        .with_details(state.conflicted_paths.join("\n")));
    }

    // `--continue` opens an editor for the commit message by default, which would
    // hang a process launched from the GUI with no terminal attached. Pointing the
    // editor at `true` makes Git accept the message it already prepared and exit.
    // `merge --continue` rejects extra arguments, so this must stay a config
    // override rather than a `--no-edit` flag.
    let arguments = vec![
        "-c".to_string(),
        "core.editor=true".to_string(),
        subcommand.to_string(),
        action.to_string(),
    ];

    execute_git(root, &arguments, None)
}

/// The porcelain status pairs Git uses for an unresolved merge conflict.
fn is_conflicted_status(code: &str) -> bool {
    matches!(code, "UU" | "AA" | "DD" | "DU" | "UD" | "AU" | "UA")
}
