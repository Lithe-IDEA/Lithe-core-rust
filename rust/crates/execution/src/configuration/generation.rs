//! Inspecting a project and generating its detected configurations.

use super::*;

/// Validates configuration and sidecar documents without mutating the workspace.
pub fn inspect(request: InspectRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layer = generated_layer::get(&root);
    let generated = layer.as_ref().map(|layer| &layer.document);
    let requirements = layer.as_ref().map(|layer| &layer.requirements);
    let local_toolchains = read_local_toolchains(&root)?;
    for relative in [
        "run/configurations.json",
        "run/local.json",
        "toolchains/local.json",
        "project.json",
    ] {
        if relative == "run/local.json" {
            if let Some(document) = request.local_document.as_ref() {
                let mut migrated = document.clone();
                migrate_document_value(&mut migrated);
                validate_version_value(&migrated)?;
                configuration_ids(&migrated)?;
                continue;
            }
        }
        if let Some(document) = read_document_value(&root, relative)? {
            if relative.starts_with("run/") {
                validate_version_value(&document)?;
                configuration_ids(&document)?;
            } else {
                validate_sidecar_version_value(&document)?;
                if relative == "toolchains/local.json"
                    && document
                        .get("toolchains")
                        .and_then(Value::as_object)
                        .is_none()
                {
                    return Err(CoreError::new(
                        ErrorCode::ParseFailed,
                        "Local toolchains must contain a toolchains object",
                    ));
                }
            }
        }
    }
    let mut diagnostics = Vec::new();
    if request.check_fingerprint == Some(false) {
        if let Some(metadata) = generated.and_then(|document| document.generator.as_ref()) {
            if !metadata.unreadable_paths.is_empty() {
                diagnostics.push(incomplete_scan_diagnostic(&metadata.unreadable_paths));
            }
        }
    }
    let local = local_layer_document(&root, request.local_document.clone())?;
    let team = read_document_value(&root, "run/configurations.json")?
        .unwrap_or_else(|| json!({"version": VERSION, "configurations": []}));
    if let Some(metadata) = generated
        .and_then(|document| document.generator.as_ref())
        .filter(|_| request.check_fingerprint != Some(false))
    {
        let current_inputs = project_inputs(&root)?;
        if !current_inputs.unreadable_paths.is_empty() || !metadata.unreadable_paths.is_empty() {
            let unknown = current_inputs
                .unreadable_paths
                .union(&metadata.unreadable_paths)
                .cloned()
                .collect();
            diagnostics.push(incomplete_scan_diagnostic(&unknown));
        } else if metadata.fingerprint != fingerprint_from_inputs(&current_inputs.inputs) {
            // Stored inputs that no longer reproduce the stored fingerprint were
            // recorded by another generator revision, whose input rules (such
            // as which manifests are hashed) may differ from today's.
            let generator_changed =
                fingerprint_from_inputs(&metadata.inputs) != metadata.fingerprint;
            let message = if metadata.inputs.is_empty() {
                "Project inputs changed after run configuration generation".to_string()
            } else if generator_changed || metadata.inputs == current_inputs.inputs {
                "Run configuration generator changed; regenerate configurations".to_string()
            } else {
                input_change_summary(&metadata.inputs, &current_inputs.inputs)
            };
            diagnostics.push(json!({
                "code": "staleFingerprint",
                "message": message
            }));
        }
    }
    if let Some(runs) = &request.provider_runs {
        crate::provider_runs::validate(runs)?;
        if let Some(document) = generated {
            let expected = crate::provider_runs::fingerprint(runs);
            if document
                .generator
                .as_ref()
                .and_then(|metadata| metadata.provider_run_fingerprint.as_deref())
                != Some(expected.as_str())
            {
                diagnostics.push(json!({"code":"staleProviderModel", "message":"Build provider model changed; regenerate Run configurations"}));
            }
        }
    }
    Ok(json!({
        "status": if generated.is_some() || has_saved_build_run(&local) || has_saved_build_run(&team) { "ready" } else { "missing" },
        // Saved provider tasks remain runnable before detection, but must not
        // suppress discovery of the other entries after a process restart.
        "requiresGeneration": generated.is_none(),
        "generated": generated,
        "toolchainRequirements": requirements,
        "localToolchains": local_toolchains,
        "toolchain": local.get("toolchain"),
        "diagnostics": diagnostics,
        "paths": { "configurations": ".lithe/run/configurations.json", "local": ".lithe/run/local.json" }
    }))
}

/// Detects runnable project entries and replaces the workspace's detected layer.
///
/// The layer is held in memory (and in the host's cache directory when given),
/// never written into the workspace.
pub fn generate(request: GenerateRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let cache_directory = request.cache_directory.map(PathBuf::from);
    let previous = previous_generation(&root, cache_directory.as_deref());
    let mut configurations = detected_configurations(&root)?;
    if let Some(runs) = &request.provider_runs {
        crate::provider_runs::validate(runs)?;
        let claimed = configurations
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();
        configurations.extend(
            crate::provider_runs::configurations(runs)
                .into_iter()
                .filter(|item| !claimed.contains(&item.id)),
        );
    }
    let mut snapshot = project_inputs(&root)?;
    // Unknown subtrees cannot delete known configurations or baseline inputs.
    // Invalid required documents still fail instead of being silently overwritten.
    let authoritative_service_ids = request.provider_runs.as_ref().map(|_| {
        configurations
            .iter()
            .filter(|item| item.provider == "build.provider")
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>()
    });

    if !snapshot.unreadable_paths.is_empty() {
        if let Some(previous) = previous {
            retain_unknown_inputs(&mut configurations, &mut snapshot, previous);
        }
    }
    if let Some(ids) = authoritative_service_ids {
        configurations.retain(|item| item.provider != "build.provider" || ids.contains(&item.id));
    }
    disambiguate_configuration_names(&mut configurations);
    let entry_count = configurations.len();
    let requirements = detect_requirements(&root, &configurations)?;
    let generated = RunConfigurationDocument {
        version: VERSION,
        generator: Some(GeneratorMetadata {
            provider_run_fingerprint: request
                .provider_runs
                .as_deref()
                .map(crate::provider_runs::fingerprint),
            fingerprint: fingerprint_from_inputs(&snapshot.inputs),
            inputs: snapshot.inputs,
            unreadable_paths: snapshot.unreadable_paths,
        }),
        configurations,
    };
    let response = json!({
        "generated": generated,
        "toolchainRequirements": requirements,
        "entryCount": entry_count
    });
    if let Some(cache_directory) = cache_directory.as_deref() {
        generated_layer::cache_document(cache_directory, &root, &generated);
    }
    generated_layer::store(
        &root,
        GeneratedLayer {
            document: generated,
            requirements,
        },
    );
    Ok(response)
}

/// The detected layer the previous generation produced: this session's, else
/// the one cached by an earlier session, else the `.lithe/run/generated.json`
/// older Lithe versions wrote into the workspace (read, never written).
///
/// Generation uses it only to keep entries under a subtree it cannot read:
/// replaying them keeps ids and user overrides attached until the subtree is
/// readable again. An unreadable or invalid previous document contributes
/// nothing, because this generation replaces it.
fn previous_generation(
    root: &Path,
    cache_directory: Option<&Path>,
) -> Option<RunConfigurationDocument> {
    if let Some(layer) = generated_layer::get(root) {
        return Some(layer.document.clone());
    }
    let mut document = cache_directory
        .and_then(|directory| generated_layer::cached_document(directory, root))
        .or_else(|| {
            read_document_value(root, "run/generated.json")
                .ok()
                .flatten()
        })?;
    migrate_document_value(&mut document);
    serde_json::from_value::<RunConfigurationDocument>(document)
        .ok()
        .filter(|document| validate_version(document.version).is_ok())
}

/// Qualifies display names that repeat across directories or modules.
///
/// Ids already carry the directory, but the Run list shows only the name: three
/// Compose files each contributing `compose up` are otherwise
/// indistinguishable. The qualifier is the first
/// candidate that separates every entry in the group, so the shortest useful
/// label wins and unique names are never decorated.
fn disambiguate_configuration_names(configurations: &mut [RunConfiguration]) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, configuration) in configurations.iter().enumerate() {
        groups
            .entry(configuration.name.to_lowercase())
            .or_default()
            .push(index);
    }
    for indexes in groups.into_values().filter(|indexes| indexes.len() > 1) {
        let candidates: [fn(&RunConfiguration) -> Option<String>; 2] = [
            |configuration| Some(configuration.cwd.clone()).filter(|cwd| cwd != "."),
            |configuration| configuration.source.clone(),
        ];
        let Some(qualifier) = candidates.into_iter().find(|candidate| {
            let values = indexes
                .iter()
                .map(|index| candidate(&configurations[*index]))
                .collect::<Option<BTreeSet<_>>>();
            values.is_some_and(|values| values.len() == indexes.len())
        }) else {
            continue;
        };
        for index in indexes {
            if let Some(value) = qualifier(&configurations[index]) {
                configurations[index].name = format!("{} ({value})", configurations[index].name);
            }
        }
    }
}

/// Translates detector output into the run-configuration contract.
///
/// Nearly every detection is process-based: the detector resolved the command,
/// so there is no ecosystem-specific launch assembly. A process may still name
/// a runtime requirement for scoped compatibility diagnostics; a detection that
/// transfers executable ownership to toolchains carries no command.
fn detected_configurations(root: &Path) -> Result<Vec<RunConfiguration>, CoreError> {
    Ok(crate::detectors::detect_all(root)?
        .into_iter()
        .map(|item| RunConfiguration {
            id: item.id(),
            name: item.name,
            provider: item.provider,
            execution: item.execution,
            category: item.category,
            command: item.command,
            args: item.args,
            cwd: item.cwd,
            env: item.env,
            confidence: item.confidence,
            toolchains: item.toolchains,
            debug: None,
            members: Vec::new(),
            extensions: item.extensions,
            disabled: false,
            source: Some(item.source),
        })
        .filter(|item| validate_configuration(item).is_ok())
        .collect())
}
