//! Read-only function routing within the requesting workspace and native owner.
//! Runtime registration remains authoritative; native facts only restrict which
//! locally owned clients, and installed versions, may receive the result.

use super::engine;
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use url::Url;

const MAX_CANDIDATES: usize = 64;
const MAX_ID_BYTES: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    #[serde(rename = "sessionID")]
    session_id: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    version: String,
    function: String,
    /// Canonical file URI supplied by the native workspace owner.
    root_uri: String,
    candidates: Vec<Candidate>,
}

// Only the engine constructs these facts in production. Tests supply inert
// snapshots to cover selection independently of process scheduling.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeFact {
    #[serde(rename = "sessionID")]
    session_id: String,
    #[serde(rename = "pluginID")]
    plugin_id: String,
    root_uri: String,
    serves: bool,
}

fn invalid() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid function-provider selection request",
    )
}

fn root_identity(uri: &str) -> Result<String, CoreError> {
    if uri.len() > lithe_document_uri::MAX_PATH_BYTES || uri.contains('\0') {
        return Err(invalid());
    }
    let parsed = Url::parse(uri).map_err(|_| invalid())?;
    if parsed.scheme() != "file"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return Err(invalid());
    }
    // Both inputs originate in lsp.documentUri at session creation and workspace
    // selection. This does not resolve filesystem aliases or follow symlinks.
    Ok(parsed.to_string().trim_end_matches('/').to_owned())
}

fn candidates(request: &Request) -> Result<BTreeMap<&str, &str>, CoreError> {
    if request.candidates.len() > MAX_CANDIDATES {
        return Err(invalid());
    }
    for value in [&request.plugin_id, &request.version, &request.function] {
        if value.is_empty() || value.len() > MAX_ID_BYTES || value.contains('\0') {
            return Err(invalid());
        }
    }
    root_identity(&request.root_uri)?;
    let mut result = BTreeMap::new();
    for candidate in &request.candidates {
        if candidate.session_id.is_empty()
            || candidate.session_id.len() > MAX_ID_BYTES
            || candidate.version.len() > MAX_ID_BYTES
            || result
                .insert(candidate.session_id.as_str(), candidate.version.as_str())
                .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(result)
}

fn select(request: &Request, facts: &[RuntimeFact]) -> Result<Option<String>, CoreError> {
    let candidates = candidates(request)?;
    let root = root_identity(&request.root_uri)?;
    for (id, version) in candidates {
        if version != request.version {
            continue;
        }
        if facts.iter().any(|fact| {
            fact.session_id == id
                && fact.plugin_id == request.plugin_id
                && fact.serves
                && root_identity(&fact.root_uri).ok().as_deref() == Some(root.as_str())
        }) {
            return Ok(Some(id.to_owned()));
        }
    }
    Ok(None)
}

/// Selects only an already running, registered provider among native-owned
/// clients. It never starts a process, consumes events, or crosses workspaces.
pub fn function_provider_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|_| invalid())?;
    candidates(&request)?;
    let sessions = {
        let registry = engine()
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        request
            .candidates
            .iter()
            .filter_map(|candidate| registry.get(&candidate.session_id).cloned())
            .collect::<Vec<_>>()
    };
    // Release the registry lock before inspecting any session's protocol state.
    let facts = sessions
        .iter()
        .map(|session| RuntimeFact {
            session_id: session.id.clone(),
            plugin_id: session.plugin_id.clone().unwrap_or_default(),
            root_uri: session.root_uri.clone(),
            serves: session.serves_function(&request.function),
        })
        .collect::<Vec<_>>();
    Ok(json!({"sessionID":select(&request,&facts)?}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn function_provider_selection_matches_shared_workspace_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/plugins/settings-function-provider.json"
        )))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let request: Request = serde_json::from_value(case["request"].clone()).unwrap();
            let facts: Vec<RuntimeFact> = serde_json::from_value(case["facts"].clone()).unwrap();
            let result = select(&request, &facts);
            if case["invalid"] == true {
                assert!(result.is_err(), "{case}")
            } else {
                assert_eq!(
                    serde_json::to_value(result.unwrap()).unwrap(),
                    case["sessionID"],
                    "{case}"
                )
            }
        }
    }
}
