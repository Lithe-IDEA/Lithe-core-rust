//! Generic settings page catalog, navigation and committed-change routing.
//! Native adapters supply installed identities and directory equality facts;
//! these commands never discover files, subscribe windows or launch plugins.

use crate::{validate_manifest_value, SettingsOwner};
use lithe_plugin_model::{decode_payload, outcome, PluginRejection};
use lithe_protocol::CoreError;
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
                None,
            ) {
                let key = page["key"].as_str().unwrap_or_default().to_owned();
                let target = pages.entry(key.clone()).or_insert_with(|| json!({
                    "key":key,"kind":page["kind"],"id":page["id"],
                    "title":titles.get(&key).or_else(|| titles.get(&format!("language:{}",page["id"].as_str().unwrap_or_default()))).or_else(|| titles.get(&format!("buildSystem:{}",page["id"].as_str().unwrap_or_default()))).cloned().unwrap_or_else(|| page["id"].as_str().unwrap_or_default().to_owned()),"contributors":[]
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
/// search, expansion, folding and ancestor retention are shared projection rules.
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
    /// Shows at most this many children outside search; the rest fold into one
    /// overflow row that opens the group's own directory page.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigationRequest {
    items: Vec<NavigationItem>,
    query: String,
}

/// The deepest level a row may sit at: group, kind fold, page.
const MAX_LEVEL: usize = 2;

struct Navigation<'a> {
    items: &'a [NavigationItem],
    query: String,
    rows: Vec<Value>,
}

impl<'a> Navigation<'a> {
    fn matches(&self, item: &NavigationItem) -> bool {
        !self.query.is_empty()
            && (item.title.to_lowercase().contains(&self.query)
                || item.keywords.to_lowercase().contains(&self.query))
    }

    fn children(&self, id: &str) -> Vec<&'a NavigationItem> {
        self.items
            .iter()
            .filter(|item| item.parent.as_deref() == Some(id))
            .collect()
    }

    /// Whether the item, or any item below it, matches the search.
    fn hit(&self, item: &NavigationItem) -> bool {
        self.matches(item) || self.children(&item.id).iter().any(|child| self.hit(child))
    }

    fn emit(&mut self, item: &'a NavigationItem, level: usize, path: &[&'a str], shown: bool) {
        let searching = !self.query.is_empty();
        let own = self.matches(item);
        // A matched item shows everything below it; otherwise only matching branches.
        if searching && !shown && !self.hit(item) {
            return;
        }
        self.rows.push(json!({
            "id":item.id,"title":item.title,"group":item.group,
            "expanded":searching || item.expanded,"level":level,
            "matched":own,"path":path,
        }));
        if item.group && !item.expanded && !searching {
            return;
        }
        let children = self.children(&item.id);
        let limit = match item.limit {
            Some(limit) if !searching => limit,
            _ => children.len(),
        };
        let mut below = path.to_vec();
        below.push(&item.title);
        for child in children.iter().take(limit) {
            self.emit(child, level + 1, &below, shown || own);
        }
        if children.len() > limit {
            self.rows.push(json!({
                "id":item.id,"title":item.title,"group":false,"expanded":false,
                "level":level + 1,"matched":false,"path":below,"overflow":children.len(),
            }));
        }
    }
}

/// Projects a bounded settings tree of up to three levels: a matched item keeps its
/// ancestors and shows its descendants, including dynamic surfaces. Each row carries
/// `matched` and the ancestor `path`, so a host can list search hits flat with their
/// location.
pub fn settings_navigation_command(payload: Value) -> Result<Value, CoreError> {
    let request: NavigationRequest = decode_payload(payload, "plugins.settings.navigation")?;
    Ok(outcome((|| {
        if request.items.len() > MAX_PAGES + 64 || request.query.len() > 1024 {
            return Err(rejected("navigation"));
        }
        let mut ids = BTreeSet::new();
        for item in &request.items {
            if item.id.is_empty()
                || item.id.len() > 256
                || item.title.len() > 1024
                || item.keywords.len() > 16384
                || item
                    .limit
                    .is_some_and(|limit| limit == 0 || limit > MAX_PAGES)
                || !ids.insert(&item.id)
            {
                return Err(rejected("items"));
            }
        }
        let parent_of = |id: &str| {
            request
                .items
                .iter()
                .find(|candidate| candidate.id == id)
                .map(|candidate| candidate.parent.as_deref())
        };
        for item in &request.items {
            // Walking up must reach a root within MAX_LEVEL steps; this also rules out cycles.
            let mut current = item.parent.as_deref();
            let mut depth = 0;
            while let Some(parent) = current {
                depth += 1;
                current = match parent_of(parent) {
                    Some(next) if depth <= MAX_LEVEL => next,
                    _ => return Err(rejected("parent")),
                };
            }
        }
        let mut navigation = Navigation {
            items: &request.items,
            query: request.query.trim().to_lowercase(),
            rows: Vec::new(),
        };
        for root in request.items.iter().filter(|item| item.parent.is_none()) {
            navigation.emit(root, 0, &[], false);
        }
        Ok(("rows", json!(navigation.rows)))
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
