//! Projects the workspace's remote watch stream into one plugin subscription.

use crate::plugin_files::normalize_path;
use lithe_core_rules::visibility::VisibilityRules;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// A bounded batch of host paths associated with one workspace attachment.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    /// The root ID owned by the subscribing plugin.
    #[serde(rename = "rootID")]
    root_id: String,
    /// Remote absolute root of the watch stream.
    host_root: String,
    /// Normalized subscription path relative to that root.
    path: String,
    /// Whether the subscribed resource was a directory.
    directory: bool,
    /// Whether changes below a direct child should be included.
    recursive: bool,
    /// Recent paths on the host, before applying workspace visibility.
    host_paths: Vec<String>,
    /// Whether delivery detected a gap in the stream.
    requires_rescan: bool,
}

/// Returns a subscription event or null when nothing in its scope changed.
/// This operation does no I/O, including when the remote host has disconnected.
pub fn changes(request: Request) -> Value {
    let mut requires_rescan = request.requires_rescan;
    let mut changed_paths = BTreeSet::new();
    if !requires_rescan {
        let root = request.host_root.trim_end_matches('/');
        let prefix = format!("{root}/");
        let visibility = VisibilityRules::new(Vec::new(), Vec::new());
        for host in request.host_paths {
            // An event for a containing directory may have invalidated all
            // children at once, even if the host omitted their names.
            if host == request.host_root || host == root {
                requires_rescan = true;
                continue;
            }
            let Some(relative) = host.strip_prefix(&prefix) else {
                continue;
            };
            let Ok(path) = normalize_path(relative) else {
                requires_rescan = true;
                continue;
            };
            if path != relative || path.is_empty() {
                requires_rescan = true;
                continue;
            }
            if visibility.is_hidden_of_kind(&path, None) {
                continue;
            }
            if path == request.path {
                if request.directory {
                    requires_rescan = true;
                } else {
                    changed_paths.insert(path);
                }
                continue;
            }
            if below(&request.path, &path).is_some() {
                requires_rescan = true;
                continue;
            }
            if request.directory
                && below(&path, &request.path)
                    .is_some_and(|suffix| request.recursive || !suffix.contains('/'))
            {
                changed_paths.insert(path);
            }
        }
    }
    // A rescan supersedes path details from this batch. Bound the worst-case
    // JSON escaping as well as the amount of data buffered by the host.
    if requires_rescan || changed_paths.iter().map(|path| path.len()).sum::<usize>() > 128 * 1024 {
        requires_rescan = true;
        changed_paths.clear();
    }
    if changed_paths.is_empty() && !requires_rescan {
        return Value::Null;
    }
    json!({
        "rootID": request.root_id,
        "changedPaths": changed_paths,
        "requiresRescan": requires_rescan,
    })
}

fn below<'a>(path: &'a str, parent: &str) -> Option<&'a str> {
    if parent.is_empty() && !path.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(parent)?.strip_prefix('/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        path: &str,
        directory: bool,
        recursive: bool,
        host_paths: &[&str],
        lost: bool,
    ) -> Value {
        changes(Request {
            root_id: "test-root".into(),
            host_root: "/home/dev/work".into(),
            path: path.into(),
            directory,
            recursive,
            host_paths: host_paths.iter().map(ToString::to_string).collect(),
            requires_rescan: lost,
        })
    }

    #[test]
    fn scopes_and_deduplicates_paths() {
        let input = [
            "/home/dev/working/src/unrelated",
            "/home/dev/work/src-other/unrelated",
            "/home/dev/work/src/a",
            "/home/dev/work/src/.git/config",
            "/home/dev/work/src/nested/b",
            "/home/dev/work/src/a",
        ];
        assert_eq!(
            event("src", true, true, &input, false)["changedPaths"],
            json!(["src/a", "src/nested/b"])
        );
        assert_eq!(
            event("src", true, false, &input, false)["changedPaths"],
            json!(["src/a"])
        );
        assert_eq!(
            event("src/a", false, true, &input, false)["changedPaths"],
            json!(["src/a"])
        );
        assert!(event("src/missing", false, false, &input, false).is_null());
    }

    #[test]
    fn gaps_and_changes_to_a_containing_folder_require_rescan() {
        let expected = json!({"rootID":"test-root", "changedPaths":[], "requiresRescan":true});
        assert_eq!(
            event("a/b", false, false, &["/home/dev/work/a"], false),
            expected
        );
        assert_eq!(
            event("a/b", true, false, &["/home/dev/work"], false),
            expected
        );
        assert_eq!(
            event("a/b", true, false, &["/home/dev/work/a/../b"], false),
            expected
        );
        assert_eq!(event("a/b", true, false, &[], true), expected);
    }
}
