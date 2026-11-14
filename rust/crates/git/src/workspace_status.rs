//! Plans publication of fresh and previously confirmed repository snapshots.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Roots discovery reports now, the roots published before, and one read outcome per current root.
pub struct Request {
    roots: Vec<String>,
    previous_roots: Vec<String>,
    outcomes: Vec<Outcome>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    root: String,
    success: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// Which snapshot each root publishes, and which roots are now confirmed or failed.
pub struct Plan {
    entries: Vec<Entry>,
    confirmed_roots: Vec<String>,
    failed_roots: Vec<String>,
}

#[derive(Serialize)]
struct Entry {
    root: String,
    source: &'static str,
}

fn invalid() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid workspace Git status outcomes",
    )
}

/// A failed read preserves an existing snapshot, never proves a clean tree,
/// and never resurrects a root that discovery has removed. Hosts retain the
/// actual snapshots and apply this plan under their own publication generation.
pub fn plan(request: Request) -> Result<Plan, CoreError> {
    if request.roots.len() > 10_000
        || request.previous_roots.len() > 10_000
        || request.outcomes.len() != request.roots.len()
    {
        return Err(invalid());
    }
    let mut roots = BTreeSet::new();
    for root in &request.roots {
        if root.is_empty() || !roots.insert(root.as_str()) {
            return Err(invalid());
        }
    }
    let mut previous = BTreeSet::new();
    for root in &request.previous_roots {
        if root.is_empty() || !previous.insert(root.as_str()) {
            return Err(invalid());
        }
    }
    let mut outcomes = BTreeMap::new();
    for outcome in &request.outcomes {
        if !roots.contains(outcome.root.as_str())
            || outcomes
                .insert(outcome.root.as_str(), outcome.success)
                .is_some()
        {
            return Err(invalid());
        }
    }
    let mut entries = Vec::with_capacity(request.roots.len());
    let mut confirmed_roots = Vec::new();
    let mut failed_roots = Vec::new();
    for root in request.roots {
        let success = *outcomes.get(root.as_str()).ok_or_else(invalid)?;
        let source = if success {
            confirmed_roots.push(root.clone());
            "current"
        } else {
            failed_roots.push(root.clone());
            if previous.contains(root.as_str()) {
                "previous"
            } else {
                "missing"
            }
        };
        entries.push(Entry { root, source });
    }
    Ok(Plan {
        entries,
        confirmed_roots,
        failed_roots,
    })
}
