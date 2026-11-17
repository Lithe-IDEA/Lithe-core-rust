//! Workspace-owned indexing exclusions, independent of tree visibility and Git writes.

use super::files::{existing_root, is_nested_linked_worktree, relative_path, VisibilityRules};
use lithe_platform::paths::{invalid_relative_path, relative_path_contains};
use lithe_platform::Platform;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// User-owned policy. Missing policies preserve legacy Core callers' behavior.
pub struct IndexingPolicy {
    /// Workspace-relative directories the user excluded.
    pub manual_excluded_directories: Vec<String>,
    /// Workspace-relative directories kept indexed even when Git ignores them.
    pub included_directories: Vec<String>,
    /// Whether directories Git ignores are excluded too.
    pub git_ignore_enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
/// Installs a policy before workspace consumers start; refresh discards Git decisions.
pub struct ConfigureIndexingRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Runtime identity. Omitted by legacy callers, which share one guarded lease.
    #[serde(default)]
    pub owner_id: Option<String>,
    /// Policy to install for `root`.
    #[serde(flatten)]
    pub policy: IndexingPolicy,
    /// Discard cached Git decisions even when the policy is unchanged.
    #[serde(default)]
    pub refresh: bool,
    /// Workspace-relative directories whose exclusion reason the response reports.
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// Effective directory reasons and Git availability, never a claim about third-party scans.
pub struct IndexingStatus {
    /// Lease token the caller passes back to release or refresh.
    pub revision: u64,
    /// Excluded workspace-relative directories mapped to `manual` or `git`.
    pub excluded_directories: BTreeMap<String, String>,
    /// Whether Git could not answer, so Git-ignored directories stayed indexed.
    pub git_ignore_failed: bool,
}

/// Immutable policy generation with lazily cached Git directory decisions.
pub struct IndexingContext {
    /// Policy generation; each installation advances it.
    pub revision: u64,
    /// Watcher scan refreshes retain the installation being committed or rolled back.
    installation_revision: u64,
    /// Legacy release token; watcher refreshes preserve host ownership.
    owner_revision: u64,
    /// Shared across scan generations so one owner cannot retire another owner's root.
    leases: Arc<Mutex<BTreeMap<Option<String>, IndexingLease>>>,
    root: PathBuf,
    policy: IndexingPolicy,
    /// Last successfully returned policy; failed generations never become the fallback.
    fallback_policy: Option<IndexingPolicy>,
    published: AtomicBool,
    decisions: Mutex<HashMap<String, bool>>,
    /// Git answers fetched a directory level at a time before a walk. A parent's answers
    /// reach `decisions` only when the walk asks about it, so prefetching never changes
    /// which directories providers see as excluded.
    prefetched: Mutex<HashMap<String, GitAnswer>>,
    /// Visibility rules each finished prefetch walked with; the lock also serializes them.
    prefetched_scopes: Mutex<Vec<PrefetchScope>>,
    pub(crate) retired: AtomicBool,
    git_failed: AtomicBool,
    git_resolved: AtomicBool,
}

/// What Git said about one directory during a prefetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitAnswer {
    /// Not ignored, holding tracked content, or a nested repository.
    Kept,
    /// Ignored, with nothing tracked below it.
    Excluded,
    /// `check-ignore` failed; the walk records the directory as kept and Git as failed.
    Unavailable,
}

/// The parts of `VisibilityRules` that decide where a prefetch descends.
#[derive(PartialEq, Eq)]
struct PrefetchScope {
    hidden_directories: Vec<String>,
    hidden_file_patterns: Vec<String>,
    delegated_roots: Vec<PathBuf>,
}

impl PrefetchScope {
    fn of(rules: &VisibilityRules) -> Self {
        Self {
            hidden_directories: rules.hidden_directories.clone(),
            hidden_file_patterns: rules.hidden_file_patterns.clone(),
            delegated_roots: rules.delegated_roots.clone(),
        }
    }
}

/// A directory whose children the next prefetch level asks Git about.
struct PrefetchDirectory {
    path: PathBuf,
    /// Where Git runs: the nearest directory holding `.git`, or the workspace root.
    repository: PathBuf,
    /// `path` relative to `repository`, ending in `/` unless empty.
    prefix: String,
}

/// One child directory found by a prefetch level.
struct PrefetchChild {
    path: PathBuf,
    /// Workspace-relative key, the same form `prepare_parent` stores.
    key: String,
    /// Literal pathspec relative to the group's repository.
    pathspec: String,
}

/// A failed installer may reclaim a token only before any caller has received it.
struct IndexingLease {
    revision: u64,
    published: bool,
    /// Remains releasable until a replacement token has actually been returned.
    previous_published: Option<u64>,
}

static POLICIES: OnceLock<Mutex<HashMap<PathBuf, Arc<IndexingContext>>>> = OnceLock::new();
static REVISION: AtomicU64 = AtomicU64::new(1);

fn policies() -> &'static Mutex<HashMap<PathBuf, Arc<IndexingContext>>> {
    POLICIES.get_or_init(Mutex::default)
}

fn validate_paths(paths: &mut Vec<String>) -> Result<(), CoreError> {
    for path in paths.iter_mut() {
        *path = path.trim_end_matches('/').to_string();
        if path != "."
            && (invalid_relative_path(path, Platform::HOST)
                || path.split('/').any(|part| part.is_empty() || part == "."))
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid indexing directory",
            ));
        }
    }
    paths.sort();
    paths.dedup();
    Ok(())
}

/// Registers a project policy atomically. In-flight readers cannot publish a retired generation.
///
/// `update_providers` reconciles the running providers (language servers) of a
/// root with the new policy; the caller that owns them passes it in.
pub fn configure(
    mut request: ConfigureIndexingRequest,
    mut update_providers: impl FnMut(&Path) -> Result<(), CoreError>,
) -> Result<IndexingStatus, CoreError> {
    if request
        .owner_id
        .as_ref()
        .is_some_and(|owner| owner.is_empty())
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Indexing owner is empty",
        ));
    }
    let root = existing_root(&request.root)?;
    validate_paths(&mut request.policy.manual_excluded_directories)?;
    validate_paths(&mut request.policy.included_directories)?;
    validate_paths(&mut request.paths)?;
    let mut changed = false;
    let (context, lease_revision, installed_lease) = {
        let mut entries = policies()
            .lock()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing policy lock was poisoned"))?;
        let unchanged = entries.get(&root).filter(|old| {
            old.policy == request.policy && !request.refresh && !old.retired.load(Ordering::Acquire)
        });
        let context = if let Some(old) = unchanged {
            Arc::clone(old)
        } else {
            changed = true;
            let active = entries
                .get(&root)
                .filter(|old| !old.retired.load(Ordering::Acquire));
            let leases = active
                .map(|old| Arc::clone(&old.leases))
                .unwrap_or_default();
            let legacy_revision = active.map(|old| old.owner_revision);
            let fallback_policy = active.and_then(|old| {
                if old.published.load(Ordering::Acquire) {
                    Some(old.policy.clone())
                } else {
                    old.fallback_policy.clone()
                }
            });
            if let Some(old) = entries.get(&root) {
                old.retired.store(true, Ordering::Release);
            }
            let revision = REVISION.fetch_add(1, Ordering::Relaxed);
            let next = Arc::new(IndexingContext {
                revision,
                installation_revision: revision,
                owner_revision: if request.owner_id.is_none() {
                    revision
                } else {
                    legacy_revision.unwrap_or(revision)
                },
                leases,
                root: root.clone(),
                policy: request.policy,
                fallback_policy,
                published: AtomicBool::new(false),
                decisions: Mutex::default(),
                prefetched: Mutex::default(),
                prefetched_scopes: Mutex::default(),
                retired: AtomicBool::new(false),
                git_failed: AtomicBool::new(false),
                git_resolved: AtomicBool::new(false),
            });
            entries.insert(root.clone(), Arc::clone(&next));
            next
        };
        let (lease_revision, installed_lease) = {
            let mut leases = context.leases.lock().map_err(|_| {
                CoreError::new(ErrorCode::Unknown, "Indexing lease lock was poisoned")
            })?;
            let installed = changed || !leases.contains_key(&request.owner_id);
            if installed {
                let previous_published = leases.get(&request.owner_id).and_then(|lease| {
                    if lease.published {
                        Some(lease.revision)
                    } else {
                        lease.previous_published
                    }
                });
                let token = if request.owner_id.is_none() {
                    context.owner_revision
                } else {
                    REVISION.fetch_add(1, Ordering::Relaxed)
                };
                leases.insert(
                    request.owner_id.clone(),
                    IndexingLease {
                        revision: token,
                        published: false,
                        previous_published,
                    },
                );
            }
            (leases[&request.owner_id].revision, installed)
        };
        (context, lease_revision, installed_lease)
    };
    if changed {
        super::search_index::invalidate_root(&root);
    }
    let result = (|| {
        let mut excluded_directories = BTreeMap::new();
        for path in request.paths {
            if let Some(reason) = context.reason(&path, true)? {
                excluded_directories.insert(path, reason.to_string());
            }
        }
        for path in &context.policy.manual_excluded_directories {
            excluded_directories.insert(path.clone(), "manual".into());
        }
        if let Ok(decisions) = context.decisions.lock() {
            for (path, excluded) in decisions.iter() {
                if *excluded
                    && !context
                        .policy
                        .included_directories
                        .iter()
                        .any(|included| contains(included, path) || contains(path, included))
                {
                    excluded_directories
                        .entry(path.clone())
                        .or_insert_with(|| "git".into());
                }
            }
        }
        update_providers(&root)?;
        {
            // Publication and policy replacement share the registry lock. A
            // scan cannot pass its generation check and then publish an old set.
            let entries = policies().lock().map_err(|_| {
                CoreError::new(ErrorCode::Unknown, "Indexing policy lock was poisoned")
            })?;
            if !entries
                .get(&root)
                .is_some_and(|current| Arc::ptr_eq(current, &context))
            {
                return Err(CoreError::new(
                    ErrorCode::Cancelled,
                    "Indexing policy changed",
                ));
            }
            context.check()?;
            let mut leases = context.leases.lock().map_err(|_| {
                CoreError::new(ErrorCode::Unknown, "Indexing lease lock was poisoned")
            })?;
            let lease = leases
                .get_mut(&request.owner_id)
                .filter(|lease| lease.revision == lease_revision)
                .ok_or_else(|| CoreError::new(ErrorCode::Cancelled, "Indexing lease changed"))?;
            lease.published = true;
            lease.previous_published = None;
            context.published.store(true, Ordering::Release);
        }
        Ok(IndexingStatus {
            revision: lease_revision,
            excluded_directories,
            git_ignore_failed: context.git_failed.load(Ordering::Acquire),
        })
    })();
    if result.is_err() && installed_lease {
        // A caller receiving no token cannot release a failed installation.
        if let Err(error) = release_lease(
            ReleaseIndexingRequest {
                root: root.to_string_lossy().into_owned(),
                revision: lease_revision,
                owner_id: request.owner_id,
            },
            true,
        ) {
            // Failed installation owns recovery; WARN keeps the original failure.
            lithe_diagnostics::core_events::recovery_failed(
                "app.indexing.rollback_failed",
                "lease",
                serde_json::to_value(&error.code)
                    .expect("Core code encodes")
                    .as_str()
                    .expect("Core code token"),
                "core",
            );
        }
        if rollback_failed_policy(&root, &context)? {
            super::search_index::invalidate_root(&root);
            // Restore providers that received only part of the failed update.
            // Preserve the original error if shutdown/cancellation also blocks recovery.
            if let Err(error) = update_providers(&root) {
                lithe_diagnostics::core_events::recovery_failed(
                    "app.indexing.rollback_failed",
                    "provider",
                    serde_json::to_value(&error.code)
                        .expect("Core code encodes")
                        .as_str()
                        .expect("Core code token"),
                    "core",
                );
            }
        }
    }
    result
}

fn rollback_failed_policy(root: &Path, failed: &Arc<IndexingContext>) -> Result<bool, CoreError> {
    let mut entries = policies()
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing policy lock was poisoned"))?;
    let Some(current) = entries.get(root).cloned() else {
        return Ok(false);
    };
    if current.installation_revision != failed.installation_revision
        || current.retired.load(Ordering::Acquire)
        || current.published.load(Ordering::Acquire)
    {
        return Ok(false);
    }
    let Some(policy) = current.fallback_policy.clone() else {
        return Ok(false);
    };
    let leases = current
        .leases
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing lease lock was poisoned"))?;
    if leases.is_empty() {
        return Ok(false);
    }
    let revision = REVISION.fetch_add(1, Ordering::Relaxed);
    let next = Arc::new(IndexingContext {
        revision,
        installation_revision: revision,
        owner_revision: leases
            .get(&None)
            .map_or(current.owner_revision, |lease| lease.revision),
        leases: Arc::clone(&current.leases),
        root: root.to_path_buf(),
        policy: policy.clone(),
        fallback_policy: Some(policy),
        published: AtomicBool::new(true),
        decisions: Mutex::default(),
        prefetched: Mutex::default(),
        prefetched_scopes: Mutex::default(),
        retired: AtomicBool::new(false),
        git_failed: AtomicBool::new(false),
        git_resolved: AtomicBool::new(false),
    });
    current.retired.store(true, Ordering::Release);
    drop(leases);
    entries.insert(root.to_path_buf(), next);
    Ok(true)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
/// Revision-guarded release; a late old window must not retire a newly opened project.
pub struct ReleaseIndexingRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Lease token from the configure response.
    pub revision: u64,
    /// Releases only this runtime's lease, never another runtime using the same root.
    #[serde(default)]
    pub owner_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
/// Refreshes external Git facts without replacing the caller's published lease.
pub struct RefreshIndexingRequest {
    /// Absolute workspace root that bounds every path in the request.
    pub root: String,
    /// Runtime that owns the lease.
    pub owner_id: String,
    /// Existing release token, not the internal scan generation.
    pub revision: u64,
}

/// Invalidates cached decisions for a live owner and reconciles existing providers.
/// No content index is built, and a late request cannot acquire or revive a lease.
/// `update_providers` reconciles the root's running providers, as for [`configure`].
pub fn refresh_indexing(
    request: RefreshIndexingRequest,
    mut update_providers: impl FnMut(&Path) -> Result<(), CoreError>,
) -> Result<IndexingStatus, CoreError> {
    let root = existing_root(&request.root)?;
    let owner = Some(request.owner_id);
    let check_lease = |context: &IndexingContext| -> Result<(), CoreError> {
        context.check()?;
        let leases = context
            .leases
            .lock()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing lease lock was poisoned"))?;
        if !leases.get(&owner).is_some_and(|lease| {
            (lease.published && lease.revision == request.revision)
                || lease.previous_published == Some(request.revision)
        }) {
            return Err(CoreError::new(
                ErrorCode::Cancelled,
                "Indexing lease changed",
            ));
        }
        Ok(())
    };
    let next = {
        let mut entries = policies()
            .lock()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing policy lock was poisoned"))?;
        let old = entries
            .get(&root)
            .ok_or_else(|| CoreError::new(ErrorCode::Cancelled, "Indexing workspace closed"))?;
        check_lease(old)?;
        let next = refreshed_context(old);
        old.retired.store(true, Ordering::Release);
        entries.insert(root.clone(), Arc::clone(&next));
        next
    };
    super::search_index::invalidate_root(&root);
    update_providers(&root)?;
    // Release and replacement use this same lock: never acknowledge a retired owner.
    let _entries = policies()
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing policy lock was poisoned"))?;
    check_lease(&next)?;
    let mut excluded_directories = next
        .policy
        .manual_excluded_directories
        .iter()
        .map(|path| (path.clone(), "manual".to_string()))
        .collect::<BTreeMap<_, _>>();
    let decisions = next
        .decisions
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing decisions lock was poisoned"))?;
    for (path, excluded) in decisions.iter() {
        if *excluded
            && !next
                .policy
                .included_directories
                .iter()
                .any(|included| contains(included, path) || contains(path, included))
        {
            excluded_directories
                .entry(path.clone())
                .or_insert_with(|| "git".into());
        }
    }
    Ok(IndexingStatus {
        revision: request.revision,
        excluded_directories,
        git_ignore_failed: next.git_failed.load(Ordering::Acquire),
    })
}

/// Releases a lease; returns whether a live lease was released.
pub fn release(request: ReleaseIndexingRequest) -> Result<bool, CoreError> {
    release_lease(request, false)
}

fn release_lease(
    request: ReleaseIndexingRequest,
    only_unpublished: bool,
) -> Result<bool, CoreError> {
    let requested = PathBuf::from(request.root);
    let root =
        super::search_index::canonicalize_with_missing_components(&requested).unwrap_or(requested);
    let entries = policies()
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing policy lock was poisoned"))?;
    let Some(context) = entries.get(&root) else {
        return Ok(false);
    };
    let mut leases = context
        .leases
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing lease lock was poisoned"))?;
    let Some(lease) = leases.get(&request.owner_id) else {
        return Ok(false);
    };
    let matches = lease.revision == request.revision
        || (!only_unpublished && lease.previous_published == Some(request.revision));
    if !matches || (only_unpublished && lease.published) {
        return Ok(false);
    }
    if only_unpublished {
        if let Some(previous) = lease.previous_published {
            leases.insert(
                request.owner_id,
                IndexingLease {
                    revision: previous,
                    published: true,
                    previous_published: None,
                },
            );
            return Ok(false);
        }
    }
    leases.remove(&request.owner_id);
    if !leases.is_empty() {
        return Ok(true);
    }
    context.retired.store(true, Ordering::Release);
    context
        .decisions
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing decision lock was poisoned"))?
        .clear();
    // The retired record stays installed to cancel late readers; free its walk-sized caches.
    context
        .prefetched
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing prefetch lock was poisoned"))?
        .clear();
    drop(leases);
    drop(entries);
    super::search_index::invalidate_root(&root);
    Ok(true)
}

/// Installed policy for `root`, if any.
pub fn context(root: &Path) -> Option<Arc<IndexingContext>> {
    policies().lock().ok()?.get(root).cloned()
}

/// Provider requests without a live owner retain their standalone behavior.
/// Retired records still cancel late inventory readers through `context`.
pub fn active_provider_context(root: &Path) -> Option<Arc<IndexingContext>> {
    context(root).filter(|context| !context.retired.load(Ordering::Acquire))
}

pub(crate) fn refresh(root: &Path) -> Result<(), CoreError> {
    {
        let mut entries = policies()
            .lock()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing policy lock was poisoned"))?;
        let Some(old) = entries.get(root) else {
            return Ok(());
        };
        // Closing a workspace must never be undone by a late watcher event.
        old.check()?;
        let next = refreshed_context(old);
        old.retired.store(true, Ordering::Release);
        entries.insert(root.to_path_buf(), next);
    }
    super::search_index::invalidate_root(root);
    // The caller rebuilds Git decisions before publishing provider settings.
    Ok(())
}

fn refreshed_context(old: &IndexingContext) -> Arc<IndexingContext> {
    Arc::new(IndexingContext {
        revision: REVISION.fetch_add(1, Ordering::Relaxed),
        installation_revision: old.installation_revision,
        owner_revision: old.owner_revision,
        leases: Arc::clone(&old.leases),
        root: old.root.clone(),
        policy: old.policy.clone(),
        fallback_policy: old.fallback_policy.clone(),
        published: AtomicBool::new(old.published.load(Ordering::Acquire)),
        decisions: Mutex::default(),
        prefetched: Mutex::default(),
        prefetched_scopes: Mutex::default(),
        retired: AtomicBool::new(false),
        git_failed: AtomicBool::new(false),
        git_resolved: AtomicBool::new(false),
    })
}

/// Resolve native Git decisions before a live provider receives a new generation.
/// This reuses the inventory walker (no file text reads) and prunes excluded trees.
pub fn prepare_provider_exclusions(root: &Path) -> Result<Option<Arc<IndexingContext>>, CoreError> {
    let Some(context) = active_provider_context(root) else {
        return Ok(None);
    };
    context.check()?;
    if context.policy.git_ignore_enabled && !context.git_resolved.load(Ordering::Acquire) {
        // The same built-in rules as the snapshot below, which then reuses this prefetch.
        context.prefetch_git_answers(&VisibilityRules::new(Vec::new(), Vec::new()))?;
        context.prepare_parent(root, "")?;
        super::files::snapshot(super::files::WorkspaceSnapshotRequest {
            root: root.to_string_lossy().into_owned(),
            workspace_roots: Vec::new(),
            hidden_directory_names: Vec::new(),
            hidden_file_patterns: Vec::new(),
            include_tree: false,
        })?;
        context.check()?;
        context.git_resolved.store(true, Ordering::Release);
    }
    Ok(Some(context))
}

/// Returns the live provider context without walking the workspace, and whether
/// its native Git decisions are still unresolved. A language-server start uses
/// this so a large workspace cannot hold the process launch behind a full scan;
/// the caller resolves the pending decisions with `prepare_provider_exclusions`
/// off the start path and then publishes them to the running sessions.
pub fn provider_context_without_scan(
    root: &Path,
) -> Result<(Option<Arc<IndexingContext>>, bool), CoreError> {
    let Some(context) = active_provider_context(root) else {
        return Ok((None, false));
    };
    context.check()?;
    let pending =
        context.policy.git_ignore_enabled && !context.git_resolved.load(Ordering::Acquire);
    Ok((Some(context), pending))
}

fn contains(directory: &str, path: &str) -> bool {
    relative_path_contains(directory, path, Platform::HOST)
}

impl IndexingContext {
    /// Fails with `cancelled` when the request was cancelled or this policy was replaced.
    pub fn check(&self) -> Result<(), CoreError> {
        lithe_protocol::cancellation::check()?;
        if self.retired.load(Ordering::Acquire) {
            return Err(CoreError::new(
                ErrorCode::Cancelled,
                "Indexing policy changed",
            ));
        }
        Ok(())
    }

    pub(crate) fn reason(
        &self,
        relative: &str,
        is_directory: bool,
    ) -> Result<Option<&'static str>, CoreError> {
        self.check()?;
        if self
            .policy
            .manual_excluded_directories
            .iter()
            .any(|dir| contains(dir, relative))
        {
            return Ok(Some("manual"));
        }
        if !self.policy.git_ignore_enabled
            || self
                .policy
                .included_directories
                .iter()
                .any(|dir| contains(dir, relative))
        {
            return Ok(None);
        }
        if relative.is_empty() || relative == "." {
            return Ok(None);
        }
        if is_directory
            && self
                .policy
                .included_directories
                .iter()
                .any(|included| contains(relative, included))
        {
            return Ok(None);
        }
        let parts = relative.split('/').collect::<Vec<_>>();
        let count = parts.len().saturating_sub(usize::from(!is_directory));
        for length in 1..=count {
            let directory = parts[..length].join("/");
            self.prepare_parent(
                self.root.join(&directory).parent().unwrap_or(&self.root),
                &directory,
            )?;
            if self
                .decisions
                .lock()
                .map_err(|_| {
                    CoreError::new(ErrorCode::Unknown, "Indexing decision lock was poisoned")
                })?
                .get(&directory)
                == Some(&true)
            {
                return Ok(Some("git"));
            }
        }
        Ok(None)
    }

    /// Queries sibling directories in one batch, without enumerating their contents.
    fn prepare_parent(&self, parent: &Path, requested: &str) -> Result<(), CoreError> {
        self.check()?;

        if self
            .decisions
            .lock()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "Indexing decision lock was poisoned"))?
            .contains_key(requested)
        {
            return Ok(());
        }
        let Ok(entries) = std::fs::read_dir(parent) else {
            return Ok(());
        };
        let candidates = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        if let Some(decisions) = self.prefetched_decisions(parent, &candidates)? {
            return self.store_decisions(decisions);
        }
        let ignored = lithe_git::ignored_paths(parent, &candidates)?;
        let mut decisions = HashMap::new();
        if let Some(ignored) = ignored {
            // The index is authoritative for tracked descendants; check-ignore on a directory
            // alone does not protect tracked files below an otherwise ignored directory.
            // A lazy tree listing has no scan prefetch. Ask for its ignored siblings together
            // instead of starting one process per directory; keep the native fallback when
            // Git cannot map a batched answer reliably (for example, normalized Unicode).
            let ignored_directories = candidates
                .iter()
                .filter(|name| ignored.contains(*name))
                .cloned()
                .collect::<Vec<_>>();
            let tracked = lithe_git::tracked_directories(parent, &ignored_directories)?;
            for name in candidates {
                let excluded =
                    if parent.join(&name).join(".git").exists() || !ignored.contains(&name) {
                        // A nested repository owns its ignore rules and tracked content.
                        false
                    } else if let Some(tracked) = &tracked {
                        !tracked.contains(&name)
                    } else {
                        let response = lithe_git::execute_git_readonly(
                            &parent.to_string_lossy(),
                            &[
                                "--literal-pathspecs".into(),
                                "ls-files".into(),
                                "-z".into(),
                                "--".into(),
                                name.clone(),
                            ],
                            None,
                        );
                        self.check()?;
                        let Ok(response) = response else {
                            self.git_failed.store(true, Ordering::Release);
                            continue;
                        };
                        if response.exit_code != 0 {
                            self.git_failed.store(true, Ordering::Release);
                            false
                        } else {
                            response.stdout.is_empty()
                        }
                    };
                decisions.insert(relative_path(&parent.join(name), &self.root), excluded);
            }
        } else {
            self.git_failed.store(true, Ordering::Release);
            for name in candidates {
                decisions.insert(relative_path(&parent.join(name), &self.root), false);
            }
        }
        self.store_decisions(decisions)
    }

    fn store_decisions(&self, decisions: HashMap<String, bool>) -> Result<(), CoreError> {
        self.check()?;
        let mut cache = self.decisions.lock().map_err(|_| {
            CoreError::new(ErrorCode::Unknown, "Indexing decision lock was poisoned")
        })?;
        cache.extend(decisions);
        Ok(())
    }

    /// Decisions for `parent`'s `candidates` from a prefetch, or `None` when any candidate
    /// was not answered there, such as a directory created after the prefetch.
    fn prefetched_decisions(
        &self,
        parent: &Path,
        candidates: &[String],
    ) -> Result<Option<HashMap<String, bool>>, CoreError> {
        let prefetched = self.prefetched.lock().map_err(|_| {
            CoreError::new(ErrorCode::Unknown, "Indexing prefetch lock was poisoned")
        })?;
        let mut decisions = HashMap::with_capacity(candidates.len());
        let mut git_failed = false;
        for name in candidates {
            let key = relative_path(&parent.join(name), &self.root);
            let excluded = match prefetched.get(&key) {
                None => return Ok(None),
                Some(GitAnswer::Kept) => false,
                Some(GitAnswer::Excluded) => true,
                Some(GitAnswer::Unavailable) => {
                    git_failed = true;
                    false
                }
            };
            decisions.insert(key, excluded);
        }
        if git_failed {
            self.git_failed.store(true, Ordering::Release);
        }
        Ok(Some(decisions))
    }

    /// Answers, ahead of a walk with `rules`, every Git question the walk's
    /// `prepare_parent` calls would ask, using one `check-ignore` per directory level and
    /// repository instead of one per directory (Lithe-IDEA#1994). Asking per directory
    /// took about 2,200 Git processes and 29 s on Kubernetes before gopls could start.
    ///
    /// Descends only where a walk can ask: visible, undelegated directories that are
    /// neither excluded nor covered by a manual or included directory.
    pub(crate) fn prefetch_git_answers(&self, rules: &VisibilityRules) -> Result<(), CoreError> {
        if !self.policy.git_ignore_enabled {
            return Ok(());
        }
        let mut scopes = self.prefetched_scopes.lock().map_err(|_| {
            CoreError::new(ErrorCode::Unknown, "Indexing prefetch lock was poisoned")
        })?;
        let scope = PrefetchScope::of(rules);
        if scopes.contains(&scope) {
            return Ok(());
        }
        let mut frontier = vec![PrefetchDirectory {
            path: self.root.clone(),
            repository: self.root.clone(),
            prefix: String::new(),
        }];
        while !frontier.is_empty() {
            frontier = self.prefetch_level(frontier, rules)?;
        }
        scopes.push(scope);
        Ok(())
    }

    /// Answers the children of one directory level and returns the next level.
    fn prefetch_level(
        &self,
        frontier: Vec<PrefetchDirectory>,
        rules: &VisibilityRules,
    ) -> Result<Vec<PrefetchDirectory>, CoreError> {
        // The same candidates `prepare_parent` collects, grouped by where Git runs.
        let mut groups = BTreeMap::<PathBuf, Vec<PrefetchChild>>::new();
        for directory in frontier {
            self.check()?;
            let Ok(entries) = std::fs::read_dir(&directory.path) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let path = directory.path.join(&name);
                let child = PrefetchChild {
                    key: relative_path(&path, &self.root),
                    pathspec: format!("{}{name}", directory.prefix),
                    path,
                };
                groups
                    .entry(directory.repository.clone())
                    .or_default()
                    .push(child);
            }
        }
        let mut next = Vec::new();
        for (repository, children) in groups {
            let known = self.known_answers(children.iter().map(|child| &child.key))?;
            let unknown = children
                .iter()
                .filter(|child| !known.contains_key(&child.key))
                .collect::<Vec<_>>();
            let answers = self.answer_group(&repository, &unknown)?;
            self.prefetched
                .lock()
                .map_err(|_| {
                    CoreError::new(ErrorCode::Unknown, "Indexing prefetch lock was poisoned")
                })?
                .extend(answers.iter().map(|(key, answer)| (key.clone(), *answer)));
            for child in children {
                let excluded = match known.get(&child.key) {
                    Some(excluded) => *excluded,
                    // An unanswered directory is left to `prepare_parent`, and so is
                    // everything below it.
                    None => match answers.get(&child.key) {
                        Some(answer) => *answer == GitAnswer::Excluded,
                        None => continue,
                    },
                };
                if excluded || !self.walk_can_ask_below(&child, rules) {
                    continue;
                }
                // A nested repository owns its ignore rules and tracked content.
                let (repository, prefix) = if child.path.join(".git").exists() {
                    (child.path.clone(), String::new())
                } else {
                    (repository.clone(), format!("{}/", child.pathspec))
                };
                next.push(PrefetchDirectory {
                    path: child.path,
                    repository,
                    prefix,
                });
            }
        }
        Ok(next)
    }

    /// Whether a walk with `rules` can ask Git about the children of `child`.
    fn walk_can_ask_below(&self, child: &PrefetchChild, rules: &VisibilityRules) -> bool {
        let covered = |directories: &[String]| {
            directories
                .iter()
                .any(|directory| contains(directory, &child.key))
        };
        !rules.is_hidden(&child.key, true)
            && !rules.is_delegated(&child.path)
            && !is_nested_linked_worktree(&child.path)
            && !covered(&self.policy.manual_excluded_directories)
            && !covered(&self.policy.included_directories)
    }

    /// Answers already held for `keys`, from an earlier prefetch or a walk; `true`
    /// means excluded.
    fn known_answers<'a>(
        &self,
        keys: impl Iterator<Item = &'a String>,
    ) -> Result<HashMap<String, bool>, CoreError> {
        let decisions = self.decisions.lock().map_err(|_| {
            CoreError::new(ErrorCode::Unknown, "Indexing decision lock was poisoned")
        })?;
        let prefetched = self.prefetched.lock().map_err(|_| {
            CoreError::new(ErrorCode::Unknown, "Indexing prefetch lock was poisoned")
        })?;
        Ok(keys
            .filter_map(|key| {
                let excluded = decisions.get(key).copied().or_else(|| {
                    prefetched
                        .get(key)
                        .map(|answer| *answer == GitAnswer::Excluded)
                })?;
                Some((key.clone(), excluded))
            })
            .collect())
    }

    /// Answers `children` of one repository with the same rules as `prepare_parent`,
    /// keyed by workspace-relative path. Children Git cannot answer reliably are left
    /// out, so `prepare_parent` asks about them itself.
    fn answer_group(
        &self,
        repository: &Path,
        children: &[&PrefetchChild],
    ) -> Result<HashMap<String, GitAnswer>, CoreError> {
        if children.is_empty() {
            return Ok(HashMap::new());
        }
        let pathspecs = children
            .iter()
            .map(|child| child.pathspec.clone())
            .collect::<Vec<_>>();
        let Some(ignored) = lithe_git::ignored_paths(repository, &pathspecs)? else {
            return Ok(children
                .iter()
                .map(|child| (child.key.clone(), GitAnswer::Unavailable))
                .collect());
        };
        self.check()?;
        // Git echoes each ignored pathspec; anything else, such as a differently
        // normalized Unicode name, cannot be matched to a directory.
        let requested = pathspecs.iter().collect::<HashSet<_>>();
        if ignored.iter().any(|path| !requested.contains(path)) {
            return Ok(HashMap::new());
        }
        let mut answers = HashMap::with_capacity(children.len());
        let mut ignored_children = Vec::new();
        for child in children {
            if child.path.join(".git").exists() || !ignored.contains(&child.pathspec) {
                answers.insert(child.key.clone(), GitAnswer::Kept);
            } else {
                ignored_children.push(*child);
            }
        }
        if ignored_children.is_empty() {
            return Ok(answers);
        }
        // The index is authoritative for tracked descendants of an ignored directory.
        let pathspecs = ignored_children
            .iter()
            .map(|child| child.pathspec.clone())
            .collect::<Vec<_>>();
        if let Some(tracked) = lithe_git::tracked_directories(repository, &pathspecs)? {
            for child in ignored_children {
                let answer = if tracked.contains(&child.pathspec) {
                    GitAnswer::Kept
                } else {
                    GitAnswer::Excluded
                };
                answers.insert(child.key.clone(), answer);
            }
        }
        self.check()?;
        Ok(answers)
    }
}

pub(crate) fn is_excluded(root: &Path, path: &Path, is_directory: bool) -> Result<bool, CoreError> {
    match context(root) {
        Some(policy) => Ok(policy
            .reason(&relative_path(path, root), is_directory)?
            .is_some()),
        None => Ok(false),
    }
}

/// Resolved roots for provider adapters. Git decisions are populated by the project scan.
pub fn excluded_roots(root: &Path) -> Vec<String> {
    let Some(context) = active_provider_context(root) else {
        return Vec::new();
    };
    let mut roots = context.policy.manual_excluded_directories.clone();
    if let Ok(decisions) = context.decisions.lock() {
        roots.extend(
            decisions
                .iter()
                .filter(|(path, excluded)| {
                    **excluded
                        && !context
                            .policy
                            .included_directories
                            .iter()
                            .any(|included| contains(included, path) || contains(path, included))
                })
                .map(|(path, _)| path.clone()),
        );
    }
    roots.sort();
    roots.dedup();
    roots
}

/// File URIs are normalized through the same workspace root as index consumers.
pub fn uri_is_excluded(root_uri: &str, uri: &str) -> Result<bool, CoreError> {
    let root = url::Url::parse(root_uri)
        .ok()
        .and_then(|value| value.to_file_path().ok());
    let path = url::Url::parse(uri)
        .ok()
        .and_then(|value| value.to_file_path().ok());
    match (root, path) {
        (Some(root), Some(path)) => {
            let root = root.canonicalize().unwrap_or(root);
            // A server may report a canonical URI while its workspace uses a
            // symlink alias. Deleted files still need their surviving ancestor.
            let path =
                super::search_index::canonicalize_with_missing_components(&path).unwrap_or(path);
            if !path.starts_with(&root) {
                return Ok(false);
            }
            if active_provider_context(&root).is_none() {
                return Ok(false);
            }
            is_excluded(&root, &path, false)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{files, search_index};
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn owned_refresh_rechecks_git_without_replacing_any_release_token() {
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        let install = |owner| {
            configure(
                serde_json::from_value(json!({
                    "root":workspace.0,"ownerId":owner,"manualExcludedDirectories":[],
                    "includedDirectories":[],"gitIgnoreEnabled":true
                }))
                .unwrap(),
                |_| Ok(()),
            )
            .unwrap()
            .revision
        };
        let first = install("first");
        let second = install("second");
        assert!(workspace
            .files()
            .contains(&"ignored/deep/Excluded.txt".into()));
        let stale = context(&workspace.0).unwrap();
        fs::write(workspace.0.join(".gitignore"), "ignored/\n").unwrap();
        let refreshed = refresh_indexing(
            RefreshIndexingRequest {
                root: workspace.0.to_string_lossy().into_owned(),
                owner_id: "first".into(),
                revision: first,
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(refreshed.revision, first);
        assert!(stale.check().is_err());
        assert!(!workspace
            .files()
            .contains(&"ignored/deep/Excluded.txt".into()));
        fs::write(workspace.0.join(".gitignore"), "").unwrap();
        refresh_indexing(
            RefreshIndexingRequest {
                root: workspace.0.to_string_lossy().into_owned(),
                owner_id: "second".into(),
                revision: second,
            },
            |_| Ok(()),
        )
        .unwrap();
        assert!(workspace
            .files()
            .contains(&"ignored/deep/Excluded.txt".into()));
        for (owner, revision) in [("first", first), ("second", second)] {
            assert!(release(ReleaseIndexingRequest {
                root: workspace.0.to_string_lossy().into_owned(),
                owner_id: Some(owner.into()),
                revision
            })
            .unwrap());
        }
        let revision = install("reopened");
        let current = context(&workspace.0).unwrap();
        let error = refresh_indexing(
            RefreshIndexingRequest {
                root: workspace.0.to_string_lossy().into_owned(),
                owner_id: "first".into(),
                revision: first,
            },
            |_| Ok(()),
        )
        .err()
        .expect("closed owner must fail");
        assert!(matches!(error.code, ErrorCode::Cancelled));
        assert!(Arc::ptr_eq(&current, &context(&workspace.0).unwrap()));
        assert!(release(ReleaseIndexingRequest {
            root: workspace.0.to_string_lossy().into_owned(),
            owner_id: Some("reopened".into()),
            revision
        })
        .unwrap());
    }

    #[test]
    fn directory_action_fixture_preserves_real_git_search_eligibility() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/workspace/directory-actions-v1.json"
        ))
        .unwrap();
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        fs::create_dir_all(workspace.0.join("archive")).unwrap();
        fs::write(workspace.0.join("archive/Needle.txt"), "Needle").unwrap();
        fs::write(workspace.0.join(".gitignore"), "archive/\n").unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            for step in case["steps"].as_array().unwrap() {
                let excluded = step["role"] == "excluded";
                let included = step["included"].as_bool().unwrap();
                workspace.configure(
                    if excluded { &["archive"] } else { &[] },
                    if included { &["archive"] } else { &[] },
                    true,
                );
                let found = files::search_everywhere(
                    serde_json::from_value(json!({
                        "root":workspace.0,"query":"Needle","includeGeneratedFileNames":true
                    }))
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    found
                        .matches
                        .iter()
                        .any(|item| item.path == "archive/Needle.txt"),
                    included && !excluded,
                    "{}",
                    case["id"]
                );
            }
        }
    }

    #[test]
    fn scoped_everywhere_search_filters_each_source_before_limits_and_pagination() {
        let workspace = Workspace::new();
        workspace.configure(&[], &[], false);
        for folder in ["a-outside", "src", "src-other"] {
            fs::create_dir_all(workspace.0.join(folder)).unwrap();
            for name in ["NeedleA", "NeedleB"] {
                fs::write(
                    workspace.0.join(folder).join(format!("{name}.txt")),
                    format!("{name}\n"),
                )
                .unwrap();
            }
        }
        let kind = "file";
        let request = json!({
            "root":workspace.0,"query":"Needle","scopePaths":["src"],"maxResults":1,
            "maxFileResults":1,"maxSymbolResults":0,"maxContentResults":0
        });
        let response = files::search_everywhere(serde_json::from_value(request).unwrap()).unwrap();
        assert_eq!(response.matches.len(), 1);
        assert!(response.matches[0].path.starts_with("src/"));

        let mut request = json!({
            "root":workspace.0,"query":"Needle","scopePaths":["src"],
            "pageSize":1,"resultKinds":[kind]
        });
        let mut paths = std::collections::BTreeSet::new();
        let mut complete = false;
        for _ in 0..4 {
            let page = super::super::search_paging::search_page(
                serde_json::from_value(request.clone()).unwrap(),
            )
            .unwrap();
            for item in page["matches"].as_array().unwrap() {
                assert_eq!(item["kind"], kind);
                let path = item["path"].as_str().unwrap();
                assert!(path.starts_with("src/"), "out-of-scope result: {path}");
                assert!(paths.insert(path.to_owned()), "duplicate result: {path}");
            }
            if page["nextCursor"].is_null() {
                complete = true;
                break;
            }
            request["cursor"] = page["nextCursor"].clone();
        }
        assert!(complete, "scoped pagination did not terminate");
        assert_eq!(
            paths,
            ["src/NeedleA.txt".to_owned(), "src/NeedleB.txt".to_owned()]
                .into_iter()
                .collect()
        );
    }

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            let root = crate::test_support::temporary_root("indexing-policy");
            fs::create_dir_all(root.join("ignored/deep")).unwrap();
            fs::create_dir_all(root.join("src")).unwrap();
            fs::write(root.join("ignored/deep/Excluded.txt"), "Excluded needle").unwrap();
            fs::write(root.join("src/Included.txt"), "Included needle").unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn configure(&self, manual: &[&str], included: &[&str], git: bool) -> IndexingStatus {
            configure(
                serde_json::from_value(json!({"root": self.0,
                "manualExcludedDirectories": manual, "includedDirectories": included,
                "gitIgnoreEnabled": git, "refresh": true}))
                .unwrap(),
                |_| Ok(()),
            )
            .unwrap()
        }
        fn files(&self) -> Vec<String> {
            files::snapshot(
                serde_json::from_value(json!({"root": self.0, "includeTree": false})).unwrap(),
            )
            .unwrap()
            .files
        }
        fn git(&self, args: &[&str]) {
            let response = lithe_git::execute_git_readonly(
                &self.0.to_string_lossy(),
                &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                None,
            )
            .unwrap();
            assert_eq!(response.exit_code, 0, "{}", response.stderr);
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            if let Some(context) = policies().lock().unwrap().remove(&self.0) {
                context.retired.store(true, Ordering::Release);
            }
            search_index::invalidate_root(&self.0);
            files::DIRECTORY_VISITS.with_borrow_mut(|trace| *trace = None);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nested_roots_prune_real_inventories_search_and_incremental_refills() {
        let parent = Workspace::new();
        let nested = Workspace(parent.0.join("ignored"));
        parent.configure(&[], &[], false);
        fs::write(nested.0.join("Visible.txt"), "Visible needle").unwrap();
        let roots = vec![parent.0.clone(), nested.0.clone()];
        let search = |root: &Path, roots: &[PathBuf]| {
            files::search(
                serde_json::from_value(json!({
                    "root": root, "workspaceRoots": roots, "query":"needle", "maxFileResults":0
                }))
                .unwrap(),
            )
            .unwrap()
        };
        assert_eq!(
            search(&parent.0, &[]).matches.len(),
            3,
            "warm the legacy parent index"
        );
        for git in [false, true] {
            if git {
                nested.git(&["init", "--quiet"]);
                fs::write(nested.0.join(".gitignore"), "deep/\n").unwrap();
            }
            nested.configure(if git { &[] } else { &["deep"] }, &[], git);
            files::DIRECTORY_VISITS.with_borrow_mut(|trace| *trace = Some(Vec::new()));
            let inventory = files::snapshot(
                serde_json::from_value(json!({
                    "root":parent.0, "workspaceRoots":roots, "includeTree":false
                }))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(inventory.files, ["src/Included.txt"]);
            files::DIRECTORY_VISITS.with_borrow(|trace| {
                assert!(!trace
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|path| path.starts_with(&nested.0)))
            });
            assert_eq!(search(&parent.0, &roots).matches.len(), 1);
            assert_eq!(
                search(&nested.0, &roots)
                    .matches
                    .iter()
                    .map(|item| item.path.as_str())
                    .collect::<Vec<_>>(),
                ["Visible.txt"]
            );
            let warm = files::warm_search_index(
                serde_json::from_value(json!({
                    "root":parent.0,"workspaceRoots":roots
                }))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(warm.file_count, 1);
            search_index::update_cached_file(&parent.0, &nested.0.join("Visible.txt"));
            let updated = files::update_search_index(
                serde_json::from_value(json!({
                    "root":parent.0,"workspaceRoots":roots,"paths":["ignored/Visible.txt"]
                }))
                .unwrap(),
                |_| Ok(()),
            )
            .unwrap();
            assert_eq!(
                updated.file_count, 1,
                "watcher updates cannot refill a delegated subtree"
            );
            let preview = files::replace_preview(
                serde_json::from_value(json!({
                    "root":parent.0,"workspaceRoots":roots,"query":"needle","replacement":"changed",
                    "paths":["ignored/Visible.txt"],"textOverrides":{"ignored/Visible.txt":"needle"}
                }))
                .unwrap(),
            )
            .unwrap();
            assert!(
                preview.files.is_empty(),
                "explicit paths and dirty overrides also obey ownership"
            );
            let names = files::search_everywhere(serde_json::from_value(json!({
                "root":parent.0,"workspaceRoots":roots,"query":"Visible","includeGeneratedFileNames":true
            })).unwrap()).unwrap();
            assert!(names.matches.is_empty());
        }
        // Another window's registered policy does not delegate this window's scan.
        assert_eq!(search(&parent.0, &[]).matches.len(), 3);
        let status = context(&nested.0).unwrap();
        release(ReleaseIndexingRequest {
            owner_id: None,
            root: nested.0.to_string_lossy().into_owned(),
            revision: status.owner_revision,
        })
        .unwrap();
        assert_eq!(
            search(&parent.0, std::slice::from_ref(&parent.0))
                .matches
                .len(),
            3,
            "removing an added root restores parent scanning even while its policy is retired"
        );
    }

    #[test]
    fn unavailable_roots_do_not_break_scoped_search_or_missing_descendant_boundaries() {
        let workspace = Workspace::new();
        workspace.configure(&[], &[], false);
        let unavailable_sibling = workspace.0.with_extension("disconnected");
        let roots = vec![workspace.0.clone(), unavailable_sibling];
        let response = files::search(
            serde_json::from_value(json!({
                "root":workspace.0,"workspaceRoots":roots,"scopePaths":["src"],
                "query":"needle","maxFileResults":0,"maxResults":1
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(response.matches.len(), 1);
        let nested = workspace.0.join("ignored");
        fs::remove_dir_all(&nested).unwrap();
        let request =
            json!({"root":workspace.0,"workspaceRoots":[workspace.0,nested],"includeTree":false});
        assert_eq!(
            files::snapshot(serde_json::from_value(request.clone()).unwrap())
                .unwrap()
                .files,
            ["src/Included.txt"]
        );
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("Reappeared.txt"), "Reappeared").unwrap();
        assert_eq!(
            files::snapshot(serde_json::from_value(request).unwrap())
                .unwrap()
                .files,
            ["src/Included.txt"],
            "reappearing delegated content cannot leak into the parent"
        );
    }

    #[test]
    fn nested_root_fixture_keeps_scopes_caps_and_tree_browsing_independent() {
        let parent = Workspace::new();
        let nested = Workspace(parent.0.join("ignored"));
        parent.configure(&[], &[], false);
        nested.configure(&["deep"], &[], false);
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/workspace/indexing-policy-v1.json"
        ))
        .unwrap();
        for case in fixture["ownershipCases"].as_array().unwrap() {
            let roots = if case["nestedRegistered"].as_bool().unwrap() {
                vec![parent.0.clone(), nested.0.clone()]
            } else {
                vec![parent.0.clone()]
            };
            let response = files::search(serde_json::from_value(json!({
                "root":parent.0,"workspaceRoots":roots,"query":"needle","scopePaths":["ignored"],
                "maxFileResults":0,"maxResults":1
            })).unwrap()).unwrap();
            assert_eq!(
                response.matches.len(),
                case["parentScopedMatches"].as_u64().unwrap() as usize
            );
        }
        let tree = files::list_directories(
            serde_json::from_value(json!({
                "root":parent.0,"paths":["ignored/deep"]
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(tree.directories[0].children[0].name, "Excluded.txt");
    }

    #[test]
    fn manual_exclusion_prunes_enumeration_but_keeps_tree_browsing() {
        let workspace = Workspace::new();
        workspace.configure(&["ignored"], &[], false);
        files::DIRECTORY_VISITS.with_borrow_mut(|trace| *trace = Some(Vec::new()));
        assert_eq!(workspace.files(), ["src/Included.txt"]);
        files::DIRECTORY_VISITS.with_borrow(|trace| {
            assert!(!trace
                .as_ref()
                .unwrap()
                .iter()
                .any(|path| path.starts_with(workspace.0.join("ignored"))))
        });
        let tree = files::list_directories(
            serde_json::from_value(
                json!({"root": workspace.0, "paths": ["", "ignored", "ignored/deep"]}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(tree.directories.len(), 3);
        assert!(tree.directories[2]
            .children
            .iter()
            .any(|node| node.name == "Excluded.txt"));
        assert_eq!(
            workspace.files(),
            ["src/Included.txt"],
            "Browsing must not add candidates"
        );
    }

    #[test]
    fn policy_change_retires_old_readers_and_blocks_warm_cache_refill() {
        let workspace = Workspace::new();
        workspace.configure(&[], &[], false);
        let old = context(&workspace.0).unwrap();
        let query = || {
            files::search_everywhere(serde_json::from_value(json!({"root":workspace.0,"query":"Excluded", "includeGeneratedFileNames":true})).unwrap()).unwrap()
        };
        assert!(!query().matches.is_empty());
        workspace.configure(&["ignored"], &["ignored/deep"], false);
        assert!(matches!(
            old.check().unwrap_err().code,
            ErrorCode::Cancelled
        ));
        assert!(
            query().matches.is_empty(),
            "A child exception cannot bypass a manual ancestor"
        );
        search_index::update_cached_file(
            &workspace.0,
            &workspace.0.join("ignored/deep/Excluded.txt"),
        );
        assert!(query().matches.is_empty());
        workspace.configure(&[], &[], false);
        assert!(!query().matches.is_empty());
    }

    /// Integration fixture uses the bounded native Git runner; no user repository or Git config is modified.
    #[test]
    fn git_directory_rules_preserve_tracked_files_and_include_exceptions() {
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        fs::write(
            workspace.0.join(".gitignore"),
            "ignored/\n*.log\ntracked/\n",
        )
        .unwrap();
        fs::write(workspace.0.join("single.log"), "still a candidate").unwrap();
        fs::create_dir_all(workspace.0.join("tracked")).unwrap();
        fs::write(workspace.0.join("tracked/keep.txt"), "Keep").unwrap();
        workspace.git(&["add", "-f", "tracked/keep.txt"]);
        workspace.configure(&[], &[], true);
        let files = workspace.files();
        assert!(!files.contains(&"ignored/deep/Excluded.txt".into()));
        assert!(files.contains(&"single.log".into()));
        assert!(files.contains(&"tracked/keep.txt".into()));
        workspace.configure(&[], &["ignored/deep"], true);
        assert!(workspace
            .files()
            .contains(&"ignored/deep/Excluded.txt".into()));
        workspace.configure(&["ignored/deep"], &["ignored"], true);
        assert!(!workspace
            .files()
            .contains(&"ignored/deep/Excluded.txt".into()));
        workspace.configure(&[], &[], false);
        assert!(workspace
            .files()
            .contains(&"ignored/deep/Excluded.txt".into()));
    }
    #[test]
    fn watcher_refresh_preserves_host_release_and_rejects_both_stale_and_closed_readers() {
        let workspace = Workspace::new();
        let installed = workspace.configure(&["ignored"], &[], false);
        let before = context(&workspace.0).unwrap();
        files::update_search_index(
            serde_json::from_value(json!({
                "root": workspace.0, "paths": [".gitignore"]
            }))
            .unwrap(),
            |_| Ok(()),
        )
        .unwrap();
        let after = context(&workspace.0).unwrap();
        assert_ne!(before.revision, after.revision);
        assert!(before.check().is_err());
        assert_eq!(workspace.files(), ["src/Included.txt"]);
        assert!(release(ReleaseIndexingRequest {
            owner_id: None,
            root: workspace.0.to_string_lossy().into_owned(),
            revision: installed.revision,
        })
        .unwrap());
        assert!(after.check().is_err());
        assert!(
            refresh(&workspace.0).is_err(),
            "A late watcher cannot reopen a closed policy"
        );
        assert!(
            files::snapshot(serde_json::from_value(json!({ "root": workspace.0 })).unwrap())
                .is_err()
        );
        let reopened = workspace.configure(&[], &[], false);
        assert!(!release(ReleaseIndexingRequest {
            owner_id: None,
            root: workspace.0.to_string_lossy().into_owned(),
            revision: installed.revision,
        })
        .unwrap());
        assert_ne!(reopened.revision, installed.revision);
        assert_eq!(workspace.files().len(), 2);
    }

    #[test]
    fn release_rejects_late_work_and_cannot_retire_a_new_generation() {
        let workspace = Workspace::new();
        let first = workspace.configure(&[], &[], false);
        let old = context(&workspace.0).unwrap();
        assert!(release(ReleaseIndexingRequest {
            owner_id: None,
            root: workspace.0.to_string_lossy().into_owned(),
            revision: first.revision
        })
        .unwrap());
        assert!(old.check().is_err());
        let second = workspace.configure(&["ignored"], &[], false);
        assert!(second.revision > first.revision);
        assert!(!release(ReleaseIndexingRequest {
            owner_id: None,
            root: workspace.0.to_string_lossy().into_owned(),
            revision: first.revision
        })
        .unwrap());
        assert_eq!(workspace.files(), ["src/Included.txt"]);
    }

    #[test]
    fn shared_root_leases_survive_other_owner_refresh_close_and_late_release() {
        let workspace = Workspace::new();
        let install = |owner: &str, excluded: bool, refresh: bool| {
            configure(serde_json::from_value(json!({
            "root": workspace.0, "ownerId": owner, "manualExcludedDirectories": if excluded { vec!["ignored"] } else { vec![] },
            "includedDirectories": [], "gitIgnoreEnabled": false, "refresh": refresh
        })).unwrap(), |_| Ok(())).unwrap().revision
        };
        let close = |owner: &str, revision| {
            release(ReleaseIndexingRequest {
                root: workspace.0.to_string_lossy().into_owned(),
                owner_id: Some(owner.into()),
                revision,
            })
            .unwrap()
        };
        let first = install("window-a/workspace-a", false, false);
        let second = install("window-b/workspace-b", false, false);
        assert_ne!(first, second);
        assert!(
            !release_lease(
                ReleaseIndexingRequest {
                    root: workspace.0.to_string_lossy().into_owned(),
                    owner_id: Some("window-b/workspace-b".into()),
                    revision: second,
                },
                true
            )
            .unwrap(),
            "a failed concurrent installer cannot reclaim a token already returned to a caller"
        );
        assert!(close("window-a/workspace-a", first));
        assert_eq!(
            workspace.files().len(),
            2,
            "the other workspace still owns this root"
        );
        let reopened = install("window-a/workspace-reopened", true, true);
        assert!(!close("window-a/workspace-a", first));
        assert!(
            close("window-b/workspace-b", second),
            "policy replacement preserves other leases"
        );
        refresh(&workspace.0).unwrap();
        assert_eq!(workspace.files(), ["src/Included.txt"]);
        let replacement = install("window-a/workspace-reopened", false, true);
        assert!(!close("window-a/workspace-reopened", reopened));
        assert_eq!(workspace.files().len(), 2);
        assert!(close("window-a/workspace-reopened", replacement));
        assert!(context(&workspace.0).unwrap().check().is_err());
        assert!(
            refresh(&workspace.0).is_err(),
            "watchers must not resurrect the last closed lease"
        );
    }

    #[test]
    fn failed_policy_update_restores_the_last_successful_policy_and_owner_leases() {
        assert_failed_policy_restores_published_state(false);
    }

    #[test]
    fn failed_policy_update_restores_the_policy_after_a_concurrent_watcher_refresh() {
        assert_failed_policy_restores_published_state(true);
    }

    fn assert_failed_policy_restores_published_state(watcher_refresh: bool) {
        let workspace = Workspace::new();
        let request = |owner: &str, excluded: bool| {
            serde_json::from_value(json!({
            "root":workspace.0,"ownerId":owner,"manualExcludedDirectories":if excluded {vec!["ignored"]} else {vec![]},
            "includedDirectories":[],"gitIgnoreEnabled":false
        })).unwrap()
        };
        let original = configure(request("owner-a", false), |_| Ok(())).unwrap();
        let other = configure(request("owner-b", false), |_| Ok(())).unwrap();
        let old_context = context(&workspace.0).unwrap();
        let mut calls = 0;
        let result = configure(request("owner-a", true), |_| {
            calls += 1;
            if calls == 1 {
                if watcher_refresh {
                    refresh(&workspace.0)?;
                }
                Err(CoreError::new(
                    ErrorCode::Cancelled,
                    "provider update cancelled",
                ))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(calls, 2, "providers receive a best-effort restoration");
        assert!(
            old_context.check().is_err(),
            "old in-flight scans stay retired after rollback"
        );
        assert_eq!(
            workspace.files().len(),
            2,
            "failed exclusions must not remain active"
        );
        for (owner, revision) in [("owner-a", original.revision), ("owner-b", other.revision)] {
            assert!(release(ReleaseIndexingRequest {
                root: workspace.0.to_string_lossy().into_owned(),
                owner_id: Some(owner.into()),
                revision
            })
            .unwrap());
        }
        assert!(context(&workspace.0).unwrap().check().is_err());
    }

    #[test]
    fn another_successful_installer_keeps_an_adopted_policy_after_the_first_caller_fails() {
        let workspace = Workspace::new();
        let request = |excluded| {
            serde_json::from_value(json!({
            "root":workspace.0,"ownerId":"owner-a","manualExcludedDirectories":if excluded {vec!["ignored"]} else {vec![]},
            "includedDirectories":[],"gitIgnoreEnabled":false
        })).unwrap()
        };
        configure(request(false), |_| Ok(())).unwrap();
        let mut received = 0;
        let result = configure(request(true), |_| {
            received = configure(request(true), |_| Ok(()))?.revision;
            Err(CoreError::new(
                ErrorCode::Cancelled,
                "caller cancelled after concurrent publication",
            ))
        });
        assert!(result.is_err());
        assert_eq!(workspace.files(), ["src/Included.txt"]);
        assert!(release(ReleaseIndexingRequest {
            root: workspace.0.to_string_lossy().into_owned(),
            owner_id: Some("owner-a".into()),
            revision: received
        })
        .unwrap());
    }

    #[test]
    fn cancelled_reconfiguration_preserves_the_last_published_owner_lease() {
        let workspace = Workspace::new();
        let request = |owner: &str, excluded: bool, refresh: bool| {
            serde_json::from_value(json!({
            "root":workspace.0,"ownerId":owner,"manualExcludedDirectories":if excluded {vec!["ignored"]} else {vec![]},
            "includedDirectories":[],"gitIgnoreEnabled":false,"refresh":refresh
        })).unwrap()
        };
        let original = configure(request("owner-a", false, false), |_| Ok(())).unwrap();
        let mut other = 0;
        let cancelled = configure(request("owner-a", false, true), |_| {
            other = configure(request("owner-b", true, false), |_| Ok(()))?.revision;
            Ok(())
        });
        assert!(matches!(
            cancelled,
            Err(CoreError {
                code: ErrorCode::Cancelled,
                ..
            })
        ));
        assert!(release(ReleaseIndexingRequest {
            root: workspace.0.to_string_lossy().into_owned(),
            owner_id: Some("owner-b".into()),
            revision: other
        })
        .unwrap());
        assert_eq!(
            workspace.files(),
            ["src/Included.txt"],
            "the still-open owner keeps the latest root policy alive"
        );
        assert!(release(ReleaseIndexingRequest {
            root: workspace.0.to_string_lossy().into_owned(),
            owner_id: Some("owner-a".into()),
            revision: original.revision
        })
        .unwrap());
        assert!(context(&workspace.0).unwrap().check().is_err());
    }

    #[test]
    fn cancelled_installer_cannot_withdraw_a_shared_token_returned_by_another_call() {
        let workspace = Workspace::new();
        let request = |owner: &str, excluded: bool| {
            serde_json::from_value(json!({
            "root":workspace.0,"ownerId":owner,"manualExcludedDirectories":if excluded {vec!["ignored"]} else {vec![]},
            "includedDirectories":[],"gitIgnoreEnabled":false
        })).unwrap()
        };
        let mut received = 0;
        let mut other = 0;
        let cancelled = configure(request("owner-a", false), |_| {
            received = configure(request("owner-a", false), |_| Ok(()))?.revision;
            other = configure(request("owner-b", true), |_| Ok(()))?.revision;
            Ok(())
        });
        assert!(cancelled.is_err());
        assert!(release(ReleaseIndexingRequest {
            root: workspace.0.to_string_lossy().into_owned(),
            owner_id: Some("owner-b".into()),
            revision: other
        })
        .unwrap());
        assert_eq!(workspace.files(), ["src/Included.txt"]);
        assert!(release(ReleaseIndexingRequest {
            root: workspace.0.to_string_lossy().into_owned(),
            owner_id: Some("owner-a".into()),
            revision: received
        })
        .unwrap());
    }

    #[test]
    fn closing_the_published_lease_also_cancels_an_unpublished_replacement() {
        let workspace = Workspace::new();
        let request = |refresh| {
            serde_json::from_value(json!({
            "root":workspace.0,"ownerId":"owner-a","manualExcludedDirectories":[],"includedDirectories":[],"gitIgnoreEnabled":false,"refresh":refresh
        })).unwrap()
        };
        let original = configure(request(false), |_| Ok(())).unwrap();
        let cancelled = configure(request(true), |_| {
            assert!(release(ReleaseIndexingRequest {
                root: workspace.0.to_string_lossy().into_owned(),
                owner_id: Some("owner-a".into()),
                revision: original.revision
            })?);
            Ok(())
        });
        assert!(cancelled.is_err());
        assert!(context(&workspace.0).unwrap().check().is_err());
    }

    #[test]
    fn provider_preparation_returns_the_exact_generation_that_was_scanned() {
        let workspace = Workspace::new();
        workspace.configure(&[], &[], true);
        let prepared = prepare_provider_exclusions(&workspace.0).unwrap().unwrap();
        assert!(prepared.git_resolved.load(Ordering::Acquire));
        refresh(&workspace.0).unwrap();
        assert!(prepared.check().is_err());
        let next = context(&workspace.0).unwrap();
        assert!(!next.git_resolved.load(Ordering::Acquire));
        assert!(next.check().is_ok());
    }

    #[test]
    fn provider_context_without_scan_reports_pending_git_decisions_without_resolving_them() {
        let workspace = Workspace::new();
        assert!(provider_context_without_scan(&workspace.0)
            .unwrap()
            .0
            .is_none());
        workspace.configure(&[], &[], true);
        let (context, pending) = provider_context_without_scan(&workspace.0).unwrap();
        let context = context.unwrap();
        assert!(pending, "Git decisions are unresolved before any scan");
        assert!(
            !context.git_resolved.load(Ordering::Acquire),
            "the start path must not walk the workspace"
        );
        let prepared = prepare_provider_exclusions(&workspace.0).unwrap().unwrap();
        assert_eq!(prepared.revision, context.revision);
        let (_, pending) = provider_context_without_scan(&workspace.0).unwrap();
        assert!(!pending, "a resolved generation needs no background scan");
    }

    #[test]
    fn provider_context_without_scan_needs_no_scan_when_git_ignore_is_disabled() {
        let workspace = Workspace::new();
        workspace.configure(&[], &[], false);
        let (context, pending) = provider_context_without_scan(&workspace.0).unwrap();
        assert!(context.is_some());
        assert!(!pending);
    }

    #[test]
    fn git_negation_nested_rules_new_directories_and_unicode_are_native() {
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        fs::write(
            workspace.0.join(".gitignore"),
            "projects/*/repos/\n晚到目录/\n",
        )
        .unwrap();
        fs::create_dir_all(workspace.0.join("projects/中文 空格/repos")).unwrap();
        fs::write(
            workspace.0.join("projects/.gitignore"),
            "!中文 空格/repos/\n",
        )
        .unwrap();
        fs::write(
            workspace.0.join("projects/中文 空格/repos/kept.txt"),
            "kept",
        )
        .unwrap();
        workspace.configure(&[], &[], true);
        assert!(workspace
            .files()
            .contains(&"projects/中文 空格/repos/kept.txt".into()));
        fs::create_dir_all(workspace.0.join("晚到目录")).unwrap();
        fs::write(workspace.0.join("晚到目录/hidden.txt"), "hidden").unwrap();
        assert!(
            !workspace.files().contains(&"晚到目录/hidden.txt".into()),
            "A cached parent must still check a newly created directory"
        );
    }

    /// Listing one expanded directory must not start one Git process per ignored sibling.
    /// This exercises the lazy tree path before a snapshot or provider scan warms its cache.
    #[test]
    fn directory_listing_batches_tracked_checks_for_ignored_siblings() {
        use lithe_git::console::QueryCategory;
        let _deadline = lithe_protocol::cancellation::Scope::begin(None, Some(10_000));
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        fs::write(workspace.0.join(".gitignore"), "generated*/\n").unwrap();
        for index in 0..64 {
            fs::create_dir(workspace.0.join(format!("generated {index:03}"))).unwrap();
        }
        fs::write(workspace.0.join("generated 000/kept.txt"), "tracked").unwrap();
        workspace.git(&["add", "-f", "generated 000/kept.txt"]);
        for name in ["generated [literal]", "generated literal"] {
            fs::create_dir(workspace.0.join(name)).unwrap();
        }
        fs::write(workspace.0.join("generated [literal]/kept.txt"), "tracked").unwrap();
        workspace.git(&["add", "-f", "generated [literal]/kept.txt"]);
        let nested = workspace.0.join("generated nested");
        fs::create_dir(&nested).unwrap();
        let initialized = lithe_git::execute_git_readonly(
            &nested.to_string_lossy(),
            &["init".into(), "--quiet".into()],
            None,
        )
        .unwrap();
        assert_eq!(initialized.exit_code, 0, "{}", initialized.stderr);
        workspace.configure(&[], &[], true);

        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = calls.clone();
        let listings = lithe_request_context::events::with_sink(
            Arc::new(move |event| {
                let event: serde_json::Value = serde_json::from_str(event).unwrap();
                if event["type"] == "started" {
                    for argument in event["arguments"].as_array().unwrap() {
                        if let Some(command @ ("check-ignore" | "ls-files")) = argument.as_str() {
                            recorded.lock().unwrap().push(command.to_owned());
                        }
                    }
                }
            }),
            || {
                let _policy = lithe_git::execution_policy::Scope::begin(Some(
                    lithe_git::execution_policy::GitExecutionOptions {
                        console_queries: vec![QueryCategory::Checks],
                        query_category: Some(QueryCategory::Checks),
                        ..Default::default()
                    },
                ))
                .unwrap();
                let request =
                    || serde_json::from_value(json!({"root":workspace.0,"paths":[""]})).unwrap();
                let first = files::list_directories(request()).unwrap();
                let second = files::list_directories(request()).unwrap();
                assert_eq!(
                    first.directories[0].children.len(),
                    second.directories[0].children.len()
                );
                first
            },
        );
        let children = &listings.directories[0].children;
        for index in 0..64 {
            let name = format!("generated {index:03}");
            assert!(children.iter().any(|node| node.name == name));
        }
        let policy = context(&workspace.0).unwrap();
        assert_eq!(policy.reason("generated 000", true).unwrap(), None);
        assert_eq!(policy.reason("generated 001", true).unwrap(), Some("git"));
        assert_eq!(policy.reason("generated [literal]", true).unwrap(), None);
        assert_eq!(
            policy.reason("generated literal", true).unwrap(),
            Some("git")
        );
        assert_eq!(policy.reason("generated nested", true).unwrap(), None);
        assert!(!policy.git_failed.load(Ordering::Acquire));
        let calls = calls.lock().unwrap();
        let count = |command: &str| calls.iter().filter(|value| *value == command).count();
        assert_eq!(count("check-ignore"), 1, "queries: {calls:?}");
        assert_eq!(count("ls-files"), 1, "queries: {calls:?}");
    }

    /// A failed or unmatchable batch still uses the native per-directory answer.
    #[cfg(unix)]
    #[test]
    fn directory_listing_falls_back_when_batched_tracked_checks_are_unavailable() {
        use std::os::unix::fs::PermissionsExt;
        let _deadline = lithe_protocol::cancellation::Scope::begin(None, Some(10_000));
        for batch_result in ["exit 128", "printf 'outside-request/file\\000'; exit 0"] {
            let workspace = Workspace::new();
            workspace.git(&["init", "--quiet"]);
            fs::write(
                workspace.0.join(".gitignore"),
                "ignored/\nother/\ntracked/\n",
            )
            .unwrap();
            fs::create_dir(workspace.0.join("other")).unwrap();
            fs::create_dir(workspace.0.join("tracked")).unwrap();
            fs::write(workspace.0.join("tracked/kept.txt"), "tracked").unwrap();
            workspace.git(&["add", "-f", "tracked/kept.txt"]);
            let wrapper = workspace.0.join(".git/listing-test-git");
            fs::write(
                &wrapper,
                format!(
                    "#!/bin/sh\ntracked=0\npaths=0\ncount=0\n\
                     for argument in \"$@\"; do\n\
                       if [ \"$argument\" = ls-files ]; then tracked=1; fi\n\
                       if [ \"$paths\" = 1 ]; then count=$((count + 1)); fi\n\
                       if [ \"$argument\" = -- ]; then paths=1; fi\n\
                     done\n\
                     if [ \"$tracked\" = 1 ] && [ \"$count\" -gt 1 ]; then\n\
                       printf 'batch\\n' >> \"$0.attempts\"\n\
                       {batch_result}\n\
                     fi\n\
                     exec git \"$@\"\n"
                ),
            )
            .unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
            workspace.configure(&[], &[], true);
            let _policy = lithe_git::execution_policy::Scope::begin(Some(
                lithe_git::execution_policy::GitExecutionOptions {
                    executable: Some(wrapper.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            ))
            .unwrap();
            let listing = files::list_directories(
                serde_json::from_value(json!({"root":workspace.0,"paths":[""]})).unwrap(),
            )
            .unwrap();
            assert!(listing.directories[0]
                .children
                .iter()
                .any(|node| node.name == "ignored"));
            let policy = context(&workspace.0).unwrap();
            assert_eq!(policy.reason("ignored", true).unwrap(), Some("git"));
            assert_eq!(policy.reason("other", true).unwrap(), Some("git"));
            assert_eq!(policy.reason("tracked", true).unwrap(), None);
            assert!(!policy.git_failed.load(Ordering::Acquire));
            assert_eq!(
                fs::read_to_string(wrapper.with_extension("attempts")).unwrap(),
                "batch\n"
            );
        }
    }

    /// Cancelling a tracked query must not start the per-directory fallback or cache answers.
    #[test]
    fn directory_listing_tracked_check_cancellation_does_not_fall_back() {
        use lithe_git::console::QueryCategory;
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        fs::write(workspace.0.join(".gitignore"), "ignored/\nother/\n").unwrap();
        fs::create_dir(workspace.0.join("other")).unwrap();
        workspace.configure(&[], &[], true);
        let operation = format!("listing-cancel-{}", workspace.0.display());
        let _deadline =
            lithe_protocol::cancellation::Scope::begin(Some(operation.clone()), Some(5_000));
        let attempts = Arc::new(AtomicU64::new(0));
        let recorded = attempts.clone();
        let result = lithe_request_context::events::with_sink(
            Arc::new(move |event| {
                let event: serde_json::Value = serde_json::from_str(event).unwrap();
                if event["type"] == "started"
                    && event["arguments"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|arg| arg == "ls-files")
                {
                    recorded.fetch_add(1, Ordering::Relaxed);
                    assert!(lithe_protocol::cancellation::cancel(&operation));
                }
            }),
            || {
                let _policy = lithe_git::execution_policy::Scope::begin(Some(
                    lithe_git::execution_policy::GitExecutionOptions {
                        console_queries: vec![QueryCategory::Checks],
                        query_category: Some(QueryCategory::Checks),
                        ..Default::default()
                    },
                ))
                .unwrap();
                context(&workspace.0).unwrap().reason("ignored", true)
            },
        );
        assert!(matches!(result.unwrap_err().code, ErrorCode::Cancelled));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        assert!(context(&workspace.0)
            .unwrap()
            .decisions
            .lock()
            .unwrap()
            .is_empty());
    }

    /// Regression for Lithe-IDEA#1994: preparing provider exclusions asked Git once per
    /// parent directory, which delayed gopls by about 30 s on Kubernetes. A wrapper
    /// executable counts the Git processes: they must grow with directory depth and the
    /// number of repositories, not with how many directories exist, and the exclusions
    /// must stay the native per-directory result.
    #[cfg(unix)]
    #[test]
    fn provider_preparation_asks_git_per_directory_level_not_per_parent() {
        use std::os::unix::fs::PermissionsExt;
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        fs::write(workspace.0.join(".gitignore"), "generated/\n").unwrap();
        for package in 0..8 {
            for module in 0..4 {
                let module = workspace.0.join(format!("pkg{package}/mod{module}"));
                fs::create_dir_all(module.join("src/main")).unwrap();
                fs::create_dir_all(module.join("generated/out")).unwrap();
                fs::write(module.join("src/main/Main.txt"), "main").unwrap();
                fs::write(module.join("generated/out/Out.txt"), "out").unwrap();
            }
        }
        // A tracked file keeps its otherwise ignored directory indexed.
        fs::write(
            workspace.0.join("pkg0/mod0/generated/Tracked.txt"),
            "tracked",
        )
        .unwrap();
        workspace.git(&["add", "-f", "pkg0/mod0/generated/Tracked.txt"]);
        // A nested repository answers with its own ignore rules.
        let nested = workspace.0.join("pkg1/nested");
        fs::create_dir_all(nested.join("cache")).unwrap();
        fs::write(nested.join("cache/Cached.txt"), "cached").unwrap();
        fs::write(nested.join(".gitignore"), "cache/\n").unwrap();
        let init = lithe_git::execute_git_readonly(
            &nested.to_string_lossy(),
            &["init".into(), "--quiet".into()],
            None,
        )
        .unwrap();
        assert_eq!(init.exit_code, 0, "{}", init.stderr);

        let trace = crate::test_support::temporary_root("git-trace");
        fs::create_dir_all(&trace).unwrap();
        let log = trace.join("calls.log");
        let wrapper = trace.join("git");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nfor argument in \"$@\"; do\n  case \"$argument\" in\n    \
                 check-ignore|ls-files) echo \"$argument\" >> '{}' ;;\n  esac\ndone\n\
                 exec git \"$@\"\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        workspace.configure(&[], &[], true);
        let files = {
            let _scope = lithe_git::execution_policy::Scope::begin(Some(
                lithe_git::execution_policy::GitExecutionOptions {
                    executable: Some(wrapper.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            ))
            .unwrap();
            prepare_provider_exclusions(&workspace.0).unwrap().unwrap();
            // Later walkers of the same generation reuse the decisions.
            workspace.files()
        };
        let calls = fs::read_to_string(&log).unwrap_or_default();
        let _ = fs::remove_dir_all(&trace);

        let mut expected = (0..8)
            .flat_map(|package| {
                (0..4).map(move |module| format!("pkg{package}/mod{module}/generated"))
            })
            .filter(|path| path != "pkg0/mod0/generated")
            .chain(["pkg0/mod0/generated/out".into(), "pkg1/nested/cache".into()])
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(excluded_roots(&workspace.0), expected);
        assert!(files.contains(&"pkg0/mod0/generated/Tracked.txt".into()));
        assert!(files.contains(&"pkg7/mod3/src/main/Main.txt".into()));
        assert!(!files.contains(&"pkg7/mod3/generated/out/Out.txt".into()));
        assert!(!files.contains(&"pkg1/nested/cache/Cached.txt".into()));
        assert!(!context(&workspace.0)
            .unwrap()
            .git_failed
            .load(Ordering::Acquire));

        // Four directory levels in the outer repository plus one in the nested one; asking
        // per parent directory took 76 `check-ignore` and 33 `ls-files` processes.
        let count = |command: &str| calls.lines().filter(|line| *line == command).count();
        let (check_ignore, ls_files) = (count("check-ignore"), count("ls-files"));
        assert!(
            check_ignore <= 5 && ls_files <= 3,
            "check-ignore ran {check_ignore} times, ls-files {ls_files} times"
        );

        // Closing the workspace frees the prefetched answers along with the decisions.
        let policy = context(&workspace.0).unwrap();
        assert!(!policy.prefetched.lock().unwrap().is_empty());
        assert!(release(ReleaseIndexingRequest {
            owner_id: None,
            root: workspace.0.to_string_lossy().into_owned(),
            revision: policy.owner_revision,
        })
        .unwrap());
        assert!(policy.prefetched.lock().unwrap().is_empty());
    }

    #[test]
    fn git_failure_does_not_drop_manual_exclusions() {
        let workspace = Workspace::new();
        workspace.configure(&["ignored"], &[], true);
        assert_eq!(workspace.files(), ["src/Included.txt"]);
        let status = configure(serde_json::from_value(json!({"root":workspace.0,
            "manualExcludedDirectories":["ignored"],"includedDirectories":[],"gitIgnoreEnabled":true})).unwrap(), |_| Ok(())).unwrap();
        assert!(status.git_ignore_failed);
        assert_eq!(status.excluded_directories["ignored"], "manual");
    }
    #[test]
    fn scoped_search_keeps_project_policy_and_applies_scope_before_limit() {
        let workspace = Workspace::new();
        workspace.configure(&[], &[], false);
        let search = || {
            files::search(
                serde_json::from_value(json!({
                    "root":workspace.0,"query":"needle","scopePaths":["src"],
                    "maxResults":1,"maxFileResults":0
                }))
                .unwrap(),
            )
            .unwrap()
        };
        assert_eq!(search().matches[0].path, "src/Included.txt");
        workspace.configure(&["src"], &[], false);
        assert!(search().matches.is_empty());
    }
    #[test]
    fn repository_archive_fixture_reduces_visits_and_resident_index_files() {
        let workspace = Workspace::new();
        workspace.git(&["init", "--quiet"]);
        fs::write(
            workspace.0.join(".gitignore"),
            "bare-repos/\nprojects/*/repos/\n",
        )
        .unwrap();
        for directory in [
            "bare-repos/archive",
            "projects/alpha/repos/archive",
            "projects/beta/repos/archive",
        ] {
            fs::create_dir_all(workspace.0.join(directory)).unwrap();
            for index in 0..32 {
                fs::write(
                    workspace.0.join(directory).join(format!("file{index}.txt")),
                    "needle",
                )
                .unwrap();
            }
        }
        let measure = |git| {
            workspace.configure(&[], &[], git);
            files::DIRECTORY_VISITS.with_borrow_mut(|trace| *trace = Some(Vec::new()));
            let stats = files::warm_search_index(
                serde_json::from_value(json!({"root":workspace.0})).unwrap(),
            )
            .unwrap();
            let visits = files::DIRECTORY_VISITS.with_borrow(|trace| trace.as_ref().unwrap().len());
            (stats.file_count, visits)
        };
        let before = measure(false);
        let after = measure(true);
        assert_eq!(before.0 - after.0, 96);
        assert!(after.1 < before.1);
        eprintln!(
            "archive fixture: indexed files {} -> {}, directory visits {} -> {}",
            before.0, after.0, before.1, after.1
        );
    }
    #[cfg(unix)]
    #[test]
    fn inaccessible_optional_directory_reports_an_incomplete_inventory() {
        use std::os::unix::fs::PermissionsExt;
        let workspace = Workspace::new();
        workspace.configure(&[], &[], false);
        let directory = workspace.0.join("ignored/deep");
        struct Restore(std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
        }
        let _restore = Restore(directory.clone());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&directory).is_ok() {
            return;
        } // Privileged hosts cannot exercise permission denial.
        for include_tree in [true, false] {
            let snapshot = files::snapshot(
                serde_json::from_value(json!({"root":workspace.0,"includeTree":include_tree}))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(snapshot.incomplete_directories, ["ignored/deep"]);
            assert!(snapshot.files.contains(&"src/Included.txt".into()));
        }
    }
}
