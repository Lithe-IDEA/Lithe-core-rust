//! Filtering workspace change events down to the ones a snapshot cares about.

use super::*;

/// Whether an ordinary workspace event belongs to visible project inputs.
/// Callers first relativize to the most specific explicitly watched root; native
/// event-loss flags always bypass this filter and request recovery.
pub fn is_workspace_event_relevant(relative: &str) -> bool {
    is_workspace_event_relevant_with(relative, &[])
}

/// Like [`is_workspace_event_relevant`], but a built-in folder the user turned
/// off with a `!` hidden-path entry is visible, so its events are kept.
fn is_workspace_event_relevant_with(relative: &str, hidden_file_patterns: &[String]) -> bool {
    let disabled = disabled_built_ins(hidden_file_patterns);
    !relative.split(['/', '\\']).any(|component| {
        BUILT_IN_HIDDEN_DIRECTORIES.iter().any(|name| {
            component.eq_ignore_ascii_case(name) && !is_disabled_directory(name, &disabled)
        })
    })
}

/// Changed paths for `workspace.filterChanges`: a bare path list, or the
/// paths with the hidden-path entries whose `!` lines turn built-in folders off.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceChangeFilterRequest {
    /// Changed workspace-relative paths, filtered with the built-in rules only.
    Paths(Vec<String>),
    /// Changed paths plus the hidden-path entries that adjust the built-in folders.
    #[serde(rename_all = "camelCase")]
    WithRules {
        /// Changed workspace-relative paths.
        paths: Vec<String>,
        /// Hidden-path entries; only their `!` lines matter here.
        #[serde(default)]
        hidden_file_patterns: Vec<String>,
    },
}

/// Keeps the changed paths outside the effective built-in hidden folders.
pub fn filter_workspace_changes(request: WorkspaceChangeFilterRequest) -> Vec<String> {
    let (paths, patterns) = match request {
        WorkspaceChangeFilterRequest::Paths(paths) => (paths, Vec::new()),
        WorkspaceChangeFilterRequest::WithRules {
            paths,
            hidden_file_patterns,
        } => (paths, hidden_file_patterns),
    };
    paths
        .into_iter()
        .filter(|path| is_workspace_event_relevant_with(path, &patterns))
        .collect()
}
