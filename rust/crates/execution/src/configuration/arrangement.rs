//! How the run configuration list is arranged: folders and order.
//!
//! A folder is display metadata saved in the configuration itself, the way
//! IntelliJ IDEA saves `folderName`, so a configuration in the project layer
//! carries its folder to the team. The order and whether the list is sorted
//! by name are this machine's choice and live only in the local Lithe
//! document, under `arrangement`:
//!
//! ```yaml
//! arrangement:
//!   sortAlphabetically: false
//!   order: [user:build, user:serve]
//! ```
//!
//! Resolution returns the configurations in this order, so every list a host
//! draws agrees. Launching ignores both.

use super::*;

/// Longest folder name, in bytes.
const MAXIMUM_FOLDER_BYTES: usize = 256;

/// Most configuration ids one saved order holds.
const MAXIMUM_ORDER_LENGTH: usize = 4096;

/// Longest configuration id a saved order may name, in bytes.
const MAXIMUM_ID_BYTES: usize = 4096;

/// Whether `folder` can name a folder: not empty, without surrounding
/// whitespace or control characters, and at most 256 bytes.
pub(crate) fn valid_folder(folder: &str) -> bool {
    !folder.is_empty()
        && folder.len() <= MAXIMUM_FOLDER_BYTES
        && folder.trim() == folder
        && !folder.chars().any(char::is_control)
}

/// The arrangement the local layer saves, with its defaults: sorted by name
/// and no manual order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Arrangement {
    /// List by name instead of the manual order.
    pub(crate) sort_alphabetically: bool,
    /// Configuration ids in the order the user placed them. Ids that no
    /// longer resolve are skipped; configurations it does not name follow.
    pub(crate) order: Vec<String>,
}

impl Default for Arrangement {
    fn default() -> Self {
        Self {
            sort_alphabetically: true,
            order: Vec::new(),
        }
    }
}

impl Arrangement {
    /// The arrangement of `local`, the local layer's document. A malformed
    /// value is reported and the defaults are used, so a hand-edited file
    /// never hides the list.
    pub(crate) fn read(local: &Value, diagnostics: &mut Vec<Value>) -> Self {
        let Some(value) = local.get("arrangement").filter(|value| !value.is_null()) else {
            return Self::default();
        };
        match Self::parse(value) {
            Some(arrangement) => arrangement,
            None => {
                diagnostics.push(json!({
                    "code": "invalidArrangement",
                    "path": ".lithe/run/local.yaml",
                    "message": "The saved run configuration order is invalid and is ignored"
                }));
                Self::default()
            }
        }
    }

    fn parse(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let mut arrangement = Self::default();
        if let Some(sort) = object.get("sortAlphabetically") {
            arrangement.sort_alphabetically = sort.as_bool()?;
        }
        if let Some(order) = object.get("order") {
            let ids = order
                .as_array()?
                .iter()
                .map(|id| id.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()?;
            arrangement.order = valid_order(ids).ok()?;
        }
        Some(arrangement)
    }

    /// Orders `configurations` as the list shows them: by name, ignoring
    /// case, when sorted; otherwise as the saved order places them, with the
    /// configurations it does not name after it by name.
    pub(crate) fn apply(&self, configurations: &mut [RunConfiguration]) {
        let positions = if self.sort_alphabetically {
            BTreeMap::new()
        } else {
            self.order
                .iter()
                .enumerate()
                .map(|(index, id)| (id.as_str(), index))
                .collect()
        };
        configurations.sort_by_cached_key(|configuration| {
            (
                positions
                    .get(configuration.id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
                configuration.name.to_lowercase(),
                configuration.name.clone(),
                configuration.id.clone(),
            )
        });
    }

    /// Saves the arrangement in `local`, the local layer's document; the
    /// defaults remove the key so an untouched project writes nothing.
    fn write(&self, local: &mut Value) {
        let object = local.as_object_mut().expect("validated layers are objects");
        if *self == Self::default() {
            object.remove("arrangement");
            return;
        }
        let mut saved = serde_json::Map::new();
        saved.insert(
            "sortAlphabetically".to_string(),
            json!(self.sort_alphabetically),
        );
        if !self.order.is_empty() {
            saved.insert("order".to_string(), json!(self.order));
        }
        object.insert("arrangement".to_string(), Value::Object(saved));
    }
}

/// `ids` without repeats, or an error when it is too long or names an
/// impossible id.
fn valid_order(ids: Vec<String>) -> Result<Vec<String>, CoreError> {
    if ids.len() > MAXIMUM_ORDER_LENGTH
        || ids
            .iter()
            .any(|id| id.is_empty() || id.len() > MAXIMUM_ID_BYTES || id.contains('\0'))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid run configuration order",
        ));
    }
    let mut seen = BTreeSet::new();
    Ok(ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request to move configurations between folders and to save the order.
pub struct ArrangeRequest {
    /// Canonical workspace root.
    pub root: String,
    /// The folder each listed configuration moves to; `null` takes it out of
    /// its folder. Configurations not listed keep theirs.
    #[serde(default)]
    pub folders: BTreeMap<String, Option<String>>,
    /// The whole manual order, replacing the saved one; omitted keeps it.
    #[serde(default)]
    pub order: Option<Vec<String>>,
    /// Whether the list is sorted by name; omitted keeps the saved choice.
    #[serde(default)]
    pub sort_alphabetically: Option<bool>,
    /// The installed plugins, whose identity a written plugin document records.
    #[serde(default)]
    pub plugins: Option<Vec<InstalledPlugin>>,
}

/// The changes that move configurations into folders and save the order.
///
/// A folder is written to the layer that defines the configuration: the
/// project layer for a configuration saved there, otherwise the local
/// layer, where a generated configuration gets a sparse `{id, folder}`
/// override. The other layer's folder for it is removed, so the choice is
/// the one that shows. The order and the sort choice are written to the
/// local layer only.
pub fn arrange(request: ArrangeRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layers = read_layers(&root, None, None)?;
    let mut project = layers.project.document.clone();
    let mut local = layers.local.document.clone();
    if !request.folders.is_empty() {
        let resolved = resolve_layers(&root, &layers, None)?;
        let listed = resolved["configurations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["id"].as_str())
            .collect::<BTreeSet<_>>();
        for (id, folder) in &request.folders {
            if !listed.contains(id.as_str()) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Run configuration was not found",
                )
                .with_details(id));
            }
            if folder
                .as_deref()
                .is_some_and(|folder| !valid_folder(folder))
            {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid run configuration folder",
                )
                .with_details(id));
            }
            set_folder(&mut project, &mut local, id, folder.as_deref())?;
        }
    }
    if request.order.is_some() || request.sort_alphabetically.is_some() {
        let mut arrangement = Arrangement::read(&local, &mut Vec::new());
        if let Some(order) = request.order {
            arrangement.order = valid_order(order)?;
        }
        if let Some(sort) = request.sort_alphabetically {
            arrangement.sort_alphabetically = sort;
        }
        arrangement.write(&mut local);
    }
    let writing = write_context(&root, request.plugins.as_deref());
    let mut changes = layers.project.changes(&project, &writing.context())?;
    changes.extend(layers.local.changes(&local, &writing.context())?);
    Ok(json!({ "changes": changes }))
}

/// Puts configuration `id` in `folder`, or out of every folder for `None`,
/// in the two saved layer documents.
fn set_folder(
    project: &mut Value,
    local: &mut Value,
    id: &str,
    folder: Option<&str>,
) -> Result<(), CoreError> {
    let defined_in_project = entries(project)?
        .iter()
        .any(|item| item["id"] == id && item.get("provider").is_some());
    let (target, other) = if defined_in_project {
        (project, local)
    } else {
        (local, project)
    };
    let items = entries(other)?;
    for item in items.iter_mut().filter(|item| item["id"] == id) {
        if let Some(object) = item.as_object_mut() {
            object.remove("folder");
        }
    }
    // An override left holding only its id changes nothing.
    items.retain(|item| item["id"] != id || item.as_object().is_none_or(|object| object.len() > 1));
    let items = entries(target)?;
    match items.iter_mut().find(|item| item["id"] == id) {
        Some(item) => {
            let object = item.as_object_mut().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::ParseFailed,
                    "Run configuration must be an object",
                )
            })?;
            match folder {
                Some(folder) => {
                    object.insert("folder".to_string(), json!(folder));
                }
                None => {
                    object.remove("folder");
                }
            }
        }
        None => {
            if let Some(folder) = folder {
                items.push(json!({ "id": id, "folder": folder }));
                items.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
            }
        }
    }
    items.retain(|item| item["id"] != id || item.as_object().is_none_or(|object| object.len() > 1));
    Ok(())
}

fn entries(document: &mut Value) -> Result<&mut Vec<Value>, CoreError> {
    document["configurations"]
        .as_array_mut()
        .ok_or_else(|| CoreError::new(ErrorCode::ParseFailed, "configurations must be an array"))
}
