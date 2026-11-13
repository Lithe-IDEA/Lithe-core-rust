//! Writing user changes back to the configuration documents.

use super::*;

/// Produces a Run-document replacement without materializing legacy defaults.
pub fn update_tool_preferences(request: UpdateToolPreferencesRequest) -> Result<Value, CoreError> {
    let mut document = request.local_document;
    // A preference edit must not trigger legacy Run migration or silently
    // narrow an unknown 64-bit version into a supported document version.
    if !matches!(document["version"].as_u64(), Some(1 | 2)) {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "Unsupported run configuration version",
        ));
    }
    configuration_ids(&document)?;
    validate_tool_preferences(&document)?;
    lithe_toolchain::preferences::validate(&request.preferences)
        .map_err(|error| CoreError::new(ErrorCode::InvalidRequest, error.message))?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "Invalid local Run document"))?;
    let toolchain = object.entry("toolchain").or_insert_with(|| json!({}));
    let toolchain = toolchain.as_object_mut().ok_or_else(|| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid local tool preferences")
    })?;
    toolchain.insert("preferences".into(), request.preferences);
    Ok(json!({"document":serde_json::to_string_pretty(&document).expect("validated Run document")}))
}

/// Persists editable configuration options in the requested ownership layer.
pub fn update_options(mut request: UpdateOptionsRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    if let Some(toolchain) = request.toolchain.take() {
        if request.scope != "local" {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Toolchain paths can only be saved in the local layer",
            ));
        }
        let document = update_toolchain_document(&root, request.local_document, toolchain)?;
        return Ok(json!({
            "document": serde_json::to_string_pretty(&document).expect("document should encode")
        }));
    }
    let document = update_configuration_options(&root, request)?;
    Ok(json!({
        "document": serde_json::to_string_pretty(&document).expect("document should encode")
    }))
}

/// Produces every document needed to save the run-configuration editor as one operation.
pub fn save_editor_changes(mut request: UpdateOptionsRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let toolchain = request.toolchain.take().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration editor changes require a project toolchain",
        )
    })?;
    let toolchain_document =
        update_local_toolchains_document(&root, &toolchain.runtime_executable_paths)?;
    let mut local_document =
        update_toolchain_document(&root, request.local_document.take(), toolchain)?;
    request.local_document = Some(local_document.clone());
    let scope = request.scope.clone();
    let options_document = update_configuration_options(&root, request)?;
    let project_document = if scope == "project" {
        Some(serde_json::to_string_pretty(&options_document).expect("document should encode"))
    } else {
        local_document = options_document;
        None
    };
    Ok(json!({
        "localDocument": serde_json::to_string_pretty(&local_document).expect("document should encode"),
        "projectDocument": project_document,
        "toolchainDocument": toolchain_document.map(|document| {
            serde_json::to_string_pretty(&document).expect("document should encode")
        })
    }))
}

/// Replaces only the generic preferences. Other document-level toolchain
/// fields belong to whichever host or plugin wrote them, so a Run editor save
/// must not reset them.
fn update_toolchain_document(
    root: &Path,
    local_document: Option<Value>,
    toolchain: ToolchainPaths,
) -> Result<Value, CoreError> {
    let mut document = local_layer_document(root, local_document)?;
    validate_version_value(&document)?;
    let mut saved = document["toolchain"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(preferences) = toolchain.preferences {
        saved.insert("preferences".to_string(), preferences);
    }
    document["toolchain"] = Value::Object(saved);
    validate_tool_preferences(&document)?;
    Ok(document)
}

fn update_local_toolchains_document(
    root: &Path,
    runtime_executable_paths: &BTreeMap<String, String>,
) -> Result<Option<Value>, CoreError> {
    if runtime_executable_paths.is_empty() {
        return Ok(None);
    }
    let mut document = read_local_toolchains(root)?
        .unwrap_or_else(|| json!({"version": SIDECAR_VERSION, "toolchains": {}}));
    validate_sidecar_version_value(&document)?;
    let toolchains = document
        .get_mut("toolchains")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "Local toolchains must contain a toolchains object",
            )
        })?;
    for (id, executable_path) in runtime_executable_paths {
        if executable_path.trim().is_empty() {
            toolchains.remove(id);
        } else {
            toolchains.insert(id.clone(), json!({ "executable": executable_path.trim() }));
        }
    }
    Ok(Some(document))
}

fn update_configuration_options(
    root: &Path,
    request: UpdateOptionsRequest,
) -> Result<Value, CoreError> {
    let relative = scope_document(&request.scope)?;
    let resolved = resolve(ResolveRequest {
        team_document: None,
        root: request.root.clone(),
        toolchain_candidates: Vec::new(),
        local_document: request.local_document.clone(),
    })?;
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
    let mut document = if request.scope == "local" {
        local_layer_document(root, request.local_document)?
    } else {
        read_document_value(root, relative)?
            .unwrap_or_else(|| json!({"version": VERSION, "configurations": []}))
    };
    validate_version_value(&document)?;
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

/// Only explicit provider references make an ungenerated workspace runnable.
pub(crate) fn has_saved_build_run(document: &Value) -> bool {
    document["configurations"]
        .as_array()
        .is_some_and(|items| items.iter().any(crate::provider_runs::is_explicit))
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
    let mut document = read_document_value(root, "run/local.json")?
        .unwrap_or_else(|| json!({"version":VERSION,"configurations":[]}));
    validate_version_value(&document)?;
    let mut ids: std::collections::BTreeSet<String> =
        configuration_ids(&document)?.into_keys().collect();
    ids.extend(detected_ids(root));
    if let Some(other) = read_document_value(root, "run/configurations.json")? {
        validate_version_value(&other)?;
        ids.extend(configuration_ids(&other)?.into_keys());
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
        saved == identity
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
    let typed: RunConfiguration = serde_json::from_value(configuration.clone())
        .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid saved run configuration"))?;
    validate_configuration(&typed)?;
    if let Some(index) = existing {
        items[index] = configuration;
    } else {
        items.push(configuration);
    }
    items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Ok(
        json!({"id":id,"document":serde_json::to_string_pretty(&document).expect("document encodes")}),
    )
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

fn unique_user_configuration_id(
    name: &str,
    existing_ids: &std::collections::BTreeSet<String>,
) -> String {
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
