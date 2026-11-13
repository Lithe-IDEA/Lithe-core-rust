//! Which files and directories a snapshot shows.

use super::*;

/// Effective hidden-path rules plus the nested roots this request delegates.
///
/// Matching is `lithe-core-rules`' [`shared_visibility::VisibilityRules`], the same code the
/// hosts reach through `workspace.classifyVisibility`; only the delegated roots, which depend on
/// the file system, are added here.
pub struct VisibilityRules {
    pub(crate) hidden_directories: Vec<String>,
    pub(crate) hidden_file_patterns: Vec<String>,
    /// Canonical nested roots delegated by this request, never by the global policy registry.
    pub(crate) delegated_roots: Vec<PathBuf>,
}

impl VisibilityRules {
    /// Adds the built-in rules except those turned off by a `!` entry.
    pub fn new(hidden_directories: Vec<String>, hidden_files: Vec<String>) -> Self {
        let rules = shared_visibility::VisibilityRules::new(hidden_directories, hidden_files);
        Self {
            hidden_directories: rules.hidden_directories,
            hidden_file_patterns: rules.hidden_file_patterns,
            delegated_roots: Vec::new(),
        }
    }

    /// Binds scan ownership to the caller's workspace without changing directory browsing.
    pub(crate) fn with_workspace_roots(mut self, root: &Path, roots: &[String]) -> Self {
        for value in roots {
            // Disconnected siblings must not fail an otherwise valid scoped request.
            // Missing descendants still retain their boundary until the host removes the root.
            let candidate = search_index::canonicalize_with_missing_components(Path::new(value))
                .unwrap_or_else(|| PathBuf::from(value));
            if candidate != root && candidate.starts_with(root) {
                self.delegated_roots.push(candidate);
            }
        }
        self.delegated_roots.sort();
        self.delegated_roots.dedup();
        self
    }

    pub(crate) fn is_delegated(&self, path: &Path) -> bool {
        self.delegated_roots
            .iter()
            .any(|root| path.starts_with(root))
    }

    /// Whether the workspace-relative `path` is hidden by these rules.
    pub fn is_hidden(&self, path: &str, is_directory: bool) -> bool {
        shared_visibility::is_path_hidden(
            &self.hidden_directories,
            &self.hidden_file_patterns,
            path,
            Some(is_directory),
        )
    }
}

/// How `visible_children` orders the siblings it returns. Directories always come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildOrder {
    /// The rows the project tree displays: IDEA's natural file-name order, where digit
    /// runs compare by value (see `lithe_core_rules::file_names`).
    Tree,
    /// The sequence behind flat file lists and the search index: lowercased names in
    /// byte order. Consumers that keep only the first N entries of that sequence, such
    /// as Windows Quick Open (the first 100 matches of the snapshot list), depend on it,
    /// so a display-order change must not reorder it.
    Traversal,
}

/// Visible children of one directory in the requested order, plus the nested linked
/// worktrees that were left out of it.
pub(crate) struct VisibleDirectory {
    /// Child paths and whether each is a directory, sorted by the requested `ChildOrder`.
    pub(crate) children: Vec<(PathBuf, bool)>,
    pub(crate) nested_worktrees: Vec<PathBuf>,
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// Scoped enumeration trace for pruning regressions; absent in production builds.
    pub static DIRECTORY_VISITS: std::cell::RefCell<Option<Vec<PathBuf>>> = const { std::cell::RefCell::new(None) };
}

/// Lists the children that workspace scans descend into. Snapshot and search
/// share this so the tree, the file list, and the index always agree.
pub(crate) fn visible_children(
    path: &Path,
    root: &Path,
    rules: &VisibilityRules,
    order: ChildOrder,
) -> Result<VisibleDirectory, CoreError> {
    #[cfg(any(test, feature = "test-support"))]
    DIRECTORY_VISITS.with_borrow_mut(|visits| {
        if let Some(visits) = visits {
            visits.push(path.to_path_buf());
        }
    });
    let mut nested_worktrees = Vec::new();
    let mut children = fs::read_dir(path)
        .map_err(CoreError::from)?
        .filter_map(Result::ok)
        .filter_map(|entry| visible_child(entry, root, rules, &mut nested_worktrees))
        .collect::<Vec<_>>();
    match order {
        // Siblings follow IDEA's Project view: directories first, then natural file-name
        // order, so `v0.1.4.md` precedes `v0.1.12.md`.
        ChildOrder::Tree => children.sort_by_cached_key(|(path, is_directory)| {
            (
                std::cmp::Reverse(*is_directory),
                FileNameKey::new(
                    path.file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default(),
                ),
            )
        }),
        ChildOrder::Traversal => children.sort_by(|left, right| {
            right.1.cmp(&left.1).then_with(|| {
                left.0
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_lowercase()
                    .cmp(
                        &right
                            .0
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or_default()
                            .to_lowercase(),
                    )
            })
        }),
    }
    nested_worktrees.sort();
    Ok(VisibleDirectory {
        children,
        nested_worktrees,
    })
}

/// Checks only until the first visible entry; unreadable directories remain expandable.
pub(crate) fn directory_is_visibly_empty(
    path: &Path,
    root: &Path,
    rules: &VisibilityRules,
) -> bool {
    let Ok(mut entries) = fs::read_dir(path) else {
        return false;
    };
    !entries.any(|entry| match entry {
        Ok(entry) => visible_child(entry, root, rules, &mut Vec::new()).is_some(),
        Err(_) => true,
    })
}

/// Applies the same visibility boundary to full listings and leaf probes.
fn visible_child(
    entry: fs::DirEntry,
    root: &Path,
    rules: &VisibilityRules,
    nested_worktrees: &mut Vec<PathBuf>,
) -> Option<(PathBuf, bool)> {
    let child_path = entry.path();
    // Both native and remote readDir already supplied the non-following type;
    // re-statting every file would cost one network round trip per entry.
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink()
        || rules.is_hidden(&relative_path(&child_path, root), file_type.is_dir())
    {
        return None;
    }
    if file_type.is_dir() && is_nested_linked_worktree(&child_path) {
        nested_worktrees.push(child_path);
        return None;
    }
    Some((child_path, file_type.is_dir()))
}

/// A `.git` pointer file is a single `gitdir:` line; anything larger is not one.
const MAX_GIT_POINTER_FILE_SIZE: u64 = 4 * 1024;

/// Returns whether `directory` is a linked worktree checked out inside the main
/// worktree of the repository that owns it, such as `.claude/worktrees/<name>`.
///
/// Such a checkout repeats the project's own files, and agent tools can create
/// several of them, so scanning them multiplies the tree, index, and watcher
/// load. Git records a linked worktree as `<common>/.git/worktrees/<id>`;
/// submodules (`.git/modules/...`), bare repositories, and worktrees placed
/// outside their main checkout do not match and stay visible.
pub(crate) fn is_nested_linked_worktree(directory: &Path) -> bool {
    let marker = directory.join(".git");
    let Ok(metadata) = fs::symlink_metadata(&marker) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > MAX_GIT_POINTER_FILE_SIZE {
        return false;
    }
    let Ok(text) = fs::read_to_string(&marker) else {
        return false;
    };
    let Some(git_directory) = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let git_directory = directory.join(git_directory);
    let Some(worktrees) = git_directory.parent() else {
        return false;
    };
    if worktrees.file_name().and_then(|name| name.to_str()) != Some("worktrees") {
        return false;
    }
    let Some(common_directory) = worktrees.parent() else {
        return false;
    };
    if common_directory.file_name().and_then(|name| name.to_str()) != Some(".git") {
        return false;
    }
    let Some(main_worktree) = common_directory.parent() else {
        return false;
    };
    let (Ok(main_worktree), Ok(directory)) =
        (fs::canonicalize(main_worktree), fs::canonicalize(directory))
    else {
        return false;
    };
    directory != main_worktree && directory.starts_with(&main_worktree)
}

/// Returns whether a workspace-relative path lies below a nested linked
/// worktree. Used for watcher-reported paths that never went through a scan.
pub(crate) fn is_inside_nested_linked_worktree(root: &Path, relative: &str) -> bool {
    let components = relative
        .split('/')
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let mut directory = root.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        directory.push(component);
        if is_nested_linked_worktree(&directory) {
            return true;
        }
    }
    false
}
