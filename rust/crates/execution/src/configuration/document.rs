//! The stored run-configuration document, its migrations and toolchain requirements.

use super::*;

/// Persisted workspace identity for re-preparing a build-provider application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildProviderRunReference {
    /// Plugin that declared the build system.
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    /// Build system identifier within that plugin.
    #[serde(rename = "buildSystemID")]
    pub build_system_id: String,
    /// Workspace-relative provider root, forward-slashed.
    pub root: String,
    /// Run declaration identifier within the build system.
    pub run: String,
    /// Opaque project identity reported by the build model.
    #[serde(rename = "projectID")]
    pub project_id: String,
    /// Opaque target identity reported by the build model.
    #[serde(rename = "targetID")]
    pub target_id: String,
    /// Omitted older references are applications; the current declaration must match at launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Execution>,
    /// User arguments appended after the provider's own arguments.
    #[serde(default)]
    pub user_arguments: Vec<String>,
}

impl BuildProviderRunReference {
    /// Stable generated identity excludes mutable display names, execution kind and arguments.
    pub(crate) fn generated_id(&self) -> String {
        use sha2::{Digest, Sha256};
        let selector = json!([
            self.plugin_id,
            self.build_system_id,
            self.root,
            self.run,
            self.project_id,
            self.target_id
        ]);
        let bytes = serde_json::to_vec(&selector).expect("selector contains only strings");
        format!("build.provider:{:x}", Sha256::digest(bytes))
    }

    /// Persistence validates identities only; current model membership is checked at launch.
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value.len() <= 200
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        };
        if self.execution == Some(Execution::Group)
            || !identifier(&self.plugin_id)
            || !identifier(&self.build_system_id)
            || !identifier(&self.run)
            || [&self.project_id, &self.target_id]
                .iter()
                .any(|id| id.is_empty() || id.len() > 2048 || id.contains('\0'))
            || self.user_arguments.len() > 64
            || self
                .user_arguments
                .iter()
                .any(|arg| arg.len() > 4096 || arg.contains('\0'))
            || invalid_relative_path(&self.root)
            || self.root.is_empty()
            || self.root.contains('\\')
            || self.root.contains('\0')
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid build provider run reference",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
/// Versioned run-configuration document stored below `.lithe/run`.
pub struct RunConfigurationDocument {
    /// Document schema version; v1 documents are migrated on read.
    pub version: u32,
    /// Generation record; present only in the generated layer.
    #[serde(default)]
    pub generator: Option<GeneratorMetadata>,
    /// Entries in document order; later layers override by `id`.
    #[serde(default)]
    pub configurations: Vec<RunConfiguration>,
}

/// Rewrites a v1 document in place into the v2 shape.
///
/// Ids are preserved verbatim. The three-layer merge matches configurations by
/// id, so rewriting them would silently detach every override a user wrote in
/// the team or local layer -- a failure with no error message.
pub fn migrate_document_value(document: &mut Value) -> bool {
    if document.get("version").and_then(Value::as_u64) != Some(LEGACY_VERSION as u64) {
        return false;
    }
    if let Some(items) = document
        .get_mut("configurations")
        .and_then(Value::as_array_mut)
    {
        for item in items {
            migrate_configuration_value(item);
        }
    }
    document["version"] = json!(VERSION);
    true
}

fn migrate_configuration_value(item: &mut Value) {
    let Some(object) = item.as_object_mut() else {
        return;
    };
    // v1 entries kept tool-specific launch fields at the top level. Core no
    // longer interprets any of them; they stay inert because RunConfiguration
    // ignores unknown keys, and the owning provider regenerates its payload.
    let legacy_type = object
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_string))
        .filter(|value| valid_provider(value))
        .unwrap_or_else(|| "legacy.unknown".to_string());
    if let Some(working_directory) = object.remove("workingDirectory") {
        object.insert("cwd".to_string(), working_directory);
    }
    object.insert("provider".to_string(), json!(legacy_type));
    object.insert("execution".to_string(), json!("task"));
    object.insert("confidence".to_string(), json!("native"));
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Fingerprint and inputs used to decide whether generated output is stale.
///
/// `inputs` maps each workspace-relative input to `sha256:<hex>` of its bytes.
pub struct GeneratorMetadata {
    /// Combined hash over `inputs` and the generator revision.
    pub fingerprint: String,
    /// Identity of generic run declarations and opaque targets, separate from
    /// the file inputs so a provider answer alone can mark the layer stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_run_fingerprint: Option<String>,
    /// Unreadable inputs retained as unknown; a partial baseline cannot prove freshness.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unreadable_paths: BTreeSet<String>,
    /// Workspace-relative input path mapped to `sha256:<hex>` of its bytes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Debug adapter supported by a runnable configuration.
pub struct DebugCapability {
    /// Stable adapter identifier declared by the provider that owns the entry.
    pub adapter: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Ecosystem-neutral command, identity, and launch metadata for one runnable item.
pub struct RunConfiguration {
    /// Stable identity the team and local layers use to override this entry.
    pub id: String,
    /// Display name shown in the Run list.
    pub name: String,
    /// Open namespaced discriminator, e.g. `npm.script`, `cargo.run`.
    /// Deliberately not an enum: new ecosystems must not require a contract change.
    pub provider: String,
    /// How the entry behaves once started.
    #[serde(default)]
    pub execution: Execution,
    /// Whether the entry runs this project or the infrastructure it depends on.
    /// Omitted for project entries so existing documents stay byte-identical.
    #[serde(default, skip_serializing_if = "RunCategory::is_project")]
    pub category: RunCategory,
    /// Executable to start; absent for groups and provider-prepared launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments passed to `command`, without shell interpretation.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory relative to the workspace root.
    #[serde(default = "dot")]
    pub cwd: String,
    /// Environment variables added for this entry.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// How the entry was discovered; higher confidence wins deduplication.
    #[serde(default)]
    pub confidence: Confidence,
    /// Toolchain role mapped to the requirement identity it consumes, such as
    /// `runtime` to `project-node`.
    #[serde(default)]
    pub toolchains: BTreeMap<String, String>,
    /// Debug adapter that can launch this entry, when one is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugCapability>,
    /// Configuration identities started together when `execution` is `group`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// Ecosystem-specific payload keyed by namespace (`npm`, `build`, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
    /// Hidden from the resolved list; set by a user or when the working directory is missing.
    #[serde(default)]
    pub disabled: bool,
    /// Workspace-relative manifest or source file that produced the configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn dot() -> String {
    ".".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Versioned requirements written separately from run configurations.
pub struct ToolchainRequirementsDocument {
    /// Document version the reader must accept before trusting any requirement.
    pub version: u32,
    /// Requirement per toolchain kind, keyed by the kind the caller asks about.
    #[serde(default)]
    pub toolchains: BTreeMap<String, ToolchainRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Constraints the host uses when selecting one toolchain candidate.
pub struct ToolchainRequirement {
    /// Toolchain role matched against [`ToolchainCandidate::kind`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Lowest acceptable version; newer releases also satisfy it.
    #[serde(default)]
    pub minimum_version: Option<String>,
    /// Vendor preferred when several candidates satisfy the version.
    #[serde(default)]
    pub preferred_vendor: Option<String>,
    /// Exact version prefix; [`Self::minimum_version`] accepts newer releases.
    #[serde(default)]
    pub version: Option<String>,
}
