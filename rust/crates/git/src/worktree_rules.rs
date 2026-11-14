//! What a registered worktree shows and allows, and what the dialog that creates one suggests.
//!
//! The Worktrees tab of every client shows one state per worktree and offers removal and
//! locking only where Git and Core accept them. Those rules live here so clients only map the
//! state and the reason codes to their words: `status` ranks what is true of a worktree,
//! `actions` says which writes Core would refuse and with which code, and `creation_preview`
//! keeps the creation dialog's choices, branch name, suggested destination and readiness.
//! Fixtures live in `rust/fixtures/git/worktree-rules-v1.json` and
//! `rust/fixtures/git/worktree-creation-preview-v1.json`.

use std::path::Path;

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::branch_names::{
    preview as branch_name_preview, GitBranchNamePreviewRequest, GitBranchNamePreviewResponse,
    GitBranchNameReference, GitBranchNameReferenceKind,
};

/// The one state a worktree's row shows, the most pressing of what is true of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitWorktreeStatus {
    /// Git reports the checkout gone; its registration can be pruned.
    Missing,
    /// The worktree is locked.
    Locked,
    /// The checkout has local changes; only known when they were counted.
    Modified,
    /// The worktree is the one the request was made from.
    Current,
    /// None of the above.
    Available,
}

/// Why Core refuses a write to a worktree. The first three are the error codes the write
/// returns for the same refusal; Core lets Git decide on a missing checkout, which clients
/// prune instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitWorktreeBlock {
    /// The primary worktree can neither be removed nor locked.
    #[serde(rename = "git_worktree_primary")]
    Primary,
    /// The worktree the request was made from cannot remove itself.
    #[serde(rename = "git_worktree_current")]
    Current,
    /// A locked worktree is unlocked before it is removed.
    #[serde(rename = "git_worktree_locked")]
    Locked,
    /// The checkout is gone; pruning drops the registration.
    #[serde(rename = "git_worktree_missing")]
    Missing,
}

/// Whether one write to a worktree can run, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktreeAction {
    /// The write can be offered.
    pub allowed: bool,
    /// Why it cannot; `null` when it can.
    pub reason: Option<GitWorktreeBlock>,
}

impl GitWorktreeAction {
    fn from(reason: Option<GitWorktreeBlock>) -> Self {
        Self {
            allowed: reason.is_none(),
            reason,
        }
    }
}

/// The writes a worktree's menu and details offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktreeActions {
    /// Removing the worktree with `removeWorktree`.
    pub remove: GitWorktreeAction,
    /// Locking or unlocking it with `lockWorktree` or `unlockWorktree`.
    pub lock: GitWorktreeAction,
}

/// The registration facts the rules read.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeFacts {
    #[serde(default)]
    pub(crate) is_primary: bool,
    #[serde(default)]
    pub(crate) is_current: bool,
    #[serde(default)]
    pub(crate) is_locked: bool,
    #[serde(default)]
    pub(crate) is_prunable: bool,
}

/// The state of a worktree with `changes` local changes, `None` while they were not counted:
/// a missing checkout, then a lock, local changes, being the current worktree, and otherwise
/// available.
pub(crate) fn status(facts: WorktreeFacts, changes: Option<usize>) -> GitWorktreeStatus {
    if facts.is_prunable {
        GitWorktreeStatus::Missing
    } else if facts.is_locked {
        GitWorktreeStatus::Locked
    } else if changes.is_some_and(|changes| changes > 0) {
        GitWorktreeStatus::Modified
    } else if facts.is_current {
        GitWorktreeStatus::Current
    } else {
        GitWorktreeStatus::Available
    }
}

/// Which writes the worktree allows. Removal is refused for the primary worktree, then the
/// current one, a missing checkout and a lock, in that order; changing the lock for the
/// primary worktree and a missing checkout.
pub(crate) fn actions(facts: WorktreeFacts) -> GitWorktreeActions {
    let remove = if facts.is_primary {
        Some(GitWorktreeBlock::Primary)
    } else if facts.is_current {
        Some(GitWorktreeBlock::Current)
    } else if facts.is_prunable {
        Some(GitWorktreeBlock::Missing)
    } else if facts.is_locked {
        Some(GitWorktreeBlock::Locked)
    } else {
        None
    };
    let lock = if facts.is_primary {
        Some(GitWorktreeBlock::Primary)
    } else if facts.is_prunable {
        Some(GitWorktreeBlock::Missing)
    } else {
        None
    };
    GitWorktreeActions {
        remove: GitWorktreeAction::from(remove),
        lock: GitWorktreeAction::from(lock),
    }
}

/// Last part of a suggested destination when nothing usable names it.
const LEAF_FALLBACK: &str = "worktree";
/// Characters of a typed revision a suggested destination is named after.
const REVISION_LEAF_CHARS: usize = 9;
/// Upper bound of the references one preview reads.
const MAX_REFERENCES: usize = 1_000_000;

/// How the dialog creates the worktree, as `writeWorktree`'s `mode` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CreationMode {
    NewBranch,
    ExistingBranch,
    Detached,
}

/// One reference the host loaded with `git.references`; other fields are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreationReference {
    full_name: String,
    short_name: String,
    kind: GitBranchNameReferenceKind,
    #[serde(default)]
    is_current: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreationPreviewRequest {
    /// Root of the repository the worktree is created from.
    root: String,
    mode: CreationMode,
    /// Kind of reference listed while the mode is not `existingBranch`.
    #[serde(default = "local_kind")]
    kind: GitBranchNameReferenceKind,
    /// Full name of the reference the dialog holds; empty for none.
    #[serde(default)]
    reference: String,
    /// New branch name as typed.
    #[serde(default)]
    name: String,
    /// `true` on submit, where an empty branch name is an error.
    #[serde(default)]
    require_name: bool,
    /// Revision as typed, for a detached worktree.
    #[serde(default)]
    revision: String,
    /// Destination as typed.
    #[serde(default)]
    destination: String,
    /// The suggestion the destination last followed, empty before the first preview. While
    /// the destination still holds it, the user has not edited it and it follows the new one.
    #[serde(default)]
    last_suggestion: String,
    references: Vec<CreationReference>,
    /// Full names of the branches registered worktrees have checked out.
    #[serde(default)]
    checked_out_branches: Vec<String>,
}

fn local_kind() -> GitBranchNameReferenceKind {
    GitBranchNameReferenceKind::Local
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreationPreviewResponse {
    /// Indices into the request's references the dialog lists, in their order.
    choices: Vec<usize>,
    /// The reference the dialog holds after the inputs changed; `null` without choices.
    reference: Option<String>,
    /// The cleaned and checked branch name for `newBranch`, `null` for the other modes.
    branch: Option<GitBranchNamePreviewResponse>,
    /// Where the inputs suggest the checkout goes; the next preview's `lastSuggestion`.
    suggested_destination: String,
    /// What the destination field holds: the suggestion while the field held the last one,
    /// else the typed destination.
    destination: String,
    /// The destination, trimmed, is an absolute path.
    destination_is_absolute: bool,
    /// The dialog can create the worktree.
    ready: bool,
}

/// Handles `git.worktreeCreationPreview`.
///
/// `existingBranch` lists the local branches no worktree has checked out, the other modes the
/// references of `kind`. The held reference stays while it is listed, else the current local
/// branch among the choices is taken, else the first choice. The suggested destination sits
/// beside the repository, named `<repository>-<leaf>`, the leaf being the last `/` segment of
/// the new branch, the first nine characters of a typed revision or the held reference's
/// short name, with every character that is not a letter, a digit, `-` or `_` replaced by `-`,
/// and `worktree` when nothing is left. The destination follows the suggestion for as long as
/// it holds the last one, that is until the user edits it.
pub fn creation_preview_command(payload: Value) -> Result<Value, CoreError> {
    let request: CreationPreviewRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid Git worktree creation preview request",
        )
        .with_details(error.to_string())
    })?;
    if request.references.len() > MAX_REFERENCES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many references for one worktree creation preview",
        ));
    }
    let existing = request.mode == CreationMode::ExistingBranch;
    let kind = if existing {
        GitBranchNameReferenceKind::Local
    } else {
        request.kind
    };
    let choices: Vec<usize> = request
        .references
        .iter()
        .enumerate()
        .filter(|(_, reference)| reference.kind == kind)
        .filter(|(_, reference)| {
            !existing || !request.checked_out_branches.contains(&reference.full_name)
        })
        .map(|(index, _)| index)
        .collect();
    let held = choices
        .iter()
        .copied()
        .find(|&index| request.references[index].full_name == request.reference)
        .or_else(|| {
            choices.iter().copied().find(|&index| {
                let reference = &request.references[index];
                reference.kind == GitBranchNameReferenceKind::Local && reference.is_current
            })
        })
        .or_else(|| choices.first().copied())
        .map(|index| &request.references[index]);

    let branch = match request.mode {
        CreationMode::NewBranch => Some(branch_name_preview(GitBranchNamePreviewRequest {
            raw_name: request.name.clone(),
            require_name: request.require_name,
            references: request
                .references
                .iter()
                .map(|reference| GitBranchNameReference {
                    kind: reference.kind,
                    short_name: reference.short_name.clone(),
                })
                .collect(),
            reference: None,
        })?),
        _ => None,
    };
    let revision = request.revision.trim();
    let source: String = match (&branch, request.mode) {
        (Some(branch), _) if !branch.cleaned_name.is_empty() => branch.cleaned_name.clone(),
        (_, CreationMode::Detached) if !revision.is_empty() => {
            revision.chars().take(REVISION_LEAF_CHARS).collect()
        }
        _ => held
            .map(|reference| reference.short_name.clone())
            .unwrap_or_default(),
    };
    let suggested_destination = beside_repository(&request.root, &source);
    let destination = if request.destination == request.last_suggestion {
        suggested_destination.clone()
    } else {
        request.destination.clone()
    };
    let destination_is_absolute = Path::new(destination.trim()).is_absolute();
    let ready = destination_is_absolute
        && match request.mode {
            CreationMode::NewBranch => {
                held.is_some()
                    && branch.as_ref().is_some_and(|branch| {
                        branch.error.is_none() && !branch.cleaned_name.is_empty()
                    })
            }
            CreationMode::ExistingBranch => held.is_some(),
            CreationMode::Detached => held.is_some() || !revision.is_empty(),
        };
    let response = CreationPreviewResponse {
        choices,
        reference: held.map(|reference| reference.full_name.clone()),
        branch,
        suggested_destination,
        destination,
        destination_is_absolute,
        ready,
    };
    Ok(serde_json::to_value(response).expect("worktree creation preview encodes"))
}

/// The last part of a destination named after `source`.
fn leaf(source: &str) -> String {
    let last = source.rsplit('/').next().unwrap_or_default();
    let leaf: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if leaf.is_empty() {
        LEAF_FALLBACK.to_string()
    } else {
        leaf
    }
}

/// The checkout beside the repository at `root` named after it and the leaf of `source`.
fn beside_repository(root: &str, source: &str) -> String {
    let root = Path::new(root);
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let beside = root.parent().unwrap_or(root);
    beside
        .join(format!("{name}-{}", leaf(source)))
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        worktree: WorktreeFacts,
        changes: Option<usize>,
        status: GitWorktreeStatus,
        actions: GitWorktreeActions,
    }

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[test]
    fn shared_fixture_ranks_states_and_names_refusals() {
        let fixture: Fixture =
            serde_json::from_str(include_str!("../../../fixtures/git/worktree-rules-v1.json"))
                .expect("worktree rules fixture decodes");
        assert!(!fixture.cases.is_empty());
        for case in fixture.cases {
            assert_eq!(
                status(case.worktree, case.changes),
                case.status,
                "{}",
                case.name
            );
            assert_eq!(actions(case.worktree), case.actions, "{}", case.name);
        }
    }

    #[test]
    fn leaf_keeps_letters_digits_dashes_and_underscores() {
        assert_eq!(leaf("feature/login"), "login");
        assert_eq!(leaf("fix bug#12"), "fix-bug-12");
        assert_eq!(leaf("版本"), "版本");
        assert_eq!(leaf(""), LEAF_FALLBACK);
        assert_eq!(leaf("refs/heads/"), LEAF_FALLBACK);
    }
}
