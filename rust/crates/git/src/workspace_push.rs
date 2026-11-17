//! Reviews a push of several workspace repositories at once (`git.workspacePushPreview`).
//!
//! IntelliJ IDEA's Push dialog lists every repository of a project with the branch
//! it would push, the destination, and the commits that remote lacks, and checks
//! the ones that have something to send. Core reads each repository the host
//! binds and says whether it can push and, when it cannot, why: it has no commit
//! yet, HEAD is detached, no remote is configured, or Git failed to answer. Each
//! repository that can push carries its `git.pushPreview`, which the host sends
//! back unchanged as the `expectedPush` of that repository's reviewed `push`.
//!
//! The list comes in push order: a submodule before every listed repository that
//! records it, and otherwise in the order the host listed them. Pushing the
//! checked repositories in that order lets `--recurse-submodules=check` find each
//! recorded commit on its remote. `records` names those submodules, so a host
//! skips a parent whose submodule push failed instead of letting Git refuse it.
//!
//! Every repository is read in turn on the calling thread, because Git's
//! execution options, such as the operation's cancellation, are thread-local.

use super::{
    commit_state, execute_git_readonly, push_preview, validate_root, GitCommitState,
    GitPushPreviewRequest, GitPushPreviewResponse, GitPushTargetKind, PushTargetChoiceRequest,
};
use crate::workspace_commit::{commit_order, relations, validate_bindings, RepositoryBinding};
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bounds one workspace's repository list, as `workspace.repositories` does.
const MAX_REPOSITORIES: usize = 10_000;
/// Commits each preview lists when the host does not ask for another number.
const DEFAULT_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to review pushing the checked-out branch of every bound repository.
pub struct PreviewRequest {
    /// The repositories to review, in the order the host lists them.
    pub repositories: Vec<RepositoryBinding>,
    /// Commits each preview lists, clamped like `git.pushPreview`.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Tag scope every preview resolves: `none`, `all`, or `reachable`.
    #[serde(default)]
    pub push_tags: Option<String>,
    /// Destinations the user picked for divergent upstreams, by repository ID.
    #[serde(default)]
    pub push_targets: BTreeMap<String, PushTargetChoiceRequest>,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// Every bound repository in push order.
pub struct PreviewResponse {
    /// The bound repositories, submodules before the repositories that contain them.
    pub repositories: Vec<RepositoryPush>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
/// Whether a repository can push, and why not.
pub enum PushState {
    /// The preview has commits to send or publishes a new remote branch.
    Ready,
    /// The remote already has every commit of the branch.
    UpToDate,
    /// No remote is configured, so there is nowhere to push.
    NoRemote,
    /// The checked-out branch has no commit yet.
    EmptyRepository,
    /// HEAD names a commit instead of a branch.
    DetachedHead,
    /// Git could not read the repository or resolve its destination; see `detail`.
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// The push review of one repository.
pub struct RepositoryPush {
    /// The repository's binding ID.
    pub id: String,
    /// Whether the repository can push, and why not.
    pub state: PushState,
    /// Short name of the checked-out branch; absent when HEAD is detached or the
    /// repository could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The `git.pushPreview` of a `ready` or `upToDate` repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<GitPushPreviewResponse>,
    /// True when the upstream has another name and the request picked no
    /// destination for it; the push is refused until one is picked.
    pub needs_target: bool,
    /// IDs of the listed repositories this one records as submodules. They come
    /// earlier in the list, and this repository should not push after one of
    /// them failed to.
    pub records: Vec<String>,
    /// What Git or Core said about a `failed` repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl RepositoryPush {
    fn new(id: &str, state: PushState) -> Self {
        Self {
            id: id.into(),
            state,
            branch: None,
            preview: None,
            needs_target: false,
            records: Vec::new(),
            detail: None,
        }
    }

    fn failed(id: &str, error: CoreError) -> Self {
        let mut push = Self::new(id, PushState::Failed);
        push.detail = Some(match error.details {
            Some(details) if !details.trim().is_empty() => {
                format!("{}: {}", error.message, details.trim())
            }
            _ => error.message,
        });
        push
    }
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

/// Reviews pushing every bound repository and orders the list for pushing.
pub fn preview(request: PreviewRequest) -> Result<PreviewResponse, CoreError> {
    if request.repositories.len() > MAX_REPOSITORIES {
        return Err(invalid("Too many workspace repositories"));
    }
    validate_bindings(&request.repositories)?;
    // Checked once here: every preview would otherwise fail alike.
    if !matches!(
        request.push_tags.as_deref(),
        None | Some("none" | "all" | "reachable")
    ) {
        return Err(invalid("Unsupported Git push tag scope"));
    }
    if let Some(id) = request.push_targets.keys().find(|id| {
        !request
            .repositories
            .iter()
            .any(|binding| &binding.id == *id)
    }) {
        return Err(invalid(&format!(
            "Push target names a repository that is not listed: {id}"
        )));
    }

    let mut reviews = BTreeMap::new();
    let mut read = Vec::new();
    let mut states = BTreeMap::new();
    for binding in &request.repositories {
        let (review, state) = review(binding, &request);
        if let Some((root, state)) = state {
            read.push(RepositoryBinding {
                id: binding.id.clone(),
                root,
            });
            states.insert(binding.id.clone(), state);
        }
        reviews.insert(binding.id.clone(), review);
    }

    let relations = relations(&read, &states)?;
    let order = commit_order(
        request.repositories.iter().map(|b| b.id.clone()).collect(),
        &relations,
    )?;
    let repositories = order
        .into_iter()
        .map(|id| {
            let mut review = reviews
                .remove(&id)
                .expect("every listed repository is reviewed");
            review.records = relations
                .iter()
                .filter(|relation| relation.parent == id)
                .map(|relation| relation.child.clone())
                .collect();
            review
        })
        .collect();
    Ok(PreviewResponse { repositories })
}

/// Reviews one repository, and returns the state its submodule records are read
/// from when the repository could be read.
fn review(
    binding: &RepositoryBinding,
    request: &PreviewRequest,
) -> (RepositoryPush, Option<(String, GitCommitState)>) {
    let id = binding.id.as_str();
    let root = match validate_root(&binding.root) {
        Ok(root) => root,
        Err(error) => return (RepositoryPush::failed(id, error), None),
    };
    let state = match commit_state::inspect_root(&root) {
        Ok(state) => state,
        Err(error) => return (RepositoryPush::failed(id, error), None),
    };
    let push = review_state(id, &root, &state, request);
    (push, Some((root, state)))
}

fn review_state(
    id: &str,
    root: &str,
    state: &GitCommitState,
    request: &PreviewRequest,
) -> RepositoryPush {
    let branch = state.branch.as_deref().map(|branch| {
        branch
            .strip_prefix("refs/heads/")
            .unwrap_or(branch)
            .to_string()
    });
    let Some(branch) = branch else {
        return RepositoryPush::new(id, PushState::DetachedHead);
    };
    let mut push = if state.head.is_none() {
        RepositoryPush::new(id, PushState::EmptyRepository)
    } else {
        match has_remote(root) {
            Ok(false) => RepositoryPush::new(id, PushState::NoRemote),
            Ok(true) => preview_branch(id, root, request),
            Err(error) => RepositoryPush::failed(id, error),
        }
    };
    push.branch = Some(branch);
    push
}

fn has_remote(root: &str) -> Result<bool, CoreError> {
    let remotes = execute_git_readonly(root, &["remote".into()], None)?;
    if remotes.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not list Git remotes")
                .with_details(remotes.output),
        );
    }
    Ok(remotes.output.lines().any(|line| !line.trim().is_empty()))
}

fn preview_branch(id: &str, root: &str, request: &PreviewRequest) -> RepositoryPush {
    let target = request.push_targets.get(id).copied();
    let preview = push_preview(GitPushPreviewRequest {
        root: root.into(),
        reference: None,
        git_reference: None,
        limit: request.limit,
        push_tags: request.push_tags.clone(),
        push_target: target,
    });
    match preview {
        Ok(preview) => {
            let state = if preview.commits.is_empty() && !preview.creates_remote_branch {
                PushState::UpToDate
            } else {
                PushState::Ready
            };
            let mut push = RepositoryPush::new(id, state);
            push.needs_target =
                preview.target_kind == GitPushTargetKind::Divergent && target.is_none();
            push.preview = Some(preview);
            push
        }
        Err(error) => RepositoryPush::failed(id, error),
    }
}
