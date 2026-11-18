//! Inspecting a project and generating its provider run configurations.

use super::*;

/// Validates configuration and sidecar documents without mutating the workspace.
pub fn inspect(request: InspectRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layer = generated_layer::get(&root);
    let generated = layer.as_deref();
    let local_toolchains = read_local_toolchains(&root)?;
    if let Some((manifest, _)) = read_sidecar(&root, "project.json")? {
        validate_sidecar_version_value(&manifest)?;
    }
    let layers = read_layers(&root, None, request.local_document.clone())?;
    let team = &layers.project.document;
    let local = &layers.local.document;
    let mut diagnostics = layers.diagnostics.clone();
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
        "status": if generated.is_some() || has_saved_runnable(local) || has_saved_runnable(team) { "ready" } else { "missing" },
        // Saved runs remain runnable before generation, but must not
        // suppress generation of the other entries after a process restart.
        "requiresGeneration": generated.is_none(),
        "generated": generated,
        "localToolchains": local_toolchains,
        "toolchain": local.get("toolchain"),
        "diagnostics": diagnostics,
        "paths": {
            "configurations": format!(".lithe/{}", Scope::Project.document()),
            "local": format!(".lithe/{}", Scope::Local.document()),
            "plugins": ".lithe/run/plugin"
        }
    }))
}

/// Materializes the provider run snapshot and replaces the workspace's
/// generated layer.
///
/// Core discovers nothing itself: every generated entry is a run a build
/// provider plugin declared. The layer is held in memory, never written into
/// the workspace.
pub fn generate(request: GenerateRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let runs = request.provider_runs.unwrap_or_default();
    crate::provider_runs::validate(&runs)?;
    let mut configurations = crate::provider_runs::configurations(&runs);
    disambiguate_configuration_names(&mut configurations);
    let entry_count = configurations.len();
    let generated = RunConfigurationDocument {
        version: VERSION,
        generator: Some(GeneratorMetadata {
            provider_run_fingerprint: Some(crate::provider_runs::fingerprint(&runs)),
        }),
        configurations,
    };
    let response = json!({
        "generated": generated,
        "entryCount": entry_count
    });
    generated_layer::store(&root, generated);
    Ok(response)
}

/// Qualifies display names that repeat across provider roots.
///
/// Ids already carry the selector, but the Run list shows only the name: three
/// modules that each declare a `serve` entry are otherwise indistinguishable.
/// A repeated name gains its working directory when that separates every entry
/// in the group, so unique names are never decorated.
fn disambiguate_configuration_names(configurations: &mut [RunConfiguration]) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, configuration) in configurations.iter().enumerate() {
        groups
            .entry(configuration.name.to_lowercase())
            .or_default()
            .push(index);
    }
    for indexes in groups.into_values().filter(|indexes| indexes.len() > 1) {
        let directories = indexes
            .iter()
            .map(|index| configurations[*index].cwd.as_str())
            .filter(|cwd| *cwd != ".")
            .collect::<BTreeSet<_>>()
            .len();
        if directories != indexes.len() {
            continue;
        }
        for index in indexes {
            let configuration = &mut configurations[index];
            configuration.name = format!("{} ({})", configuration.name, configuration.cwd);
        }
    }
}
