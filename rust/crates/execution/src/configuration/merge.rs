//! Merging the shared, local and detected layers of a configuration.

use super::*;

pub(crate) fn merge_values(
    base: &Value,
    team: &Value,
    local: &Value,
) -> Result<Vec<RunConfiguration>, CoreError> {
    let mut result: BTreeMap<String, Value> = BTreeMap::new();
    let mut saved = BTreeMap::<String, Value>::new();
    let mut complete_ids = BTreeSet::new();
    for source in [team, local] {
        for item in source["configurations"].as_array().into_iter().flatten() {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            if crate::provider_runs::is_explicit(item) {
                complete_ids.insert(id.to_string());
            }
            if let Some(previous) = saved.get_mut(id) {
                merge_configuration_value(previous, item)?;
            } else {
                saved.insert(id.to_string(), item.clone());
            }
        }
    }
    let explicit_selectors = saved
        .values()
        .filter(|item| {
            crate::provider_runs::is_explicit(item)
                && item["id"]
                    .as_str()
                    .is_some_and(|id| complete_ids.contains(id))
        })
        .filter_map(crate::provider_runs::selector)
        .collect::<BTreeSet<_>>();
    for (source_index, source) in [base, team, local].into_iter().enumerate() {
        let Some(items) = source.get("configurations").and_then(Value::as_array) else {
            return Err(CoreError::new(
                ErrorCode::ParseFailed,
                "configurations must be an array",
            ));
        };
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                return Err(CoreError::new(
                    ErrorCode::ParseFailed,
                    "Run configuration id is required",
                ));
            };
            // A complete saved selector supersedes only its automatically generated
            // counterpart. Distinct user configurations for that target remain distinct.
            if source_index == 0
                && crate::provider_runs::selector(item)
                    .is_some_and(|selector| explicit_selectors.contains(&selector))
            {
                continue;
            }
            if source_index > 0
                && !result.contains_key(id)
                && !id.starts_with("user:")
                && !crate::provider_runs::is_explicit(item)
            {
                continue;
            }
            if let Some(existing) = result.get_mut(id) {
                merge_configuration_value(existing, item)?;
                let target = existing
                    .as_object_mut()
                    .expect("merged configuration is an object");
                target.insert(
                    "source".to_string(),
                    json!(["generated", "project", "local"][source_index]),
                );
            } else {
                let mut value = item.clone();
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "source".to_string(),
                        json!(["generated", "project", "local"][source_index]),
                    );
                }
                result.insert(id.to_string(), value);
            }
        }
    }
    result
        .into_values()
        .map(|item| {
            serde_json::from_value::<RunConfiguration>(item).map_err(|e| {
                CoreError::new(ErrorCode::ParseFailed, "Invalid merged run configuration")
                    .with_details(e.to_string())
            })
        })
        .collect()
}

/// Applies one sparse configuration patch using the same rules for claim discovery and resolution.
fn merge_configuration_value(existing: &mut Value, item: &Value) -> Result<(), CoreError> {
    let (Some(target), Some(patch)) = (existing.as_object_mut(), item.as_object()) else {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Run configuration must be an object",
        ));
    };
    for (key, value) in patch {
        if key == "source" {
            continue;
        }
        if key == "extensions" {
            merge_extensions(target, &mut value.clone())?;
            continue;
        }
        if key == "toolchains" {
            let target_map = target
                .entry(key.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::ParseFailed,
                        "Run configuration toolchains must be an object",
                    )
                })?;
            let patch_map = value.as_object().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::ParseFailed,
                    "Run configuration toolchains must be an object",
                )
            })?;
            for (toolchain_kind, toolchain_id) in patch_map {
                target_map.insert(toolchain_kind.clone(), toolchain_id.clone());
            }
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

/// Reconciles runtime bindings from the effective command for generated v2
/// documents written before detectors declared their consumption explicitly.
pub(crate) fn normalize_runtime_consumption(configurations: &mut [RunConfiguration]) {
    for configuration in configurations {
        let command = configuration
            .command
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            command.as_str(),
            "npm" | "npm.cmd" | "pnpm" | "pnpm.cmd" | "yarn" | "yarn.cmd"
        ) {
            configuration
                .toolchains
                .entry("runtime".to_string())
                .or_insert_with(|| "project-node".to_string());
        } else if matches!(command.as_str(), "bun" | "bun.exe")
            && configuration.toolchains.get("runtime").map(String::as_str) == Some("project-node")
        {
            configuration.toolchains.remove("runtime");
        }
    }
}

/// Deep-merges the `extensions` object one namespace at a time.
///
/// A shallow insert would let a layer that touches a single `build-run` key
/// drop the whole `npm` namespace it never mentioned.
pub(crate) fn merge_extensions(
    target: &mut serde_json::Map<String, Value>,
    patch: &mut Value,
) -> Result<(), CoreError> {
    let patch_map = patch.as_object().ok_or_else(|| {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Run configuration extensions must be an object",
        )
    })?;
    let target_map = target
        .entry("extensions".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "Run configuration extensions must be an object",
            )
        })?;
    for (namespace, value) in patch_map {
        let slot = target_map
            .entry(namespace.clone())
            .or_insert_with(|| json!({}));
        match (slot.as_object_mut(), value.as_object()) {
            (Some(existing), Some(incoming)) => {
                for (key, item) in incoming {
                    if namespace == "build-run" && key == "tools" {
                        let tools = item.as_object().ok_or_else(|| {
                            CoreError::new(
                                ErrorCode::ParseFailed,
                                "Run tool selections must be an object",
                            )
                        })?;
                        let slot = existing.entry(key.clone()).or_insert_with(|| json!({}));
                        let destination = slot.as_object_mut().ok_or_else(|| {
                            CoreError::new(
                                ErrorCode::ParseFailed,
                                "Run tool selections must be an object",
                            )
                        })?;
                        destination.extend(tools.clone());
                        continue;
                    }
                    existing.insert(key.clone(), item.clone());
                }
            }
            _ => {
                *slot = value.clone();
            }
        }
    }
    Ok(())
}

pub(crate) fn configuration_ids(document: &Value) -> Result<BTreeMap<String, ()>, CoreError> {
    let Some(items) = document.get("configurations").and_then(Value::as_array) else {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "configurations must be an array",
        ));
    };
    let mut result = BTreeMap::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return Err(CoreError::new(
                ErrorCode::ParseFailed,
                "Run configuration id is required",
            ));
        };
        result.insert(id.to_string(), ());
    }
    Ok(result)
}
