//! Plugin workspace watch plans and bounded event projection (API-07).
//!
//! The desktop owns native registrations. Core owns path containment, visibility,
//! scope and loss semantics; no watcher state or filesystem contents are retained.

use crate::files::VisibilityRules;
use crate::workspace_read::{check, decode, failure, parent, root, validate_path};
use lithe_platform::scoped_read::Kind;
use lithe_protocol::CoreError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant};

const EVENT_LIMIT: usize = 1000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanRequest {
    root: String,
    path: String,
    #[serde(default)]
    recursive: bool,
}

/// Validates a watch target through no-follow handles before native registration.
/// The native stream covers the root so ancestor moves also invalidate a target.
pub fn plan_command(payload: Value) -> Result<Value, CoreError> {
    let request: PlanRequest = decode(payload)?;
    validate_path(&request.path)?;
    lithe_protocol::cancellation::check()?;
    let handle = root(&request.root)?;
    let info = if request.path.is_empty() {
        handle.metadata().map_err(failure)?
    } else {
        let (parent, name) = parent(handle, &request.path)?;
        parent.child_metadata(OsStr::new(name)).map_err(failure)?
    };
    if !matches!(info.kind, Kind::File | Kind::Directory) {
        return Err(failure(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "A watch target must not be a link or special entry",
        )));
    }
    Ok(
        json!({"root":Path::new(&request.root).canonicalize().map_err(failure)?,
        "path":request.path,"isDirectory":info.kind==Kind::Directory,"recursive":request.recursive}),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventsRequest {
    root: String,
    path: String,
    is_directory: bool,
    #[serde(default)]
    recursive: bool,
    paths: Vec<String>,
    #[serde(default)]
    requires_rescan: bool,
    #[serde(default)]
    hidden_directory_names: Vec<String>,
    #[serde(default)]
    hidden_file_patterns: Vec<String>,
}

/// Projects root-relative native names into one subscription, without following
/// ancestor links. Missing entries remain visible so deletions are not lost.
pub fn events_command(payload: Value) -> Result<Value, CoreError> {
    let request: EventsRequest = decode(payload)?;
    validate_path(&request.path)?;
    let rules = VisibilityRules::new(request.hidden_directory_names, request.hidden_file_patterns);
    let mut paths = BTreeSet::new();
    let mut rescan = request.requires_rescan || request.paths.len() > EVENT_LIMIT;
    let handle = root(&request.root)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    for path in request.paths.into_iter().take(EVENT_LIMIT) {
        check(deadline)?;
        if path.is_empty() {
            rescan = true;
            continue;
        }
        if validate_path(&path).is_err() {
            rescan = true;
            continue;
        }
        let ancestor = !request.path.is_empty() && request.path.starts_with(&format!("{path}/"));
        if ancestor {
            rescan = true;
            continue;
        }
        let descendant = path.strip_prefix(&request.path).and_then(|suffix| {
            if request.path.is_empty() {
                Some(suffix)
            } else {
                suffix.strip_prefix('/')
            }
        });
        let in_scope = path == request.path
            || request.is_directory
                && descendant.is_some_and(|suffix| request.recursive || !suffix.contains('/'));
        if !in_scope || rules.is_hidden(&path, true) || rules.is_hidden(&path, false) {
            continue;
        }
        let metadata = child_metadata(&handle, &path, deadline);
        match metadata {
            Ok(info) if info.kind == Kind::Symlink => continue,
            Ok(_) => {
                paths.insert(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                paths.insert(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                // A replaced ancestor may now be a link or an unreadable directory.
                rescan = true;
            }
            Err(error) => return Err(failure(error)),
        }
    }
    // Keep the validated root handle alive through the batch.
    drop(handle);
    let mut changed = Vec::new();
    let mut bytes = 0;
    for path in paths {
        bytes += serde_json::to_vec(&path).expect("event path encodes").len();
        if bytes > 900 * 1024 {
            rescan = true;
            break;
        }
        changed.push(path);
    }
    Ok(json!({"changedPaths":changed,"requiresRescan":rescan}))
}

fn child_metadata(
    root: &lithe_platform::scoped_read::Directory,
    path: &str,
    deadline: Instant,
) -> std::io::Result<lithe_platform::scoped_read::EntryMetadata> {
    let (parents, name) = path.rsplit_once('/').unwrap_or(("", path));
    let mut current = None;
    for component in parents.split('/').filter(|part| !part.is_empty()) {
        if Instant::now() >= deadline {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        let parent = current.as_ref().unwrap_or(root);
        current = Some(parent.directory(OsStr::new(component))?);
    }
    current
        .as_ref()
        .unwrap_or(root)
        .child_metadata(OsStr::new(name))
}
