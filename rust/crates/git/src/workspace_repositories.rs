//! Pure projections over the repositories one opened folder holds.
//!
//! `workspace.repositories` lists every repository of an opened folder: the one
//! that contains the folder, if any, and the independent repositories nested
//! below it. A folder without `.git` that holds several repositories lists only
//! nested ones. Hosts pass those discovery entries back unchanged, together with
//! what they read from each repository, and Core decides which repositories a
//! view acts on, how the repositories' changes group and stage, how their
//! histories interleave in one log, which branches they all have, and how a
//! branch action runs across them. Views keep drawing, folding, and the native
//! Git reads and writes the plans name.

mod branch_plan;
mod change_groups;
mod common_references;
mod history_merge;
mod selection;
#[cfg(test)]
mod tests;

pub use branch_plan::command as branch_plan_command;
pub use change_groups::command as change_groups_command;
pub use common_references::command as common_references_command;
pub use history_merge::command as history_merge_command;
pub use selection::command as selection_command;

use crate::contracts::WorkspaceRepositoryRole;
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use std::collections::HashSet;

/// Bounds one workspace's repository list; larger lists are host bugs, not user input.
const MAX_REPOSITORIES: usize = 10_000;
/// Matches the repository color assignment bound on one identity.
const MAX_ID_BYTES: usize = 4096;

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

/// One `workspace.repositories` entry, passed back unchanged.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiscoveredRepository {
    /// Core slash path of the repository root.
    pub(crate) path: String,
    /// Identity relative to the opened folder.
    pub(crate) id: String,
    /// Label the views show.
    pub(crate) name: String,
    /// How the repository relates to the opened folder.
    pub(crate) role: WorkspaceRepositoryRole,
}

/// Rejects discovery lists `workspace.repositories` cannot produce; see
/// [`validate_identities`].
pub(crate) fn validate_discovered(repositories: &[DiscoveredRepository]) -> Result<(), CoreError> {
    validate_identities(repositories.iter().map(|repository| Identity {
        id: &repository.id,
        path: &repository.path,
        name: &repository.name,
        role: repository.role,
    }))
}

/// The identity fields every command checks on a discovery entry.
struct Identity<'a> {
    id: &'a str,
    path: &'a str,
    name: &'a str,
    role: WorkspaceRepositoryRole,
}

/// Rejects lists discovery cannot produce: too many entries, an empty, oversized,
/// absolute, or backslash ID, an empty path or name, a repeated ID or path, or
/// more than one repository that contains the opened folder.
fn validate_identities<'a>(
    identities: impl ExactSizeIterator<Item = Identity<'a>>,
) -> Result<(), CoreError> {
    if identities.len() > MAX_REPOSITORIES {
        return Err(invalid("Too many workspace repositories"));
    }
    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    let mut workspace = false;
    for identity in identities {
        if !valid_id(identity.id) {
            return Err(invalid("Invalid workspace repository ID"));
        }
        if identity.path.is_empty() || identity.path.contains('\0') {
            return Err(invalid("Invalid workspace repository path"));
        }
        if identity.name.is_empty() {
            return Err(invalid("A workspace repository needs a name"));
        }
        if !ids.insert(identity.id) || !paths.insert(identity.path) {
            return Err(invalid("Duplicate workspace repository"));
        }
        if identity.role == WorkspaceRepositoryRole::Workspace {
            if workspace {
                return Err(invalid("More than one workspace repository"));
            }
            workspace = true;
        }
    }
    Ok(())
}

/// A repository ID is the `/`-separated discovery identity: never empty, never
/// absolute, and free of backslashes and NUL so it stays portable.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && !id.starts_with('/')
        && !id.contains('\\')
        && !id.contains('\0')
}
