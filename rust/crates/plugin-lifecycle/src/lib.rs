//! Which installed plugins are enabled, and what a host does, in which order, when the user
//! confirms an installation, enables, disables or uninstalls a plugin.
//!
//! The host keeps the packages, their approval records, the user's enablement choices and the
//! plugins' programs; it hands their identities to `plugins.installed.lifecycle` and carries
//! out the steps it gets back. This crate reads no file and stops nothing.
//!
//! Enablement is per plugin, not per version, and a plugin the user made no choice for is
//! disabled. Confirming an installation is the user saying they want the plugin, so it records
//! the plugin as enabled unless the user already chose: a plugin disabled before an update
//! stays disabled. Disabling records the choice before it stops the plugin's programs, so
//! nothing starts the plugin again while it stops. Uninstalling stops the programs first, since
//! one still running could write its state again and Windows cannot move the directory of a
//! running program; then it removes every installed version, because the plugin page shows one
//! row per plugin and an older version left behind would keep the plugin installed; then it
//! forgets the choice and removes the state the programs wrote, so a later installation starts
//! as a first one.
//!
//! A language has one language server: two plugins that both serve it would race for every
//! document of it, and neither would know the other answers too. So enabling a plugin, or
//! confirming its installation while it is enabled, turns off every other enabled plugin whose
//! newest installed version serves one of the same languages, and the host records that and
//! stops their programs. Choices made before this rule can still enable two such plugins;
//! enablement then keeps the first by ID and reports the others disabled, without changing the
//! choices, so enabling one of them later is what settles it. Fixtures live in
//! `rust/fixtures/plugins/installed-lifecycle.json`.

use std::collections::{BTreeMap, BTreeSet};

use lithe_plugin_model::PluginVersion;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upper bound of installed versions and of choices in one request.
const MAX_ENTRIES: usize = 4096;

/// Upper bound of the languages one installed version serves; a manifest declares at most one
/// language server per language it declares.
const MAX_LANGUAGE_SERVERS: usize = 256;

/// Upper bound of one language ID, in bytes.
const MAX_LANGUAGE_ID_BYTES: usize = 256;

/// Rejection of an action on a plugin that has no installed version: enabling it would
/// silently enable a later installation, and there is nothing to uninstall.
pub const NOT_INSTALLED: &str = "plugin.lifecycle.notInstalled";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// Every installed plugin version, in any order.
    installed: Vec<InstalledVersion>,
    /// Every enablement choice the user made, one per plugin.
    #[serde(default)]
    choices: Vec<Choice>,
    /// What the user does; without it the response only reports enablement.
    #[serde(default)]
    action: Option<Action>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledVersion {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    version: String,
    /// The `languageID` of every language server this version's manifest declares; only the
    /// plugin's newest installed version counts, since it is the one that runs.
    #[serde(default, rename = "languageServers")]
    language_servers: Vec<String>,
}

/// One plugin and whether it is enabled: a choice in the request, a plugin's enablement in the
/// response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Action {
    kind: ActionKind,
    #[serde(rename = "pluginID")]
    plugin_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ActionKind {
    /// The user confirmed installing a version of the plugin, which is now on disk.
    ConfirmInstall,
    /// The user turned the plugin on.
    Enable,
    /// The user turned the plugin off.
    Disable,
    /// The user removed the plugin.
    Uninstall,
}

/// One thing the host does for the action, on the action's plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Step {
    /// Record the user's choice for the plugin.
    RecordChoice { enabled: bool },
    /// Drop the user's choice for the plugin.
    ForgetChoice,
    /// Stop the plugin's language server and backend, returning once they ended or gave up.
    StopPrograms,
    /// Remove one installed version, then forget the approval recorded for it; the approval
    /// stays when the version could not be removed.
    RemoveVersion { version: String },
    /// Remove the state directory the plugin's programs wrote.
    RemoveState,
    /// Turn another plugin off because the action's plugin now serves `language_ids` too:
    /// record the choice `enabled: false` for `plugin_id`, then stop its programs. Unlike the
    /// other steps it is on `plugin_id`, not on the action's plugin.
    DisplacePlugin {
        #[serde(rename = "pluginID")]
        plugin_id: String,
        #[serde(rename = "languageIDs")]
        language_ids: Vec<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    /// Every installed plugin once, by ID, with its enablement once the steps ran; an
    /// uninstalled plugin is left out.
    plugins: Vec<Choice>,
    /// Whether the steps change the user's choice for the action's plugin.
    changed: bool,
    /// What the host does, in order.
    steps: Vec<Step>,
    /// Why the action does not apply; it then has no steps and `plugins` is as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    rejection: Option<&'static str>,
}

/// Handles `plugins.installed.lifecycle`.
///
/// Installed versions and choices are identities only; a repeated version or a second choice
/// for one plugin is an invalid request, as is an empty plugin ID or version. A choice for a
/// plugin that is not installed is kept and reported nowhere: it is what a later installation
/// of the plugin finds.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid plugin lifecycle request",
        )
        .with_details(error.to_string())
    })?;
    if request.installed.len() > MAX_ENTRIES || request.choices.len() > MAX_ENTRIES {
        return Err(invalid("Too many plugins for one lifecycle request"));
    }
    let mut versions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for entry in &request.installed {
        if entry.plugin_id.is_empty() || entry.version.is_empty() {
            return Err(invalid("An installed plugin has an empty ID or version"));
        }
        validate_language_servers(&entry.language_servers)?;
        if !versions
            .entry(&entry.plugin_id)
            .or_default()
            .insert(&entry.version)
        {
            return Err(invalid("An installed plugin version is listed twice"));
        }
    }
    let mut choices: BTreeMap<&str, bool> = BTreeMap::new();
    for choice in &request.choices {
        if choice.plugin_id.is_empty() {
            return Err(invalid("A plugin choice has an empty ID"));
        }
        if choices.insert(&choice.plugin_id, choice.enabled).is_some() {
            return Err(invalid("A plugin has two choices"));
        }
    }
    let served = served_languages(&request.installed);
    let Some(action) = &request.action else {
        return Ok(encode(Response {
            plugins: enablement(&versions, &choices, &served, None),
            changed: false,
            steps: Vec::new(),
            rejection: None,
        }));
    };
    if action.plugin_id.is_empty() {
        return Err(invalid(
            "The plugin lifecycle action has an empty plugin ID",
        ));
    }
    let id = action.plugin_id.as_str();
    let Some(installed) = versions.get(id) else {
        return Ok(encode(Response {
            plugins: enablement(&versions, &choices, &served, None),
            changed: false,
            steps: Vec::new(),
            rejection: Some(NOT_INSTALLED),
        }));
    };
    let chosen = choices.get(id).copied();
    let mut steps = Vec::new();
    let mut uninstalled = None;
    let after = match action.kind {
        ActionKind::ConfirmInstall => match chosen {
            Some(enabled) => Some(enabled),
            None => {
                steps.push(Step::RecordChoice { enabled: true });
                Some(true)
            }
        },
        ActionKind::Enable => {
            if chosen != Some(true) {
                steps.push(Step::RecordChoice { enabled: true });
            }
            Some(true)
        }
        ActionKind::Disable => {
            if chosen != Some(false) {
                steps.push(Step::RecordChoice { enabled: false });
            }
            steps.push(Step::StopPrograms);
            Some(false)
        }
        ActionKind::Uninstall => {
            steps.push(Step::StopPrograms);
            steps.extend(installed.iter().map(|version| Step::RemoveVersion {
                version: (*version).to_string(),
            }));
            if chosen.is_some() {
                steps.push(Step::ForgetChoice);
            }
            steps.push(Step::RemoveState);
            uninstalled = Some(id);
            None
        }
    };
    if let Some(enabled) = after {
        choices.insert(id, enabled);
    }
    if after == Some(true) {
        steps.extend(displace_rivals(id, &served, &mut choices));
    }
    Ok(encode(Response {
        plugins: enablement(&versions, &choices, &served, uninstalled),
        changed: after != chosen,
        steps,
        rejection: None,
    }))
}

/// Every installed plugin but `left_out`, by ID, enabled only when its choice says so and no
/// plugin before it by ID that is enabled already serves one of its languages.
fn enablement(
    versions: &BTreeMap<&str, BTreeSet<&str>>,
    choices: &BTreeMap<&str, bool>,
    served: &BTreeMap<&str, BTreeSet<&str>>,
    left_out: Option<&str>,
) -> Vec<Choice> {
    let none = BTreeSet::new();
    let mut claimed: BTreeSet<&str> = BTreeSet::new();
    versions
        .keys()
        .filter(|id| Some(**id) != left_out)
        .map(|id| {
            let languages = served.get(id).unwrap_or(&none);
            let enabled =
                choices.get(id).copied().unwrap_or(false) && languages.is_disjoint(&claimed);
            if enabled {
                claimed.extend(languages.iter().copied());
            }
            Choice {
                plugin_id: (*id).to_string(),
                enabled,
            }
        })
        .collect()
}

/// The languages each installed plugin's newest version serves.
///
/// Versions are compared as `major.minor.patch`; a version that is not one ranks below every
/// one that is, and of two such versions the first listed counts.
fn served_languages(installed: &[InstalledVersion]) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut newest: BTreeMap<&str, (Option<PluginVersion>, &InstalledVersion)> = BTreeMap::new();
    for entry in installed {
        let version = PluginVersion::parse(&entry.version);
        let current = newest.entry(&entry.plugin_id).or_insert((version, entry));
        if version > current.0 {
            *current = (version, entry);
        }
    }
    newest
        .into_iter()
        .map(|(id, (_, entry))| {
            (
                id,
                entry.language_servers.iter().map(String::as_str).collect(),
            )
        })
        .collect()
}

/// Turns off, in `choices`, every other enabled plugin that serves one of `id`'s languages, and
/// returns the steps that make the host do the same, in plugin ID order.
fn displace_rivals<'a>(
    id: &str,
    served: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    choices: &mut BTreeMap<&'a str, bool>,
) -> Vec<Step> {
    let Some(languages) = served.get(id).filter(|languages| !languages.is_empty()) else {
        return Vec::new();
    };
    let mut steps = Vec::new();
    for (other, other_languages) in served {
        if *other == id || choices.get(other) != Some(&true) {
            continue;
        }
        let shared: Vec<String> = languages
            .intersection(other_languages)
            .map(|language| (*language).to_string())
            .collect();
        if shared.is_empty() {
            continue;
        }
        choices.insert(other, false);
        steps.push(Step::DisplacePlugin {
            plugin_id: (*other).to_string(),
            language_ids: shared,
        });
    }
    steps
}

/// Rejects a malformed `languageServers` list: too long, an empty or overlong ID, or an ID
/// listed twice, which a valid manifest never produces.
fn validate_language_servers(languages: &[String]) -> Result<(), CoreError> {
    if languages.len() > MAX_LANGUAGE_SERVERS {
        return Err(invalid(
            "An installed plugin version serves too many languages",
        ));
    }
    let mut seen = BTreeSet::new();
    for language in languages {
        if language.is_empty() || language.len() > MAX_LANGUAGE_ID_BYTES {
            return Err(invalid(
                "An installed plugin version serves an empty or overlong language ID",
            ));
        }
        if !seen.insert(language.as_str()) {
            return Err(invalid(
                "An installed plugin version serves a language twice",
            ));
        }
    }
    Ok(())
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

fn encode(response: Response) -> Value {
    serde_json::to_value(response).expect("plugin lifecycle response encodes")
}
