//! File-group projections for the loaded search window, independent of desktop widgets.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Native clients retain text and submit only stable path identities.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPresentationRequest {
    /// Paths in loaded result order; one entry for every matching source line.
    pub paths: Vec<String>,
    /// Collapsed file identities. Unmentioned files start expanded.
    #[serde(default)]
    pub collapsed: HashSet<String>,
}

/// A file heading or an index into the native client's loaded results.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPresentationRow {
    /// Project-relative file identity.
    pub path: String,
    /// Loaded matching line count for headings; zero for match rows.
    pub count: usize,
    /// Source result index, absent for a heading.
    pub match_index: Option<usize>,
}

/// Grouped rows and loaded totals; a continuation means these totals are lower bounds.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPresentation {
    /// Virtual rows in deterministic path and source-line order.
    pub rows: Vec<SearchPresentationRow>,
    /// Number of loaded files.
    pub files: usize,
    /// Number of loaded matching lines.
    pub lines: usize,
}

/// Groups lightweight result identities; source contents never cross this boundary again.
pub fn search_presentation(
    request: SearchPresentationRequest,
) -> Result<SearchPresentation, CoreError> {
    if request.paths.len() > 1_000_000 || request.paths.iter().any(|path| path.len() > 32_768) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Search presentation exceeds the window budget",
        ));
    }
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, path) in request.paths.iter().enumerate() {
        groups.entry(path.clone()).or_default().push(index);
    }
    let files = groups.len();
    let mut rows = Vec::with_capacity(request.paths.len() + files);
    for (path, indices) in groups {
        rows.push(SearchPresentationRow {
            path: path.clone(),
            count: indices.len(),
            match_index: None,
        });
        if !request.collapsed.contains(&path) {
            rows.extend(indices.into_iter().map(|index| SearchPresentationRow {
                path: path.clone(),
                count: 0,
                match_index: Some(index),
            }));
        }
    }
    Ok(SearchPresentation {
        rows,
        files,
        lines: request.paths.len(),
    })
}
