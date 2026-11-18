//! Machine-wide SDK registrations and defaults, independent of project pins.
//!
//! Native adapters validate installations, preserve source identity and publish
//! edits atomically. This module owns bounds, path identity and document merges.

use super::paths;
use lithe_plugin_model::{is_tool_id, PluginRejection, Validation};
use lithe_protocol::json_document::parse_unique_json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_BYTES: usize = 256 * 1024;
const MAX_TRUSTED_WORKSPACES: usize = 256;

#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    homes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_home: Option<String>,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    version: u32,
    tools: BTreeMap<String, Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trusted_workspaces: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    document: Option<String>,
    platform: String,
    #[serde(default)]
    filesystem_case_sensitive: Option<bool>,
    #[serde(default)]
    workspace_root: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Edit {
    tool: String,
    homes: Vec<String>,
    default_home: Option<String>,
}

/// Explicit machine-local trust mutation for executing tool environments inside one workspace.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceTrustEdit {
    workspace: String,
    trusted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditRequest {
    document: Option<String>,
    platform: String,
    #[serde(default)]
    filesystem_case_sensitive: Option<bool>,
    #[serde(default)]
    edits: Vec<Edit>,
    #[serde(default)]
    workspace_trust: Option<WorkspaceTrustEdit>,
}

fn invalid(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.toolchain.invalidDocument",
        field,
        "Expected bounded global toolchain registrations with unique native homes.",
    )
}

impl Request {
    fn case_sensitive(&self) -> Validation<bool> {
        if !matches!(self.platform.as_str(), "macos" | "windows" | "linux") {
            return Err(invalid("platform"));
        }
        Ok(self
            .filesystem_case_sensitive
            .unwrap_or(self.platform != "windows"))
    }

    fn entry(&self, homes: &[String], default: &Option<String>) -> Validation<Entry> {
        let case_sensitive = self.case_sensitive()?;
        if homes.len() > 32 {
            return Err(invalid("homes"));
        }
        let mut identities = BTreeSet::new();
        let mut normalized = Vec::new();
        for home in homes {
            let path =
                paths::normalize(home, &self.platform, true).ok_or_else(|| invalid("homes"))?;
            let key = paths::normalize(&path, &self.platform, case_sensitive)
                .ok_or_else(|| invalid("homes"))?;
            if !identities.insert(key) {
                return Err(invalid("homes"));
            }
            normalized.push(path);
        }
        let default_home = default
            .as_ref()
            .map(|home| {
                paths::normalize(home, &self.platform, true).ok_or_else(|| invalid("defaultHome"))
            })
            .transpose()?;
        if let Some(default) = &default_home {
            let key = paths::normalize(default, &self.platform, case_sensitive)
                .ok_or_else(|| invalid("defaultHome"))?;
            if identities.insert(key) {
                normalized.push(default.clone());
            }
        }
        if normalized.len() > 32 {
            return Err(invalid("homes"));
        }
        Ok(Entry {
            homes: normalized,
            default_home,
        })
    }

    fn trusted_workspaces(&self, workspaces: &[String]) -> Validation<Vec<String>> {
        let case_sensitive = self.case_sensitive()?;
        if workspaces.len() > MAX_TRUSTED_WORKSPACES {
            return Err(invalid("trustedWorkspaces"));
        }
        let mut by_key = BTreeMap::new();
        for workspace in workspaces {
            let path = paths::normalize(workspace, &self.platform, true)
                .ok_or_else(|| invalid("trustedWorkspaces"))?;
            let key = paths::normalize(&path, &self.platform, case_sensitive)
                .ok_or_else(|| invalid("trustedWorkspaces"))?;
            if by_key.insert(key, path).is_some() {
                return Err(invalid("trustedWorkspaces"));
            }
        }
        Ok(by_key.into_values().collect())
    }

    fn read(&self) -> Validation<Catalog> {
        self.case_sensitive()?;
        let mut catalog = if let Some(text) = &self.document {
            if text.len() > MAX_BYTES {
                return Err(invalid("document"));
            }
            serde_json::from_value(parse_unique_json(text).map_err(|_| invalid("document"))?)
                .map_err(|_| invalid("document"))?
        } else {
            Catalog {
                version: 1,
                tools: BTreeMap::new(),
                trusted_workspaces: Vec::new(),
            }
        };
        if catalog.version != 1 || catalog.tools.len() > 64 {
            return Err(invalid("document"));
        }
        for (tool, entry) in &mut catalog.tools {
            if !is_tool_id(tool) {
                return Err(invalid("tool"));
            }
            *entry = self.entry(&entry.homes, &entry.default_home)?;
        }
        catalog.trusted_workspaces = self.trusted_workspaces(&catalog.trusted_workspaces)?;
        Ok(catalog)
    }

    fn workspace_trusted(&self, catalog: &Catalog, workspace: &str) -> Validation<bool> {
        if workspace.is_empty() {
            return Ok(false);
        }
        let case_sensitive = self.case_sensitive()?;
        let key = paths::normalize(workspace, &self.platform, case_sensitive)
            .ok_or_else(|| invalid("workspaceRoot"))?;
        Ok(catalog.trusted_workspaces.iter().any(|entry| {
            paths::normalize(entry, &self.platform, case_sensitive).as_ref() == Some(&key)
        }))
    }
}

/// Returns the complete validated catalog and the native read plan.
pub(crate) fn state(input: &Value) -> Validation<Value> {
    let request: Request = serde_json::from_value(input.clone()).map_err(|_| invalid("input"))?;
    let catalog = request.read()?;
    let project_code_allowed = request
        .workspace_root
        .as_deref()
        .map(|root| request.workspace_trusted(&catalog, root))
        .transpose()?;
    let mut response = json!({
        "document": catalog,
        "storage": {
            "relativePath": "toolchains/global.json",
            "maximumBytes": MAX_BYTES
        }
    });
    if let Some(allowed) = project_code_allowed {
        response["projectCodeAllowed"] = json!(allowed);
    }
    Ok(response)
}

/// Merges confirmed tool edits and explicit machine-local workspace trust, preserving dormant registrations.
pub(crate) fn edit(input: &Value) -> Validation<Value> {
    let request: EditRequest =
        serde_json::from_value(input.clone()).map_err(|_| invalid("input"))?;
    if (request.edits.is_empty() && request.workspace_trust.is_none()) || request.edits.len() > 64 {
        return Err(invalid("edits"));
    }
    let source = Request {
        document: request.document,
        platform: request.platform,
        filesystem_case_sensitive: request.filesystem_case_sensitive,
        workspace_root: None,
    };
    let case_sensitive = source.case_sensitive()?;
    let mut catalog = source.read()?;
    let original = catalog.clone();
    let mut tools = BTreeSet::new();
    for edit in request.edits {
        if !is_tool_id(&edit.tool) || !tools.insert(edit.tool.clone()) {
            return Err(invalid("tool"));
        }
        let entry = source.entry(&edit.homes, &edit.default_home)?;
        if entry.homes.is_empty() && entry.default_home.is_none() {
            catalog.tools.remove(&edit.tool);
        } else {
            catalog.tools.insert(edit.tool, entry);
        }
    }
    if catalog.tools.len() > 64 {
        return Err(invalid("tools"));
    }
    if let Some(trust) = request.workspace_trust {
        let normalized = paths::normalize(&trust.workspace, &source.platform, true)
            .ok_or_else(|| invalid("workspaceTrust.workspace"))?;
        let key = paths::normalize(&normalized, &source.platform, case_sensitive)
            .ok_or_else(|| invalid("workspaceTrust.workspace"))?;
        let mut by_key: BTreeMap<String, String> = catalog
            .trusted_workspaces
            .iter()
            .filter_map(|entry| {
                Some((
                    paths::normalize(entry, &source.platform, case_sensitive)?,
                    entry.clone(),
                ))
            })
            .collect();
        if trust.trusted {
            by_key.entry(key).or_insert(normalized);
            if by_key.len() > MAX_TRUSTED_WORKSPACES {
                return Err(invalid("trustedWorkspaces"));
            }
        } else {
            by_key.remove(&key);
        }
        catalog.trusted_workspaces = by_key.into_values().collect();
    }
    let changed = catalog != original;
    let document = if changed {
        let text = serde_json::to_string_pretty(&catalog).map_err(|_| invalid("document"))? + "\n";
        if text.len() > MAX_BYTES {
            return Err(invalid("document"));
        }
        Some(text)
    } else {
        None
    };
    Ok(json!({"changed":changed,"document":document,"catalog":catalog}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrations_and_defaults_are_independent_of_projects() {
        let initial = json!({"document":null,"platform":"macos"});
        assert_eq!(state(&initial).unwrap()["document"]["tools"], json!({}));
        let result = edit(&json!({"document":null,"platform":"macos","edits":[{"tool":"vm","homes":["/sdk/8","/sdk/17"],"defaultHome":"/sdk/17"}]})).unwrap();
        assert_eq!(
            result["catalog"]["tools"]["vm"]["homes"],
            json!(["/sdk/8", "/sdk/17"])
        );
        let reset = edit(&json!({"document":result["document"],"platform":"macos","edits":[{"tool":"vm","homes":["/sdk/8","/sdk/17"],"defaultHome":null}]})).unwrap();
        assert!(reset["catalog"]["tools"]["vm"].get("defaultHome").is_none());
        assert_eq!(
            reset["catalog"]["tools"]["vm"]["homes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn unchanged_automatic_state_creates_no_document_and_preserves_other_tools() {
        let result = edit(&json!({"document":null,"platform":"linux","edits":[{"tool":"vm","homes":[],"defaultHome":null}]})).unwrap();
        assert_eq!(result["changed"], false);
        assert_eq!(result["document"], Value::Null);
        let result = edit(&json!({"document":"{\"version\":1,\"tools\":{\"other\":{\"homes\":[\"/sdk/other\"]}}}","platform":"linux","edits":[{"tool":"vm","homes":[],"defaultHome":"/sdk/chosen"}]})).unwrap();
        assert_eq!(
            result["catalog"]["tools"]["other"]["homes"],
            json!(["/sdk/other"])
        );
        assert_eq!(
            result["catalog"]["tools"]["vm"]["homes"],
            json!(["/sdk/chosen"])
        );
    }

    #[test]
    fn windows_aliases_unknown_fields_and_duplicate_keys_are_rejected() {
        for document in [
            r#"{"version":1,"version":1,"tools":{}}"#,
            r#"{"version":1,"tools":{},"private":true}"#,
            r#"{"version":1,"tools":{"vm":{"homes":["C:/SDK","c:\\sdk"]}}}"#,
        ] {
            assert!(state(&json!({"document":document,"platform":"windows"})).is_err());
        }
        assert!(edit(&json!({"document":null,"platform":"macos","edits":[{"tool":"vm","homes":["../sdk"],"defaultHome":null}]})).is_err());
    }

    #[test]
    fn trusted_workspaces_are_normalized_and_preserved_across_sdk_and_trust_edits() {
        for (platform, workspace, lookup, other) in [
            (
                "macos",
                "/Users/dev/project",
                "/Users/dev/project/",
                "/Users/dev/other",
            ),
            (
                "windows",
                r"C:\Projects\Sample\",
                "c:/projects/sample",
                r"C:\Projects\Other",
            ),
        ] {
            // Revoking trust on an absent document is a no-op and creates no file.
            let noop = edit(&json!({
                "document": null,
                "platform": platform,
                "workspaceTrust": {"workspace": workspace, "trusted": false}
            }))
            .unwrap();
            assert_eq!(noop["changed"], false, "{platform}");
            assert_eq!(noop["document"], Value::Null, "{platform}");

            // Granting trust normalizes the path and reports projectCodeAllowed only for that root.
            let trusted = edit(&json!({
                "document": null,
                "platform": platform,
                "workspaceTrust": {"workspace": workspace, "trusted": true}
            }))
            .unwrap();
            assert_eq!(trusted["changed"], true, "{platform}");
            let doc = trusted["document"].as_str().unwrap();
            let allowed = state(&json!({
                "document": doc,
                "platform": platform,
                "workspaceRoot": lookup
            }))
            .unwrap();
            assert_eq!(allowed["projectCodeAllowed"], true, "{platform}");
            let denied = state(&json!({
                "document": doc,
                "platform": platform,
                "workspaceRoot": other
            }))
            .unwrap();
            assert_eq!(denied["projectCodeAllowed"], false, "{platform}");

            // Editing SDK homes preserves trustedWorkspaces, and revoking trust preserves SDK homes.
            let with_sdk = edit(&json!({
                "document": doc,
                "platform": platform,
                "edits": [{"tool": "vm", "homes": [other], "defaultHome": null}]
            }))
            .unwrap();
            let doc2 = with_sdk["document"].as_str().unwrap();
            assert_eq!(
                state(&json!({"document": doc2, "platform": platform, "workspaceRoot": lookup}))
                    .unwrap()["projectCodeAllowed"],
                true,
                "{platform}"
            );
            let revoked = edit(&json!({
                "document": doc2,
                "platform": platform,
                "workspaceTrust": {"workspace": lookup, "trusted": false}
            }))
            .unwrap();
            let doc3 = revoked["document"].as_str().unwrap();
            let after_revoke = state(&json!({
                "document": doc3,
                "platform": platform,
                "workspaceRoot": lookup
            }))
            .unwrap();
            assert_eq!(after_revoke["projectCodeAllowed"], false, "{platform}");
            assert_eq!(
                after_revoke["document"]["tools"]["vm"]["homes"]
                    .as_array()
                    .unwrap()
                    .len(),
                1,
                "{platform}"
            );
        }
    }
}
