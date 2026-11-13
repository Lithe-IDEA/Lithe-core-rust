//! Repository status, watch context, pull request context and ignored paths.

use super::*;

/// Resolves the Git administrative paths and references a watcher must observe.
pub fn watch_context(
    request: GitWatchContextRequest,
) -> Result<Option<GitWatchContextResponse>, CoreError> {
    let root = canonicalize_simplified(Path::new(&request.root)).map_err(git_path_error)?;
    if !fs::is_dir(&root)? {
        return Err(CoreError::new(
            ErrorCode::WorkspaceNotFound,
            "Workspace does not exist",
        ));
    }

    let repository_root = run_git(&root, &["rev-parse", "--show-toplevel"])?;
    if !repository_root.status.success() {
        return Ok(None);
    }
    let git_directory = run_git(&root, &["rev-parse", "--absolute-git-dir"])?;
    let git_common_directory = run_git(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;

    Ok(Some(GitWatchContextResponse {
        repository_root: canonical_git_output(repository_root, "repository root")?,
        git_directory: canonical_git_output(git_directory, "Git directory")?,
        git_common_directory: canonical_git_output(git_common_directory, "Git common directory")?,
    }))
}

fn canonical_git_output(output: std::process::Output, label: &str) -> Result<String, CoreError> {
    if !output.status.success() {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            format!("Could not resolve {label}"),
        )
        .with_details(String::from_utf8_lossy(&output.stderr)));
    }
    let raw_path = String::from_utf8_lossy(&output.stdout);
    let path = PathBuf::from(raw_path.trim_end_matches(['\r', '\n']));
    canonicalize_simplified(&path)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                return git_path_error(error);
            }
            CoreError::new(
                ErrorCode::ProcessFailed,
                format!("Could not resolve {label}"),
            )
            .with_details(error.to_string())
        })
}

/// Returns the checked-out branch and whether its HEAD must be pushed before a PR.
pub fn pull_request_context(
    request: GitPullRequestContextRequest,
) -> Result<GitPullRequestContextResponse, CoreError> {
    let root = validate_root(&request.root)?;
    let current_branch = optional_current_branch(&root)?;
    let detached = current_branch.is_none();
    let remote_default = command_value(
        &root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .map(|value| value.trim_start_matches("origin/").to_string());

    let suggested_base_branch = if let Some(branch) = current_branch.as_deref() {
        created_from_branch(&root, branch).or(remote_default)
    } else {
        detached_start_branch(&root).or(remote_default)
    };
    let requires_publish = current_branch
        .as_deref()
        .is_none_or(|branch| branch_requires_publish(&root, branch));
    let suggested_publish_branch = if requires_publish {
        current_branch.clone().or_else(|| {
            command_value(&root, &["rev-parse", "--short=8", "HEAD"])
                .map(|short_hash| format!("codex/pr-{short_hash}"))
        })
    } else {
        None
    };
    let has_uncommitted_changes = command_value(
        &root,
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .is_some();

    Ok(GitPullRequestContextResponse {
        current_branch,
        suggested_base_branch,
        suggested_publish_branch,
        requires_publish,
        detached,
        has_uncommitted_changes,
    })
}

pub(crate) fn command_value(root: &str, arguments: &[&str]) -> Option<String> {
    let arguments = arguments
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let response = execute_git_readonly(root, &arguments, None).ok()?;
    let value = response.output.trim();
    (response.exit_code == 0 && !value.is_empty()).then(|| value.to_string())
}

fn created_from_branch(root: &str, branch: &str) -> Option<String> {
    let reference = format!("refs/heads/{branch}");
    let response = command_value(root, &["reflog", "show", "--format=%gs", &reference])?;
    let prefix = "branch: Created from ";
    response.lines().find_map(|line| {
        let source = line.strip_prefix(prefix)?;
        if matches!(source, "HEAD" | "FETCH_HEAD" | "ORIG_HEAD") {
            // Publishing a detached worktree creates its branch from HEAD.
            // Preserve the worktree's original branch as the PR base instead
            // of falling back to origin/HEAD after the branch is published.
            detached_start_branch(root)
        } else {
            Some(source.trim_start_matches("origin/").to_string())
        }
    })
}

fn detached_start_branch(root: &str) -> Option<String> {
    let reflog = command_value(root, &["reflog", "show", "--format=%H", "HEAD"])?;
    // Reflog output is newest-first; the final entry is the commit at which
    // this worktree's HEAD was initialized.
    let starting_commit = reflog.lines().last()?.trim();
    let references = command_value(
        root,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)",
            "--points-at",
            starting_commit,
            "refs/heads",
            "refs/remotes/origin",
        ],
    )?;
    references
        .lines()
        .find_map(|reference| reference.strip_prefix("refs/heads/").map(str::to_string))
        .or_else(|| {
            references.lines().find_map(|reference| {
                reference
                    .strip_prefix("refs/remotes/origin/")
                    .filter(|branch| *branch != "HEAD")
                    .map(str::to_string)
            })
        })
}

fn branch_requires_publish(root: &str, branch: &str) -> bool {
    let origin_branch = format!("refs/remotes/origin/{branch}");
    if command_value(root, &["rev-parse", "--verify", &origin_branch]).is_none() {
        return true;
    }
    let comparison = format!("{origin_branch}..HEAD");
    match command_value(root, &["rev-list", "--count", &comparison])
        .and_then(|count| count.parse::<usize>().ok())
    {
        Some(0) => false,
        Some(_) | None => true,
    }
}

/// Returns the normalized repository status and branch context.
pub fn status(request: GitStatusRequest) -> Result<GitStatusResponse, CoreError> {
    let root = canonicalize_simplified(Path::new(&request.root)).map_err(git_path_error)?;
    if !fs::is_dir(&root)? {
        return Err(CoreError::new(
            ErrorCode::WorkspaceNotFound,
            "Workspace does not exist",
        ));
    }
    let repository_root_output = run_git(&root, &["rev-parse", "--show-toplevel"])?;
    if !repository_root_output.status.success() {
        return Ok(GitStatusResponse {
            repository_root: None,
            branch: None,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
        });
    }
    let repository_root_text = String::from_utf8_lossy(&repository_root_output.stdout);
    let repository_root_path = PathBuf::from(repository_root_text.trim_end_matches(['\r', '\n']));
    let repository_root =
        canonicalize_or_original(&repository_root_path).map_err(git_path_error)?;
    let branch = run_git(&repository_root, &["branch", "--show-current"])
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|branch| !branch.is_empty())
        .or_else(|| Some("detached".to_string()));
    // `normal` lets Git stop at each wholly untracked directory instead of
    // enumerating every file below it; the projection below restores file
    // entries for directories that are small or contain nested repositories.
    let untracked_files = if request.summarize_untracked_directories {
        "--untracked-files=normal"
    } else {
        "--untracked-files=all"
    };
    let status_output = run_git(
        &repository_root,
        &[
            "-c",
            "core.quotepath=false",
            "status",
            "--porcelain=v2",
            "--ignore-submodules=none",
            "-z",
            untracked_files,
        ],
    )?;
    if !status_output.status.success() {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Git status failed")
                .with_details(String::from_utf8_lossy(&status_output.stderr)),
        );
    }
    let mut changes = parse_status(&status_output.stdout, request.include_index_only_changes);
    if request.summarize_untracked_directories {
        changes = untracked_directories::expand_collapsed_directories(&repository_root, changes)?;
    }
    untracked_directories::retain_outside_known_roots(
        &mut changes,
        &repository_root,
        &request.repository_roots,
    )?;
    let (ahead, behind) = tracking_counts(&repository_root);
    Ok(GitStatusResponse {
        repository_root: Some(relative_or_absolute(&repository_root, &root)),
        branch,
        ahead,
        behind,
        changes,
    })
}

fn tracking_counts(repository_root: &Path) -> (usize, usize) {
    let Ok(output) = run_git(
        repository_root,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    ) else {
        return (0, 0);
    };
    if !output.status.success() {
        return (0, 0);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut values = text
        .split_whitespace()
        .filter_map(|value| value.parse().ok());
    let behind = values.next().unwrap_or(0);
    let ahead = values.next().unwrap_or(0);
    (ahead, behind)
}

/// Resolves ignore rules through Git, including info/exclude, global rules, and negations.
///
/// Paths are workspace-relative candidates owned by the search index. NUL-delimited
/// stdin preserves whitespace and avoids command-line limits. Missing Git/repositories
/// leave status unknown rather than guessing from directory names.
pub fn ignored_paths(
    directory: &Path,
    paths: &[String],
) -> Result<Option<HashSet<String>>, CoreError> {
    if paths.is_empty() {
        return Ok(Some(HashSet::new()));
    }
    let response = execute_git_readonly(
        &directory.to_string_lossy(),
        &["check-ignore".into(), "--stdin".into(), "-z".into()],
        Some(nul_pathspec_input(paths)),
    );
    lithe_protocol::cancellation::check()?;
    let Ok(response) = response else {
        return Ok(None);
    };
    if ![0, 1].contains(&response.exit_code) {
        return Ok(None);
    }
    Ok(Some(
        response
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
    ))
}

/// Upper bound on the pathspec bytes passed to one `git ls-files` call.
///
/// Windows limits a whole command line to 32,767 UTF-16 units; batching well
/// below it keeps large reactors from failing to start the query.
pub(crate) const LS_FILES_PATHSPEC_BUDGET: usize = 16 * 1024;

/// Splits `paths` into consecutive non-empty batches whose summed byte length
/// stays within `budget`; a single longer path forms its own batch.
pub(crate) fn pathspec_batches(paths: &[String], budget: usize) -> Vec<&[String]> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (index, path) in paths.iter().enumerate() {
        if index > start && used + path.len() > budget {
            batches.push(&paths[start..index]);
            start = index;
            used = 0;
        }
        used += path.len();
    }
    if start < paths.len() {
        batches.push(&paths[start..]);
    }
    batches
}

pub(crate) fn parse_status(output: &[u8], include_index_only_changes: bool) -> Vec<GitChange> {
    let mut changes = Vec::new();
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        let record = String::from_utf8_lossy(record);
        let (status, submodule, path, original_path) = match record.as_bytes().first() {
            Some(b'?') => ("??".to_string(), None, record[2..].to_string(), None),
            Some(kind @ (b'1' | b'2' | b'u')) => {
                // v2 headers have fixed field counts; the remaining path may
                // contain whitespace. A rename's old path is the next NUL record.
                let count = match kind {
                    b'1' => 9,
                    b'2' => 10,
                    _ => 11,
                };
                let fields = record.splitn(count, ' ').collect::<Vec<_>>();
                if fields.len() != count {
                    continue;
                }
                let status = fields[1].replace('.', " ");
                let submodule = if fields[2].starts_with('S') {
                    Some(crate::contracts::GitSubmoduleStatus {
                        commit_changed: fields[2].as_bytes().get(1) == Some(&b'C'),
                        tracked_changes: fields[2].as_bytes().get(2) == Some(&b'M'),
                        untracked_changes: fields[2].as_bytes().get(3) == Some(&b'U'),
                    })
                } else {
                    None
                };
                let original = if *kind == b'2' {
                    records
                        .next()
                        .map(|path| String::from_utf8_lossy(path).to_string())
                } else {
                    None
                };
                (status, submodule, fields[count - 1].to_string(), original)
            }
            _ => continue,
        };
        let bytes = status.as_bytes();
        if bytes.len() != 2 {
            continue;
        }
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        // Preserve the existing final-worktree projection used by Windows.
        if !include_index_only_changes && x == 'A' && y == 'D' {
            continue;
        }
        let can_toggle_staging = (x != ' ' && x != '?')
            || submodule.as_ref().is_none_or(|s| s.commit_changed)
            || x == 'U'
            || y == 'U'
            || y == 'D'
            || (x == 'A' && y == 'A');
        changes.push(GitChange {
            can_toggle_staging,
            path,
            original_path,
            status,
            staged: x != ' ' && x != '?',
            worktree: y != ' ' && y != '?',
            untracked: x == '?' && y == '?',
            submodule,
            untracked_file_count: None,
        });
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    changes
}
