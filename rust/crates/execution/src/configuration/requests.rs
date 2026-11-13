//! Request payloads of the run-configuration commands.

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to validate the layered configuration documents for a workspace.
pub struct InspectRequest {
    /// Canonical workspace root whose `.lithe` documents are inspected.
    pub root: String,
    /// Settings-only reads may skip source hashing; omitted preserves full inspection.
    #[serde(default)]
    pub check_fingerprint: Option<bool>,
    /// Host-owned local layer. When present, Core validates it instead of `.lithe/run/local.json`.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// Complete generic run selectors; omission preserves older callers.
    #[serde(default)]
    pub provider_runs: Option<Vec<crate::provider_runs::ProviderRunConfiguration>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to regenerate detected configurations from the current project tree.
pub struct GenerateRequest {
    /// Canonical workspace root scanned for run entries.
    pub root: String,
    /// Complete generic run selectors; omission preserves older callers.
    #[serde(default)]
    pub provider_runs: Option<Vec<crate::provider_runs::ProviderRunConfiguration>>,
    /// Application cache directory, outside the workspace. Generation records
    /// its result there so the next session can list provider entries before
    /// the providers answer again; omitted, only this process remembers it.
    #[serde(default)]
    pub cache_directory: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to merge configuration layers and resolve host toolchains.
pub struct ResolveRequest {
    /// Canonical workspace root whose layers are merged.
    pub root: String,
    /// Executables the host discovered; requirements pick among these by role.
    #[serde(default)]
    pub toolchain_candidates: Vec<ToolchainCandidate>,
    /// Host-owned local layer. When present, Core uses it instead of `.lithe/run/local.json`.
    #[serde(default)]
    pub local_document: Option<Value>,
    /// Optional prepared team layer for a host transaction preflight; never persisted by Core.
    #[serde(default)]
    pub team_document: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Host-discovered executable that may satisfy a configuration requirement.
pub struct ToolchainCandidate {
    /// Host-stable candidate identifier referenced by resolved configurations.
    pub id: String,
    /// Toolchain role such as `node`, not a display label.
    #[serde(rename = "type")]
    pub kind: String,
    /// Reported version, empty when the host could not read one.
    #[serde(default)]
    pub version: String,
    /// Distribution vendor, empty when unknown.
    #[serde(default)]
    pub vendor: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request to turn one resolved configuration into a process launch plan.
pub struct LaunchPlanRequest {
    /// Canonical workspace root whose layers are merged before planning.
    pub root: String,
    /// Merged configuration identity to plan.
    pub configuration_id: String,
    /// Host-owned local layer. When present, Core uses it instead of `.lithe/run/local.json`.
    #[serde(default)]
    pub local_document: Option<Value>,
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
    /// Explicit executables for generic runtime toolchain IDs such as `project-node`.
    #[serde(default)]
    pub runtime_executable_paths: BTreeMap<String, String>,
    /// Generic consumer preferences in the same machine-local document.
    /// Absent preserves saved choices; an empty v1 document resets them.
    #[serde(default)]
    pub preferences: Option<Value>,
}

/// Replaces only generic preferences in a captured machine-local Run document.
/// Other toolchain fields are preserved untouched for their owners.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateToolPreferencesRequest {
    /// Required captured local layer; this command never reads native files.
    pub local_document: Value,
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
    /// Host-owned local layer used when `scope` is `local` and when resolving first.
    #[serde(default)]
    pub local_document: Option<Value>,
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
}
