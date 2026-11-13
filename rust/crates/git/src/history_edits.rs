//! Commit-list edits behind the history actions: edited, deleted and squashed commits
//! replayed onto a new head.

use super::*;

/// Immutable commit fields needed to rebuild a linear history without invoking
/// an editor or losing author and committer attribution.
#[derive(Clone)]
pub(crate) struct RewriteCommit {
    /// Original commit object used to derive the patch during history replay.
    hash: String,
    tree: String,
    pub(crate) parents: Vec<String>,
    author_name: String,
    author_email: String,
    author_date: String,
    committer_name: String,
    committer_email: String,
    committer_date: String,
    pub(crate) message: String,
}

/// Current local branch context and its first-parent chain, ordered from HEAD
/// toward the root commit.
pub(crate) struct HistoryRewriteContext {
    pub(crate) branch_reference: String,
    pub(crate) original_head: String,
    pub(crate) first_parent_chain: Vec<String>,
    pub(crate) published_commits: HashSet<String>,
}

pub(crate) fn edited_history_head(
    root: &str,
    context: &HistoryRewriteContext,
    revision: &str,
    message: &str,
) -> Result<String, CoreError> {
    let target = resolve_commit_revision(root, revision)?;
    let target_index = history_commit_index(context, &target)?;
    let commits = checked_rewrite_range(root, context, target_index)?;
    let mut parent = commits[0].parents.first().cloned();

    for (index, commit) in commits.iter().enumerate() {
        let commit_message = if index == 0 { message } else { &commit.message };
        parent = Some(write_commit_tree(
            root,
            commit,
            parent.as_deref(),
            commit_message,
        )?);
    }

    Ok(parent.expect("rewrite range contains a commit"))
}

pub(crate) fn deleted_history_head(
    root: &str,
    context: &HistoryRewriteContext,
    revision: &str,
) -> Result<String, CoreError> {
    let target = resolve_commit_revision(root, revision)?;
    let target_index = history_commit_index(context, &target)?;
    let commits = checked_rewrite_range(root, context, target_index)?;
    let target_commit = &commits[0];
    let parent = target_commit.parents.first().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "The root commit cannot be deleted",
        )
    })?;

    let mut rewritten_head = parent.clone();
    for commit in commits.iter().skip(1) {
        let mut replayed_commit = commit.clone();
        replayed_commit.tree = replay_commit_tree(root, commit, &rewritten_head)?;
        rewritten_head = write_commit_tree(
            root,
            &replayed_commit,
            Some(&rewritten_head),
            &commit.message,
        )?;
    }

    Ok(rewritten_head)
}

pub(crate) fn squashed_history_head(
    root: &str,
    context: &HistoryRewriteContext,
    revisions: &[String],
    message: &str,
) -> Result<String, CoreError> {
    if revisions.len() < 2 {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Select at least two commits to squash",
        ));
    }

    let mut selected = Vec::with_capacity(revisions.len());
    for revision in revisions {
        validate_revision(revision)?;
        selected.push(resolve_commit_revision(root, revision)?);
    }
    selected.sort();
    selected.dedup();
    if selected.len() != revisions.len() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Select distinct commits to squash",
        ));
    }

    let mut selected_indices = selected
        .iter()
        .map(|commit| history_commit_index(context, commit))
        .collect::<Result<Vec<_>, _>>()?;
    selected_indices.sort_unstable();
    let newest_index = selected_indices[0];
    let oldest_index = *selected_indices
        .last()
        .expect("at least two commits were selected");
    if oldest_index - newest_index + 1 != selected_indices.len() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Only a contiguous range of commits can be squashed",
        ));
    }

    let commits = checked_rewrite_range(root, context, oldest_index)?;
    let selected_count = oldest_index - newest_index + 1;
    let oldest = &commits[0];
    let newest = &commits[selected_count - 1];
    let mut squashed = oldest.clone();
    squashed.tree.clone_from(&newest.tree);
    squashed.committer_name.clone_from(&newest.committer_name);
    squashed.committer_email.clone_from(&newest.committer_email);
    squashed.committer_date.clone_from(&newest.committer_date);

    let mut parent = Some(write_commit_tree(
        root,
        &squashed,
        oldest.parents.first().map(String::as_str),
        message,
    )?);
    for commit in commits.iter().skip(selected_count) {
        parent = Some(write_commit_tree(
            root,
            commit,
            parent.as_deref(),
            &commit.message,
        )?);
    }

    Ok(parent.expect("squash produces a commit"))
}

pub(crate) fn resolve_commit_revision(root: &str, revision: &str) -> Result<String, CoreError> {
    let response = execute_git_readonly(
        root,
        &[
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            "--end-of-options".into(),
            format!("{revision}^{{commit}}"),
        ],
        None,
    )?;
    let commit = response.output.trim();
    if response.exit_code != 0 || commit.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The selected Git commit does not exist",
        ));
    }
    Ok(commit.to_string())
}

fn history_commit_index(context: &HistoryRewriteContext, commit: &str) -> Result<usize, CoreError> {
    context
        .first_parent_chain
        .iter()
        .position(|candidate| candidate == commit)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Only commits on the current branch first-parent history can be rewritten",
            )
        })
}

fn checked_rewrite_range(
    root: &str,
    context: &HistoryRewriteContext,
    oldest_index: usize,
) -> Result<Vec<RewriteCommit>, CoreError> {
    let hashes = &context.first_parent_chain[..=oldest_index];
    if hashes
        .iter()
        .any(|commit| context.published_commits.contains(commit))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Commits published to a remote cannot be rewritten",
        ));
    }

    let mut commits = hashes
        .iter()
        .rev()
        .map(|hash| read_rewrite_commit(root, hash))
        .collect::<Result<Vec<_>, _>>()?;
    if commits.iter().any(|commit| commit.parents.len() > 1) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "A history range containing merge commits cannot be rewritten",
        ));
    }
    commits.shrink_to_fit();
    Ok(commits)
}

pub(crate) fn read_rewrite_commit(root: &str, hash: &str) -> Result<RewriteCommit, CoreError> {
    let format = "%T%x00%P%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%B%x00";
    let response = execute_git_readonly(
        root,
        &[
            "show".into(),
            "--no-patch".into(),
            "--no-show-signature".into(),
            format!("--format={format}"),
            hash.to_string(),
        ],
        None,
    )?;
    if response.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not read a commit selected for history rewrite",
        )
        .with_details(response.output));
    }

    let output = response
        .output
        .strip_suffix("\0\n")
        .or_else(|| response.output.strip_suffix('\0'))
        .unwrap_or(&response.output);
    let fields = output.splitn(9, '\0').collect::<Vec<_>>();
    if fields.len() != 9 {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Could not decode a commit selected for history rewrite",
        ));
    }
    Ok(RewriteCommit {
        hash: hash.to_string(),
        tree: fields[0].to_string(),
        parents: fields[1].split_whitespace().map(String::from).collect(),
        author_name: fields[2].to_string(),
        author_email: fields[3].to_string(),
        author_date: fields[4].to_string(),
        committer_name: fields[5].to_string(),
        committer_email: fields[6].to_string(),
        committer_date: fields[7].to_string(),
        message: fields[8].to_string(),
    })
}

fn replay_commit_tree(
    root: &str,
    commit: &RewriteCommit,
    rewritten_parent: &str,
) -> Result<String, CoreError> {
    let original_parent = commit.parents.first().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "A root commit cannot be replayed after deleting its parent",
        )
    })?;
    let patch_arguments = vec![
        "diff-tree".to_string(),
        "--binary".to_string(),
        "--full-index".to_string(),
        "--no-ext-diff".to_string(),
        "--no-renames".to_string(),
        "--no-commit-id".to_string(),
        "-p".to_string(),
        original_parent.clone(),
        commit.hash.clone(),
    ];
    let patch = execute_git_readonly(root, &patch_arguments, None)?;
    if patch.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not read changes after the deleted commit",
        )
        .with_details(patch.output));
    }

    let common_directory = git_resolved_path(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        "Git common directory",
    )?;
    let temporary_context = TemporaryGitCommitContext::prepare(root, Some(rewritten_parent))?;
    let environment = temporary_context.environment(root, &common_directory);
    let initialized = execute_git_with_environment(
        root,
        &["read-tree".to_string(), rewritten_parent.to_string()],
        None,
        false,
        &environment,
    )?;
    if initialized.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not initialize history replay",
        )
        .with_details(initialized.output));
    }

    if !patch.stdout.is_empty() {
        let applied = execute_git_with_environment(
            root,
            &[
                "apply".to_string(),
                "--cached".to_string(),
                "--whitespace=nowarn".to_string(),
                "-".to_string(),
            ],
            Some(patch.stdout),
            false,
            &environment,
        )?;
        if applied.exit_code != 0 {
            return Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Could not replay changes after the deleted commit",
            )
            .with_details(applied.output));
        }
    }

    let tree =
        execute_git_with_environment(root, &["write-tree".to_string()], None, false, &environment)?;
    if tree.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not write replayed Git tree",
        )
        .with_details(tree.output));
    }
    let tree_hash = tree.stdout.trim();
    if tree_hash.is_empty() {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Git did not return a replayed tree identifier",
        ));
    }
    Ok(tree_hash.to_string())
}

fn write_commit_tree(
    root: &str,
    commit: &RewriteCommit,
    parent: Option<&str>,
    message: &str,
) -> Result<String, CoreError> {
    let mut arguments = vec![
        "-c".into(),
        "commit.gpgSign=false".into(),
        "commit-tree".into(),
        commit.tree.clone(),
    ];
    if let Some(parent) = parent {
        arguments.extend(["-p".into(), parent.to_string()]);
    }
    arguments.extend(["-F".into(), "-".into()]);
    let environment = vec![
        ("GIT_AUTHOR_NAME".into(), commit.author_name.clone()),
        ("GIT_AUTHOR_EMAIL".into(), commit.author_email.clone()),
        ("GIT_AUTHOR_DATE".into(), commit.author_date.clone()),
        ("GIT_COMMITTER_NAME".into(), commit.committer_name.clone()),
        ("GIT_COMMITTER_EMAIL".into(), commit.committer_email.clone()),
        ("GIT_COMMITTER_DATE".into(), commit.committer_date.clone()),
    ];
    let output = capture_git_with_environment(
        root,
        &arguments,
        Some(message.to_string()),
        false,
        &environment,
    )?;
    if output.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not rebuild Git commit history",
        )
        .with_details(output.into_command_response(&arguments).output));
    }
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Git did not return a rebuilt commit identifier",
        ));
    }
    Ok(hash)
}

pub(crate) fn update_history_reference(
    root: &str,
    context: &HistoryRewriteContext,
    new_head: &str,
    reflog_message: &str,
) -> Result<GitCommandResponse, CoreError> {
    execute_git(
        root,
        &[
            "update-ref".into(),
            "-m".into(),
            reflog_message.into(),
            context.branch_reference.clone(),
            new_head.into(),
            context.original_head.clone(),
        ],
        None,
    )
}
