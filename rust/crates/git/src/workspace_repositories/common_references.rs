//! Finds the branches every repository of a workspace has in common.
//!
//! The branch popup of a folder that holds several repositories lists, below the
//! repositories, the local and remote branches that every one of them has, as
//! IntelliJ IDEA lists its Common Local Branches and Common Remote Branches. An
//! action on such a branch, a checkout or a new branch from it, runs in every
//! repository at once (see `branch_plan`). A branch that one repository lacks is
//! not common, so the popup offers it only in the panels of the repositories that
//! have it.
//!
//! Hosts pass each repository's `git.references` entries back unchanged. The
//! common branches come back in the same entry shape, so a popup draws them with
//! the rows it already draws for one repository. The lists keep the order of the
//! first repository. A common local branch is current when every repository has
//! it checked out, its upstream is common when every repository tracks the same
//! remote branch with it, and its `ahead` and `behind` add up what each
//! repository counts against its own upstream, as IDEA marks a common branch
//! with incoming or outgoing commits when any repository has them.

use super::{invalid, valid_id, MAX_REPOSITORIES};
use lithe_protocol::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Bounds the references across one request; larger lists are host bugs.
pub(super) const MAX_REFERENCES: usize = 1_000_000;
/// Git refuses longer reference names on every supported platform.
const MAX_NAME_BYTES: usize = 4096;

/// A reference's namespace, as `git.references` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Kind {
    Local,
    Remote,
    Tag,
}

impl Kind {
    /// The prefix that turns a short name of this kind into its full name.
    pub(super) fn namespace(self) -> &'static str {
        match self {
            Kind::Local => "refs/heads/",
            Kind::Remote => "refs/remotes/",
            Kind::Tag => "refs/tags/",
        }
    }
}

/// One `git.references` entry, passed back unchanged; the fields Core does not
/// read, such as `peelsToCommit`, are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Reference {
    pub(super) full_name: String,
    pub(super) short_name: String,
    pub(super) kind: Kind,
    /// Marks the checked-out local branch.
    #[serde(default)]
    pub(super) is_current: bool,
    /// The short name of the remote branch a local branch tracks.
    #[serde(default)]
    pub(super) upstream_short_name: Option<String>,
    /// Commits only the local branch has, compared with its upstream.
    #[serde(default)]
    pub(super) ahead: usize,
    /// Commits only the local branch's upstream has.
    #[serde(default)]
    pub(super) behind: usize,
}

/// One repository of the popup with the `references` its `git.references` call returned.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RepositoryReferences {
    /// The repository's `workspace.repositories` ID.
    pub(super) id: String,
    pub(super) references: Vec<Reference>,
}

impl RepositoryReferences {
    /// The checked-out local branch; `None` when HEAD is detached or unborn.
    pub(super) fn current(&self) -> Option<&Reference> {
        self.references
            .iter()
            .find(|reference| reference.kind == Kind::Local && reference.is_current)
    }

    /// The reference with this full name, when the repository has it.
    pub(super) fn find(&self, full_name: &str) -> Option<&Reference> {
        self.references
            .iter()
            .find(|reference| reference.full_name == full_name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// The repositories the popup lists, in the order it lists them.
    repositories: Vec<RepositoryReferences>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    /// The local branches every repository has, in the first repository's order.
    local: Vec<CommonReference>,
    /// The remote branches every repository has, in the first repository's order.
    remote: Vec<CommonReference>,
}

/// A common branch in the shape of a `git.references` entry.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommonReference {
    full_name: String,
    short_name: String,
    kind: Kind,
    /// Always true: a branch names a commit.
    peels_to_commit: bool,
    /// True when every repository has this local branch checked out.
    is_current: bool,
    /// The remote branch every repository tracks with this local branch; `null`
    /// when one tracks nothing or they track different branches.
    upstream_short_name: Option<String>,
    /// The repositories' `ahead` counts added up.
    ahead: usize,
    /// The repositories' `behind` counts added up.
    behind: usize,
}

/// Handles `git.workspaceCommonReferences`.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        invalid("Invalid workspace common references request").with_details(error.to_string())
    })?;
    validate(&request.repositories)?;
    Ok(serde_json::to_value(common(&request.repositories))
        .expect("workspace common references should encode"))
}

/// Rejects reference lists `git.references` cannot produce: too many repositories
/// or references, an invalid or repeated ID, a name that is empty, oversized, holds
/// NUL, or whose full name is not its short name in its kind's namespace, a
/// reference listed twice in one repository, an invalid upstream name, or a
/// current reference that is not the repository's one local branch.
pub(super) fn validate(repositories: &[RepositoryReferences]) -> Result<(), CoreError> {
    validate_bounded(repositories, MAX_REFERENCES)
}

/// [`validate`] with the reference bound as a parameter, so a test can cross it
/// without building a million entries.
pub(super) fn validate_bounded(
    repositories: &[RepositoryReferences],
    max_references: usize,
) -> Result<(), CoreError> {
    if repositories.len() > MAX_REPOSITORIES {
        return Err(invalid("Too many workspace repositories"));
    }
    let mut ids = HashSet::new();
    let mut total = 0usize;
    for repository in repositories {
        if !valid_id(&repository.id) || !ids.insert(repository.id.as_str()) {
            return Err(invalid("Invalid or repeated workspace repository ID"));
        }
        total = total.saturating_add(repository.references.len());
        if total > max_references {
            return Err(invalid("Too many workspace references"));
        }
        let mut names = HashSet::new();
        let mut current = false;
        for reference in &repository.references {
            let namespaced = reference
                .full_name
                .strip_prefix(reference.kind.namespace())
                .is_some_and(|short| short == reference.short_name);
            if !valid_name(&reference.short_name)
                || !valid_name(&reference.full_name)
                || !namespaced
            {
                return Err(invalid("Invalid reference name"));
            }
            if !names.insert(reference.full_name.as_str()) {
                return Err(invalid("Repeated reference"));
            }
            if reference
                .upstream_short_name
                .as_deref()
                .is_some_and(|name| !valid_name(name))
            {
                return Err(invalid("Invalid upstream branch name"));
            }
            if reference.is_current {
                if reference.kind != Kind::Local || current {
                    return Err(invalid("Only one local branch can be current"));
                }
                current = true;
            }
        }
    }
    Ok(())
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES && !name.contains('\0')
}

/// Intersects the repositories' branches in the first repository's order.
fn common(repositories: &[RepositoryReferences]) -> Response {
    let mut response = Response {
        local: Vec::new(),
        remote: Vec::new(),
    };
    let Some((first, rest)) = repositories.split_first() else {
        return response;
    };
    // Each other repository's references by full name.
    let others: Vec<HashMap<&str, &Reference>> = rest
        .iter()
        .map(|repository| {
            repository
                .references
                .iter()
                .map(|reference| (reference.full_name.as_str(), reference))
                .collect()
        })
        .collect();
    for reference in &first.references {
        let list = match reference.kind {
            Kind::Local => &mut response.local,
            Kind::Remote => &mut response.remote,
            Kind::Tag => continue,
        };
        let mut common = CommonReference {
            full_name: reference.full_name.clone(),
            short_name: reference.short_name.clone(),
            kind: reference.kind,
            peels_to_commit: true,
            is_current: reference.is_current,
            upstream_short_name: reference.upstream_short_name.clone(),
            ahead: reference.ahead,
            behind: reference.behind,
        };
        let shared = others.iter().all(|other| {
            let Some(theirs) = other.get(reference.full_name.as_str()) else {
                return false;
            };
            common.is_current &= theirs.is_current;
            if common.upstream_short_name != theirs.upstream_short_name {
                common.upstream_short_name = None;
            }
            common.ahead = common.ahead.saturating_add(theirs.ahead);
            common.behind = common.behind.saturating_add(theirs.behind);
            true
        });
        if shared {
            list.push(common);
        }
    }
    response
}
