//! Declared settings: validation, resolution against setting layers, and the
//! settings state, action, edit and workspace configuration commands.

use super::*;
use lithe_plugin_model::settings::LocalizationRequest;

impl DeclarativePluginManifest {
    /// Checks the declared settings, sorts them by ID, and returns the IDs
    /// launch templates may refer to.
    pub(crate) fn validate_settings(&mut self) -> Validation<DeclaredSettings> {
        self.validate_setting_actions()?;
        if self.settings.is_empty() {
            return Ok(DeclaredSettings::default());
        }
        if self.settings.len() > MAX_SETTINGS {
            return Err(invalid(
                "settings",
                "The plugin declares too many settings.",
            ));
        }
        let mut ids = BTreeSet::new();
        for (index, setting) in self.settings.iter().enumerate() {
            let field = format!("settings[{index}]");
            setting.validate(&field)?;
            if !ids.insert(setting.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "A setting is declared more than once.",
                ));
            }
        }
        self.settings.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(lithe_plugin_model::settings::declared(&self.settings))
    }

    /// Checks the settings-page actions and sorts them by ID.
    fn validate_setting_actions(&mut self) -> Validation<()> {
        if self.setting_actions.is_empty() {
            return Ok(());
        }
        if self.setting_actions.len() > MAX_SETTING_ACTIONS {
            return Err(invalid(
                "settingActions",
                "The plugin declares too many settings actions.",
            ));
        }
        let mut ids = BTreeSet::new();
        for (index, action) in self.setting_actions.iter().enumerate() {
            let field = format!("settingActions[{index}]");
            action.validate(&field)?;
            if !ids.insert(action.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "An action is declared more than once.",
                ));
            }
        }
        self.setting_actions
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }
}

impl DeclarativePluginManifest {
    /// Effective value of every declared setting and the layer it came from.
    pub(crate) fn resolve_settings(&self, layers: SettingLayers<'_>) -> Vec<ResolvedSetting> {
        lithe_plugin_model::settings::resolve(&self.settings, layers)
    }

    /// Value of every declared setting for one launch; see
    /// `lithe_plugin_model::settings::resolve` for the fallback order.
    pub(crate) fn effective_settings(&self, layers: SettingLayers<'_>) -> BTreeMap<String, Value> {
        lithe_plugin_model::settings::effective_values(&self.settings, layers)
    }
}

/// Stored setting layers of one plugin, as a host sends them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsPayload {
    manifest: Value,
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    #[serde(default)]
    project_settings: BTreeMap<String, Value>,
    /// Names the language server whose `workspaceConfiguration` to expand.
    #[serde(default, rename = "languageID")]
    language_id: Option<String>,
}

impl SettingsPayload {
    fn layers(&self) -> SettingLayers<'_> {
        SettingLayers {
            global: &self.settings,
            project: &self.project_settings,
        }
    }
}

/// `plugins.declarative.resolveSettings`: the effective value of every
/// declared setting and the layer it came from (`default`, `global`, or
/// `project`), so hosts show and launch with the same value.
pub fn resolve_settings_command(payload: Value) -> Result<Value, CoreError> {
    let payload: SettingsPayload = decode_payload(payload, "plugins.declarative.resolveSettings")?;
    Ok(outcome(
        validate_manifest_value(payload.manifest.clone()).and_then(|manifest| {
            serde_json::to_value(manifest.resolve_settings(payload.layers()))
                .map(|settings| ("settings", settings))
                .map_err(|error| PluginRejection::new("plugin.manifest.invalid", error.to_string()))
        }),
    ))
}

/// Installed package identity captured by the native registration adapter.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SettingsOwner {
    #[serde(rename = "pluginID")]
    pub(crate) plugin_id: String,
    pub(crate) version: String,
}

/// Complete stored setting documents, captured by the native storage adapter.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsStatePayload {
    manifest: Value,
    /// Optional native registration facts; this is identity binding, not admission.
    owner: Option<SettingsOwner>,
    /// Missing files are null; an existing empty or malformed file is an error.
    global_document: Option<String>,
    project_document: Option<String>,
    /// Interface language whose plugin translation to apply, such as `zh-Hans`.
    #[serde(default)]
    language: Option<String>,
    /// Translation files the native adapter found among the state's
    /// `localization.candidates`, by package-relative path. Absent until read.
    #[serde(default)]
    localizations: Option<BTreeMap<String, String>>,
}

/// `plugins.declarative.settingsState`: checked UI rows and raw declared launch
/// layers. Reading a state neither edits storage nor admits plugin capabilities.
pub fn settings_state_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "plugins.declarative.settingsState";
    let payload: SettingsStatePayload = decode_payload(payload, COMMAND)?;
    let localization =
        LocalizationRequest::new(payload.language.as_deref(), payload.localizations.as_ref())
            .map_err(|reason| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    format!("Invalid {COMMAND} payload"),
                )
                .with_details(reason)
            })?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            if payload.owner.as_ref().is_some_and(|owner| {
                owner.plugin_id != manifest.id || owner.version != manifest.version
            }) {
                return Err(PluginRejection::at(
                    "plugin.settings.ownerChanged",
                    "owner",
                    "The manifest does not match the captured installed package identity.",
                ));
            }
            let (translation, report) = localization.as_ref().map_or((None, Value::Null), |request| {
                request.select(&manifest.settings, &manifest.setting_actions)
            });
            lithe_plugin_model::settings::document_state(
                &manifest.id,
                &manifest.settings,
                &manifest.setting_actions,
                payload.global_document.as_deref(),
                payload.project_document.as_deref(),
                translation.as_ref(),
            )
            .map(|mut state| {
                state["metadata"] = json!({"displayName": manifest.display_name, "version":manifest.version, "vendor":manifest.vendor.display_name});
                state["localization"] = report;
                ("state", state)
            })
        },
    )))
}

/// Native facts for one explicitly selected installed setting action.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsActionPayload {
    manifest: Value,
    owner: SettingsOwner,
    #[serde(rename = "actionID")]
    action_id: String,
    /// Captured by the native confirmation UI, not inferred from saved preferences.
    confirmed: bool,
    /// Current native enablement choice; a stored draft cannot activate a plugin.
    enabled: bool,
    /// Describes the requesting window; runtime ownership is checked on invocation.
    workspace_available: bool,
}

/// Plans a declared settings action without starting or admitting its provider.
/// Native callers retain package/project ownership and confirmation checks.
pub fn settings_action_plan_command(payload: Value) -> Result<Value, CoreError> {
    let payload: SettingsActionPayload =
        decode_payload(payload, "plugins.declarative.settingsActionPlan")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            if payload.owner.plugin_id != manifest.id || payload.owner.version != manifest.version {
                return Err(PluginRejection::at(
                    "plugin.settings.ownerChanged",
                    "owner",
                    "The installed package identity changed before the action was planned.",
                ));
            }
            lithe_plugin_model::settings::plan_action(
                &manifest.id,
                &manifest.setting_actions,
                &payload.action_id,
                payload.confirmed,
                payload.workspace_available,
                payload.enabled,
            )
            .map(|mut plan| {
                plan["version"] = json!(manifest.version);
                ("plan", plan)
            })
        },
    )))
}

/// One layer and one atomic edit batch; the host owns its captured byte baseline.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsEditPayload {
    manifest: Value,
    scope: lithe_plugin_model::settings::SettingScope,
    document: Option<String>,
    edits: Vec<lithe_plugin_model::settings::SettingsEdit>,
}

/// `plugins.declarative.editSettings`: validates all edits and returns complete
/// text, deletion eligibility and required effects, without performing any I/O.
pub fn edit_settings_command(payload: Value) -> Result<Value, CoreError> {
    let payload: SettingsEditPayload = decode_payload(payload, "plugins.declarative.editSettings")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            lithe_plugin_model::settings::edit_document(
                &manifest.id,
                &manifest.settings,
                payload.scope,
                payload.document.as_deref(),
                payload.edits,
            )
            .map(|edit| ("edit", edit))
        },
    )))
}

/// `plugins.declarative.workspaceConfiguration`: a language server's
/// `workspaceConfiguration` expanded with the current values, which the host
/// sends to the running server after an `immediate` setting changed.
/// `configuration` is `null` when the server declares none.
pub fn workspace_configuration_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "plugins.declarative.workspaceConfiguration";
    let payload: SettingsPayload = decode_payload(payload, COMMAND)?;
    let Some(language_id) = payload.language_id.clone() else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("Invalid {COMMAND} payload"),
        )
        .with_details("languageID is required."));
    };
    Ok(outcome(
        validate_manifest_value(payload.manifest.clone()).and_then(|manifest| {
            let server = manifest
                .language_servers
                .iter()
                .find(|server| server.language_id == language_id)
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.launch.unavailable",
                        format!("The plugin does not provide a language server for {language_id}."),
                    )
                })?;
            let configuration = server
                .workspace_configuration
                .as_ref()
                .map(|value| {
                    expand_configuration(value, &manifest.effective_settings(payload.layers()))
                })
                .unwrap_or(Value::Null);
            Ok(("configuration", configuration))
        }),
    ))
}
