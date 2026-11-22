//! Resolving the configuration a launch uses.

use super::*;
use lithe_plugin_model::is_plugin_id;

/// Merges generated, team, and local layers into the configurations a launch can use.
pub fn resolve(request: ResolveRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layers = read_layers(&root, request.team_document, request.local_document)?;
    resolve_layers(&root, &layers, request.plugins.as_deref())
}

/// Resolves layers already read, so an edit validates against the same bytes
/// it changes.
///
/// One configuration that cannot run never fails the list: it is returned
/// with an `availability` explaining why, and the others stay runnable. Only
/// unreadable Lithe documents and sidecars fail the request.
pub(crate) fn resolve_layers(
    root: &Path,
    layers: &Layers,
    installed: Option<&[InstalledPlugin]>,
) -> Result<Value, CoreError> {
    let team = &layers.project.document;
    let local = &layers.local.document;
    // Before any build model is loaded nothing is generated: the saved runs
    // still resolve, and a project with none has an empty list. Overrides of
    // generated runs are only orphaned once a generation can be compared.
    let layer = generated_layer::get(root);
    let generated = match layer.as_deref() {
        Some(document) => serde_json::to_value(document).expect("run configurations encode"),
        None => json!({"version": VERSION, "configurations": []}),
    };
    let generated_ids = configuration_ids(&generated)?;
    let local_toolchains = read_local_toolchains(root)?;
    let manifest = read_sidecar(root, "project.json")?.map(|(document, _)| document);
    validate_version_value(&generated)?;
    if let Some(manifest) = manifest.as_ref() {
        validate_sidecar_version_value(manifest)?;
    }
    let mut diagnostics = layers.diagnostics.clone();
    let mut configurations = merge_values(&generated, team, local, &mut diagnostics)?;
    let resolved_ids = configurations
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    for source in [team, local].into_iter().filter(|_| layer.is_some()) {
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
    let global_toolchain = local.get("toolchain").cloned();
    let platform = run_platform();
    for configuration in &mut configurations {
        configuration.plugin = layers
            .plugin_dependency(&configuration.id)
            .cloned()
            .or_else(|| {
                configuration
                    .extensions
                    .get("build")
                    .and_then(|build| build.get("pluginID"))
                    .and_then(Value::as_str)
                    .filter(|id| {
                        matches!(
                            configuration.provider.as_str(),
                            "build.provider" | super::declared::RUN_TYPE_PROVIDER
                        ) && is_plugin_id(id)
                    })
                    .map(PluginDependency::named)
            });
        if !generated_ids.contains_key(&configuration.id) {
            configuration.availability = configuration
                .plugin
                .as_ref()
                .and_then(|dependency| super::plugin_document::availability(dependency, installed));
        }
        if let Err(error) = validate_configuration(configuration) {
            diagnostics.push(json!({
                "id": configuration.id,
                "code": "invalidConfiguration",
                "message": error.message
            }));
            configuration.availability.get_or_insert_with(|| {
                Availability::unavailable("invalidConfiguration", error.message)
            });
            continue;
        }
        if configuration.provider == "build.provider" {
            // Materialize optional defaults in the shared output so native
            // consumers receive the same shape as a newly saved selector.
            let reference: BuildProviderRunReference =
                serde_json::from_value(configuration.extensions["build"].clone())
                    .expect("validated build provider references decode");
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
        // A built-in kind this platform cannot run, such as a batch script
        // outside Windows, stays listed and editable but does not start.
        if let Some(availability) =
            super::process::provider_availability(&configuration.provider, platform)
        {
            configuration.availability.get_or_insert(availability);
        }
        if !project_directory_exists(root, &configuration.cwd) {
            let message = format!("Working directory does not exist: {}", configuration.cwd);
            diagnostics.push(json!({
                "id": configuration.id,
                "code": "missingWorkingDirectory",
                "message": message
            }));
            // A generated entry follows the build model, which detection
            // refreshes. A saved entry stays listed, so the user can fix or
            // delete it instead of losing it.
            if generated_ids.contains_key(&configuration.id) {
                configuration.disabled = true;
            } else {
                configuration.availability.get_or_insert_with(|| {
                    Availability::unavailable("missingWorkingDirectory", message)
                });
            }
        }
    }
    configurations.retain(|configuration| !configuration.disabled);
    let arrangement = Arrangement::read(local, &mut diagnostics);
    arrangement.apply(&mut configurations);
    let runnable = |id: &str| {
        configurations
            .iter()
            .any(|configuration| configuration.id == id && configuration.availability.is_none())
    };
    let mut default_run_configuration = manifest
        .as_ref()
        .and_then(|value| value.get("defaultRunConfiguration"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(default_id) = default_run_configuration.as_deref() {
        if !runnable(default_id) {
            diagnostics.push(json!({
                "code": "missingDefaultConfiguration",
                "message": format!("Default run configuration is unavailable: {default_id}")
            }));
            default_run_configuration = None;
        }
    }
    if default_run_configuration.is_none() {
        default_run_configuration = layer
            .as_deref()
            .and_then(|document| generated_default(document, &configurations))
            .filter(|id| runnable(id));
    }
    Ok(json!({
        "version": VERSION,
        "configurations": configurations,
        "diagnostics": diagnostics,
        "defaultRunConfiguration": default_run_configuration,
        "sortAlphabetically": arrangement.sort_alphabetically,
        "toolchain": global_toolchain,
        "localToolchains": local_toolchains
    }))
}

/// The resolved configuration `id`, or an error naming why it cannot be used.
pub(crate) fn runnable_configuration<'a>(
    resolved: &'a Value,
    id: &str,
) -> Result<&'a Value, CoreError> {
    let configuration = resolved["configurations"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == id))
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Run configuration was not found")
        })?;
    if let Some(availability) = configuration
        .get("availability")
        .filter(|value| !value.is_null())
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration is unavailable",
        )
        .with_details(availability["message"].as_str().unwrap_or_default()));
    }
    Ok(configuration)
}

/// Prefer a generated service when the project names no default, otherwise
/// select the first available generated entry.
fn generated_default(
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
    let candidates = generated.configurations.iter().filter_map(|configuration| {
        by_id.get(configuration.id.as_str()).copied().or_else(|| {
            let selector = crate::provider_runs::selector(
                &serde_json::to_value(configuration).expect("configuration encodes"),
            )?;
            by_selector.get(&selector).copied()
        })
    });
    candidates
        .clone()
        .find(|configuration| configuration.execution == Execution::Service)
        .or_else(|| candidates.clone().next())
        .map(|configuration| configuration.id.clone())
}
