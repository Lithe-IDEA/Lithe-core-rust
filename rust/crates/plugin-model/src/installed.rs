//! Selection of native installed-version facts without filesystem access.
//!
//! Core reuses its existing compatibility version comparator. The host keeps
//! paths and maps the returned indices back to the native records it supplied.

use crate::version::PluginVersion;
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Bound one metadata inventory and every returned selection/diagnostic list.
const MAX_INSTALLED_ENTRIES: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstalledFact {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    version: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectionRequest {
    entries: Vec<InstalledFact>,
    #[serde(default)]
    include_disabled: bool,
}

/// Selects the numeric newest version per opaque native plugin identity.
///
/// Invalid and duplicate records are returned as indexed diagnostics while
/// healthy plugins remain available. Enablement filters the selected version,
/// never reviving an older enabled record behind a disabled newest one.
pub fn select_command(payload: Value) -> Result<Value, CoreError> {
    let request: SelectionRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid installed plugin facts.")
            .with_details(error.to_string())
    })?;
    if request.entries.len() > MAX_INSTALLED_ENTRIES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Installed plugin inventory exceeds its limit.",
        ));
    }
    let mut newest: BTreeMap<&str, (usize, PluginVersion, bool)> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut rejected = Vec::new();
    for (index, entry) in request.entries.iter().enumerate() {
        if entry.plugin_id.is_empty()
            || entry.plugin_id.len() > 256
            || entry.plugin_id.trim() != entry.plugin_id
        {
            rejected.push(json!({"index":index,"code":"plugin.installed.invalidIdentity"}));
            continue;
        }
        let Some(version) = crate::is_semantic_version(&entry.version)
            .then(|| PluginVersion::parse(&entry.version))
            .flatten()
        else {
            rejected.push(json!({"index":index,"code":"plugin.installed.invalidVersion"}));
            continue;
        };
        if !seen.insert((&entry.plugin_id, &entry.version)) {
            rejected.push(json!({"index":index,"code":"plugin.installed.duplicateVersion"}));
            continue;
        }
        let selected = newest
            .entry(&entry.plugin_id)
            .or_insert((index, version, entry.enabled));
        if version > selected.1 {
            *selected = (index, version, entry.enabled);
        }
    }
    let selected: Vec<_> = newest
        .into_values()
        .filter(|(_, _, enabled)| request.include_disabled || *enabled)
        .map(|(index, _, _)| index)
        .collect();
    Ok(json!({"selected":selected,"rejected":rejected,"maximumEntries":MAX_INSTALLED_ENTRIES}))
}
