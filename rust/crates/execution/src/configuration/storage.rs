//! Where the saved run configuration layers live and how an edit reaches them.
//!
//! Each saved layer is one Lithe document plus one TOML document per plugin:
//!
//! | Layer | Lithe document | Plugin documents |
//! | --- | --- | --- |
//! | project, shared with the team | `.lithe/run/configurations.yaml` | `.lithe/run/plugin/<plugin>.toml` |
//! | local, this machine only | `.lithe/run/local.yaml` | `.lithe/run/plugin/<plugin>.local.toml` |
//!
//! Reading merges a layer's files into one document, so merging, validation
//! and editing see a layer as a single `{version, configurations, ...}` value.
//! A configuration belongs to the plugin whose build run it references, or
//! whose generated entry it overrides; every other configuration, built-in
//! process runs included, is Lithe's.
//!
//! Core never writes. An edit returns [`FileChange`]s, each carrying the
//! SHA-256 of the bytes it was computed from, and the host applies a change
//! only while the file still has those bytes, so a concurrent edit is refused
//! instead of overwritten.
//!
//! The JSON documents of earlier releases (`configurations.json`,
//! `local.json`) are still read when their YAML replacement does not exist;
//! the first edit of that layer writes the YAML document and deletes the JSON
//! one. When both exist the YAML document wins and the JSON one is reported.
//! A plugin document that cannot be read is reported and left out, so one bad
//! file never hides the built-in configurations; edits that would rewrite it
//! are refused until it is fixed or removed.

use super::plugin_document::{self, InstalledPlugin, PluginDocument};
use super::*;
use lithe_plugin_model::is_plugin_id;
use sha2::{Digest, Sha256};
use std::io::Read;

/// Largest run configuration document Core reads.
pub(crate) const MAXIMUM_FILE_BYTES: u64 = 4 << 20;

/// Most plugin documents one workspace may hold; the rest are reported.
const MAXIMUM_PLUGIN_DOCUMENTS: usize = 256;

/// Plugin documents, below `.lithe`.
const PLUGIN_DIRECTORY: &str = "run/plugin";

/// Which saved layer a document belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Shared with the team through version control.
    Project,
    /// This machine only; hidden from the project tree.
    Local,
}

impl Scope {
    pub(crate) fn parse(value: &str) -> Result<Self, CoreError> {
        match value {
            "project" => Ok(Self::Project),
            "local" => Ok(Self::Local),
            _ => Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Run configuration scope must be local or project",
            )),
        }
    }

    /// The Lithe document, below `.lithe`.
    pub(crate) fn document(self) -> &'static str {
        match self {
            Self::Project => "run/configurations.yaml",
            Self::Local => "run/local.yaml",
        }
    }

    /// The JSON document earlier releases wrote, below `.lithe`.
    pub(crate) fn legacy_document(self) -> &'static str {
        match self {
            Self::Project => "run/configurations.json",
            Self::Local => "run/local.json",
        }
    }

    /// The document of `plugin_id`'s configurations, below `.lithe`.
    pub(crate) fn plugin_document(self, plugin_id: &str) -> String {
        match self {
            Self::Project => format!("{PLUGIN_DIRECTORY}/{plugin_id}.toml"),
            Self::Local => format!("{PLUGIN_DIRECTORY}/{plugin_id}.local.toml"),
        }
    }
}

/// The file one saved configuration lives in within its layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LayerFile {
    /// The layer's Lithe document.
    Lithe,
    /// The layer's document for this plugin.
    Plugin(String),
}

/// One edit of one file, applied by the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Workspace-relative `/`-separated path, such as `.lithe/run/local.yaml`.
    pub path: String,
    /// Lowercase hex SHA-256 of the bytes the change was computed from;
    /// `null` when the file did not exist. The host applies the change only
    /// while the file still matches.
    pub base_sha256: Option<String>,
    /// The new content; `null` deletes the file.
    pub content: Option<String>,
}

impl FileChange {
    fn write(base: &FileBase, content: String) -> Self {
        Self {
            path: format!(".lithe/{}", base.relative),
            base_sha256: base.sha256.clone(),
            content: Some(content),
        }
    }

    fn delete(base: &FileBase) -> Self {
        Self {
            path: format!(".lithe/{}", base.relative),
            base_sha256: base.sha256.clone(),
            content: None,
        }
    }
}

/// A file as read: its path below `.lithe` and the hash of its bytes, `None`
/// when it did not exist.
#[derive(Debug, Clone)]
struct FileBase {
    relative: String,
    sha256: Option<String>,
}

#[derive(Debug, Clone)]
struct PluginFile {
    base: FileBase,
    document: PluginDocument,
}

/// The facts an edit needs to place configurations in their files.
pub(crate) struct WriteContext<'a> {
    /// The workspace's generated layer, whose plugin runs an override follows.
    pub(crate) generated: Option<&'a RunConfigurationDocument>,
    /// Installed plugins, whose identity a written plugin document records.
    pub(crate) installed: Option<&'a [InstalledPlugin]>,
}

/// One saved layer: its files merged into one document.
#[derive(Debug, Clone)]
pub(crate) struct LayerRead {
    scope: Scope,
    /// `{version, configurations, ...}`: the Lithe document with the plugin
    /// documents' configurations appended.
    pub(crate) document: Value,
    /// The file each configuration came from.
    origins: BTreeMap<String, LayerFile>,
    /// The Lithe document as read, to tell whether an edit changed it.
    lithe: Value,
    lithe_base: FileBase,
    /// The JSON document `lithe` came from, deleted by the first write.
    legacy_base: Option<FileBase>,
    plugins: BTreeMap<String, PluginFile>,
    /// Paths of plugin documents that were left out; never overwritten.
    unreadable: BTreeSet<String>,
    /// `false` for a layer the host supplied instead of the files.
    writable: bool,
}

/// Both saved layers and what reading them found.
#[derive(Debug, Clone)]
pub(crate) struct Layers {
    pub(crate) project: LayerRead,
    pub(crate) local: LayerRead,
    /// Diagnostics about unreadable, ignored or conflicting files.
    pub(crate) diagnostics: Vec<Value>,
}

impl Layers {
    pub(crate) fn layer(&self, scope: Scope) -> &LayerRead {
        match scope {
            Scope::Project => &self.project,
            Scope::Local => &self.local,
        }
    }

    /// The plugin document a saved configuration came from, local first.
    pub(crate) fn plugin_dependency(
        &self,
        configuration_id: &str,
    ) -> Option<&plugin_document::PluginDependency> {
        [&self.local, &self.project].into_iter().find_map(|layer| {
            match layer.origins.get(configuration_id)? {
                LayerFile::Plugin(plugin_id) => layer
                    .plugins
                    .get(plugin_id)
                    .map(|file| &file.document.dependency),
                LayerFile::Lithe => None,
            }
        })
    }
}

/// Reads both saved layers. A supplied document replaces that layer's files
/// for this request and cannot be written.
pub(crate) fn read_layers(
    root: &Path,
    team: Option<Value>,
    local: Option<Value>,
) -> Result<Layers, CoreError> {
    let mut diagnostics = Vec::new();
    let mut unreadable = BTreeSet::new();
    let (mut project_plugins, mut local_plugins) = if team.is_none() || local.is_none() {
        read_plugin_documents(root, &mut unreadable, &mut diagnostics)?
    } else {
        Default::default()
    };
    let mut read = |scope: Scope, provided: Option<Value>, plugins| match provided {
        Some(document) => LayerRead::provided(scope, document),
        None => LayerRead::from_files(root, scope, plugins, &mut unreadable, &mut diagnostics),
    };
    let project = read(Scope::Project, team, std::mem::take(&mut project_plugins))?;
    let local = read(Scope::Local, local, std::mem::take(&mut local_plugins))?;
    let (mut project, mut local) = (project, local);
    project.unreadable = unreadable.clone();
    local.unreadable = unreadable;
    Ok(Layers {
        project,
        local,
        diagnostics,
    })
}

impl LayerRead {
    fn provided(scope: Scope, mut document: Value) -> Result<Self, CoreError> {
        migrate_document_value(&mut document);
        validate_version_value(&document)?;
        let origins = configuration_ids(&document)?
            .into_keys()
            .map(|id| (id, LayerFile::Lithe))
            .collect();
        Ok(Self {
            scope,
            lithe: document.clone(),
            document,
            origins,
            lithe_base: FileBase {
                relative: scope.document().to_string(),
                sha256: None,
            },
            legacy_base: None,
            plugins: BTreeMap::new(),
            unreadable: BTreeSet::new(),
            writable: false,
        })
    }

    fn from_files(
        root: &Path,
        scope: Scope,
        plugins: BTreeMap<String, PluginFile>,
        unreadable: &mut BTreeSet<String>,
        diagnostics: &mut Vec<Value>,
    ) -> Result<Self, CoreError> {
        let current = read_file(root, scope.document())?;
        let legacy = read_file(root, scope.legacy_document())?;
        let (lithe, legacy_base) = match (current.as_ref(), legacy) {
            (Some((text, _)), legacy) => {
                if legacy.is_some() {
                    diagnostics.push(json!({
                        "code": "legacyDocumentIgnored",
                        "path": format!(".lithe/{}", scope.legacy_document()),
                        "message": format!(
                            "Using .lithe/{} and ignoring .lithe/{}; delete the JSON file",
                            scope.document(),
                            scope.legacy_document()
                        ),
                    }));
                }
                (parse_yaml(text, scope.document())?, None)
            }
            (None, Some((text, sha256))) => {
                let document = serde_json::from_str(&text).map_err(|error| {
                    CoreError::new(
                        ErrorCode::ParseFailed,
                        format!(
                            "Configuration JSON is invalid: .lithe/{}",
                            scope.legacy_document()
                        ),
                    )
                    .with_details(error.to_string())
                })?;
                (
                    document,
                    Some(FileBase {
                        relative: scope.legacy_document().to_string(),
                        sha256: Some(sha256),
                    }),
                )
            }
            (None, None) => (json!({"version": VERSION, "configurations": []}), None),
        };
        let mut lithe = lithe;
        migrate_document_value(&mut lithe);
        validate_version_value(&lithe)?;
        let mut origins: BTreeMap<String, LayerFile> = configuration_ids(&lithe)?
            .into_keys()
            .map(|id| (id, LayerFile::Lithe))
            .collect();
        let mut document = lithe.clone();
        let mut kept = BTreeMap::new();
        for (plugin_id, file) in plugins {
            if let Some(duplicate) = file
                .document
                .configurations
                .iter()
                .filter_map(|item| item["id"].as_str())
                .find(|id| origins.contains_key(*id))
            {
                diagnostics.push(json!({
                    "id": duplicate,
                    "code": "duplicateConfiguration",
                    "path": format!(".lithe/{}", file.base.relative),
                    "message": format!(
                        "Run configuration {duplicate} is saved twice; .lithe/{} is ignored",
                        file.base.relative
                    ),
                }));
                unreadable.insert(file.base.relative.clone());
                continue;
            }
            let items = document["configurations"]
                .as_array_mut()
                .expect("validated documents hold a configurations array");
            for item in &file.document.configurations {
                let id = item["id"].as_str().expect("plugin documents are validated");
                origins.insert(id.to_string(), LayerFile::Plugin(plugin_id.clone()));
                items.push(item.clone());
            }
            kept.insert(plugin_id, file);
        }
        Ok(Self {
            scope,
            document,
            origins,
            lithe,
            lithe_base: FileBase {
                relative: scope.document().to_string(),
                sha256: current.map(|(_, sha256)| sha256),
            },
            legacy_base,
            plugins: kept,
            unreadable: BTreeSet::new(),
            writable: true,
        })
    }

    /// The edits that make the layer's files hold `document`. Files whose
    /// content would not change are left alone; writes come before deletes.
    pub(crate) fn changes(
        &self,
        document: &Value,
        context: &WriteContext<'_>,
    ) -> Result<Vec<FileChange>, CoreError> {
        if !self.writable {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "A run configuration layer supplied with the request cannot be saved",
            ));
        }
        validate_version_value(document)?;
        let items = document["configurations"].as_array().ok_or_else(|| {
            CoreError::new(ErrorCode::ParseFailed, "configurations must be an array")
        })?;
        let mut lithe_items = Vec::new();
        let mut plugin_items: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for item in items {
            match self.placement(item, context) {
                LayerFile::Lithe => lithe_items.push(item.clone()),
                LayerFile::Plugin(plugin_id) => plugin_items
                    .entry(plugin_id)
                    .or_default()
                    .push(item.clone()),
            }
        }
        let mut writes = Vec::new();
        let mut deletes = Vec::new();
        let mut lithe = document.clone();
        lithe["configurations"] = Value::Array(lithe_items);
        if lithe != self.lithe {
            writes.push(FileChange::write(&self.lithe_base, render_yaml(&lithe)?));
            if let Some(legacy) = &self.legacy_base {
                deletes.push(FileChange::delete(legacy));
            }
        }
        let plugin_ids = self
            .plugins
            .keys()
            .chain(plugin_items.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for plugin_id in plugin_ids {
            let items = plugin_items
                .get(&plugin_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let previous = self.plugins.get(&plugin_id);
            match previous {
                Some(file) if file.document.configurations == items => continue,
                None if items.is_empty() => continue,
                _ => {}
            }
            let base = previous
                .map(|file| file.base.clone())
                .unwrap_or_else(|| FileBase {
                    relative: self.scope.plugin_document(&plugin_id),
                    sha256: None,
                });
            if self.unreadable.contains(&base.relative) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Fix or remove the unreadable plugin run configuration document first",
                )
                .with_details(format!(".lithe/{}", base.relative)));
            }
            if items.is_empty() {
                deletes.push(FileChange::delete(&base));
                continue;
            }
            let installed = context
                .installed
                .and_then(|plugins| plugins.iter().find(|plugin| plugin.id == plugin_id));
            let header = plugin_document::refreshed_header(
                previous.map(|file| &file.document.header),
                &plugin_id,
                installed,
            );
            writes.push(FileChange::write(
                &base,
                plugin_document::render(&header, items)?,
            ));
        }
        writes.extend(deletes);
        Ok(writes)
    }

    /// The file an edited configuration is stored in: the plugin its build
    /// run names, else the plugin of the generated entry it overrides, else
    /// the file it was read from, else the Lithe document.
    fn placement(&self, item: &Value, context: &WriteContext<'_>) -> LayerFile {
        fn build_plugin(extensions: Option<&Value>) -> Option<String> {
            extensions?
                .get("build")?
                .get("pluginID")?
                .as_str()
                .filter(|id| is_plugin_id(id))
                .map(str::to_string)
        }
        if let Some(plugin_id) = build_plugin(item.get("extensions")) {
            return LayerFile::Plugin(plugin_id);
        }
        let id = item["id"].as_str().unwrap_or_default();
        let generated = context.generated.and_then(|document| {
            document
                .configurations
                .iter()
                .find(|configuration| configuration.id == id)
        });
        if let Some(plugin_id) =
            generated.and_then(|configuration| build_plugin(Some(&json!(configuration.extensions))))
        {
            return LayerFile::Plugin(plugin_id);
        }
        self.origins.get(id).cloned().unwrap_or(LayerFile::Lithe)
    }
}

/// Reads every plugin document, split by layer. A document is the project
/// layer's when named `<id>.toml` and the local layer's when named
/// `<id>.local.toml`, where `<id>` is the plugin its `[plugin]` table names.
#[allow(clippy::type_complexity)]
fn read_plugin_documents(
    root: &Path,
    unreadable: &mut BTreeSet<String>,
    diagnostics: &mut Vec<Value>,
) -> Result<(BTreeMap<String, PluginFile>, BTreeMap<String, PluginFile>), CoreError> {
    let mut project = BTreeMap::new();
    let mut local = BTreeMap::new();
    let directory = root.join(".lithe").join(PLUGIN_DIRECTORY);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if fs::is_transport_failure(&error) => return Err(CoreError::from(error)),
        Err(_) => return Ok((project, local)),
    };
    let mut names = entries
        .filter_map(Result::ok)
        .filter(|entry| fs::is_file(entry.path()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".toml"))
        .collect::<Vec<_>>();
    names.sort();
    if names.len() > MAXIMUM_PLUGIN_DOCUMENTS {
        diagnostics.push(json!({
            "code": "tooManyPluginDocuments",
            "path": format!(".lithe/{PLUGIN_DIRECTORY}"),
            "message": format!(
                "Only the first {MAXIMUM_PLUGIN_DOCUMENTS} plugin run configuration documents are read"
            ),
        }));
        for name in names.drain(MAXIMUM_PLUGIN_DOCUMENTS..) {
            unreadable.insert(format!("{PLUGIN_DIRECTORY}/{name}"));
        }
    }
    for name in names {
        let relative = format!("{PLUGIN_DIRECTORY}/{name}");
        let mut skip = |code: &str, message: String| {
            diagnostics.push(json!({
                "code": code,
                "path": format!(".lithe/{relative}"),
                "message": message,
            }));
            unreadable.insert(relative.clone());
        };
        let (text, sha256) = match read_file(root, &relative) {
            Ok(Some(read)) => read,
            Ok(None) => continue,
            Err(error) => {
                skip(
                    "invalidPluginDocument",
                    format!("Could not read .lithe/{relative}: {}", error.message),
                );
                continue;
            }
        };
        let document = match plugin_document::parse(&text) {
            Ok(document) => document,
            Err(reason) => {
                skip(
                    "invalidPluginDocument",
                    format!(
                        "Plugin run configuration document .lithe/{relative} is invalid: {reason}"
                    ),
                );
                continue;
            }
        };
        let plugin_id = document.dependency.id.clone();
        let layer = if name == format!("{plugin_id}.local.toml") {
            &mut local
        } else if name == format!("{plugin_id}.toml") {
            &mut project
        } else {
            skip(
                "pluginDocumentMismatch",
                format!(".lithe/{relative} is named for another plugin than {plugin_id}"),
            );
            continue;
        };
        layer.insert(
            plugin_id,
            PluginFile {
                base: FileBase {
                    relative,
                    sha256: Some(sha256),
                },
                document,
            },
        );
    }
    Ok((project, local))
}

/// Reads a document below `.lithe` with the hash of its bytes, or `None`
/// when it does not exist.
pub(crate) fn read_file(
    root: &Path,
    relative: &str,
) -> Result<Option<(String, String)>, CoreError> {
    let path = root.join(".lithe").join(relative);
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if fs::is_transport_failure(&error) => return Err(CoreError::from(error)),
        Err(error) => {
            return Err(CoreError::new(
                ErrorCode::PermissionDenied,
                "Could not read run configuration",
            )
            .with_details(format!(".lithe/{relative}: {error}")))
        }
    };
    let mut bytes = Vec::new();
    file.take(MAXIMUM_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CoreError::new(
                ErrorCode::PermissionDenied,
                "Could not read run configuration",
            )
            .with_details(format!(".lithe/{relative}: {error}"))
        })?;
    if bytes.len() as u64 > MAXIMUM_FILE_BYTES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("Run configuration document is larger than 4 MiB: .lithe/{relative}"),
        ));
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let text = String::from_utf8(bytes).map_err(|_| {
        CoreError::new(
            ErrorCode::ParseFailed,
            format!("Run configuration document is not UTF-8: .lithe/{relative}"),
        )
    })?;
    Ok(Some((text, sha256)))
}

/// The change that replaces a JSON document below `.lithe` with `document`,
/// based on the bytes `sha256` describes.
pub(crate) fn json_file_change(
    relative: &str,
    sha256: Option<String>,
    document: &Value,
) -> FileChange {
    FileChange {
        path: format!(".lithe/{relative}"),
        base_sha256: sha256,
        content: Some(serde_json::to_string_pretty(document).expect("documents encode")),
    }
}

fn parse_yaml(text: &str, relative: &str) -> Result<Value, CoreError> {
    let invalid = |error: String| {
        CoreError::new(
            ErrorCode::ParseFailed,
            format!("Configuration YAML is invalid: .lithe/{relative}"),
        )
        .with_details(error)
    };
    let mut value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|error| invalid(error.to_string()))?;
    value
        .apply_merge()
        .map_err(|error| invalid(error.to_string()))?;
    if value.is_null() {
        return Ok(json!({"version": VERSION, "configurations": []}));
    }
    let document: Value =
        serde_json::to_value(value).map_err(|error| invalid(error.to_string()))?;
    if !document.is_object() {
        return Err(invalid("the document must be a mapping".to_string()));
    }
    Ok(document)
}

/// Renders a Lithe document: `version` first, `configurations` last, each
/// configuration from its identity down.
fn render_yaml(document: &Value) -> Result<String, CoreError> {
    let object = document
        .as_object()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "Invalid run document"))?;
    let mut ordered = serde_json::Map::new();
    if let Some(version) = object.get("version") {
        ordered.insert("version".to_string(), version.clone());
    }
    for (key, value) in object {
        if key != "version" && key != "configurations" {
            ordered.insert(key.clone(), value.clone());
        }
    }
    ordered.insert(
        "configurations".to_string(),
        Value::Array(
            object
                .get("configurations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(plugin_document::ordered_configuration)
                .collect(),
        ),
    );
    serde_yaml_ng::to_string(&Value::Object(ordered)).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration cannot be written as YAML",
        )
        .with_details(error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // Fixtures live on this machine.
    use std::fs;

    /// A temporary workspace, removed when dropped so a failing assertion
    /// does not leave it behind.
    struct Workspace(PathBuf);

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for Workspace {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    /// A fresh workspace no other test, run or process shares.
    fn workspace(name: &str) -> Workspace {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lithe-run-storage-{name}-{}-{nonce}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".lithe/run/plugin")).unwrap();
        Workspace(root)
    }

    fn context() -> WriteContext<'static> {
        WriteContext {
            generated: None,
            installed: None,
        }
    }

    fn apply(root: &Path, changes: &[FileChange]) {
        for change in changes {
            let path = root.join(&change.path);
            let current = fs::read(&path)
                .ok()
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
            assert_eq!(current, change.base_sha256, "{}", change.path);
            match &change.content {
                Some(content) => fs::write(path, content).unwrap(),
                None => fs::remove_file(path).unwrap(),
            }
        }
    }

    // Plugin runs leave the Lithe document; built-in runs stay in it.
    #[test]
    fn edits_split_configurations_between_lithe_and_plugin_documents() {
        let root = workspace("split");
        let layers = read_layers(&root, None, None).unwrap();
        let document = json!({"version": 2, "configurations": [
            {"id": "user:script", "name": "script", "provider": "process.shell",
             "extensions": {"process": {"script": "run.sh"}}},
            {"id": "user:serve", "name": "serve", "provider": "build.provider",
             "extensions": {"build": {"pluginID": "dev.lithe.test.build", "run": "serve"}}}
        ]});
        let changes = layers.local.changes(&document, &context()).unwrap();
        let paths = changes
            .iter()
            .map(|change| change.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                ".lithe/run/local.yaml",
                ".lithe/run/plugin/dev.lithe.test.build.local.toml"
            ]
        );
        apply(&root, &changes);

        let layers = read_layers(&root, None, None).unwrap();
        assert!(layers.diagnostics.is_empty(), "{:?}", layers.diagnostics);
        assert_eq!(layers.local.document, document);
        assert_eq!(
            layers.plugin_dependency("user:serve").unwrap().id,
            "dev.lithe.test.build"
        );
        assert!(layers
            .local
            .changes(&document, &context())
            .unwrap()
            .is_empty());

        let mut without_plugin = document.clone();
        without_plugin["configurations"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        let changes = layers.local.changes(&without_plugin, &context()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].content, None);
    }

    #[test]
    fn a_legacy_json_layer_is_read_and_replaced_by_the_first_edit() {
        let root = workspace("legacy");
        fs::write(
            root.join(".lithe/run/configurations.json"),
            r#"{"version":2,"configurations":[{"id":"user:a","name":"a","provider":"process.shell"}]}"#,
        )
        .unwrap();
        let layers = read_layers(&root, None, None).unwrap();
        assert_eq!(layers.project.document["configurations"][0]["id"], "user:a");
        let mut document = layers.project.document.clone();
        document["configurations"][0]["name"] = json!("renamed");
        let changes = layers.project.changes(&document, &context()).unwrap();
        assert_eq!(changes[0].path, ".lithe/run/configurations.yaml");
        assert_eq!(changes[1].path, ".lithe/run/configurations.json");
        assert_eq!(changes[1].content, None);
        apply(&root, &changes);

        fs::write(root.join(".lithe/run/configurations.json"), "{}").unwrap();
        let layers = read_layers(&root, None, None).unwrap();
        assert_eq!(
            layers.project.document["configurations"][0]["name"],
            "renamed"
        );
        assert_eq!(layers.diagnostics[0]["code"], "legacyDocumentIgnored");
    }

    // A broken plugin document must not hide the built-in configurations, and
    // must not be overwritten by an edit that cannot know what it held.
    #[test]
    fn unreadable_plugin_documents_are_reported_and_never_overwritten() {
        let root = workspace("unreadable");
        fs::write(
            root.join(".lithe/run/plugin/dev.lithe.test.build.toml"),
            "[",
        )
        .unwrap();
        fs::write(
            root.join(".lithe/run/plugin/dev.lithe.test.other.toml"),
            "schemaVersion = 1\n[plugin]\nid = \"dev.lithe.test.build\"\n",
        )
        .unwrap();
        fs::write(
            root.join(".lithe/run/configurations.yaml"),
            "version: 2\nconfigurations:\n- id: user:a\n  name: a\n  provider: process.shell\n",
        )
        .unwrap();
        let layers = read_layers(&root, None, None).unwrap();
        let codes = layers
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic["code"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(codes, ["invalidPluginDocument", "pluginDocumentMismatch"]);
        assert_eq!(layers.project.document["configurations"][0]["id"], "user:a");
        let mut document = layers.project.document.clone();
        document["configurations"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "user:b", "name": "b", "provider": "build.provider",
                "extensions": {"build": {"pluginID": "dev.lithe.test.build"}}
            }));
        let error = layers.project.changes(&document, &context()).unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
    }

    #[test]
    fn supplied_layers_are_read_but_not_written() {
        let root = workspace("supplied");
        let document = json!({"version": 2, "configurations": []});
        let layers = read_layers(&root, None, Some(document.clone())).unwrap();
        assert_eq!(layers.local.document, document);
        assert!(layers.local.changes(&document, &context()).is_err());
    }
}
