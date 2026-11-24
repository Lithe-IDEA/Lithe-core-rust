//! Validates and materializes generated run snapshots from current build models.
//! Tool, framework, runtime and configuration semantics remain with the selected plugin.

use super::configuration::{
    BuildProviderRunReference, RunConfiguration, RunTypeReference, RUN_TYPE_PROVIDER,
};
use super::types::{Confidence, Execution, RunCategory};
use lithe_platform::paths::invalid_portable_relative_path;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// One generated run entry derived from a loaded build model, independent of
/// language imports.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ProviderRunConfiguration {
    /// Opaque build provider run selector (`build.provider`).
    Provider(ProviderRunSelector),
    /// Generated entry of a plugin-declared `runConfigurationTypes` kind (`build.run-type`).
    RunType(GeneratedRunTypeConfiguration),
}

/// One declared provider run applied to an upstream target (`build.provider`).
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRunSelector {
    /// Workspace-relative current model project; never inferred from a descriptor.
    pub project_path: String,
    pub name: String,
    pub reference: BuildProviderRunReference,
}

/// One generated entry of a plugin-declared `runConfigurationTypes` kind (`build.run-type`).
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneratedRunTypeConfiguration {
    /// Workspace-relative current model project.
    pub project_path: String,
    pub name: String,
    /// Workspace-relative working directory (`.` for the workspace root).
    pub cwd: String,
    /// Execution mode declared by the target `runConfigurationTypes` kind.
    #[serde(default)]
    pub execution: Execution,
    /// Stable model selector identity (for example the model run entry id).
    pub selector: String,
    /// Declared kind and template-expanded field values.
    pub run_type: RunTypeReference,
}

impl ProviderRunConfiguration {
    /// Deterministic generated configuration ID for this snapshot entry.
    pub(crate) fn generated_id(&self) -> String {
        match self {
            Self::Provider(run) => run.reference.generated_id(),
            Self::RunType(run) => generated_run_type_id(&run.run_type, &run.cwd, &run.selector),
        }
    }
}

fn generated_run_type_id(run_type: &RunTypeReference, cwd: &str, selector: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(
        serde_json::to_vec(&json!({
            "pluginID": run_type.plugin_id,
            "buildSystemID": run_type.build_system_id,
            "type": run_type.type_id,
            "cwd": cwd,
            "selector": selector,
        }))
        .expect("validated run-type identity encodes"),
    );
    let mut hex = String::with_capacity(16);
    for byte in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    format!("generated:run-type:{hex}")
}

fn contains(root: &str, path: &str) -> bool {
    root == "." || root == path || path.starts_with(&(root.to_string() + "/"))
}

/// Validates a complete run snapshot without reading tool configuration or starting processes.
pub(super) fn validate(runs: &[ProviderRunConfiguration]) -> Result<(), CoreError> {
    let invalid = || CoreError::new(ErrorCode::InvalidRequest, "Invalid provider run snapshot");
    if runs.len() > 10000 {
        return Err(invalid());
    }
    let mut identities = BTreeSet::new();
    for run in runs {
        match run {
            ProviderRunConfiguration::Provider(run) => {
                run.reference.validate()?;
                if run.name.trim().is_empty()
                    || run.name.len() > 4096
                    || run.name.chars().any(char::is_control)
                    || invalid_portable_relative_path(&run.project_path)
                    || run.project_path.len() > 4096
                    || !contains(&run.reference.root, &run.project_path)
                    || !run.reference.user_arguments.is_empty()
                    || !identities.insert(run.reference.generated_id())
                {
                    return Err(invalid());
                }
            }
            ProviderRunConfiguration::RunType(run) => {
                run.run_type.validate()?;
                if run.name.trim().is_empty()
                    || run.name.len() > 4096
                    || run.name.chars().any(char::is_control)
                    || invalid_portable_relative_path(&run.project_path)
                    || run.project_path.len() > 4096
                    || invalid_portable_relative_path(&run.cwd)
                    || run.cwd.len() > 4096
                    || !contains(&run.cwd, &run.project_path)
                    || run.selector.trim().is_empty()
                    || run.selector.len() > 4096
                    || run.selector.chars().any(char::is_control)
                    || matches!(run.execution, Execution::Service | Execution::Group)
                    || !identities.insert(generated_run_type_id(
                        &run.run_type,
                        &run.cwd,
                        &run.selector,
                    ))
                {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

/// Materializes selectors without inventing commands, toolchains or adapter identities.
pub(super) fn configurations(runs: &[ProviderRunConfiguration]) -> Vec<RunConfiguration> {
    let mut ordered = runs.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|run| run.generated_id());
    ordered
        .into_iter()
        .map(|run| match run {
            ProviderRunConfiguration::Provider(run) => RunConfiguration {
                id: run.reference.generated_id(),
                name: run.name.clone(),
                folder: None,
                provider: "build.provider".to_string(),
                execution: run.reference.execution.unwrap_or_default(),
                category: RunCategory::Project,
                command: None,
                args: Vec::new(),
                cwd: run.reference.root.clone(),
                env: BTreeMap::new(),
                confidence: Confidence::Declared,
                toolchains: BTreeMap::new(),
                debug: None,
                members: Vec::new(),
                extensions: [(
                    "build".to_string(),
                    serde_json::to_value(&run.reference).expect("validated reference encodes"),
                )]
                .into_iter()
                .collect(),
                before_launch: Vec::new(),
                disabled: false,
                allow_multiple_instances: false,
                source: None,
                plugin: None,
                availability: None,
            },
            ProviderRunConfiguration::RunType(run) => RunConfiguration {
                id: generated_run_type_id(&run.run_type, &run.cwd, &run.selector),
                name: run.name.clone(),
                folder: None,
                provider: RUN_TYPE_PROVIDER.to_string(),
                execution: run.execution,
                category: RunCategory::Project,
                command: None,
                args: Vec::new(),
                cwd: run.cwd.clone(),
                env: BTreeMap::new(),
                confidence: Confidence::Declared,
                toolchains: BTreeMap::new(),
                debug: None,
                members: Vec::new(),
                extensions: [(
                    "build".to_string(),
                    serde_json::to_value(&run.run_type)
                        .expect("validated run type reference encodes"),
                )]
                .into_iter()
                .collect(),
                before_launch: Vec::new(),
                disabled: false,
                allow_multiple_instances: false,
                source: None,
                plugin: None,
                availability: None,
            },
        })
        .collect()
}

/// Identity of the complete declaration/target snapshot, independent of input ordering.
pub(super) fn fingerprint(runs: &[ProviderRunConfiguration]) -> String {
    use sha2::{Digest, Sha256};
    let mut ordered = runs.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|run| run.generated_id());
    let bytes = serde_json::to_vec(&ordered).expect("validated run snapshot encodes");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Complete saved references own their identity; an ID prefix is not an execution capability.
/// Validation still rejects malformed selectors after layer merging.
pub(super) fn is_explicit(item: &Value) -> bool {
    item["provider"] == "build.provider" && item["id"].is_string() && item["name"].is_string()
}

/// Opaque selector identity for deduplication, independent of user choices or display names.
pub(super) fn selector(item: &Value) -> Option<String> {
    (item["provider"] == "build.provider").then_some(())?;
    let reference: BuildProviderRunReference =
        serde_json::from_value(item["extensions"]["build"].clone()).ok()?;
    Some(reference.generated_id())
}
