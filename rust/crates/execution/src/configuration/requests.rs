//! Request payloads of the run-configuration commands.

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to validate the layered configuration documents for a workspace.
pub struct InspectRequest {
    /// Canonical workspace root whose `.lithe` documents are inspected.
    pub root: String,
    /// Host-owned local layer. When present, Core validates it instead of the local files.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// Current provider run snapshot. When present, a generated layer from a
    /// different snapshot is reported as stale.
    #[serde(default)]
    pub provider_runs: Option<Vec<crate::provider_runs::ProviderRunConfiguration>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to regenerate the generated layer from the current provider runs.
pub struct GenerateRequest {
    /// Canonical workspace root the generated layer belongs to.
    pub root: String,
    /// Complete provider run snapshot; omitted generates an empty layer.
    #[serde(default)]
    pub provider_runs: Option<Vec<crate::provider_runs::ProviderRunConfiguration>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to merge the generated, team and local configuration layers.
pub struct ResolveRequest {
    /// Canonical workspace root whose layers are merged.
    pub root: String,
    /// Host-owned local layer. When present, Core uses it instead of the local files.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// Optional prepared team layer for a host transaction preflight; never persisted by Core.
    #[serde(default)]
    pub team_document: Option<Value>,
    /// The installed, enabled plugins, so a plugin configuration whose plugin
    /// is missing, from another repository or too old is marked unavailable.
    /// Omitted skips that check.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to turn one resolved configuration into a process launch plan.
pub struct LaunchPlanRequest {
    /// Canonical workspace root whose layers are merged before planning.
    pub root: String,
    /// Merged configuration identity to plan.
    pub configuration_id: String,
    /// Host-owned local layer. When present, Core uses it instead of the local files.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// Absolute path prefix, in a host-owned temporary directory, that an
    /// inline script is written to; Core appends the script's suffix. Needed
    /// only to plan a process run whose script is text.
    #[serde(default)]
    pub inline_script_base: Option<String>,
}

/// Resolves saved Run choices without assembling a build tool's command line.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderLaunchInputsRequest {
    /// Canonical workspace owned by the native host.
    pub root: String,
    /// Stable identity resolved through the generated/team/local document layers.
    pub configuration_id: String,
    /// Current provider instance, supplied by the owning build model.
    pub provider_root: String,
    /// Optional host-owned replacement for the machine-local document.
    #[serde(default)]
    pub local_document: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Global machine toolchain stored at document level in the local layer.
pub struct ToolchainPaths {
    /// Explicit executables keyed by the local toolchain ID a `runtime` binding names.
    #[serde(default)]
    pub runtime_executable_paths: BTreeMap<String, String>,
    /// Generic consumer preferences in the same machine-local document.
    /// Absent preserves saved choices; an empty v1 document resets them.
    #[serde(default)]
    pub preferences: Option<Value>,
}

/// Replaces only generic preferences in the machine-local layer. Other
/// toolchain fields are preserved untouched for their owners.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateToolPreferencesRequest {
    /// Canonical workspace root whose local layer is edited.
    pub root: String,
    /// Inert v1 preferences already edited by the shared preference command.
    pub preferences: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Editable options applied to a team or machine-local configuration layer.
pub struct UpdateOptionsRequest {
    /// Canonical workspace root that owns the edited layer.
    pub root: String,
    /// Persistence layer: `project` for shared configuration or `local` for this host.
    pub scope: String,
    /// Merged configuration identity to patch.
    pub configuration_id: String,
    /// Working directory relative to the workspace root; empty keeps the merged value.
    #[serde(default)]
    pub working_directory: String,
    /// Common program arguments. `programArguments` remains a wire alias for
    /// clients from before the generic run-options migration.
    #[serde(default, alias = "programArguments")]
    pub arguments: String,
    /// Environment variables replacing the configuration's own entries.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Portable provider choices. Omission preserves the existing namespace.
    #[serde(
        default,
        deserialize_with = "crate::provider_run_options::optional_request_options"
    )]
    pub provider_run_options: Option<crate::provider_run_options::BuildProviderRunOptions>,
    /// When present, `updateOptions` writes the document-level global toolchain
    /// into the local layer instead of patching one configuration.
    #[serde(default)]
    pub toolchain: Option<ToolchainPaths>,
    /// The steps to run first, in order, as the `beforeLaunch` editor value
    /// holds them: the ID of a built-in process run, or a task step whose
    /// `task` is read. Omission keeps the saved steps; an empty list removes
    /// them from this layer and, in the local layer, also hides the
    /// project's.
    #[serde(default)]
    pub before_launch: Option<Vec<Value>>,
    /// Host-owned local layer used to resolve the configuration first. A
    /// local edit is refused while it is set: only files can be changed.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// The installed plugins, whose identity a written plugin document records.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
    /// The kinds installed plugins declare, which task steps are checked
    /// against; set by the dispatcher from `runTypePlugins`.
    #[serde(skip)]
    pub run_types: Vec<DeclaredRunType>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to add an explicitly user-authored run configuration.
pub struct CreateUserConfigurationRequest {
    /// Canonical workspace root that receives the new entry.
    pub root: String,
    /// Persistence layer: `project` for shared configuration or `local` for this host.
    pub scope: String,
    /// Display name shown in the Run list.
    pub name: String,
    /// UI configuration kind mapped to a namespaced provider during creation.
    #[serde(rename = "type")]
    pub kind: String,
    /// A typed selector, never a cached executable or resolved launch payload.
    #[serde(default)]
    pub build: Option<BuildProviderRunReference>,
    /// The provider entrypoint an entrypoint run starts, saved as
    /// `extensions.build-run.entrypoint`; it is part of the entry's identity.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// The installed plugins, whose identity a written plugin document records.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
}
