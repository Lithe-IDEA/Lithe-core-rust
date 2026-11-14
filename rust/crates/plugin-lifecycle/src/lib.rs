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
//! as a first one. Fixtures live in `rust/fixtures/plugins/installed-lifecycle.json`.

use std::collections::{BTreeMap, BTreeSet};

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upper bound of installed versions and of choices in one request.
const MAX_ENTRIES: usize = 4096;

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
    let Some(action) = &request.action else {
        return Ok(encode(Response {
            plugins: enablement(&versions, &choices, None),
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
            plugins: enablement(&versions, &choices, None),
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
    Ok(encode(Response {
        plugins: enablement(&versions, &choices, uninstalled),
        changed: after != chosen,
        steps,
        rejection: None,
    }))
}

/// Every installed plugin but `left_out`, by ID, enabled only when its choice says so.
fn enablement(
    versions: &BTreeMap<&str, BTreeSet<&str>>,
    choices: &BTreeMap<&str, bool>,
    left_out: Option<&str>,
) -> Vec<Choice> {
    versions
        .keys()
        .filter(|id| Some(**id) != left_out)
        .map(|id| Choice {
            plugin_id: (*id).to_string(),
            enabled: choices.get(id).copied().unwrap_or(false),
        })
        .collect()
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

fn encode(response: Response) -> Value {
    serde_json::to_value(response).expect("plugin lifecycle response encodes")
}
