//! Plans a branch action on a common branch across every repository of a workspace.
//!
//! IntelliJ IDEA runs a checkout or a new branch chosen on a Common Local or
//! Common Remote Branch in each repository in turn (`GitBranchOperation`). The
//! first repository that fails stops the action, and IDEA offers to roll back
//! the repositories that already succeeded, so the workspace does not stay split
//! between two branches. Core decides the steps and what undoes each one from the
//! `git.references` entries the popup already loaded; the host runs the writes
//! the plan names with its existing `git.write` calls and never decides itself
//! which repository to skip or how to undo one.
//!
//! A checkout skips the repositories already on the local branch, because the
//! write refuses the current branch. Undoing a checkout checks the previous local
//! branch out again; a repository whose HEAD was detached or unborn has no branch
//! to return to, so its step cannot be undone. Checking out a remote branch
//! creates or moves the local branch of the same name, which the rollback keeps:
//! returning to the previous branch loses no commit, and which local name the
//! write chose depends on the remotes, which the plan does not read. Undoing a new
//! branch deletes it, after checking the previous branch out again when the action
//! switched to it.

use super::common_references::{self, Kind, Reference, RepositoryReferences};
use super::invalid;
use lithe_protocol::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The branch action the user chose on a common branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Operation {
    /// Switches every repository to the common branch.
    Checkout,
    /// Creates a new local branch at the common branch in every repository.
    CreateBranch,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    operation: Operation,
    /// Full name of the common local or remote branch the action was chosen on.
    reference: String,
    /// The new branch's name for `createBranch`, as `git.branchNamePreview` cleaned it.
    #[serde(default)]
    name: Option<String>,
    /// For `createBranch`, switches every repository to the new branch.
    #[serde(default)]
    checkout: bool,
    /// The repositories the action runs in, in the order the popup lists them, each
    /// with its `git.references` entries.
    repositories: Vec<RepositoryReferences>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    /// The writes in the order they run.
    steps: Vec<Step>,
    /// IDs of the repositories a checkout leaves alone because they are already on
    /// the branch, in request order.
    skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    /// The repository's `workspace.repositories` ID.
    repository: String,
    /// This repository's entry of the common branch: what a checkout switches to,
    /// or where a new branch starts.
    reference: ReferenceIdentity,
    /// The writes that undo this step, in the order they run; `null` when nothing
    /// can, because HEAD was detached or unborn before the step switched it.
    rollback: Option<Vec<RollbackAction>>,
}

/// The identity `git.write` checks before it acts on a reference.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceIdentity {
    full_name: String,
    short_name: String,
    kind: Kind,
}

impl From<&Reference> for ReferenceIdentity {
    fn from(reference: &Reference) -> Self {
        Self {
            full_name: reference.full_name.clone(),
            short_name: reference.short_name.clone(),
            kind: reference.kind,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RollbackAction {
    operation: RollbackOperation,
    reference: ReferenceIdentity,
}

/// A `git.write` operation a rollback runs.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum RollbackOperation {
    /// Checks the previous local branch out again.
    Checkout,
    /// Deletes the branch the step created.
    DeleteBranch,
}

/// Handles `git.workspaceBranchPlan`.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        invalid("Invalid workspace branch plan request").with_details(error.to_string())
    })?;
    common_references::validate(&request.repositories)?;
    let response = plan(&request)?;
    Ok(serde_json::to_value(response).expect("workspace branch plan should encode"))
}

/// Builds the steps, or rejects an action the popup cannot offer: no repository,
/// a branch one repository lacks or that is not a local or remote branch, a new
/// branch without a valid name or that a repository already has, or a name or
/// `checkout` on a checkout.
fn plan(request: &Request) -> Result<Response, CoreError> {
    if request.repositories.is_empty() {
        return Err(invalid("A workspace branch action needs a repository"));
    }
    let new_branch = match (request.operation, request.name.as_deref()) {
        (Operation::Checkout, None) if !request.checkout => None,
        (Operation::CreateBranch, Some(name)) if common_references::valid_name(name) => {
            Some(ReferenceIdentity {
                full_name: format!("{}{name}", Kind::Local.namespace()),
                short_name: name.to_owned(),
                kind: Kind::Local,
            })
        }
        _ => return Err(invalid("Invalid workspace branch action")),
    };
    let mut response = Response {
        steps: Vec::new(),
        skipped: Vec::new(),
    };
    for repository in &request.repositories {
        let Some(reference) = repository
            .find(&request.reference)
            .filter(|reference| reference.kind != Kind::Tag)
        else {
            return Err(invalid("Not every repository has the branch"));
        };
        let previous = repository.current().map(|current| RollbackAction {
            operation: RollbackOperation::Checkout,
            reference: current.into(),
        });
        let rollback = match &new_branch {
            None => {
                if reference.kind == Kind::Local && reference.is_current {
                    response.skipped.push(repository.id.clone());
                    continue;
                }
                previous.map(|previous| vec![previous])
            }
            Some(branch) => {
                if repository.find(&branch.full_name).is_some() {
                    return Err(invalid("A repository already has the new branch"));
                }
                let delete = RollbackAction {
                    operation: RollbackOperation::DeleteBranch,
                    reference: branch.clone(),
                };
                match (request.checkout, previous) {
                    (false, _) => Some(vec![delete]),
                    (true, Some(previous)) => Some(vec![previous, delete]),
                    (true, None) => None,
                }
            }
        };
        response.steps.push(Step {
            repository: repository.id.clone(),
            reference: reference.into(),
            rollback,
        });
    }
    Ok(response)
}
