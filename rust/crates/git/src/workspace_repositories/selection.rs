//! Chooses the repositories a multi-repository view acts on.
//!
//! The branch popup and the Git Log each act on repositories of the workspace
//! and list the others as groups the user can switch to. A view keeps what the
//! user picked while discovery still lists it; otherwise it falls back to the
//! repository that contains the opened folder, and in a folder that is no
//! repository itself, to the first nested repository.
//!
//! A view acts on one repository (`selected`) or, like a Git Log that shows the
//! histories of several repositories in one list, on a set of them (`chosen`).

use super::{invalid, valid_id, validate_discovered, DiscoveredRepository, MAX_REPOSITORIES};
use crate::contracts::WorkspaceRepositoryRole;
use lithe_protocol::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// Discovery entries in discovery order.
    repositories: Vec<DiscoveredRepository>,
    /// ID the view acted on before, absent when it has not picked one.
    #[serde(default)]
    selected: Option<String>,
    /// IDs a view that acts on several repositories at once picked before, in
    /// any order; an empty list is no pick. Mutually exclusive with `selected`.
    #[serde(default)]
    chosen: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    /// ID the view acts on, `null` when the workspace has no repository. With
    /// `chosen`, the first of the repositories the view acts on.
    active: Option<String>,
    /// True when a picked repository is no longer listed, so the view drops state
    /// that belonged to the pick, such as an open history walk or a reference filter.
    fell_back: bool,
    /// Every ID the view does not act on, in discovery order: the groups the view
    /// lists for switching.
    others: Vec<String>,
    /// Present when the request carried `chosen`: the IDs the view acts on, in
    /// discovery order. They are the listed picks, or `active` alone when none is.
    #[serde(skip_serializing_if = "Option::is_none")]
    chosen: Option<Vec<String>>,
}

/// Handles `git.workspaceRepositorySelection`.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        invalid("Invalid workspace repository selection request").with_details(error.to_string())
    })?;
    validate_discovered(&request.repositories)?;
    if request.selected.as_deref().is_some_and(str::is_empty) {
        return Err(invalid(
            "A selected workspace repository ID cannot be empty",
        ));
    }
    if request.selected.is_some() && request.chosen.is_some() {
        return Err(invalid(
            "A workspace repository selection takes either selected or chosen",
        ));
    }
    let picks: HashSet<&str> = match &request.chosen {
        Some(chosen) => validate_chosen(chosen)?,
        None => request.selected.iter().map(String::as_str).collect(),
    };
    // Kept picks follow discovery order, whatever order the view sent them in.
    let kept: Vec<String> = request
        .repositories
        .iter()
        .filter(|repository| picks.contains(repository.id.as_str()))
        .map(|repository| repository.id.clone())
        .collect();
    let fell_back = kept.len() < picks.len();
    let acting: Vec<String> = if kept.is_empty() {
        fallback(&request.repositories).into_iter().collect()
    } else {
        kept
    };
    let others = request
        .repositories
        .iter()
        .filter(|repository| !acting.contains(&repository.id))
        .map(|repository| repository.id.clone())
        .collect();
    Ok(serde_json::to_value(Response {
        active: acting.first().cloned(),
        fell_back,
        others,
        chosen: request.chosen.is_some().then_some(acting),
    })
    .expect("workspace repository selection should encode"))
}

/// The repository a view acts on without a listed pick: the one that contains
/// the opened folder, otherwise the first listed one.
fn fallback(repositories: &[DiscoveredRepository]) -> Option<String> {
    repositories
        .iter()
        .find(|repository| repository.role == WorkspaceRepositoryRole::Workspace)
        .or_else(|| repositories.first())
        .map(|repository| repository.id.clone())
}

/// Rejects a `chosen` list discovery cannot form: more IDs than a workspace
/// lists, or an invalid or repeated ID.
fn validate_chosen(chosen: &[String]) -> Result<HashSet<&str>, CoreError> {
    if chosen.len() > MAX_REPOSITORIES {
        return Err(invalid("Too many chosen workspace repositories"));
    }
    let mut picks = HashSet::with_capacity(chosen.len());
    for id in chosen {
        if !valid_id(id) {
            return Err(invalid("Invalid chosen workspace repository ID"));
        }
        if !picks.insert(id.as_str()) {
            return Err(invalid("Duplicate chosen workspace repository"));
        }
    }
    Ok(picks)
}
