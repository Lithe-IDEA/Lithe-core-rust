//! Writing user changes back to the configuration documents.
//!
//! Every command here reads the saved layers from the workspace and returns
//! the [`FileChange`]s that make them hold the edit; the host applies them.

use super::*;

/// Replaces the generic tool preferences in the local layer.
pub fn update_tool_preferences(request: UpdateToolPreferencesRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    lithe_toolchain::preferences::validate(&request.preferences)
        .map_err(|error| CoreError::new(ErrorCode::InvalidRequest, error.message))?;
    let layers = read_layers(&root, None, None)?;
    let document = update_toolchain_document(
        &layers,
        ToolchainPaths {
            runtime_executable_paths: BTreeMap::new(),
            preferences: Some(request.preferences),
        },
    )?;
    let changes = layers
        .local
        .changes(&document, &write_context(&root, None).context())?;
    Ok(json!({ "changes": changes }))
}

/// Persists editable configuration options in the requested ownership layer.
pub fn update_options(mut request: UpdateOptionsRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let scope = Scope::parse(&request.scope)?;
    let layers = read_layers(&root, None, request.local_document.take())?;
    let plugins = request.plugins.take();
    let writing = write_context(&root, plugins.as_deref());
    if let Some(toolchain) = request.toolchain.take() {
        if scope != Scope::Local {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Toolchain paths can only be saved in the local layer",
            ));
        }
        let document = update_toolchain_document(&layers, toolchain)?;
        let changes = layers.local.changes(&document, &writing.context())?;
        return Ok(json!({ "changes": changes }));
    }
    let base = layers.layer(scope).document.clone();
    let document = update_configuration_options(&root, &layers, base, &request)?;
    let changes = layers.layer(scope).changes(&document, &writing.context())?;
    Ok(json!({ "changes": changes }))
}

/// Produces every change needed to save the run-configuration editor as one operation.
pub fn save_editor_changes(mut request: UpdateOptionsRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let scope = Scope::parse(&request.scope)?;
    let toolchain = request.toolchain.take().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration editor changes require a project toolchain",
        )
    })?;
    let layers = read_layers(&root, None, request.local_document.take())?;
    let plugins = request.plugins.take();
    let writing = write_context(&root, plugins.as_deref());
    let toolchain_change =
        update_local_toolchains_document(&root, &toolchain.runtime_executable_paths)?;
    let mut local_document = update_toolchain_document(&layers, toolchain)?;
    let mut changes = Vec::new();
    if scope == Scope::Project {
        let base = layers.project.document.clone();
        let document = update_configuration_options(&root, &layers, base, &request)?;
        changes.extend(layers.project.changes(&document, &writing.context())?);
    } else {
        local_document = update_configuration_options(&root, &layers, local_document, &request)?;
    }
    changes.extend(layers.local.changes(&local_document, &writing.context())?);
    changes.extend(toolchain_change);
    Ok(json!({ "changes": changes }))
}

/// The generated layer and installed plugins an edit places entries with.
pub(crate) struct Writing<'a> {
    generated: Option<std::sync::Arc<RunConfigurationDocument>>,
    installed: Option<&'a [InstalledPlugin]>,
}

impl Writing<'_> {
    pub(crate) fn context(&self) -> WriteContext<'_> {
        WriteContext {
            generated: self.generated.as_deref(),
            installed: self.installed,
        }
    }
}

pub(crate) fn write_context<'a>(
    root: &Path,
    installed: Option<&'a [InstalledPlugin]>,
) -> Writing<'a> {
    Writing {
        generated: generated_layer::get(root),
        installed,
    }
}

/// Replaces only the generic preferences. Other document-level toolchain
/// fields belong to whichever host or plugin wrote them, so a Run editor save
/// must not reset them.
fn update_toolchain_document(
    layers: &Layers,
    toolchain: ToolchainPaths,
) -> Result<Value, CoreError> {
    let mut document = layers.local.document.clone();
    let Some(preferences) = toolchain.preferences else {
        return Ok(document);
    };
    let saved = document
        .as_object_mut()
        .expect("validated layers are objects")
        .entry("toolchain")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Invalid local tool preferences")
        })?;
    saved.insert("preferences".to_string(), preferences);
    validate_tool_preferences(&document)?;
    Ok(document)
}

fn update_local_toolchains_document(
    root: &Path,
    runtime_executable_paths: &BTreeMap<String, String>,
) -> Result<Option<FileChange>, CoreError> {
    if runtime_executable_paths.is_empty() {
        return Ok(None);
    }
    let (mut document, sha256) = match read_local_toolchains_file(root)? {
        Some((document, sha256)) => (document, Some(sha256)),
        None => (json!({"version": SIDECAR_VERSION, "toolchains": {}}), None),
    };
    let toolchains = document
        .get_mut("toolchains")
        .and_then(Value::as_object_mut)
        .expect("validated local toolchains hold a toolchains object");
    for (id, executable_path) in runtime_executable_paths {
        if executable_path.trim().is_empty() {
            toolchains.remove(id);
        } else {
            toolchains.insert(id.clone(), json!({ "executable": executable_path.trim() }));
        }
    }
    Ok(Some(super::storage::json_file_change(
        "toolchains/local.json",
        sha256,
        &document,
    )))
}

/// Applies the editor's options for one configuration to `document`, a
/// layer's document, and returns it.
fn update_configuration_options(
    root: &Path,
    layers: &Layers,
    mut document: Value,
    request: &UpdateOptionsRequest,
) -> Result<Value, CoreError> {
    let resolved = resolve_layers(root, layers, None)?;
    let provider = resolved["configurations"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|value| value["id"] == request.configuration_id)
        })
        .and_then(|value| value["provider"].as_str())
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Run configuration was not found")
        })?;
    if let Some(options) = &request.provider_run_options {
        if provider != "build.provider" {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Provider run options require a build provider reference",
            ));
        }
        options.validate()?;
    }
    let working_directory = if request.working_directory.trim().is_empty() {
        None
    } else {
        Some(normalize_working_directory(
            root,
            request.working_directory.trim(),
        )?)
    };
    if let Some(ids) = &request.before_launch {
        if let Some(code) =
            super::before_launch::check_edit(root, layers, &request.configuration_id, ids)?
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "A step before launch cannot run",
            )
            .with_details(format!("beforeLaunch: {code}")));
        }
    }
    // An empty local list must still hide a list the project names.
    let project_names_steps = layers.project.document["configurations"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|item| {
            item["id"] == request.configuration_id
                && item["beforeLaunch"]
                    .as_array()
                    .is_some_and(|steps| !steps.is_empty())
        });
    let local_layer = request.scope == "local";
    let configurations = document["configurations"]
        .as_array_mut()
        .ok_or_else(|| CoreError::new(ErrorCode::ParseFailed, "configurations must be an array"))?;
    let mut patch = json!({
        "id": request.configuration_id,
        "env": request.environment
    });
    if let Some(working_directory) = working_directory.as_ref() {
        patch["cwd"] = json!(working_directory);
    }
    patch["args"] = json!(split_arguments(&request.arguments));
    let mut drop_steps = false;
    match request.before_launch.as_deref() {
        None => {}
        Some([]) if local_layer && project_names_steps => patch["beforeLaunch"] = json!([]),
        Some([]) => drop_steps = true,
        Some(ids) => patch["beforeLaunch"] = json!(super::before_launch::steps_of(ids)),
    }
    if provider == "build.provider" {
        if let Some(options) = &request.provider_run_options {
            patch["extensions"] = json!({"build-run":options});
        }
    }
    if let Some(existing) = configurations
        .iter_mut()
        .find(|value| value["id"] == patch["id"])
    {
        let target = existing.as_object_mut().ok_or_else(|| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "Run configuration must be an object",
            )
        })?;
        if working_directory.is_none() {
            target.remove("cwd");
        }
        if drop_steps {
            target.remove("beforeLaunch");
        }
        for (key, value) in patch.as_object_mut().expect("patch is an object") {
            if key == "extensions" {
                merge_extensions(target, value)?;
                continue;
            }
            target.insert(key.clone(), value.take());
        }
    } else {
        configurations.push(patch);
    }
    configurations.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or("")
            .cmp(right["id"].as_str().unwrap_or(""))
    });
    Ok(document)
}

/// Saved entries that run without a generated layer make an ungenerated
/// workspace runnable: explicit provider references and built-in process runs.
pub(crate) fn has_saved_runnable(document: &Value) -> bool {
    document["configurations"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            crate::provider_runs::is_explicit(item)
                || item["provider"]
                    .as_str()
                    .is_some_and(super::process::is_process_provider)
        })
    })
}

/// Upserts a local reference while preserving unrelated settings and stable IDs.
fn create_build_provider_configuration(
    root: &Path,
    request: &CreateUserConfigurationRequest,
) -> Result<Value, CoreError> {
    if request.scope != "local" {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Build provider references must be saved locally",
        ));
    }
    let reference = request.build.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Missing build provider run reference",
        )
    })?;
    reference.validate()?;
    normalize_project_directory(root, &reference.root)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 256 || name.contains('\0') {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid run configuration name",
        ));
    }
    let layers = read_layers(root, None, None)?;
    let mut document = layers.local.document.clone();
    let mut ids = existing_ids(root, &layers);
    ids.extend(configuration_ids(&document)?.into_keys());
    // An entrypoint run's configurations differ only by the entrypoint they start.
    let entrypoint = request.entrypoint.as_deref().unwrap_or_default();
    if entrypoint.len() > 4096 || entrypoint.contains('\0') {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid run entrypoint",
        ));
    }
    let encoded = serde_json::to_value(reference).expect("reference encodes");
    let mut identity = encoded.clone();
    identity.as_object_mut().unwrap().remove("userArguments");
    identity.as_object_mut().unwrap().remove("execution");
    let items = document["configurations"]
        .as_array_mut()
        .ok_or_else(|| CoreError::new(ErrorCode::ParseFailed, "configurations must be an array"))?;
    let existing = items.iter().position(|item| {
        if item["provider"] != "build.provider" {
            return false;
        }
        let mut saved = item["extensions"]["build"].clone();
        if let Some(object) = saved.as_object_mut() {
            object.remove("userArguments");
            object.remove("execution");
        }
        let saved_entrypoint = item["extensions"]["build-run"]["entrypoint"]
            .as_str()
            .unwrap_or_default();
        saved == identity && saved_entrypoint == entrypoint
    });
    let id = existing
        .and_then(|index| items[index]["id"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| unique_user_configuration_id(name, &ids));
    let mut configuration = existing
        .map(|index| items[index].clone())
        .unwrap_or_else(|| json!({}));
    let object = configuration.as_object_mut().ok_or_else(|| {
        CoreError::new(
            ErrorCode::ParseFailed,
            "Run configuration must be an object",
        )
    })?;
    let managed = json!({"id":id,"name":name,"provider":"build.provider","execution":reference.execution.unwrap_or_default(),
        "confidence":"declared"});
    object.entry("cwd").or_insert_with(|| json!(reference.root));
    object.extend(
        managed
            .as_object()
            .expect("managed fields are an object")
            .clone(),
    );
    // Other namespaces may hold user/plugin metadata; replacing the whole
    // entry would erase data unrelated to this selector update.
    let extensions = object
        .entry("extensions")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "Run configuration extensions must be an object",
            )
        })?;
    extensions.insert("build".to_string(), encoded);
    if !entrypoint.is_empty() {
        let options = extensions
            .entry("build-run")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::ParseFailed,
                    "Run configuration build-run options must be an object",
                )
            })?;
        options.insert("entrypoint".to_string(), json!(entrypoint));
    }
    let typed: RunConfiguration = serde_json::from_value(configuration.clone())
        .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid saved run configuration"))?;
    validate_configuration(&typed)?;
    if let Some(index) = existing {
        items[index] = configuration;
    } else {
        items.push(configuration);
    }
    items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let writing = write_context(root, request.plugins.as_deref());
    let changes = layers.local.changes(&document, &writing.context())?;
    Ok(json!({"id": id, "changes": changes}))
}

/// Every id a new user configuration must not reuse.
pub(crate) fn existing_ids(root: &Path, layers: &Layers) -> BTreeSet<String> {
    let mut ids = generated_ids(root);
    for document in [&layers.project.document, &layers.local.document] {
        if let Ok(saved) = configuration_ids(document) {
            ids.extend(saved.into_keys());
        }
    }
    ids
}

/// Creates a user configuration while preserving stable IDs in existing layers.
///
/// Only build provider references can be authored: every other runnable entry
/// is declared by the plugin that owns its launch semantics.
pub fn create_user_configuration(
    request: CreateUserConfigurationRequest,
) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    if request.kind == "buildProvider" {
        return create_build_provider_configuration(&root, &request);
    }
    Err(CoreError::new(
        ErrorCode::NotSupported,
        "Only build provider configurations can be created",
    ))
}

pub(crate) fn unique_user_configuration_id(name: &str, existing_ids: &BTreeSet<String>) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in name.to_lowercase().chars() {
        if character.is_alphanumeric() {
            slug.push(character);
            separator = false;
        } else if !slug.is_empty() {
            separator = true;
        }
        if separator && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let base = format!(
        "user:{}",
        if slug.is_empty() {
            "configuration"
        } else {
            slug
        }
    );
    if !existing_ids.contains(&base) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !existing_ids.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}
