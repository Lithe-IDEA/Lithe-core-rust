//! Interleaves the loaded histories of several workspace repositories into one list.
//!
//! A Git Log that walks several repositories at once shows their commits in one
//! list, newest first, as IntelliJ IDEA shows the roots of a project. Each
//! repository's history arrives a page at a time in Git's own order; Core
//! merges what is loaded, keeping each repository's order and placing the
//! repositories' commits by their dates.
//!
//! The list only grows at its end as pages arrive. The merge stops at the first
//! point where a repository with pages left has no loaded commit: its next page
//! may hold a newer commit than every one still to place, so placing them now
//! would move rows once that page arrives. That repository is pending, and the
//! view loads its next page before it asks again.
//!
//! The commits of different repositories are different objects even when a
//! hash repeats, such as two clones of one project in the same folder. A hash
//! that more than one repository names, as a commit or as a parent, is keyed
//! `<hash>@<id>` in each of them, so a graph laid out from the keys never joins
//! two repositories. Every other key is the hash itself.

use super::{invalid, valid_id, MAX_REPOSITORIES};
use lithe_protocol::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Matches the commit bound of `git.graphProjection`, which lays out the merged list.
const MAX_COMMITS: usize = crate::history::MAX_LOG_COMMITS;
/// SHA-256 object names are the longest Git writes.
const MAX_HASH_BYTES: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// The repositories the view walks, in the order the view lists them.
    repositories: Vec<RepositoryHistory>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepositoryHistory {
    /// The repository's `workspace.repositories` ID.
    id: String,
    /// Every commit loaded so far, in the order Git walked them.
    commits: Vec<CommitInput>,
    /// True when `commits` hold the whole walk, so no page is left to load.
    complete: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitInput {
    hash: String,
    #[serde(default)]
    parent_hashes: Vec<String>,
    /// Seconds since the Unix epoch the list places the commit by; absent when
    /// Git reported no valid date, so the commit stays beside the one before it.
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    /// The merged list, newest first.
    entries: Vec<Entry>,
    /// Indices of the repositories whose next page the list waits on, in request
    /// order. Empty when every loaded commit is placed and no page is left.
    pending: Vec<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    /// Index of the commit's repository in the request.
    repository: usize,
    /// Index of the commit in that repository's `commits`.
    index: usize,
    /// The commit's identity in the merged list.
    key: String,
    /// The keys of the commit's parents, in Git's parent order.
    parent_keys: Vec<String>,
}

/// Handles `git.workspaceHistoryMerge`.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        invalid("Invalid workspace history merge request").with_details(error.to_string())
    })?;
    validate(&request.repositories)?;
    let shared = shared_hashes(&request.repositories);
    let key = |repository: &RepositoryHistory, hash: &str| {
        if shared.contains(hash) {
            format!("{hash}@{}", repository.id)
        } else {
            hash.to_owned()
        }
    };
    let (placed, pending) = merge(&request.repositories);
    let entries = placed
        .into_iter()
        .map(|(repository_index, index)| {
            let repository = &request.repositories[repository_index];
            let commit = &repository.commits[index];
            Entry {
                repository: repository_index,
                index,
                key: key(repository, &commit.hash),
                parent_keys: commit
                    .parent_hashes
                    .iter()
                    .map(|parent| key(repository, parent))
                    .collect(),
            }
        })
        .collect();
    Ok(serde_json::to_value(Response { entries, pending })
        .expect("workspace history merge should encode"))
}

/// Rejects requests a view cannot send: too many repositories or commits, an
/// invalid or repeated ID, a hash that is not a Git object name, or a commit
/// listed twice in one repository.
fn validate(repositories: &[RepositoryHistory]) -> Result<(), CoreError> {
    if repositories.len() > MAX_REPOSITORIES {
        return Err(invalid("Too many repositories in a history merge"));
    }
    let total: usize = repositories
        .iter()
        .map(|repository| repository.commits.len())
        .sum();
    if total > MAX_COMMITS {
        return Err(invalid("Too many commits in a history merge"));
    }
    let mut ids = HashSet::with_capacity(repositories.len());
    for repository in repositories {
        if !valid_id(&repository.id) || !ids.insert(repository.id.as_str()) {
            return Err(invalid(
                "Invalid or repeated repository ID in a history merge",
            ));
        }
        let mut hashes = HashSet::with_capacity(repository.commits.len());
        for commit in &repository.commits {
            if !object_name(&commit.hash) || !commit.parent_hashes.iter().all(|p| object_name(p)) {
                return Err(invalid("Invalid commit hash in a history merge"));
            }
            if !hashes.insert(commit.hash.as_str()) {
                return Err(invalid("Repeated commit in a history merge"));
            }
        }
    }
    Ok(())
}

/// A Git object name: hexadecimal and at most as long as a SHA-256 name. The
/// shape keeps a `<hash>@<id>` key unambiguous.
fn object_name(hash: &str) -> bool {
    !hash.is_empty() && hash.len() <= MAX_HASH_BYTES && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The hashes more than one repository names, as a commit or as a parent.
fn shared_hashes(repositories: &[RepositoryHistory]) -> HashSet<&str> {
    let mut owner: HashMap<&str, usize> = HashMap::new();
    let mut shared = HashSet::new();
    for (index, repository) in repositories.iter().enumerate() {
        for commit in &repository.commits {
            let names = std::iter::once(commit.hash.as_str())
                .chain(commit.parent_hashes.iter().map(String::as_str));
            for hash in names {
                if *owner.entry(hash).or_insert(index) != index {
                    shared.insert(hash);
                }
            }
        }
    }
    shared
}

/// Places the loaded commits newest first while no repository with pages left
/// has run out, keeping each repository's own order. A commit without a date
/// takes the date of the one placed before it in its repository, so it stays
/// beside it; the first commit of a repository without one goes first. Equal
/// dates go in request order. Returns the placed `(repository, index)` pairs and
/// the repositories the merge stopped for.
fn merge(repositories: &[RepositoryHistory]) -> (Vec<(usize, usize)>, Vec<usize>) {
    let mut next = vec![0usize; repositories.len()];
    let mut last = vec![i64::MAX; repositories.len()];
    let date = |repository: usize, index: usize, last: &[i64]| {
        repositories[repository].commits[index]
            .timestamp
            .unwrap_or(last[repository])
    };
    // Newest date first, then the lowest repository index.
    let mut heads = BinaryHeap::new();
    let mut stalled = Vec::new();
    for (index, repository) in repositories.iter().enumerate() {
        if !repository.commits.is_empty() {
            heads.push((date(index, 0, &last), Reverse(index)));
        } else if !repository.complete {
            stalled.push(index);
        }
    }
    let mut placed = Vec::new();
    while stalled.is_empty() {
        let Some((placed_date, Reverse(repository))) = heads.pop() else {
            break;
        };
        placed.push((repository, next[repository]));
        last[repository] = placed_date;
        next[repository] += 1;
        if next[repository] < repositories[repository].commits.len() {
            heads.push((
                date(repository, next[repository], &last),
                Reverse(repository),
            ));
        } else if !repositories[repository].complete {
            stalled.push(repository);
        }
    }
    stalled.sort_unstable();
    (placed, stalled)
}
