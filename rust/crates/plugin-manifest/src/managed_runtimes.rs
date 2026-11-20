//! Pinned runtime declarations and deterministic installation plans. Plugins own
//! tool-specific qualification; hosts own downloads, filesystem evidence and
//! atomic publication outside immutable plugin packages.

use super::archives::{
    bounded_text, is_archive_layout_path, is_archive_platform, validate_archive_source,
    ArchivePlatform as RuntimePlatform, ArchiveSource, MAX_ARTIFACTS,
};
use super::*;
use lithe_platform::paths;
use lithe_plugin_model::is_function_id;
use sha2::{Digest, Sha256};

const MAX_DEPENDENCIES: usize = 16;
const DEFAULT_RESOLVE_TIMEOUT: u64 = 30_000;
const MAX_RESOLVE_TIMEOUT: u64 = 120_000;
const MAX_PATH_BYTES: usize = 4096;

/// A plugin-owned dependency set. One resolver qualifies both user installations
/// and newly staged archives without publishing or downloading anything itself.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedRuntimes {
    resolve: RuntimeResolver,
    dependencies: Vec<RuntimeDependency>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeResolver {
    /// The existing plugin backend function invoked through litheFunctions.
    function: String,
    #[serde(default = "default_resolve_timeout")]
    timeout_milliseconds: u64,
}

fn default_resolve_timeout() -> u64 {
    DEFAULT_RESOLVE_TIMEOUT
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeDependency {
    id: String,
    /// Opaque upstream identity, independent of the plugin's own version.
    version: String,
    /// An optional plugin-owned tool used only by this plugin's server launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    artifacts: Vec<RuntimeArtifact>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeArtifact {
    /// Exact target: macos, windows or linux, and arm64 or x86_64.
    os: String,
    arch: String,
    url: String,
    /// Optional fallback HTTPS URLs serving the exact same pinned SHA-256 archive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mirrors: Vec<String>,
    sha256: String,
    /// zip or tar.gz; the host never infers a format from a URL suffix.
    archive: String,
    /// Exact archive root removed while extracting; empty removes nothing.
    #[serde(default)]
    strip_prefix: String,
    /// Runtime home relative to the stripped extraction root; empty is its root.
    #[serde(default)]
    home: String,
    /// Tool executable relative to home. Required exactly when tool is declared.
    #[serde(default)]
    executable: String,
}

/// Rejects a platform no runtime archive may target.
fn validate_platform(platform: &RuntimePlatform) -> Validation<()> {
    if !platform.is_supported() {
        return Err(PluginRejection::at(
            "plugin.managedRuntime.unavailable",
            "platform",
            "Managed runtimes require a supported operating system and architecture.",
        ));
    }
    Ok(())
}

impl DeclarativePluginManifest {
    pub(crate) fn validate_managed_runtimes(&mut self) -> Validation<()> {
        let Some(runtimes) = &mut self.managed_runtimes else {
            return Ok(());
        };
        let Some(backend) = &self.backend else {
            return Err(invalid(
                "managedRuntimes.resolve",
                "A managed-runtime resolver needs the plugin's backend.",
            ));
        };
        // Preflight runs before installation is committed. Its backend cannot
        // itself depend on one of the runtimes that preflight is preparing.
        if !matches!(
            backend.process.launch,
            LanguageServerLaunch::Packaged { .. }
                | LanguageServerLaunch::Runtime {
                    packaged_runtime: Some(_),
                    ..
                }
        ) || !backend.process.runtime_requirements().is_empty()
        {
            return Err(invalid(
                "managedRuntimes.resolve",
                "The resolver backend must carry its executable and any runtime it needs.",
            ));
        }
        if !is_function_id(&runtimes.resolve.function)
            || runtimes.resolve.function.len() > 128
            || !(100..=MAX_RESOLVE_TIMEOUT).contains(&runtimes.resolve.timeout_milliseconds)
        {
            return Err(invalid(
                "managedRuntimes.resolve",
                "A resolver names a function and a timeout from 100 to 120000 milliseconds.",
            ));
        }
        if !(1..=MAX_DEPENDENCIES).contains(&runtimes.dependencies.len()) {
            return Err(invalid(
                "managedRuntimes.dependencies",
                "A managed runtime set contains from 1 to 16 dependencies.",
            ));
        }
        let managed_runtimes_version = self
            .requires
            .get(capabilities::MANAGED_RUNTIMES)
            .copied()
            .unwrap_or(0);
        let tool_ids: BTreeSet<&str> = self
            .tool_definitions
            .iter()
            .filter_map(|definition| definition["id"].as_str())
            .collect();
        let mut ids = BTreeSet::new();
        let mut bound_tools = BTreeSet::new();
        for (index, dependency) in runtimes.dependencies.iter_mut().enumerate() {
            let field = format!("managedRuntimes.dependencies[{index}]");
            if !is_identifier(&dependency.id)
                || dependency.id.len() > 64
                || !ids.insert(dependency.id.clone())
            {
                return Err(invalid(
                    format!("{field}.id"),
                    "Runtime IDs are unique lowercase letters, digits and hyphens, at most 64 bytes.",
                ));
            }
            if !bounded_text(&dependency.version, 128) {
                return Err(invalid(
                    format!("{field}.version"),
                    "A runtime version is a nonempty upstream identity of at most 128 bytes.",
                ));
            }
            if let Some(tool) = &dependency.tool {
                if !tool_ids.contains(tool.as_str())
                    || !self.requires_tools.contains(tool)
                    || !bound_tools.insert(tool.clone())
                {
                    return Err(invalid(
                        format!("{field}.tool"),
                        "A runtime binds one unique tool declared and required by this plugin.",
                    ));
                }
            }
            if !(1..=MAX_ARTIFACTS).contains(&dependency.artifacts.len()) {
                return Err(invalid(
                    format!("{field}.artifacts"),
                    "A dependency declares from 1 to 6 platform artifacts.",
                ));
            }
            let mut targets = BTreeSet::new();
            for (index, artifact) in dependency.artifacts.iter_mut().enumerate() {
                let field = format!("{field}.artifacts[{index}]");
                if !is_archive_platform(&artifact.os, &artifact.arch)
                    || !targets.insert((artifact.os.clone(), artifact.arch.clone()))
                {
                    return Err(invalid(
                        &field,
                        "Artifacts have unique supported operating-system and architecture pairs.",
                    ));
                }
                validate_archive_source(
                    &field,
                    ArchiveSource {
                        url: &artifact.url,
                        mirrors: &artifact.mirrors,
                        sha256: &mut artifact.sha256,
                        archive: &artifact.archive,
                    },
                    || {
                        if managed_runtimes_version < 1 {
                            return Err(invalid(
                                "requires.managedRuntimes",
                                "Managed runtime mirror URLs require requires.managedRuntimes >= 1.",
                            ));
                        }
                        Ok(())
                    },
                )?;
                for (name, path) in [
                    ("stripPrefix", &artifact.strip_prefix),
                    ("home", &artifact.home),
                    ("executable", &artifact.executable),
                ] {
                    if !is_archive_layout_path(path) {
                        return Err(invalid(
                            format!("{field}.{name}"),
                            "Runtime paths are bounded relative paths without traversal.",
                        ));
                    }
                }
                if dependency.tool.is_some() == artifact.executable.is_empty() {
                    return Err(invalid(
                        format!("{field}.executable"),
                        "An executable relative to home is required exactly for a tool-bound runtime.",
                    ));
                }
            }
            dependency
                .artifacts
                .sort_by(|left, right| (&left.os, &left.arch).cmp(&(&right.os, &right.arch)));
        }
        runtimes
            .dependencies
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }
}

impl ManagedRuntimes {
    pub(crate) fn programs(&self) -> Vec<Value> {
        self.dependencies
            .iter()
            .map(|dependency| {
                json!({
                    "kind": "managedRuntime",
                    "dependency": dependency,
                    "resolve": self.resolve,
                })
            })
            .collect()
    }

    fn plan(&self, platform: &RuntimePlatform) -> Validation<Value> {
        let mut dependencies = Vec::new();
        for dependency in &self.dependencies {
            let Some(artifact) = dependency
                .artifacts
                .iter()
                .find(|artifact| platform.matches(&artifact.os, &artifact.arch))
            else {
                return Err(PluginRejection::at(
                    "plugin.managedRuntime.unavailable",
                    format!("managedRuntimes.dependencies.{}", dependency.id),
                    "A required runtime has no artifact for this operating system and architecture.",
                ));
            };
            let mut selected = json!({
                "id": dependency.id,
                "version": dependency.version,
                "os": artifact.os,
                "arch": artifact.arch,
                "url": artifact.url,
                "sha256": artifact.sha256,
                "archive": artifact.archive,
                "stripPrefix": artifact.strip_prefix,
                "home": artifact.home,
                "executable": artifact.executable,
            });
            if let Some(tool) = &dependency.tool {
                selected["tool"] = json!(tool);
            }
            // Serialize this explicitly constructed object, not input field order.
            // Package versions and mirrors are absent, so mirror additions reuse bytes.
            let identity = Sha256::digest(selected.to_string().as_bytes());
            selected["cacheKey"] = json!(format!("{identity:x}"));
            if !artifact.mirrors.is_empty() {
                selected["mirrors"] = json!(artifact.mirrors);
            }
            dependencies.push(selected);
        }
        Ok(json!({"resolve": self.resolve, "dependencies": dependencies}))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanRequest {
    manifest: Value,
    platform: RuntimePlatform,
}

/// `plugins.managedRuntimes.plan` validates the manifest and selects exact pinned
/// artifacts. It performs no I/O and does not choose a tool-specific installation.
pub fn managed_runtime_plan_command(payload: Value) -> Result<Value, CoreError> {
    let request: PlanRequest = decode_payload(payload, "plugins.managedRuntimes.plan")?;
    Ok(outcome(validate_manifest_value(request.manifest).and_then(
        |manifest| {
            validate_platform(&request.platform)?;
            let plan = match manifest.managed_runtimes {
                Some(runtimes) => runtimes.plan(&request.platform)?,
                None => json!({"dependencies": []}),
            };
            Ok(("plan", plan))
        },
    )))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolvedRuntime {
    id: String,
    /// A qualified external or staged native home; the host never writes into
    /// an external home or replaces the user's project toolchain with it.
    home: String,
    version: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeResolution {
    runtimes: Vec<ResolvedRuntime>,
    missing: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolutionRequest {
    manifest: Value,
    platform: RuntimePlatform,
    result: Value,
}

fn invalid_resolution(message: impl Into<String>) -> PluginRejection {
    PluginRejection::at("plugin.managedRuntime.invalidResolution", "result", message)
}

/// `plugins.managedRuntimes.decodeResolution` binds a resolver's answer to the
/// declared dependency IDs. Compatibility remains the plugin's responsibility;
/// the host verifies native evidence before publishing the complete set.
pub fn managed_runtime_resolution_command(payload: Value) -> Result<Value, CoreError> {
    let request: ResolutionRequest =
        decode_payload(payload, "plugins.managedRuntimes.decodeResolution")?;
    Ok(outcome(validate_manifest_value(request.manifest).and_then(|manifest| {
        validate_platform(&request.platform)?;
        let Some(declaration) = manifest.managed_runtimes else {
            return Err(invalid_resolution("The plugin declares no managed runtime resolver."));
        };
        declaration.plan(&request.platform)?;
        let mut result: RuntimeResolution = serde_json::from_value(request.result)
            .map_err(|_| invalid_resolution("The runtime resolver answer has an invalid shape."))?;
        let expected: BTreeSet<&str> = declaration
            .dependencies
            .iter()
            .map(|dependency| dependency.id.as_str())
            .collect();
        if result.runtimes.len() + result.missing.len() != expected.len() {
            return Err(invalid_resolution("The answer must account for every declared runtime exactly once."));
        }
        let mut seen = BTreeSet::new();
        for runtime in &result.runtimes {
            if !expected.contains(runtime.id.as_str()) || !seen.insert(runtime.id.as_str()) {
                return Err(invalid_resolution("The answer contains an undeclared or repeated runtime ID."));
            }
            if !bounded_text(&runtime.home, MAX_PATH_BYTES)
                || !paths::is_absolute_path(&runtime.home, request.platform.native_paths())
                || runtime.home.split(['/', '\\']).any(|part| part == "." || part == "..")
                || !bounded_text(&runtime.version, 128)
            {
                return Err(invalid_resolution("A resolved runtime needs an absolute native home and a bounded nonempty version."));
            }
        }
        for id in &result.missing {
            if !expected.contains(id.as_str()) || !seen.insert(id.as_str()) {
                return Err(invalid_resolution("The answer contains an undeclared or repeated missing runtime ID."));
            }
        }
        result.runtimes.sort_by(|left, right| left.id.cmp(&right.id));
        result.missing.sort();
        Ok(("resolution", json!(result)))
    })))
}

#[cfg(test)]
mod tests;
