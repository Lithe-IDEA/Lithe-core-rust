//! Selects language providers from declared file selectors and actual workspace names.

use crate::files::{existing_root, SearchRequest, VisibilityRules};
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;

/// A host-read language declaration; Core owns its matching against workspace paths.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLanguageSource {
    /// Opaque language identifier returned unchanged to the provider adapter.
    pub language_id: String,
    /// Case-insensitive filename suffixes, including their leading dot.
    #[serde(default)]
    pub file_extensions: Vec<String>,
    /// Case-insensitive exact base names, including project markers.
    #[serde(default)]
    pub file_names: Vec<String>,
}

/// Finds relevant providers without opening documents or returning the file inventory.
pub fn search_languages(
    request: SearchRequest,
    sources: Vec<SearchLanguageSource>,
) -> Result<serde_json::Value, CoreError> {
    if sources.len() > 1024
        || sources.iter().any(|source| {
            source.language_id.len() > 256
                || source.file_extensions.len() + source.file_names.len() > 1024
                || source
                    .file_extensions
                    .iter()
                    .chain(&source.file_names)
                    .any(|selector| selector.len() > 1024)
        })
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many search language sources",
        ));
    }
    let root = existing_root(&request.root)?;
    let rules = VisibilityRules::new(request.hidden_directory_names, request.hidden_file_patterns)
        .with_workspace_roots(&root, &request.workspace_roots);
    let index = crate::search_index::get_names(&root, &rules)?;
    let index = index
        .read()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))?;
    let mut remaining: Vec<_> = sources.into_iter().collect();
    let mut languages = std::collections::BTreeSet::new();
    for id in index.all_file_ids() {
        lithe_protocol::cancellation::check()?;
        let Some(file) = index.file(id) else { continue };
        let name = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&file.path)
            .to_lowercase();
        remaining.retain(|source| {
            let matched = source
                .file_names
                .iter()
                .any(|file| file.to_lowercase() == name)
                || source.file_extensions.iter().any(|extension| {
                    let extension = extension.trim_start_matches('.').to_lowercase();
                    !extension.is_empty() && name.ends_with(&format!(".{extension}"))
                });
            if matched {
                languages.insert(source.language_id.clone());
            }
            !matched
        });
        if remaining.is_empty() {
            break;
        }
    }
    Ok(serde_json::json!({"languages":languages}))
}
