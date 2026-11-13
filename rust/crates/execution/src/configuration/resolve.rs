//! Resolving the configuration a launch uses.

use super::*;

/// Merges generated, team, and local layers and selects compatible toolchains.
pub fn resolve(request: ResolveRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let team = match request.team_document {
        Some(mut document) => {
            migrate_document_value(&mut document);
            document
        }
        None => read_document_value(&root, "run/configurations.json")?
            .unwrap_or_else(|| json!({"version": VERSION, "configurations": []})),
    };
    let local = local_layer_document(&root, request.local_document)?;
    let layer = generated_layer::get(&root);
    let generated = match layer.as_ref() {
        Some(layer) => serde_json::to_value(&layer.document).expect("run configurations encode"),
        None if has_saved_build_run(&local) || has_saved_build_run(&team) => {
            json!({"version": VERSION, "configurations": []})
        }
        None => {
            return Err(CoreError::new(
                ErrorCode::WorkspaceNotFound,
                "Run configuration has not been generated",
            ))
        }
    };
    let local_toolchains = read_local_toolchains(&root)?;
    let manifest = read_document_value(&root, "project.json")?;
    validate_version_value(&generated)?;
    validate_version_value(&team)?;
    validate_version_value(&local)?;
    if let Some(manifest) = manifest.as_ref() {
        validate_sidecar_version_value(manifest)?;
    }
    configuration_ids(&generated)?;
    let mut configurations = merge_values(&generated, &team, &local)?;
    let resolved_ids = configurations
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut diagnostics = Vec::new();
    for source in [&team, &local] {
        for id in configuration_ids(source)?.keys() {
            if !resolved_ids.contains(id.as_str()) && !id.starts_with("user:") {
                diagnostics.push(json!({
                    "id": id,
                    "code": "orphanedOverride",
                    "message": "Override no longer matches an automatically generated configuration"
                }));
            }
        }
    }
    normalize_runtime_consumption(&mut configurations);
    let global_toolchain = local.get("toolchain").cloned();
    diagnostics.extend(toolchain_diagnostics(
        &root,
        &request.toolchain_candidates,
        &configurations,
    )?);
    for configuration in &mut configurations {
        validate_configuration(configuration)?;
        if configuration.provider == "build.provider" {
            // Materialize optional defaults in the shared output so native
            // consumers receive the same shape as a newly saved selector.
            let reference: BuildProviderRunReference = serde_json::from_value(
                configuration.extensions["build"].clone(),
            )
            .map_err(|_| {
                CoreError::new(
                    ErrorCode::ParseFailed,
                    "Invalid build provider run reference",
                )
            })?;
            configuration.extensions.insert(
                "build".to_string(),
                serde_json::to_value(reference).expect("reference encodes"),
            );
        }
        if configuration.disabled {
            diagnostics.push(json!({
                "id": configuration.id,
                "code": "disabled",
                "message": "Run configuration is disabled"
            }));
            continue;
        }
        if !project_directory_exists(&root, &configuration.cwd) {
            configuration.disabled = true;
            diagnostics.push(json!({
                "id": configuration.id,
                "code": "missingWorkingDirectory",
                "message": format!("Working directory does not exist: {}", configuration.cwd)
            }));
        }
    }
    configurations.retain(|configuration| !configuration.disabled);
    let mut default_run_configuration = manifest
        .as_ref()
        .and_then(|value| value.get("defaultRunConfiguration"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(default_id) = default_run_configuration.as_deref() {
        if !configurations
            .iter()
            .any(|configuration| configuration.id == default_id)
        {
            diagnostics.push(json!({
                "code": "missingDefaultConfiguration",
                "message": format!("Default run configuration is unavailable: {default_id}")
            }));
            default_run_configuration = None;
        }
    }
    if default_run_configuration.is_none() {
        default_run_configuration = layer
            .as_ref()
            .and_then(|layer| detected_default(&layer.document, &configurations));
    }
    Ok(json!({
        "version": VERSION,
        "configurations": configurations,
        "diagnostics": diagnostics,
        "defaultRunConfiguration": default_run_configuration,
        "toolchain": global_toolchain,
        "localToolchains": local_toolchains
    }))
}

/// Prefer a detected service when the project names no default, otherwise
/// select the first available entry.
fn detected_default(
    generated: &RunConfigurationDocument,
    available: &[RunConfiguration],
) -> Option<String> {
    let by_id = available
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<BTreeMap<_, _>>();
    let mut by_selector = BTreeMap::new();
    for configuration in available {
        if let Some(selector) = crate::provider_runs::selector(
            &serde_json::to_value(configuration).expect("configuration encodes"),
        ) {
            by_selector.entry(selector).or_insert(configuration);
        }
    }
    let detected = generated.configurations.iter().filter_map(|configuration| {
        by_id.get(configuration.id.as_str()).copied().or_else(|| {
            let selector = crate::provider_runs::selector(
                &serde_json::to_value(configuration).expect("configuration encodes"),
            )?;
            by_selector.get(&selector).copied()
        })
    });
    detected
        .clone()
        .find(|configuration| configuration.execution == Execution::Service)
        .or_else(|| detected.clone().next())
        .map(|configuration| configuration.id.clone())
}
