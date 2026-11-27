//! The agents the installed plugins declare, with what the user granted them.
//!
//! The host sends every installed plugin's manifest and granted permissions
//! with `agents.configure`, again whenever a plugin is installed, updated,
//! removed or its grants change. Core validates each manifest itself, so a
//! session only ever runs what a valid manifest declares.

use lithe_plugin_agents::{
    catalog_entry, AgentDeclaration, PERMISSION_AGENTS_RUN, PERMISSION_AGENTS_SESSIONS,
};
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{PoisonError, RwLock};

/// Most plugins one `agents.configure` carries.
const MAX_PLUGINS: usize = 1024;

/// One plugin's agents and grants.
#[derive(Debug, Clone)]
pub(crate) struct PluginAgents {
    pub agents: Vec<AgentDeclaration>,
    pub granted: BTreeSet<String>,
}

impl PluginAgents {
    pub fn may_run(&self) -> bool {
        self.granted.contains(PERMISSION_AGENTS_RUN)
    }

    pub fn may_read_sessions(&self) -> bool {
        self.granted.contains(PERMISSION_AGENTS_SESSIONS)
    }
}

/// An agent found in the catalog.
#[derive(Debug, Clone)]
pub(crate) struct CatalogAgent {
    pub plugin_id: String,
    pub agent: AgentDeclaration,
    pub may_run: bool,
    pub may_read_sessions: bool,
}

impl CatalogAgent {
    /// The key of the agent in the session records: unique across plugins.
    pub fn record_key(&self) -> String {
        record_key(&self.plugin_id, &self.agent.id)
    }
}

/// The key of an agent in the session records.
pub(crate) fn record_key(plugin_id: &str, agent_id: &str) -> String {
    format!("{plugin_id}/{agent_id}")
}

/// Splits a record key into plugin and agent IDs.
pub(crate) fn split_record_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('/')
}

#[derive(Default)]
pub(crate) struct Catalog {
    plugins: RwLock<BTreeMap<String, PluginAgents>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigureRequest {
    plugins: Vec<ConfiguredPlugin>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfiguredPlugin {
    manifest: Value,
    #[serde(default)]
    granted_permissions: Vec<String>,
}

impl Catalog {
    /// Replaces the catalog with the agents of `payload`'s plugins. Plugins
    /// whose manifest is invalid are listed as rejected and left out.
    pub fn configure(&self, payload: Value) -> Result<Value, CoreError> {
        let request: ConfigureRequest = crate::decode(payload)?;
        if request.plugins.len() > MAX_PLUGINS {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Too many plugins in one agents.configure.",
            )
            .with_details("agents.tooManyPlugins"));
        }
        let mut plugins = BTreeMap::new();
        let mut rejected = Vec::new();
        for plugin in request.plugins {
            let plugin_id = plugin
                .manifest
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let manifest = match lithe_plugin_manifest::validate_manifest_value(plugin.manifest) {
                Ok(manifest) => manifest,
                Err(rejection) => {
                    rejected.push(json!({
                        "pluginID": plugin_id,
                        "code": rejection.code,
                        "message": rejection.message,
                    }));
                    continue;
                }
            };
            if manifest.agents().is_empty() {
                continue;
            }
            let declared: BTreeSet<&str> =
                manifest.permissions().iter().map(String::as_str).collect();
            let granted = plugin
                .granted_permissions
                .into_iter()
                .filter(|permission| declared.contains(permission.as_str()))
                .collect();
            plugins.insert(
                manifest.id().to_string(),
                PluginAgents {
                    agents: manifest.agents().to_vec(),
                    granted,
                },
            );
        }
        *self.plugins.write().unwrap_or_else(PoisonError::into_inner) = plugins;
        Ok(json!({ "agents": self.entries(), "rejected": rejected }))
    }

    /// Every declared agent as the host shows it.
    pub fn list(&self) -> Value {
        json!({ "agents": self.entries() })
    }

    fn entries(&self) -> Vec<Value> {
        let plugins = self.plugins.read().unwrap_or_else(PoisonError::into_inner);
        plugins
            .iter()
            .flat_map(|(plugin_id, plugin)| {
                plugin.agents.iter().map(move |agent| {
                    let mut entry = catalog_entry(plugin_id, agent);
                    entry["mayRun"] = json!(plugin.may_run());
                    entry["mayReadSessions"] = json!(plugin.may_read_sessions());
                    entry
                })
            })
            .collect()
    }

    /// The agent `agent_id` of `plugin_id`.
    pub fn find(&self, plugin_id: &str, agent_id: &str) -> Option<CatalogAgent> {
        let plugins = self.plugins.read().unwrap_or_else(PoisonError::into_inner);
        let plugin = plugins.get(plugin_id)?;
        let agent = plugin.agents.iter().find(|agent| agent.id == agent_id)?;
        Some(CatalogAgent {
            plugin_id: plugin_id.to_string(),
            agent: agent.clone(),
            may_run: plugin.may_run(),
            may_read_sessions: plugin.may_read_sessions(),
        })
    }

    /// The agent a record key names.
    pub fn find_key(&self, key: &str) -> Option<CatalogAgent> {
        let (plugin_id, agent_id) = split_record_key(key)?;
        self.find(plugin_id, agent_id)
    }

    /// Every agent whose session records the user lets Lithe read.
    pub fn readable(&self) -> Vec<CatalogAgent> {
        let plugins = self.plugins.read().unwrap_or_else(PoisonError::into_inner);
        plugins
            .iter()
            .filter(|(_, plugin)| plugin.may_read_sessions())
            .flat_map(|(plugin_id, plugin)| {
                plugin.agents.iter().map(move |agent| CatalogAgent {
                    plugin_id: plugin_id.clone(),
                    agent: agent.clone(),
                    may_run: plugin.may_run(),
                    may_read_sessions: true,
                })
            })
            .filter(|agent| !agent.agent.session_sources.is_empty())
            .collect()
    }
}
