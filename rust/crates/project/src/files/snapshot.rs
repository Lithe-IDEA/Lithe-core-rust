//! Workspace snapshots and directory listings.

use super::*;

/// Scans visible workspace entries and invalidates any stale search index.
pub fn snapshot(request: WorkspaceSnapshotRequest) -> Result<WorkspaceSnapshotResponse, CoreError> {
    let root = existing_root(&request.root)?;
    let indexing_policy = crate::indexing::context(&root);
    search_index::invalidate_root(&root);
    let rules = VisibilityRules::new(request.hidden_directory_names, request.hidden_file_patterns)
        .with_workspace_roots(&root, &request.workspace_roots);
    let mut files = Vec::new();
    let mut nested_worktrees = Vec::new();
    let mut incomplete_directories = Vec::new();
    let node = if request.include_tree {
        scan_node(
            &root,
            &root,
            &rules,
            &mut files,
            &mut nested_worktrees,
            &mut incomplete_directories,
        )?
    } else {
        collect_snapshot_files(
            &root,
            &root,
            &rules,
            &mut files,
            &mut nested_worktrees,
            &mut incomplete_directories,
        )?;
        WorkspaceNode {
            path: String::new(),
            name: node_name(&root),
            is_directory: true,
            children: None,
        }
    };
    if let Some(policy) = indexing_policy {
        policy.check()?;
    }
    Ok(WorkspaceSnapshotResponse {
        root: node,
        files,
        nested_worktrees,
        incomplete_directories,
    })
}

/// Lists the visible children of each requested directory, one level deep.
///
/// The project tree loads directories as they are expanded, so a workspace
/// with many files no longer builds a node for each of them up front. Paths
/// that no longer exist, are hidden, or lie inside a nested linked worktree
/// are left out of the response instead of failing the whole request, because
/// a directory can disappear between the host's expand and this call.
pub fn list_directories(
    request: DirectoryListingRequest,
) -> Result<DirectoryListingResponse, CoreError> {
    let root = existing_root(&request.root)?;
    let rules = VisibilityRules::new(request.hidden_directory_names, request.hidden_file_patterns);
    let mut directories = Vec::new();
    for relative in request.paths {
        lithe_protocol::cancellation::check()?;
        let Some(directory) = listable_directory(&root, &relative, &rules) else {
            continue;
        };
        let listing = match visible_children(&directory, &root, &rules, ChildOrder::Tree) {
            Ok(listing) => listing,
            Err(error)
                if matches!(
                    error.code,
                    ErrorCode::PermissionDenied | ErrorCode::WorkspaceNotFound
                ) =>
            {
                continue
            }
            Err(error) => return Err(error),
        };
        let children = listing
            .children
            .into_iter()
            .map(|(path, is_directory)| WorkspaceNode {
                path: relative_path(&path, &root),
                name: node_name(&path),
                is_directory,
                children: if is_directory
                    && !crate::indexing::is_excluded(&root, &path, true).unwrap_or(true)
                    && directory_is_visibly_empty(&path, &root, &rules)
                {
                    Some(Vec::new())
                } else {
                    None
                },
            })
            .collect();
        directories.push(DirectoryListing {
            path: relative_path(&directory, &root),
            children,
            nested_worktrees: listing
                .nested_worktrees
                .iter()
                .map(|worktree| relative_path(worktree, &root))
                .collect(),
        });
    }
    Ok(DirectoryListingResponse { directories })
}

/// Resolves a requested directory without following symlinks, so a listing
/// never reaches outside the workspace or into a directory scans skip.
fn listable_directory(root: &Path, relative: &str, rules: &VisibilityRules) -> Option<PathBuf> {
    if relative.is_empty() {
        return Some(root.to_path_buf());
    }
    if Path::new(relative).is_absolute() || invalid_relative_path(relative) {
        return None;
    }
    let mut current = root.to_path_buf();
    for component in relative.split('/') {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).ok()?;
        if !metadata.is_dir() || rules.is_hidden(&relative_path(&current, root), true) {
            return None;
        }
    }
    let nested =
        is_inside_nested_linked_worktree(root, relative) || is_nested_linked_worktree(&current);
    (!nested).then_some(current)
}

fn scan_node(
    path: &Path,
    root: &Path,
    rules: &VisibilityRules,
    files: &mut Vec<String>,
    nested_worktrees: &mut Vec<String>,
    incomplete_directories: &mut Vec<String>,
) -> Result<WorkspaceNode, CoreError> {
    lithe_protocol::cancellation::check()?;
    let metadata = fs::symlink_metadata(path)?;
    let relative = relative_path(path, root);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Workspace")
        .to_string();
    if rules.is_delegated(path) || crate::indexing::is_excluded(root, path, metadata.is_dir())? {
        return Ok(WorkspaceNode {
            path: relative,
            name,
            is_directory: metadata.is_dir(),
            children: None,
        });
    }
    if !metadata.is_dir() {
        files.push(relative.clone());
        return Ok(WorkspaceNode {
            path: relative,
            name,
            is_directory: false,
            children: None,
        });
    }

    let listing = visible_children(path, root, rules, ChildOrder::Traversal)?;
    nested_worktrees.extend(
        listing
            .nested_worktrees
            .iter()
            .map(|worktree| relative_path(worktree, root)),
    );
    let mut visible_children = Vec::new();
    for (child_path, _) in listing.children {
        lithe_protocol::cancellation::check()?;
        match scan_node(
            &child_path,
            root,
            rules,
            files,
            nested_worktrees,
            incomplete_directories,
        ) {
            Ok(child) => visible_children.push(child),
            Err(error)
                if matches!(
                    error.code,
                    ErrorCode::PermissionDenied | ErrorCode::WorkspaceNotFound
                ) =>
            {
                incomplete_directories.push(relative_path(&child_path, root));
                lithe_diagnostics::core_events::directory_skipped(
                    "app.workspace.directory_skipped",
                    &relative_path(&child_path, root),
                    &error.code,
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(WorkspaceNode {
        path: relative,
        name,
        is_directory: true,
        children: Some(visible_children),
    })
}

/// Collects the snapshot file list without building tree nodes.
fn collect_snapshot_files(
    path: &Path,
    root: &Path,
    rules: &VisibilityRules,
    files: &mut Vec<String>,
    nested_worktrees: &mut Vec<String>,
    incomplete_directories: &mut Vec<String>,
) -> Result<(), CoreError> {
    lithe_protocol::cancellation::check()?;
    if rules.is_delegated(path) || crate::indexing::is_excluded(root, path, true)? {
        return Ok(());
    }
    let listing = visible_children(path, root, rules, ChildOrder::Traversal)?;
    nested_worktrees.extend(
        listing
            .nested_worktrees
            .iter()
            .map(|worktree| relative_path(worktree, root)),
    );
    for (child_path, is_directory) in listing.children {
        if is_directory {
            // A directory that vanished mid-scan is skipped, as in `scan_node`.
            if let Err(error) = collect_snapshot_files(
                &child_path,
                root,
                rules,
                files,
                nested_worktrees,
                incomplete_directories,
            ) {
                if !matches!(
                    error.code,
                    ErrorCode::PermissionDenied | ErrorCode::WorkspaceNotFound
                ) {
                    return Err(error);
                }
                incomplete_directories.push(relative_path(&child_path, root));
                lithe_diagnostics::core_events::directory_skipped(
                    "app.workspace.directory_skipped",
                    &relative_path(&child_path, root),
                    &error.code,
                );
            }
        } else {
            if crate::indexing::is_excluded(root, &child_path, false)? {
                continue;
            }
            files.push(relative_path(&child_path, root));
        }
    }
    Ok(())
}

fn node_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Workspace")
        .to_string()
}
