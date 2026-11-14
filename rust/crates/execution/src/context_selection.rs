//! Chooses an existing configuration for a provider-supplied entry identity.

use serde::Deserialize;

/// One already-resolved configuration; Core performs no provider-specific discovery.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextCandidate {
    /// Stable configuration identity returned to the caller.
    id: String,
    /// Provider-supplied source identity, when this configuration has one.
    source_path: Option<String>,
    /// Provider-supplied entry symbol, when this configuration has one.
    symbol: Option<String>,
    /// Whether the configuration comes from generated detection.
    generated: bool,
    /// Disabled entries must never be selected for a launch.
    disabled: bool,
}

/// Identity and ordered candidates for a context launch or editor action.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSelectionRequest {
    /// Source identity supplied by the platform's provider.
    source_path: String,
    /// Entry symbol supplied by the platform's provider.
    symbol: String,
    /// Current selection, if any; it wins over an otherwise equal match.
    preferred_id: Option<String>,
    /// Windows path identities compare case-insensitively with either separator.
    case_insensitive_paths: bool,
    /// Effective configurations in their existing display order.
    candidates: Vec<ContextCandidate>,
}

/// Returns the matching preferred or edited entry, preserving input order on ties.
pub fn select_context_configuration(request: ContextSelectionRequest) -> Option<String> {
    let path_key = |path: &str| {
        if request.case_insensitive_paths {
            path.replace('\\', "/").to_lowercase()
        } else {
            path.to_owned()
        }
    };
    let source_key = path_key(&request.source_path);
    request
        .candidates
        .iter()
        .filter(|candidate| {
            !candidate.disabled
                && candidate.symbol.as_deref() == Some(request.symbol.as_str())
                && candidate.source_path.as_deref().map(&path_key).as_deref()
                    == Some(source_key.as_str())
        })
        .min_by_key(|candidate| {
            let preferred = request.preferred_id.as_deref() == Some(candidate.id.as_str());
            (!preferred as u8 * 2) + candidate.generated as u8
        })
        .map(|candidate| candidate.id.clone())
}
