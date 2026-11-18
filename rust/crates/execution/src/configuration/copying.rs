//! Copying a run configuration under a new name.

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to save a copy of a configuration as a user configuration.
pub struct CopyConfigurationRequest {
    /// Canonical workspace root.
    pub root: String,
    /// The resolved configuration to copy, of any kind.
    pub configuration_id: String,
    /// Display name of the copy.
    pub name: String,
    /// The installed plugins, whose identity a written plugin document records.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
}

/// Replies `{id, changes}`: the copy's new `user:` id and the changes that
/// save it.
///
/// The copy is the resolved configuration with every saved choice, its folder
/// included, under a new id and name. It is saved in the project layer when
/// the original is a user configuration defined there with no local
/// override, so local choices never reach the team, and in the local layer
/// otherwise.
///
/// A saved build provider run supersedes the generated run with the same
/// target, so copying a generated one would hide the original. The original
/// is then also saved whole in the local layer under its own id, which keeps
/// both listed; it stops following later name changes of the build model.
pub fn copy_configuration(request: CopyConfigurationRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 256 || name.contains('\0') {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid run configuration name",
        ));
    }
    let layers = read_layers(&root, None, None)?;
    let resolved = resolve_layers(&root, &layers, None)?;
    let original = resolved["configurations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["id"] == request.configuration_id)
        .cloned()
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Run configuration was not found")
        })?;
    let original = saved_form(original);
    let id = unique_user_configuration_id(name, &existing_ids(&root, &layers));
    let mut copy = original.clone();
    copy["id"] = json!(id);
    copy["name"] = json!(name);
    let typed: RunConfiguration = serde_json::from_value(copy.clone())
        .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid saved run configuration"))?;
    validate_configuration(&typed)?;
    let has_entry = |document: &Value, defining: bool| {
        document["configurations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|item| {
                item["id"] == request.configuration_id
                    && (!defining || item.get("provider").is_some())
            })
    };
    let user = request.configuration_id.starts_with("user:");
    let target = if user
        && has_entry(&layers.project.document, true)
        && !has_entry(&layers.local.document, false)
    {
        Scope::Project
    } else {
        Scope::Local
    };
    let mut project = layers.project.document.clone();
    let mut local = layers.local.document.clone();
    if !user && typed.provider == "build.provider" {
        replace_entry(&mut local, original)?;
    }
    match target {
        Scope::Project => replace_entry(&mut project, copy)?,
        Scope::Local => replace_entry(&mut local, copy)?,
    }
    let writing = write_context(&root, request.plugins.as_deref());
    let mut changes = layers.project.changes(&project, &writing.context())?;
    changes.extend(layers.local.changes(&local, &writing.context())?);
    Ok(json!({ "id": id, "changes": changes }))
}

/// A resolved configuration as a layer saves it: without the values
/// resolution computes and without empty defaults.
fn saved_form(mut configuration: Value) -> Value {
    if let Some(object) = configuration.as_object_mut() {
        for key in ["source", "plugin", "availability", "disabled"] {
            object.remove(key);
        }
        for key in ["args", "env", "toolchains", "members", "extensions"] {
            let empty = match object.get(key) {
                Some(Value::Array(items)) => items.is_empty(),
                Some(Value::Object(entries)) => entries.is_empty(),
                _ => false,
            };
            if empty {
                object.remove(key);
            }
        }
    }
    configuration
}

/// Puts `entry` in `document` in place of any entry with its id, in id order.
fn replace_entry(document: &mut Value, entry: Value) -> Result<(), CoreError> {
    let items = document["configurations"]
        .as_array_mut()
        .ok_or_else(|| CoreError::new(ErrorCode::ParseFailed, "configurations must be an array"))?;
    items.retain(|item| item["id"] != entry["id"]);
    items.push(entry);
    items.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    Ok(())
}
