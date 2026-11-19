//! Machine-wide SDK registrations and defaults, independent of project pins,
//! and the machine-local workspace trust decisions.
//!
//! Native adapters validate installations, preserve source identity and publish
//! edits atomically. This module owns bounds, path identity, document merges and
//! the order in which trust decisions apply to a workspace.

use super::paths;
use lithe_plugin_model::{is_tool_id, PluginRejection, Validation};
use lithe_protocol::json_document::parse_unique_json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_BYTES: usize = 256 * 1024;
/// Bound of each trust list: trusted and distrusted workspaces, and trusted locations.
const MAX_TRUST_ENTRIES: usize = 256;

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
    /// Workspaces the user chose to keep in safe mode. An explicit decision on a
    /// workspace wins over a trusted location that contains it. Written only once
    /// used, so a catalog without decisions stays readable by older hosts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    distrusted_workspaces: Vec<String>,
    /// Folders whose every workspace, at any depth, is trusted unless distrusted
    /// itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trusted_locations: Vec<String>,
}

/// A workspace's trust: `trusted` allows project code to run, `distrusted` is a
/// recorded choice of safe mode, and `unknown` means the user was not asked yet.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum TrustState {
    Trusted,
    Distrusted,
    Unknown,
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

/// Explicit machine-local trust decision on one workspace: `trusted: true` lets
/// its project code run, `trusted: false` records safe mode. `location` also
/// trusts the workspace's parent folder, and is refused with `trusted: false` or
/// when the workspace has no parent to offer.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceTrustEdit {
    workspace: String,
    trusted: bool,
    #[serde(default)]
    location: bool,
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

    /// Normalizes one trust list, refusing duplicates under the platform's case rules.
    fn trust_list(&self, entries: &[String], field: &str) -> Validation<Vec<String>> {
        Ok(self.trust_keys(entries, field)?.into_values().collect())
    }

    /// One trust list by identity key, so an edit adds or removes a path whatever
    /// its spelling.
    fn trust_keys(&self, entries: &[String], field: &str) -> Validation<BTreeMap<String, String>> {
        let case_sensitive = self.case_sensitive()?;
        if entries.len() > MAX_TRUST_ENTRIES {
            return Err(invalid(field));
        }
        let mut by_key = BTreeMap::new();
        for entry in entries {
            let path =
                paths::normalize(entry, &self.platform, true).ok_or_else(|| invalid(field))?;
            let key = paths::normalize(&path, &self.platform, case_sensitive)
                .ok_or_else(|| invalid(field))?;
            if by_key.insert(key, path).is_some() {
                return Err(invalid(field));
            }
        }
        Ok(by_key)
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
                distrusted_workspaces: Vec::new(),
                trusted_locations: Vec::new(),
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
        catalog.trusted_workspaces =
            self.trust_list(&catalog.trusted_workspaces, "trustedWorkspaces")?;
        catalog.distrusted_workspaces =
            self.trust_list(&catalog.distrusted_workspaces, "distrustedWorkspaces")?;
        catalog.trusted_locations =
            self.trust_list(&catalog.trusted_locations, "trustedLocations")?;
        Ok(catalog)
    }

    /// The trust of `workspace`, as IntelliJ IDEA orders it: an explicit decision on
    /// the workspace first, then a trusted location containing it. The matching
    /// location is returned with a location-derived trust.
    fn workspace_trust(
        &self,
        catalog: &Catalog,
        workspace: &str,
    ) -> Validation<(TrustState, Option<String>)> {
        let case_sensitive = self.case_sensitive()?;
        let key = paths::normalize(workspace, &self.platform, case_sensitive)
            .ok_or_else(|| invalid("workspaceRoot"))?;
        let names = |entries: &[String]| {
            entries.iter().any(|entry| {
                paths::normalize(entry, &self.platform, case_sensitive).as_ref() == Some(&key)
            })
        };
        if names(&catalog.trusted_workspaces) {
            return Ok((TrustState::Trusted, None));
        }
        if names(&catalog.distrusted_workspaces) {
            return Ok((TrustState::Distrusted, None));
        }
        let location = catalog
            .trusted_locations
            .iter()
            .find(|location| paths::within(workspace, location, &self.platform, case_sensitive));
        Ok(match location {
            Some(location) => (TrustState::Trusted, Some(location.clone())),
            None => (TrustState::Unknown, None),
        })
    }

    /// The folder a trust prompt may offer to trust with `workspace`: its parent,
    /// unless the parent is a filesystem root (`/`, a drive or a UNC share), which
    /// would trust every project on the volume.
    fn offered_location(&self, workspace: &str) -> Option<String> {
        let path = paths::normalize(workspace, &self.platform, true)?;
        let (parent, _) = path.rsplit_once('/')?;
        let root = parent.is_empty()
            || (self.platform == "windows"
                && (parent.len() == 2 && parent.ends_with(':')
                    || parent.starts_with("//") && parent[2..].split('/').count() <= 2));
        if root {
            return None;
        }
        paths::normalize(parent, &self.platform, true)
    }
}

/// Returns the complete validated catalog and the native read plan.
pub(crate) fn state(input: &Value) -> Validation<Value> {
    let request: Request = serde_json::from_value(input.clone()).map_err(|_| invalid("input"))?;
    let catalog = request.read()?;
    let mut response = json!({
        "document": catalog,
        "storage": {
            "relativePath": "toolchains/global.json",
            "maximumBytes": MAX_BYTES
        }
    });
    // An absent or empty root asks for the catalog only.
    if let Some(root) = request
        .workspace_root
        .as_deref()
        .filter(|root| !root.is_empty())
    {
        let (trust, location) = request.workspace_trust(&catalog, root)?;
        response["projectCodeAllowed"] = json!(trust == TrustState::Trusted);
        response["workspaceTrust"] = json!({
            "state": trust,
            "location": location,
            "offeredLocation": request.offered_location(root),
        });
    }
    Ok(response)
}

/// Merges confirmed tool edits and one explicit machine-local workspace trust decision,
/// preserving dormant registrations and the other trust decisions.
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
        let workspace = [trust.workspace.clone()];
        let (key, normalized) = source
            .trust_keys(&workspace, "workspaceTrust.workspace")?
            .into_iter()
            .next()
            .ok_or_else(|| invalid("workspaceTrust.workspace"))?;
        let mut trusted = source.trust_keys(&catalog.trusted_workspaces, "trustedWorkspaces")?;
        let mut distrusted =
            source.trust_keys(&catalog.distrusted_workspaces, "distrustedWorkspaces")?;
        // A decision replaces the other one, so the two lists never name one workspace.
        if trust.trusted {
            distrusted.remove(&key);
            trusted.insert(key, normalized);
        } else {
            if trust.location {
                return Err(invalid("workspaceTrust.location"));
            }
            trusted.remove(&key);
            distrusted.insert(key, normalized);
        }
        if trust.location {
            let location = source
                .offered_location(&trust.workspace)
                .ok_or_else(|| invalid("workspaceTrust.location"))?;
            let mut locations =
                source.trust_keys(&catalog.trusted_locations, "trustedLocations")?;
            let key = paths::normalize(&location, &source.platform, case_sensitive)
                .ok_or_else(|| invalid("workspaceTrust.location"))?;
            locations.entry(key).or_insert(location);
            catalog.trusted_locations = locations.into_values().collect();
        }
        if [
            trusted.len(),
            distrusted.len(),
            catalog.trusted_locations.len(),
        ]
        .iter()
        .any(|len| *len > MAX_TRUST_ENTRIES)
        {
            return Err(invalid("workspaceTrust"));
        }
        catalog.trusted_workspaces = trusted.into_values().collect();
        catalog.distrusted_workspaces = distrusted.into_values().collect();
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

    /// The trust a `state` request reports for `workspace`.
    fn trust(document: &Value, platform: &str, workspace: &str) -> Value {
        let state =
            state(&json!({"document": document, "platform": platform, "workspaceRoot": workspace}))
                .unwrap();
        let mut trust = state["workspaceTrust"].clone();
        trust["projectCodeAllowed"] = state["projectCodeAllowed"].clone();
        trust
    }

    // A workspace starts undecided, a choice of safe mode is recorded rather than
    // forgotten, and trusting it afterwards replaces that choice: the trust prompt asks
    // only while the state is unknown.
    #[test]
    fn a_workspace_moves_between_unknown_distrusted_and_trusted() {
        for (platform, workspace, spelling) in [
            ("macos", "/Users/dev/code/app", "/Users/dev/code/app/"),
            ("windows", r"C:\Code\App", "c:/code/app"),
        ] {
            let unknown = trust(&Value::Null, platform, workspace);
            assert_eq!(unknown["state"], "unknown", "{platform}");
            assert_eq!(unknown["projectCodeAllowed"], false, "{platform}");

            let distrusted = edit(&json!({
                "document": null,
                "platform": platform,
                "workspaceTrust": {"workspace": workspace, "trusted": false}
            }))
            .unwrap();
            assert_eq!(distrusted["changed"], true, "{platform}");
            let document = distrusted["document"].clone();
            let after = trust(&document, platform, spelling);
            assert_eq!(after["state"], "distrusted", "{platform}");
            assert_eq!(after["projectCodeAllowed"], false, "{platform}");

            let trusted = edit(&json!({
                "document": document,
                "platform": platform,
                "workspaceTrust": {"workspace": spelling, "trusted": true}
            }))
            .unwrap();
            assert_eq!(
                trusted["catalog"]["distrustedWorkspaces"],
                Value::Null,
                "{platform}"
            );
            assert_eq!(
                trusted["catalog"]["trustedWorkspaces"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            let after = trust(&trusted["document"], platform, workspace);
            assert_eq!(after["state"], "trusted", "{platform}");
            assert_eq!(after["location"], Value::Null, "{platform}");
            assert_eq!(after["projectCodeAllowed"], true, "{platform}");
        }
    }

    // Trusting a location trusts every workspace beneath it, but not a sibling whose
    // name only starts the same, and a workspace distrusted explicitly stays in safe
    // mode inside a trusted location, as in IntelliJ IDEA.
    #[test]
    fn a_trusted_location_covers_its_workspaces_unless_one_is_distrusted() {
        for (platform, workspace, nested, sibling, parent) in [
            (
                "macos",
                "/Users/dev/code/app",
                "/Users/dev/code/libs/util",
                "/Users/dev/code-old/app",
                "/Users/dev/code",
            ),
            (
                "windows",
                r"C:\Code\App",
                r"C:\code\Libs\Util",
                r"C:\Code-old\App",
                "C:/Code",
            ),
        ] {
            let offered = trust(&Value::Null, platform, workspace);
            assert_eq!(offered["offeredLocation"], parent, "{platform}");

            let trusted = edit(&json!({
                "document": null,
                "platform": platform,
                "workspaceTrust": {"workspace": workspace, "trusted": true, "location": true}
            }))
            .unwrap();
            assert_eq!(
                trusted["catalog"]["trustedLocations"],
                json!([parent]),
                "{platform}"
            );
            let document = trusted["document"].clone();
            let covered = trust(&document, platform, nested);
            assert_eq!(covered["state"], "trusted", "{platform}");
            assert_eq!(covered["location"], parent, "{platform}");
            assert_eq!(
                trust(&document, platform, sibling)["state"],
                "unknown",
                "{platform}"
            );

            let distrusted = edit(&json!({
                "document": document,
                "platform": platform,
                "workspaceTrust": {"workspace": nested, "trusted": false}
            }))
            .unwrap();
            let after = trust(&distrusted["document"], platform, nested);
            assert_eq!(after["state"], "distrusted", "{platform}");
            assert_eq!(after["projectCodeAllowed"], false, "{platform}");
        }
    }

    // Trusting the parent of a workspace at the top of a volume would trust every
    // project on it, so no location is offered and an edit asking for one is refused.
    #[test]
    fn a_filesystem_root_is_never_offered_as_a_location() {
        for (platform, workspace) in [
            ("macos", "/app"),
            ("windows", r"D:\App"),
            ("windows", r"\\server\share\App"),
        ] {
            assert_eq!(
                trust(&Value::Null, platform, workspace)["offeredLocation"],
                Value::Null,
                "{workspace}"
            );
            assert!(
                edit(&json!({
                    "document": null,
                    "platform": platform,
                    "workspaceTrust": {"workspace": workspace, "trusted": true, "location": true}
                }))
                .is_err(),
                "{workspace}"
            );
        }
        assert!(edit(&json!({
            "document": null,
            "platform": "macos",
            "workspaceTrust": {"workspace": "/Users/dev/app", "trusted": false, "location": true}
        }))
        .is_err());
    }
}
