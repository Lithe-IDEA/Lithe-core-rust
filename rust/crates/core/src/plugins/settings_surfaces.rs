//! Generic settings page catalog, navigation and committed-change routing.
//! Native adapters supply installed identities and directory equality facts;
//! these commands never discover files, subscribe windows or launch plugins.

use super::{validate_manifest_value, SettingsOwner};
use crate::protocol::CoreError;
use lithe_plugin_model::{decode_payload, outcome, PluginRejection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Limits catalog transfer and the number of native package reads per request.
const MAX_PLUGINS: usize = 64;
/// Bounds both dynamic surfaces and the complete navigation projection.
const MAX_PAGES: usize = 512;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Contribution {
    manifest: Value,
    owner: SettingsOwner,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogRequest {
    plugins: Vec<Contribution>,
}

fn rejected(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.settings.invalidSurface",
        field,
        "Expected bounded, unique settings surface facts.",
    )
}

/// Builds a deterministic catalog from complete validated installed manifests.
/// Disabled packages cannot contribute pages or rename another plugin's page.
pub fn settings_catalog_command(payload: Value) -> Result<Value, CoreError> {
    let request: CatalogRequest = decode_payload(payload, "plugins.settings.catalog")?;
    Ok(outcome((|| {
        if request.plugins.len() > MAX_PLUGINS {
            return Err(rejected("plugins"));
        }
        let mut owners = BTreeSet::new();
        let mut manifests = BTreeMap::new();
        for entry in request.plugins {
            let manifest = validate_manifest_value(entry.manifest.clone())?;
            if manifest.id != entry.owner.plugin_id || manifest.version != entry.owner.version {
                return Err(PluginRejection::at(
                    "plugin.settings.ownerChanged",
                    "owner",
                    "Installed package identity does not match its manifest.",
                ));
            }
            if !owners.insert(manifest.id.clone()) {
                return Err(rejected("plugins"));
            }
            if entry.enabled {
                manifests.insert(manifest.id.clone(), (manifest, entry.manifest));
            }
        }
        let mut titles = BTreeMap::new();
        // Plugin ID order resolves competing display names deterministically.
        for (_, raw) in manifests.values() {
            for (field, kind) in [("languages", "language"), ("buildSystems", "buildSystem")] {
                if let Some(entries) = raw[field].as_array() {
                    for entry in entries {
                        if let (Some(id), Some(title)) =
                            (entry["id"].as_str(), entry["displayName"].as_str())
                        {
                            titles
                                .entry(format!("{kind}:{id}"))
                                .or_insert_with(|| title.to_owned());
                        }
                    }
                }
            }
        }
        let mut pages = BTreeMap::<String, Value>::new();
        for (id, (manifest, _)) in manifests {
            for page in lithe_plugin_model::settings::page_sections(
                &manifest.settings,
                &manifest.setting_actions,
            ) {
                let key = page["key"].as_str().unwrap_or_default().to_owned();
                let target = pages.entry(key.clone()).or_insert_with(|| json!({
                    "key":key,"kind":page["kind"],"id":page["id"],
                    "title":titles.get(&key).cloned().unwrap_or_else(|| page["id"].as_str().unwrap_or_default().to_owned()),"contributors":[]
                }));
                target["contributors"].as_array_mut().expect("catalog array").push(json!({
                    "pluginID":id,"version":manifest.version,"title":manifest.display_name,"sections":page["sections"]
                }));
            }
        }
        if pages.len() > MAX_PAGES {
            return Err(rejected("pages"));
        }
        Ok(("pages", json!(pages.into_values().collect::<Vec<_>>())))
    })()))
}

/// Native static page labels and stable identifiers. Hierarchy is supplied once;
/// search, expansion and ancestor retention are shared projection rules.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NavigationItem {
    id: String,
    title: String,
    parent: Option<String>,
    group: bool,
    expanded: bool,
    #[serde(default)]
    keywords: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigationRequest {
    items: Vec<NavigationItem>,
    query: String,
}

/// Projects a bounded two-level settings tree; matched descendants retain their
/// parent and a matched parent shows its children, including dynamic surfaces.
pub fn settings_navigation_command(payload: Value) -> Result<Value, CoreError> {
    let request: NavigationRequest = decode_payload(payload, "plugins.settings.navigation")?;
    Ok(outcome((|| {
        if request.items.len() > MAX_PAGES + 64 || request.query.len() > 1024 {
            return Err(rejected("navigation"));
        }
        let query = request.query.trim().to_lowercase();
        let mut ids = BTreeSet::new();
        for item in &request.items {
            if item.id.is_empty()
                || item.id.len() > 256
                || item.title.len() > 1024
                || item.keywords.len() > 16384
                || !ids.insert(&item.id)
            {
                return Err(rejected("items"));
            }
        }
        for item in &request.items {
            if let Some(parent) = &item.parent {
                if !request
                    .items
                    .iter()
                    .any(|candidate| &candidate.id == parent && candidate.parent.is_none())
                {
                    return Err(rejected("parent"));
                }
            }
        }
        let matches = |item: &NavigationItem| {
            item.title.to_lowercase().contains(&query)
                || item.keywords.to_lowercase().contains(&query)
        };
        let row = |item: &NavigationItem, level: usize| json!({"id":item.id,"title":item.title,"group":item.group,"expanded":!query.is_empty() || item.expanded,"level":level});
        let mut result = Vec::new();
        for parent in request.items.iter().filter(|item| item.parent.is_none()) {
            let children: Vec<_> = request
                .items
                .iter()
                .filter(|item| item.parent.as_ref() == Some(&parent.id))
                .collect();
            let own = matches(parent);
            if !own && !children.iter().any(|child| matches(child)) {
                continue;
            }
            result.push(row(parent, 0));
            if !parent.group || parent.expanded || !query.is_empty() {
                result.extend(
                    children
                        .into_iter()
                        .filter(|child| own || matches(child))
                        .map(|child| row(child, 1)),
                );
            }
        }
        Ok(("rows", json!(result)))
    })()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeliveryRequest {
    scope: String,
    same_preferences: bool,
    same_workspace: bool,
    same_version: bool,
    enabled: bool,
    effects: Vec<String>,
}

/// Routes only committed changes to matching native owners. Global delivery
/// deliberately carries no values: each receiver expands its own project layer.
pub fn settings_delivery_command(payload: Value) -> Result<Value, CoreError> {
    let request: DeliveryRequest = decode_payload(payload, "plugins.settings.delivery")?;
    Ok(outcome((|| {
        if !matches!(request.scope.as_str(), "global" | "project")
            || request.effects.len() > 4
            || request.effects.iter().any(|effect| {
                !matches!(
                    effect.as_str(),
                    "restart" | "reload" | "immediate" | "nextRun"
                )
            })
        {
            return Err(rejected("delivery"));
        }
        let refresh = request.same_preferences
            && request.same_version
            && (request.scope == "global" || request.same_workspace);
        let effects: BTreeSet<_> = if refresh && request.enabled {
            request.effects.into_iter().collect()
        } else {
            BTreeSet::new()
        };
        Ok(("delivery", json!({"refresh":refresh,"effects":effects})))
    })()))
}
