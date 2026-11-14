//! Commit lookups, commit comparisons and commits of selected paths through a
//! temporary index.

use super::*;

/// Owns an isolated Git administration directory for commits or read-only diffs.
pub(crate) struct TemporaryGitCommitContext {
    pub(crate) directory: PathBuf,
    pub(crate) index_path: PathBuf,
    temporary_reference: Option<String>,
}

impl TemporaryGitCommitContext {
    pub(crate) fn prepare(root: &str, head: Option<&str>) -> Result<Self, CoreError> {
        let common_directory = git_resolved_path(
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            "Git common directory",
        )?;
        Self::prepare_in(&common_directory, "lithe-commit", head)
    }

    pub(crate) fn prepare_snapshot(head: Option<&str>) -> Result<Self, CoreError> {
        if let Some(directory) =
            fs::remote_temp_directory("lithe-git-snapshot").map_err(line_history_io_error)?
        {
            let context = Self {
                index_path: directory.join("index"),
                directory,
                temporary_reference: None,
            };
            let contents = head
                .map(|head| format!("{head}\n"))
                .unwrap_or_else(|| "ref: refs/heads/lithe-snapshot\n".into());
            fs::write(context.directory.join("HEAD"), contents).map_err(line_history_io_error)?;
            return Ok(context);
        }
        // Creating even an ignored child inside .git changes its parent directory
        // on Windows. A read-only diff must not invalidate the Git watcher that
        // requested it; keep its disposable index outside repository metadata.
        Self::prepare_in(&std::env::temp_dir(), "lithe-git-snapshot", head)
    }

    pub(crate) fn prepare_in(
        parent: &Path,
        prefix: &str,
        head: Option<&str>,
    ) -> Result<Self, CoreError> {
        let (directory, sequence) = loop {
            let sequence = TEMPORARY_INDEX_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!("{prefix}-{}-{sequence}", std::process::id()));
            // Public POSIX temp roots must not expose tracked paths/object IDs.
            // Set permissions at atomic creation, before writing HEAD or index.
            match fs::create_dir_mode(&candidate, false, Some(0o700)) {
                Ok(()) => break (candidate, sequence),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(CoreError::new(
                        ErrorCode::Unknown,
                        "Could not create an isolated Git commit context",
                    )
                    .with_details(error.to_string()));
                }
            }
        };
        let temporary_reference = head.is_none().then(|| {
            format!(
                "refs/lithe/selected-commit-{}-{sequence}",
                std::process::id()
            )
        });
        let head_contents = head.map(|head| format!("{head}\n")).unwrap_or_else(|| {
            format!(
                "ref: {}\n",
                temporary_reference
                    .as_deref()
                    .expect("an unborn repository needs a temporary reference")
            )
        });
        // Establish ownership before initializing HEAD so errors also remove
        // the temporary directory through Drop.
        let context = Self {
            index_path: directory.join("index"),
            directory,
            temporary_reference,
        };
        fs::write(context.directory.join("HEAD"), head_contents).map_err(|error| {
            CoreError::new(
                ErrorCode::Unknown,
                "Could not initialize an isolated Git commit context",
            )
            .with_details(error.to_string())
        })?;
        Ok(context)
    }

    pub(crate) fn environment(&self, root: &str, common_directory: &Path) -> Vec<(String, String)> {
        vec![
            (
                "GIT_DIR".to_string(),
                self.directory.to_string_lossy().into_owned(),
            ),
            (
                "GIT_COMMON_DIR".to_string(),
                common_directory.to_string_lossy().into_owned(),
            ),
            ("GIT_WORK_TREE".to_string(), root.to_string()),
            (
                "GIT_INDEX_FILE".to_string(),
                self.index_path.to_string_lossy().into_owned(),
            ),
        ]
    }

    fn cleanup_reference(&self, root: &str) -> Result<(), CoreError> {
        let Some(reference) = self.temporary_reference.as_ref() else {
            return Ok(());
        };
        let arguments = vec!["update-ref".into(), "-d".into(), reference.clone()];
        let removed = capture_git_with_options(root, &arguments, None, false)?;
        if removed.exit_code != 0 {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Could not remove temporary Git reference",
            )
            .with_details(String::from_utf8_lossy(&removed.stderr)));
        }
        Ok(())
    }
}

impl Drop for TemporaryGitCommitContext {
    fn drop(&mut self) {
        fs::cleanup_directory(&self.directory);
    }
}

pub(crate) fn read_commit_log(
    root: &str,
    selectors: Vec<String>,
    limit: usize,
    failure_message: &str,
) -> Result<(Vec<GitCommitResponse>, bool), CoreError> {
    let mut arguments = vec!["log".to_string()];
    arguments.extend(selectors);
    arguments.extend([
        "--topo-order".to_string(),
        "--decorate=short".to_string(),
        "-n".to_string(),
        (limit.saturating_add(1)).to_string(),
        GIT_COMMIT_DATE_ARGUMENT.to_string(),
        "--pretty=format:%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%ad%x1f%s%x1f%D".to_string(),
    ]);
    let commit_output = readonly_command(GitCommandRequest {
        root: root.to_string(),
        arguments,
        input: None,
    })?;
    if commit_output.exit_code != 0 {
        return Err(CoreError::new(ErrorCode::ProcessFailed, failure_message)
            .with_details(commit_output.output));
    }

    let all_commits = commit_output
        .output
        .lines()
        .filter_map(parse_commit)
        .collect::<Vec<_>>();
    let has_more = all_commits.len() > limit;
    Ok((all_commits.into_iter().take(limit).collect(), has_more))
}

/// Resolves one commit, its parent metadata, and the message body after the subject.
pub fn commit(request: GitCommitRequest) -> Result<GitCommitLookupResponse, CoreError> {
    let root = validate_root(&request.root)?;
    validate_revision(&request.commit)?;
    let response = readonly_command(GitCommandRequest {
        root,
        arguments: vec![
            "show".to_string(),
            "-s".to_string(),
            // `log.showSignature` would otherwise print verification output ahead of
            // the machine-parsed format.
            "--no-show-signature".to_string(),
            GIT_COMMIT_DATE_ARGUMENT.to_string(),
            // The body is free-form multi-line text that may contain the unit
            // separator, so it follows a NUL that Git never emits in the header fields.
            "--pretty=format:%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%ad%x1f%s%x1f%D%x00%b".to_string(),
            request.commit,
        ],
        input: None,
    })?;
    if response.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git commit lookup failed")
                .with_details(response.output),
        );
    }
    parse_commit_lookup(&response.stdout)
        .ok_or_else(|| CoreError::new(ErrorCode::ProcessFailed, "Git commit was not found"))
}

/// Splits `git show` output produced by [`commit`] into header metadata and body.
pub(crate) fn parse_commit_lookup(output: &str) -> Option<GitCommitLookupResponse> {
    let (header, body) = output.split_once('\0')?;
    let commit = parse_commit(header.trim_end_matches(['\r', '\n']))?;
    Some(GitCommitLookupResponse {
        commit,
        // Git terminates `%b` with a line break; trailing blank lines carry no content.
        body: body.trim_end().to_string(),
    })
}

/// Lists workspace-relative files changed by one commit.
pub fn commit_files(request: GitCommitFilesRequest) -> Result<GitFilesResponse, CoreError> {
    let root = validate_root(&request.root)?;
    validate_revision(&request.commit)?;
    let response = readonly_command(GitCommandRequest {
        root,
        arguments: vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "show".to_string(),
            "--pretty=format:".to_string(),
            "--name-status".to_string(),
            "--find-renames".to_string(),
            request.commit,
        ],
        input: None,
    })?;
    if response.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git commit files failed")
                .with_details(response.output),
        );
    }
    Ok(GitFilesResponse {
        files: parse_name_status(&response.output),
    })
}

/// Computes ahead/behind counts and changed files for a reference comparison.
pub fn comparison(request: GitComparisonRequest) -> Result<GitComparisonResponse, CoreError> {
    let root = validate_root(&request.root)?;
    if request.include_untracked
        && (request.target_git_reference.is_some()
            || request
                .reference
                .as_deref()
                .is_some_and(|value| value.contains("..")))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Untracked files require a worktree comparison",
        ));
    }
    let reference = typed_reference_range(
        &root,
        request.git_reference.as_ref(),
        request.target_git_reference.as_ref(),
        request.reference.as_deref(),
    )?
    .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "Missing Git reference"))?;
    validate_revision(&reference)?;
    let response = readonly_command(GitCommandRequest {
        root: root.clone(),
        arguments: vec![
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "diff".to_string(),
            "--name-status".to_string(),
            "--find-renames".to_string(),
            reference,
            "--".to_string(),
        ],
        input: None,
    })?;
    if response.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git comparison failed")
                .with_details(response.output),
        );
    }
    let files = parse_name_status(&response.output);
    if !request.include_untracked {
        return Ok(GitComparisonResponse { files });
    }
    let snapshot = status(GitStatusRequest {
        root,
        include_index_only_changes: false,
        repository_roots: Vec::new(),
        summarize_untracked_directories: false,
    })?;
    let mut by_path: std::collections::BTreeMap<_, _> = files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect();
    for change in snapshot
        .changes
        .into_iter()
        .filter(|change| change.untracked)
    {
        by_path
            .entry(change.path.clone())
            .or_insert(GitFileResponse {
                status: "A".into(),
                path: change.path,
                is_untracked: true,
            });
    }
    Ok(GitComparisonResponse {
        files: by_path.into_values().collect(),
    })
}

pub(crate) fn commit_selected_paths(
    root: &str,
    paths: Vec<String>,
    message: String,
    amend: bool,
    identity: Vec<String>,
) -> Result<GitCommandResponse, CoreError> {
    let operation = operation_state(GitOperationStateRequest {
        root: root.to_string(),
    })?;
    if !operation.kind.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Finish or abort the current Git operation before committing selected files",
        ));
    }

    let branch = execute_git_readonly(
        root,
        &["symbolic-ref".into(), "--quiet".into(), "HEAD".into()],
        None,
    )?;
    let branch_reference = branch.stdout.trim();
    if branch.exit_code != 0 || !branch_reference.starts_with("refs/heads/") {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Selected files can only be committed on a local branch",
        ));
    }

    let head = execute_git_readonly(
        root,
        &[
            "rev-parse".to_string(),
            "--verify".to_string(),
            "-q".to_string(),
            "HEAD".to_string(),
        ],
        None,
    )?;
    if head.exit_code > 1 {
        return Ok(head);
    }
    let original_head = (head.exit_code == 0).then(|| head.stdout.trim().to_string());
    if amend && original_head.is_none() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "An initial commit cannot be amended",
        ));
    }

    // Git's own commit command still runs hooks and signing, but its HEAD and
    // index live in an isolated administration directory. The reviewed branch
    // is updated only after the resulting snapshot passes validation.
    let common_directory = git_resolved_path(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        "Git common directory",
    )?;
    let temporary_context = TemporaryGitCommitContext::prepare(root, original_head.as_deref())?;
    let environment = temporary_context.environment(root, &common_directory);
    let pathspec_input = nul_pathspec_input(&paths);
    let initialize_arguments = if let Some(head) = original_head.as_ref() {
        vec!["read-tree".to_string(), head.clone()]
    } else {
        vec!["read-tree".to_string(), "--empty".to_string()]
    };
    let initialized =
        execute_git_with_environment(root, &initialize_arguments, None, false, &environment)?;
    if initialized.exit_code != 0 {
        return Ok(initialized);
    }
    let stage_arguments = vec![
        "add".to_string(),
        "-A".to_string(),
        "--pathspec-from-file=-".to_string(),
        "--pathspec-file-nul".to_string(),
    ];
    let staged = execute_git_with_environment(
        root,
        &stage_arguments,
        Some(pathspec_input.clone()),
        false,
        &environment,
    )?;
    if staged.exit_code != 0 {
        return Ok(staged);
    }

    let allowed_paths = staged_changed_paths(root, &environment)?;

    let marker_paths = staged_conflict_marker_paths_with_environment(root, &environment, true)?;
    if !marker_paths.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Conflict markers remain in selected files",
        )
        .with_details(marker_paths.join(", ")));
    }

    let mut arguments = vec!["commit".into()];
    if amend {
        arguments.push("--amend".into());
    }
    arguments.extend(identity);
    arguments.extend(["-m".into(), message]);
    let committed = execute_git_with_environment(root, &arguments, None, false, &environment)?;
    if committed.exit_code != 0 {
        return Ok(committed);
    }

    let committed_head = execute_git_with_environment(
        root,
        &["rev-parse".into(), "--verify".into(), "HEAD".into()],
        None,
        true,
        &environment,
    )?;
    if committed_head.exit_code != 0 {
        if let Err(error) = temporary_context.cleanup_reference(root) {
            // Preserve the commit result; WARN reports a separate leaked reference.
            lithe_diagnostics::core_events::core_failed(
                "vcs.commit.cleanup_failed",
                "reference",
                &error,
            );
        }
        return Ok(committed_head);
    }
    let committed_head = committed_head.stdout.trim().to_string();
    let committed_paths = commit_changed_paths(root, original_head.as_deref(), &committed_head)?;
    let unexpected_paths = committed_paths
        .difference(&allowed_paths)
        .cloned()
        .collect::<Vec<_>>();
    if !unexpected_paths.is_empty() {
        if let Err(error) = temporary_context.cleanup_reference(root) {
            // Preserve the commit result; WARN reports a separate leaked reference.
            lithe_diagnostics::core_events::core_failed(
                "vcs.commit.cleanup_failed",
                "reference",
                &error,
            );
        }
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "A Git hook added files outside the selected commit",
        )
        .with_details(unexpected_paths.join(", ")));
    }

    let expected_old = match original_head.as_ref() {
        Some(head) => head.clone(),
        None => null_object_id(root)?,
    };
    let updated = execute_git(
        root,
        &[
            "update-ref".into(),
            "-m".into(),
            if amend {
                "commit (amend): selected files".into()
            } else {
                "commit: selected files".into()
            },
            branch_reference.to_string(),
            committed_head,
            expected_old,
        ],
        None,
    )?;
    if updated.exit_code != 0 {
        if let Err(error) = temporary_context.cleanup_reference(root) {
            // Preserve the commit result; WARN reports a separate leaked reference.
            lithe_diagnostics::core_events::core_failed(
                "vcs.commit.cleanup_failed",
                "reference",
                &error,
            );
        }
        return Ok(updated);
    }

    // Reconcile only committed paths in the real index. Unrelated staging that
    // another Git process created while hooks ran remains intact.
    let mut reconciled = execute_git(
        root,
        &[
            "reset".to_string(),
            "-q".to_string(),
            "HEAD".to_string(),
            "--pathspec-from-file=-".to_string(),
            "--pathspec-file-nul".to_string(),
        ],
        Some(pathspec_input),
    )?;
    if reconciled.exit_code != 0 {
        reconciled.warnings.push(GitOperationWarning::new(
            "git_index_reconcile_failed",
            "The commit succeeded, but the Git index could not be reconciled",
            Some(reconciled.output.clone()),
        ));
        reconciled.exit_code = 0;
    }
    if let Err(error) = temporary_context.cleanup_reference(root) {
        reconciled.warnings.push(GitOperationWarning::new(
            "git_temporary_reference_cleanup_failed",
            "The commit succeeded, but its temporary Git reference could not be removed",
            error.details,
        ));
    }
    Ok(reconciled)
}

fn staged_changed_paths(
    root: &str,
    environment: &[(String, String)],
) -> Result<HashSet<String>, CoreError> {
    let changed = execute_git_with_environment(
        root,
        &[
            "diff".into(),
            "--cached".into(),
            "--name-only".into(),
            "-z".into(),
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
    Ok(changed
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

fn commit_changed_paths(
    root: &str,
    original_head: Option<&str>,
    committed_head: &str,
) -> Result<HashSet<String>, CoreError> {
    let arguments = if let Some(original_head) = original_head {
        vec![
            "diff".into(),
            "--name-only".into(),
            "-z".into(),
            original_head.into(),
            committed_head.into(),
        ]
    } else {
        vec![
            "diff-tree".into(),
            "--root".into(),
            "--no-commit-id".into(),
            "--name-only".into(),
            "-z".into(),
            committed_head.into(),
        ]
    };
    let changed = execute_git_readonly(root, &arguments, None)?;
    if changed.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not validate the selected commit",
        )
        .with_details(changed.output));
    }
    Ok(changed
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

fn null_object_id(root: &str) -> Result<String, CoreError> {
    let format = execute_git_readonly(
        root,
        &["rev-parse".into(), "--show-object-format".into()],
        None,
    )?;
    if format.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not determine Git object format",
        )
        .with_details(format.output));
    }
    Ok(match format.stdout.trim() {
        "sha256" => "0".repeat(64),
        _ => "0".repeat(40),
    })
}

/// The `git commit` options for a chosen author and a sign-off.
pub(crate) fn commit_identity(
    author: Option<&str>,
    sign_off: bool,
) -> Result<Vec<String>, CoreError> {
    let mut arguments = Vec::new();
    if let Some(author) = author {
        arguments.push(format!("--author={}", validated_author(author)?));
    }
    if sign_off {
        arguments.push("--signoff".into());
    }
    Ok(arguments)
}

/// An exact `Name <email>` identity. Git reads any other `--author` value as a pattern
/// that picks a matching earlier author, which is never what a typed author means.
fn validated_author(author: &str) -> Result<&str, CoreError> {
    let invalid = || {
        CoreError::new(
            ErrorCode::GitCommitAuthorInvalid,
            "The author must be a name followed by an email address in angle brackets",
        )
    };
    let (name, email) = author
        .strip_suffix('>')
        .and_then(|rest| rest.rsplit_once(" <"))
        .ok_or_else(invalid)?;
    let name_valid = !name.trim().is_empty()
        && name.trim() == name
        && !name.contains(['<', '>'])
        && !name.chars().any(char::is_control);
    let email_valid = !email.is_empty()
        && !email.contains(['<', '>'])
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    if name_valid && email_valid {
        Ok(author)
    } else {
        Err(invalid())
    }
}

pub(crate) fn required_text(value: Option<&str>, label: &str) -> Result<String, CoreError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    match value {
        Some(value) if !value.contains('\0') => Ok(value.to_string()),
        _ => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("Missing or invalid Git {label}"),
        )),
    }
}
