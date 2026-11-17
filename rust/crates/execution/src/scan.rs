//! The directory listings build-system detection matches markers against.

use crate::workspace_scan::{scan_workspace, IgnoredPaths};
use lithe_platform::paths::slash_separated_path;
use lithe_platform::Platform;
use lithe_protocol::{CoreError, ErrorCode};
use std::collections::BTreeSet;
use std::path::Path;

/// Build-system markers sit near the top of a project. Anything deeper is
/// vendored code or build output that slipped past the prune list.
const MAX_DEPTH: usize = 6;

/// One directory plus the file names directly inside it.
///
/// Detection reads this instead of touching the filesystem, so checking whether
/// a directory holds a marker is a set lookup rather than one stat per build
/// system per directory.
#[derive(Debug, Clone)]
pub struct DirectoryContext {
    /// Project-relative, forward-slashed, `.` for the project root.
    pub relative: String,
    files: BTreeSet<String>,
}

impl DirectoryContext {
    /// Reports whether the directory contains an entry with the exact name.
    pub fn has(&self, name: &str) -> bool {
        self.files.contains(name)
    }

    /// Turns a file name in this directory into a project-relative path.
    pub fn join_relative(&self, name: &str) -> String {
        if self.relative == "." {
            name.to_string()
        } else {
            format!("{}/{}", self.relative, name)
        }
    }
}

/// Walks the project, pruning uninteresting subtrees *before* descending rather
/// than filtering afterwards, so a hidden tree is never read at all. `ignored`
/// adds the workspace's own patterns, which Build options keep.
pub fn scan_ignoring(
    root: &Path,
    ignored: &IgnoredPaths,
) -> Result<Vec<DirectoryContext>, CoreError> {
    scan_workspace(root, MAX_DEPTH, ignored)?
        .into_iter()
        .map(|directory| {
            Ok(DirectoryContext {
                relative: relative_path(root, &directory.path)?,
                files: directory.files,
            })
        })
        .collect()
}

fn relative_path(root: &Path, directory: &Path) -> Result<String, CoreError> {
    let relative = directory.strip_prefix(root).map_err(|_| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Scanned directory escaped the project root",
        )
    })?;
    let value = slash_separated_path(relative, Platform::HOST);
    Ok(if value.is_empty() {
        ".".to_string()
    } else {
        value
    })
}
