//! Groups the changed files of a commit, a stash or a shelf by folder, in the order the
//! changed-file lists show them.
//!
//! The grouping is a pure projection of repository-relative paths: a file belongs to the
//! folder its path names before the last `/`, the top of the repository being the folder
//! `""`. Folders sort by their path one segment at a time, and files by name, both in the
//! project tree's natural order (`lithe_core_rules::file_names`), so `v0.1.4` comes before
//! `v0.1.12` and a folder comes right before its subfolders. Which folders are open, the rows
//! they make and how they draw stay in the views.

use std::cmp::Ordering;
use std::collections::HashMap;

use lithe_core_rules::file_names::FileNameKey;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upper bound for one grouping; Git lists for larger changes are paged by the host.
const MAX_PATHS: usize = 200_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// Repository-relative paths with `/` separators, in the order Git listed them.
    paths: Vec<String>,
}

/// One folder and the files directly in it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Folder {
    /// Path of the folder from the top of the repository; `""` for the top itself.
    path: String,
    /// Indices into the request's `paths` of the files in the folder, in name order.
    files: Vec<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    folders: Vec<Folder>,
}

/// Handles `git.changedFileGroups`.
///
/// Every path lands in exactly one folder. Folder paths compare segment by segment in natural
/// name order, a folder before the folders inside it; file names compare in the same order. A
/// tie of names that differ only where the natural order sees no difference falls back to the
/// raw text, and a repeated path keeps the order of the request.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git changed file groups request",
        )
        .with_details(error.to_string())
    })?;
    if request.paths.len() > MAX_PATHS {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many changed files for one grouping",
        ));
    }
    let mut at: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (index, path) in request.paths.iter().enumerate() {
        let folder = folder_of(path);
        let group = *at.entry(folder).or_insert_with(|| {
            groups.push((folder, Vec::new()));
            groups.len() - 1
        });
        groups[group].1.push(index);
    }
    let mut keyed = groups
        .into_iter()
        .map(|(folder, files)| {
            let mut named = files
                .into_iter()
                .map(|index| {
                    let name = name_of(&request.paths[index]);
                    (FileNameKey::new(name), name, index)
                })
                .collect::<Vec<_>>();
            named.sort();
            let files = named.into_iter().map(|(_, _, index)| index).collect();
            let segments = folder
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(FileNameKey::new)
                .collect::<Vec<_>>();
            (segments, folder, files)
        })
        .collect::<Vec<_>>();
    keyed.sort_by(|(left_key, left, _), (right_key, right, _)| {
        compare_segments(left_key, right_key).then_with(|| left.cmp(right))
    });
    let folders = keyed
        .into_iter()
        .map(|(_, folder, files)| Folder {
            path: folder.to_string(),
            files,
        })
        .collect();
    Ok(serde_json::to_value(Response { folders }).expect("changed file groups encode"))
}

/// The folder `path` names before its last `/`, or `""` for a file at the top.
fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |end| &path[..end])
}

/// The name `path` ends with after its last `/`.
fn name_of(path: &str) -> &str {
    path.rfind('/').map_or(path, |end| &path[end + 1..])
}

/// Natural order of folder paths given as segments: the first differing segment decides, and a
/// folder comes before the folders inside it.
fn compare_segments(left: &[FileNameKey], right: &[FileNameKey]) -> Ordering {
    left.iter()
        .zip(right)
        .map(|(left, right)| left.cmp(right))
        .find(|order| order.is_ne())
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}
