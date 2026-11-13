//! Toolchain requirements of a project and their diagnostics.

use super::*;

/// Toolchain requirements of `root`: those of this session's latest
/// generation, else those derived from the project's own declarations, so a
/// runtime can be checked before the Run list has been generated.
pub fn read_requirements(root: &Path) -> Result<Option<ToolchainRequirementsDocument>, CoreError> {
    if let Some(layer) = generated_layer::get(root) {
        return Ok(Some(layer.requirements.clone()));
    }
    if !root.is_dir() {
        return Ok(None);
    }
    detect_requirements(root, &[]).map(Some)
}

pub(crate) fn read_local_toolchains(root: &Path) -> Result<Option<Value>, CoreError> {
    let Some(document) = read_document_value(root, "toolchains/local.json")? else {
        return Ok(None);
    };
    validate_sidecar_version_value(&document)?;
    if document
        .get("toolchains")
        .and_then(Value::as_object)
        .is_none()
    {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Local toolchains must contain a toolchains object",
        ));
    }
    Ok(Some(document))
}

/// Only roles a Run actually consumes become requirements; Core reads the
/// generic declarations it understands and leaves tool-specific version files to
/// the plugin that owns the toolchain.
pub(crate) fn detect_requirements(
    root: &Path,
    configurations: &[RunConfiguration],
) -> Result<ToolchainRequirementsDocument, CoreError> {
    let mut toolchains = BTreeMap::new();
    let consumes = |toolchain: &str| {
        configurations.iter().any(|configuration| {
            configuration
                .toolchains
                .values()
                .any(|candidate| candidate == toolchain)
        })
    };
    if consumes("project-node") {
        toolchains.insert(
            "project-node".to_string(),
            generic_requirement("node", declared_node_version(root)),
        );
    }
    Ok(ToolchainRequirementsDocument {
        version: SIDECAR_VERSION,
        toolchains,
    })
}

fn generic_requirement(kind: &str, minimum_version: Option<String>) -> ToolchainRequirement {
    ToolchainRequirement {
        kind: kind.to_string(),
        minimum_version,
        preferred_vendor: None,
        version: None,
    }
}

pub(crate) fn toolchain_diagnostics(
    root: &Path,
    candidates: &[ToolchainCandidate],
    configurations: &[RunConfiguration],
) -> Result<Vec<Value>, CoreError> {
    let Some(requirements) = read_requirements(root)? else {
        return Ok(Vec::new());
    };
    validate_sidecar_version(requirements.version)?;
    let mut diagnostics = Vec::new();
    for (id, requirement) in requirements.toolchains {
        let mut consumer_ids = configurations
            .iter()
            .filter(|configuration| {
                configuration
                    .toolchains
                    .values()
                    .any(|toolchain| toolchain == &id)
            })
            .map(|configuration| configuration.id.clone())
            .collect::<Vec<_>>();
        consumer_ids.sort();
        consumer_ids.dedup();
        let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.id == id && candidate.kind == requirement.kind)
        else {
            append_toolchain_diagnostics(
                &mut diagnostics,
                &consumer_ids,
                json!({
                    "code": "missingToolchain",
                    "toolchain": id,
                    "message": format!("No local {} toolchain is selected", requirement.kind)
                }),
            );
            continue;
        };
        let required_version = requirement
            .minimum_version
            .as_deref()
            .or(requirement.version.as_deref());
        if let Some(required) = required_version {
            // `version` is an exact prefix match; only `minimumVersion` accepts
            // newer releases.
            let treat_as_minimum = requirement.minimum_version.is_some();
            if !version_satisfies(&candidate.version, required, treat_as_minimum) {
                append_toolchain_diagnostics(
                    &mut diagnostics,
                    &consumer_ids,
                    json!({
                        "code": "toolchainVersionMismatch",
                        "toolchain": id,
                        "message": format!(
                            "{} {} does not satisfy required version {}",
                            requirement.kind, candidate.version, required
                        )
                    }),
                );
            }
        }
        if let Some(vendor) = requirement.preferred_vendor.as_deref() {
            if !candidate
                .vendor
                .to_lowercase()
                .contains(&vendor.to_lowercase())
            {
                append_toolchain_diagnostics(
                    &mut diagnostics,
                    &consumer_ids,
                    json!({
                        "code": "toolchainVendorMismatch",
                        "toolchain": id,
                        "message": format!("Preferred {} vendor is {vendor}", requirement.kind)
                    }),
                );
            }
        }
    }
    Ok(diagnostics)
}

fn append_toolchain_diagnostics(
    diagnostics: &mut Vec<Value>,
    consumer_ids: &[String],
    diagnostic: Value,
) {
    if consumer_ids.is_empty() {
        return;
    }
    for configuration_id in consumer_ids {
        let mut scoped = diagnostic.clone();
        scoped["id"] = json!(configuration_id);
        diagnostics.push(scoped);
    }
}

/// Compares numeric version components. A minimum accepts any later version;
/// otherwise `required` must be a component prefix of `actual` (`20` matches
/// `20.11.1`). Tool-specific version schemes are normalized by their owner first.
pub fn version_satisfies(actual: &str, required: &str, minimum: bool) -> bool {
    let actual_parts = version_parts(actual);
    let required_parts = version_parts(required);
    if actual_parts.is_empty() || required_parts.is_empty() {
        return false;
    }
    if minimum {
        actual_parts >= required_parts
    } else {
        actual_parts.starts_with(&required_parts)
    }
}

/// Splits a version into its numeric parts, ignoring separators and suffixes.
///
/// An input with no digits yields an empty list, which callers treat as "not a
/// version" rather than as zero.
pub fn version_parts(value: &str) -> Vec<u32> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}
