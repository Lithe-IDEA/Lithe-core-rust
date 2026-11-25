//! Lists the session files a plugin's declared source matches.
//!
//! A source is a root directory, already resolved from the plugin's
//! declaration, and glob patterns relative to it. Symbolic links are not
//! followed, so a listing never leaves the root the user agreed to.

use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use lithe_protocol::{CoreError, ErrorCode};
use walkdir::WalkDir;

/// Deepest directory level below a root that a listing enters.
pub const MAX_DISCOVERY_DEPTH: usize = 16;

/// Most files one source lists; a listing that reaches it stops and says so.
pub const MAX_FILES_PER_SOURCE: usize = 100_000;

/// A directory of session files and the files in it to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSource {
    /// The directory, with the plugin's templates already resolved.
    pub root: PathBuf,
    /// Glob patterns matched against paths relative to `root`, with `/`
    /// between components on every platform. `*` stays within one component;
    /// `**` crosses components.
    pub include: Vec<String>,
}

/// The files a source matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    /// Matching regular files, sorted, so syncs read them in the same order.
    pub files: Vec<PathBuf>,
    /// Whether the listing stopped at [`MAX_FILES_PER_SOURCE`].
    pub truncated: bool,
    /// Entries that could not be listed, for example a directory without
    /// read permission, or names that are not valid Unicode, which the cache
    /// cannot record.
    pub unreadable_entries: u64,
}

/// Lists the files of `source`. A root that does not exist lists nothing: the
/// agent has not written a session yet.
pub fn discover(source: &SessionSource) -> Result<Discovery, CoreError> {
    let patterns = glob_set(&source.include)?;
    let mut discovery = Discovery::default();
    if !source.root.is_dir() {
        return Ok(discovery);
    }
    let walker = WalkDir::new(&source.root)
        .follow_links(false)
        .max_depth(MAX_DISCOVERY_DEPTH)
        .sort_by_file_name();
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                discovery.unreadable_entries += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(relative) = relative_glob_path(&source.root, entry.path()) else {
            discovery.unreadable_entries += 1;
            continue;
        };
        if !patterns.is_match(&relative) {
            continue;
        }
        if discovery.files.len() == MAX_FILES_PER_SOURCE {
            discovery.truncated = true;
            break;
        }
        discovery.files.push(entry.into_path());
    }
    discovery.files.sort();
    Ok(discovery)
}

fn glob_set(patterns: &[String]) -> Result<GlobSet, CoreError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "A session source has an invalid file pattern",
                )
                .with_details(error.to_string())
            })?;
        builder.add(glob);
    }
    builder.build().map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "A session source has an invalid file pattern",
        )
        .with_details(error.to_string())
    })
}

/// The path of `path` below `root` with `/` separators, or `None` when a
/// component is not valid Unicode.
fn relative_glob_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut joined = String::new();
    for component in relative.components() {
        if !joined.is_empty() {
            joined.push('/');
        }
        joined.push_str(component.as_os_str().to_str()?);
    }
    path.to_str()?;
    Some(joined)
}
