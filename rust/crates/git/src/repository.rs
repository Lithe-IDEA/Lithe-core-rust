//! Repository discovery and path validation: workspace repository scans, root and
//! `.git` resolution, and the pathspec checks every command applies.

use super::*;

/// Discovers Git repositories for an opened workspace using shared traversal rules.
pub fn workspace_repositories(
    request: WorkspaceRepositoriesRequest,
) -> Result<WorkspaceRepositoriesResponse, CoreError> {
    let workspace_root = PathBuf::from(validate_root(&request.root)?);
    let max_directories = request.max_directories;
    let max_depth = request.max_depth;
    let mut discovered_repositories = HashSet::new();
    let containing_repository = discover_containing_repository(&workspace_root)?;

    if let Some(repository) = &containing_repository {
        discovered_repositories.insert(repository.clone());
    }

    let mut queue = VecDeque::from([(workspace_root.clone(), 0usize)]);
    let mut visited_directories = HashSet::new();

    while let Some((directory, depth)) = queue.pop_front() {
        lithe_protocol::cancellation::check()?;
        if visited_directories.len() >= max_directories {
            break;
        }

        let canonical_directory = match canonicalize_simplified(&directory) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(repository_scan_error(error)),
        };
        if !canonical_directory.starts_with(&workspace_root) {
            continue;
        }
        if !visited_directories.insert(canonical_directory.clone()) {
            continue;
        }

        let entries = fs::read_dir(&canonical_directory).map_err(repository_scan_error)?;
        let mut child_directories = Vec::new();
        for entry in entries {
            lithe_protocol::cancellation::check()?;
            let entry = entry.map_err(repository_scan_error)?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                // Only openable repositories join the workspace list; a dangling gitfile
                // entry must not poison status and reference loads for sibling roots.
                if git_entry_opens_as_repository(&entry.path()) {
                    discovered_repositories.insert(canonical_directory.clone());
                }
                continue;
            }
            if !repository_scan_directory_is_visible(&name) {
                continue;
            }
            let file_type = entry.file_type().map_err(repository_scan_error)?;
            if file_type.is_dir() {
                child_directories.push(entry.path());
            }
        }
        child_directories.sort_by(|left, right| {
            path_sort_key(left)
                .cmp(&path_sort_key(right))
                .then_with(|| left.cmp(right))
        });

        if depth >= max_depth {
            continue;
        }
        for child_directory in child_directories {
            queue.push_back((child_directory, depth + 1));
        }
    }

    let mut repositories = discovered_repositories
        .into_iter()
        .collect::<Vec<PathBuf>>();
    sort_workspace_repository_paths(&mut repositories, &workspace_root);
    if let Some(repository) = containing_repository {
        repositories.retain(|path| path != &repository);
        repositories.insert(0, repository);
    }

    Ok(WorkspaceRepositoriesResponse {
        repositories: repositories
            .into_iter()
            .map(|path| WorkspaceRepositoryResponse {
                path: path.to_string_lossy().replace('\\', "/"),
            })
            .collect(),
    })
}

fn repository_scan_directory_is_visible(name: &str) -> bool {
    // These intentional checkout containers are hidden in the file tree but
    // remain discoverable in Git. Apply exclusions to their children as usual.
    if name.eq_ignore_ascii_case(".worktree") || name.eq_ignore_ascii_case(".worktrees") {
        return true;
    }
    // Filter descendants, never the explicitly opened root or its containing
    // repository. Git ignore rules govern tracked files, not repository ownership.
    !name.starts_with('.')
        && !lithe_core_rules::visibility::BUILT_IN_HIDDEN_DIRECTORIES
            .iter()
            .any(|hidden| name.eq_ignore_ascii_case(hidden))
}

/// Rejects Windows names that cannot round-trip through ordinary path identifiers.
/// POSIX names keep their native meaning; Windows validates relative paths too.
pub(crate) fn validate_windows_git_path(path: &Path) -> std::io::Result<()> {
    let text = path.to_string_lossy().replace('\\', "/");
    let windows_path = cfg!(windows)
        || text.starts_with("//")
        || (text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic());
    if !windows_path {
        return Ok(());
    }
    let verbatim = text.strip_prefix("//?/");
    let ordinary = match verbatim {
        Some(rest)
            if rest
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("UNC/")) =>
        {
            format!("//{}", &rest[4..])
        }
        Some(rest) => rest.to_string(),
        None => text.clone(),
    };
    let drive = ordinary.as_bytes().get(1..3) == Some(b":/")
        && ordinary.as_bytes()[0].is_ascii_alphabetic();
    let network = ordinary.starts_with("//");
    // Server/share names identify the UNC route, not local file components.
    let components: Vec<_> = ordinary
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let skip = if network {
        2
    } else if drive {
        1
    } else {
        0
    };
    let invalid_component = components.iter().skip(skip).any(|part| {
        if matches!(*part, "." | "..") {
            return verbatim.is_some();
        }
        let basename = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let reserved = matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || basename
                .strip_prefix("COM")
                .or_else(|| basename.strip_prefix("LPT"))
                .is_some_and(|number| {
                    matches!(
                        number,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                });
        part.ends_with('.') || part.ends_with(' ') || reserved
    });
    if text.starts_with("//./")
        || (verbatim.is_some() && !drive && !(network && components.len() >= 2))
        || invalid_component
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Git does not support Windows paths requiring verbatim semantics (trailing dots/spaces, reserved names or device namespaces)",
        ));
    }
    Ok(())
}

/// Removes a Windows verbatim prefix only for paths safe in ordinary native form.
/// Canonical roots cross the UI boundary and return as Git working directories;
/// unsupported names fail explicitly instead of silently accessing a sibling.
pub fn simplified_canonical_path(path: PathBuf) -> std::io::Result<PathBuf> {
    validate_windows_git_path(&path)?;
    let text = path.to_string_lossy();
    let normalized = text.replace('\\', "/");
    if normalized
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/UNC/"))
    {
        Ok(PathBuf::from(format!(r"\\{}", &text[8..])))
    } else if normalized.starts_with("//?/") {
        Ok(PathBuf::from(&text[4..]))
    } else {
        Ok(path)
    }
}

/// Validates before filesystem I/O and again after canonicalization (symlinks).
pub(crate) fn canonicalize_simplified(path: &Path) -> std::io::Result<PathBuf> {
    validate_windows_git_path(path)?;
    fs::canonicalize(path).and_then(simplified_canonical_path)
}

/// Missing worktrees may still be listed, but invalid path identities never fall back.
pub(crate) fn canonicalize_or_original(path: &Path) -> std::io::Result<PathBuf> {
    match canonicalize_simplified(path) {
        Err(error) if error.kind() != std::io::ErrorKind::InvalidInput => Ok(path.to_path_buf()),
        result => result,
    }
}

pub(crate) fn git_path_error(error: std::io::Error) -> CoreError {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        CoreError::new(ErrorCode::InvalidRequest, error.to_string())
    } else {
        CoreError::new(ErrorCode::WorkspaceNotFound, "Workspace does not exist")
    }
}

pub(crate) fn validate_root(raw_root: &str) -> Result<String, CoreError> {
    let root = canonicalize_simplified(Path::new(raw_root)).map_err(git_path_error)?;
    if !fs::is_dir(&root)? {
        return Err(CoreError::new(
            ErrorCode::WorkspaceNotFound,
            "Workspace does not exist",
        ));
    }
    Ok(root.to_string_lossy().to_string())
}

fn repository_scan_error(error: std::io::Error) -> CoreError {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        return git_path_error(error);
    }
    CoreError::new(
        ErrorCode::Unknown,
        "Could not complete workspace repository discovery",
    )
    .with_details(error.to_string())
}

/// Resolves the gitdir a `.git` gitfile points at, mirroring Git's `read_gitfile`:
/// the target may be absolute or relative to the gitfile's own directory.
/// Returns `None` when the file is not a valid gitfile Git would open.
fn resolve_gitfile_gitdir(gitfile: &Path, contents: &str) -> Option<PathBuf> {
    let line = contents.lines().next()?;
    let target = line.strip_prefix("gitdir:")?.trim().trim_matches('"');
    if target.is_empty() {
        return None;
    }
    let target_path = Path::new(target);
    if target_path.is_absolute() {
        return Some(target_path.to_path_buf());
    }
    Some(gitfile.parent()?.join(target_path))
}

/// A discovered `.git` entry only counts as a repository when Git can open it:
/// a directory needs `HEAD`, and a gitfile pointer must resolve to a gitdir that
/// holds `HEAD`. Stale submodule or worktree pointers (for example a leftover
/// submodule gitfile whose `.git/modules` directory was removed) would otherwise
/// be listed as a workspace repository and fail every status or reference load
/// for the healthy sibling repositories.
fn git_entry_opens_as_repository(git_path: &Path) -> bool {
    if git_path.is_dir() {
        return git_path.join("HEAD").is_file();
    }
    let contents = match std::fs::read_to_string(git_path) {
        Ok(contents) => contents,
        Err(_) => return false,
    };
    resolve_gitfile_gitdir(git_path, &contents)
        .map(|gitdir| gitdir.join("HEAD").is_file())
        .unwrap_or(false)
}

pub(crate) fn discover_containing_repository(root: &Path) -> Result<Option<PathBuf>, CoreError> {
    let output = execute_git_readonly(
        &root.to_string_lossy(),
        &["rev-parse".into(), "--show-toplevel".into()],
        None,
    )?;
    if output.exit_code != 0 {
        return Ok(None);
    }
    let repository_root = output.stdout.trim_end_matches(['\r', '\n']);
    if repository_root.is_empty() {
        return Ok(None);
    }
    // Discovery must not return an unprojected host path after an IO failure.
    let path = canonicalize_simplified(Path::new(repository_root)).map_err(git_path_error)?;
    Ok(Some(path))
}

fn sort_workspace_repository_paths(paths: &mut [PathBuf], workspace_root: &Path) {
    paths.sort_by(|left, right| {
        let left_is_workspace = left == workspace_root;
        let right_is_workspace = right == workspace_root;
        if left_is_workspace != right_is_workspace {
            return right_is_workspace.cmp(&left_is_workspace);
        }

        let left_inside_workspace = left.starts_with(workspace_root);
        let right_inside_workspace = right.starts_with(workspace_root);
        if left_inside_workspace != right_inside_workspace {
            return right_inside_workspace.cmp(&left_inside_workspace);
        }

        let left_depth = left.components().count();
        let right_depth = right.components().count();
        left_depth
            .cmp(&right_depth)
            .then_with(|| path_sort_key(left).cmp(&path_sort_key(right)))
            .then_with(|| left.cmp(right))
    });
}

fn path_sort_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

pub(crate) fn validate_paths(paths: &[String]) -> Result<Vec<String>, CoreError> {
    if paths.is_empty() || paths.iter().any(|path| !is_safe_pathspec(path)) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git operation contains an invalid path",
        ));
    }
    Ok(paths.to_vec())
}

pub(crate) fn repository_root(root: &str) -> Result<PathBuf, CoreError> {
    git_resolved_path(root, &["rev-parse", "--show-toplevel"], "repository root")
}

pub(crate) fn git_path(root: &str, path: &str) -> Result<PathBuf, CoreError> {
    git_resolved_path(
        root,
        &["rev-parse", "--path-format=absolute", "--git-path", path],
        "Git metadata path",
    )
}

pub(crate) fn git_resolved_path(
    root: &str,
    arguments: &[&str],
    label: &str,
) -> Result<PathBuf, CoreError> {
    let arguments = arguments
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let response = execute_git_readonly(root, &arguments, None)?;
    if response.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            format!("Could not resolve {label}"),
        )
        .with_details(response.output));
    }
    let path = response.output.trim_end_matches(['\r', '\n']);
    if path.is_empty() {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            format!("Could not resolve {label}"),
        ));
    }
    validate_windows_git_path(Path::new(path)).map_err(git_path_error)?;
    Ok(lithe_workspace::local_path(Path::new(path)))
}

pub(crate) fn nul_pathspec_input(paths: &[String]) -> String {
    paths
        .iter()
        .flat_map(|path| [path.as_str(), "\0"])
        .collect()
}

pub(crate) fn empty_tree_oid(root: &str) -> Result<String, CoreError> {
    let hashed = execute_git_readonly(
        root,
        &[
            "hash-object".to_string(),
            "-t".to_string(),
            "tree".to_string(),
            "--stdin".to_string(),
        ],
        Some(String::new()),
    )?;
    if hashed.exit_code != 0 {
        return Err(CoreError::new(
            ErrorCode::ProcessFailed,
            "Could not resolve Git's empty tree",
        )
        .with_details(hashed.output));
    }
    let oid = hashed.stdout.trim();
    validate_revision(oid)?;
    Ok(oid.to_string())
}

pub(crate) fn is_safe_pathspec(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    !normalized.is_empty()
        && !normalized.starts_with('/')
        && !normalized.split('/').any(|component| component == "..")
        && !normalized.contains(':')
}

pub(crate) fn null_device() -> &'static str {
    #[cfg(windows)]
    {
        "NUL"
    }
    #[cfg(not(windows))]
    {
        "/dev/null"
    }
}

pub(crate) fn relative_or_absolute(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| {
            let value = relative.to_string_lossy().replace('\\', "/");
            if value.is_empty() {
                ".".to_string()
            } else {
                value
            }
        })
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}
