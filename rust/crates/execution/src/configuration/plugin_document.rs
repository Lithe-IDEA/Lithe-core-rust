//! The TOML documents below `.lithe/run/plugin/` that hold the run
//! configurations a plugin launches, together with the plugin they need.
//!
//! A built-in configuration only needs Lithe, so `run/configurations.yaml`
//! records nothing about where it came from. A plugin configuration stops
//! working when its plugin is missing, replaced by another plugin with the
//! same ID, or too old, so its document names the plugin by ID and source
//! repository and records the version it was written with and the versions it
//! accepts. Resolution compares that record with the installed plugins and
//! marks the configurations it cannot run instead of failing the whole list.

use super::*;
use lithe_plugin_model::{
    github_repository, is_plugin_id, PluginVersion, PluginVersionRequirement,
};

/// `schemaVersion` of a plugin run document.
pub(crate) const PLUGIN_DOCUMENT_SCHEMA_VERSION: i64 = 1;

/// TOML has no null. A `null` tool choice clears an inherited one, so plugin
/// documents spell it as this string.
const AUTOMATIC_TOOL: &str = "automatic";

/// The plugin a configuration needs, as its document records it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDependency {
    /// Plugin ID.
    pub id: String,
    /// Display name when the document was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Owner of the source repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// `https://github.com/<owner>/<name>` source repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Plugin version the configurations were last saved with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Plugin versions that can run the configurations, such as `>=0.3.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_requirement: Option<String>,
}

impl PluginDependency {
    /// A dependency known only by ID, for a configuration outside a plugin
    /// document that names its plugin.
    pub(crate) fn named(id: &str) -> Self {
        Self {
            id: id.to_string(),
            ..Self::default()
        }
    }
}

/// Why a resolved configuration cannot run. Absent when it can.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    /// Always `unavailable`.
    pub status: String,
    /// `pluginMissing`, `pluginRepositoryMismatch`, `pluginVersionUnsupported`,
    /// `invalidConfiguration` or `unsupportedPlatform`.
    pub code: String,
    /// English explanation for logs; hosts show text for `code`.
    pub message: String,
}

impl Availability {
    pub(crate) fn unavailable(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: "unavailable".to_string(),
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// One installed, enabled plugin as the host reports it: its `plugin.json`,
/// of which only these fields are read.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    /// Plugin ID.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Installed version, `major.minor.patch`.
    pub version: String,
    /// Source repository, when the manifest declares one.
    #[serde(default)]
    pub repository: Option<String>,
    /// Plugin API version of the manifest.
    #[serde(default, rename = "pluginAPIVersion")]
    pub plugin_api_version: Option<u32>,
    /// Capability versions the manifest requires.
    #[serde(default)]
    pub requires: Option<Value>,
}

/// A parsed plugin run document.
#[derive(Debug, Clone)]
pub(crate) struct PluginDocument {
    /// Every top-level key except `configurations`, kept so a newer Lithe's
    /// additions survive an edit.
    pub(crate) header: serde_json::Map<String, Value>,
    pub(crate) dependency: PluginDependency,
    pub(crate) configurations: Vec<Value>,
}

/// Parses a plugin run document; the error explains what is wrong with it.
pub(crate) fn parse(text: &str) -> Result<PluginDocument, String> {
    let table = text
        .parse::<toml::Table>()
        .map_err(|error| format!("TOML is invalid: {}", error.message()))?;
    let Value::Object(mut header) = toml_to_json(toml::Value::Table(table)) else {
        unreachable!("a TOML table converts to an object")
    };
    if header.get("schemaVersion").and_then(Value::as_i64) != Some(PLUGIN_DOCUMENT_SCHEMA_VERSION) {
        return Err(format!(
            "schemaVersion must be {PLUGIN_DOCUMENT_SCHEMA_VERSION}"
        ));
    }
    let dependency: PluginDependency = header
        .get("plugin")
        .filter(|plugin| plugin.is_object())
        .and_then(|plugin| serde_json::from_value(plugin.clone()).ok())
        .ok_or_else(|| "the [plugin] table must give the plugin id".to_string())?;
    if !is_plugin_id(&dependency.id) {
        return Err(format!("plugin.id is not a plugin ID: {}", dependency.id));
    }
    if let Some(repository) = &dependency.repository {
        if github_repository(repository).is_none() {
            return Err(format!(
                "plugin.repository must be https://github.com/<owner>/<name>: {repository}"
            ));
        }
    }
    let mut configurations = match header.remove("configurations") {
        None => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => return Err("configurations must be an array of tables".to_string()),
    };
    for configuration in &mut configurations {
        if configuration.get("id").and_then(Value::as_str).is_none() {
            return Err("every configuration needs a string id".to_string());
        }
        restore_automatic_tools(configuration);
    }
    Ok(PluginDocument {
        header,
        dependency,
        configurations,
    })
}

/// Renders a plugin run document: `schemaVersion`, the `[plugin]` table, any
/// other header keys, then one `[[configurations]]` table per entry.
pub(crate) fn render(
    header: &serde_json::Map<String, Value>,
    configurations: &[Value],
) -> Result<String, CoreError> {
    let mut table = toml::Table::new();
    table.insert(
        "schemaVersion".to_string(),
        toml::Value::Integer(PLUGIN_DOCUMENT_SCHEMA_VERSION),
    );
    for key in std::iter::once("plugin").chain(
        header
            .keys()
            .map(String::as_str)
            .filter(|key| !["schemaVersion", "plugin"].contains(key)),
    ) {
        if let Some(value) = header
            .get(key)
            .and_then(|value| json_to_toml(value).transpose())
        {
            table.insert(key.to_string(), value?);
        }
    }
    let mut items = Vec::with_capacity(configurations.len());
    for configuration in configurations {
        let mut configuration = configuration.clone();
        spell_automatic_tools(&mut configuration);
        if let Some(value) = json_to_toml(&ordered_configuration(&configuration))? {
            items.push(value);
        }
    }
    table.insert("configurations".to_string(), toml::Value::Array(items));
    toml::to_string_pretty(&table).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration cannot be written as TOML",
        )
        .with_details(error.to_string())
    })
}

/// The header a write stores: the previous one with the installed plugin's
/// identity, version and capabilities refreshed. A version requirement the
/// user wrote is kept; a new document accepts the saving version and later.
pub(crate) fn refreshed_header(
    existing: Option<&serde_json::Map<String, Value>>,
    plugin_id: &str,
    installed: Option<&InstalledPlugin>,
) -> serde_json::Map<String, Value> {
    let mut header = existing.cloned().unwrap_or_default();
    header.insert(
        "schemaVersion".to_string(),
        json!(PLUGIN_DOCUMENT_SCHEMA_VERSION),
    );
    let mut plugin = header
        .get("plugin")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    plugin.insert("id".to_string(), json!(plugin_id));
    if let Some(installed) = installed {
        if let Some(name) = installed.display_name.as_deref() {
            plugin.insert("name".to_string(), json!(name));
        }
        if let Some(repository) = installed.repository.as_deref() {
            if let Some((owner, _)) = github_repository(repository) {
                plugin.insert("author".to_string(), json!(owner));
                plugin.insert("repository".to_string(), json!(repository));
            }
        }
        plugin.insert("version".to_string(), json!(installed.version));
        plugin
            .entry("versionRequirement")
            .or_insert_with(|| json!(format!(">={}", installed.version)));
        if let Some(api) = installed.plugin_api_version {
            plugin.insert("pluginAPIVersion".to_string(), json!(api));
        }
        if let Some(requires) = installed
            .requires
            .as_ref()
            .filter(|value| value.is_object())
        {
            plugin.insert("capabilities".to_string(), requires.clone());
        }
    }
    header.insert("plugin".to_string(), Value::Object(plugin));
    header
}

/// Why `dependency` cannot run with the installed plugins, or `None` when it
/// can or the host did not report them.
pub(crate) fn availability(
    dependency: &PluginDependency,
    installed: Option<&[InstalledPlugin]>,
) -> Option<Availability> {
    let installed = installed?;
    let Some(plugin) = installed.iter().find(|plugin| plugin.id == dependency.id) else {
        return Some(Availability::unavailable(
            "pluginMissing",
            format!("Plugin {} is not installed or not enabled", dependency.id),
        ));
    };
    if let (Some(recorded), Some(actual)) = (
        dependency.repository.as_deref().and_then(github_repository),
        plugin.repository.as_deref().and_then(github_repository),
    ) {
        if !recorded.0.eq_ignore_ascii_case(actual.0) || !recorded.1.eq_ignore_ascii_case(actual.1)
        {
            return Some(Availability::unavailable(
                "pluginRepositoryMismatch",
                format!(
                    "Plugin {} comes from {}/{}, not {}/{}",
                    dependency.id, actual.0, actual.1, recorded.0, recorded.1
                ),
            ));
        }
    }
    let requirement = dependency.version_requirement.as_deref()?;
    let Some(parsed) = PluginVersionRequirement::parse(requirement) else {
        return Some(Availability::unavailable(
            "invalidConfiguration",
            format!("Plugin version requirement is invalid: {requirement}"),
        ));
    };
    if PluginVersion::parse(&plugin.version).is_some_and(|version| parsed.matches(version)) {
        return None;
    }
    Some(Availability::unavailable(
        "pluginVersionUnsupported",
        format!(
            "Plugin {} {} does not satisfy {requirement}",
            dependency.id, plugin.version
        ),
    ))
}

/// Puts `id`, `name`, `provider` and `execution` first so a stored entry
/// reads from its identity down; the other keys keep their order.
pub(crate) fn ordered_configuration(configuration: &Value) -> Value {
    let Some(object) = configuration.as_object() else {
        return configuration.clone();
    };
    const LEADING: [&str; 4] = ["id", "name", "provider", "execution"];
    let mut ordered = serde_json::Map::new();
    for key in LEADING {
        if let Some(value) = object.get(key) {
            ordered.insert(key.to_string(), value.clone());
        }
    }
    for (key, value) in object {
        if !LEADING.contains(&key.as_str()) {
            ordered.insert(key.clone(), value.clone());
        }
    }
    Value::Object(ordered)
}

fn tool_choices(configuration: &mut Value) -> Option<&mut serde_json::Map<String, Value>> {
    configuration
        .get_mut("extensions")?
        .get_mut("build-run")?
        .get_mut("tools")?
        .as_object_mut()
}

fn spell_automatic_tools(configuration: &mut Value) {
    if let Some(tools) = tool_choices(configuration) {
        for choice in tools.values_mut().filter(|choice| choice.is_null()) {
            *choice = json!(AUTOMATIC_TOOL);
        }
    }
}

fn restore_automatic_tools(configuration: &mut Value) {
    if let Some(tools) = tool_choices(configuration) {
        for choice in tools.values_mut() {
            if choice.as_str() == Some(AUTOMATIC_TOOL) {
                *choice = Value::Null;
            }
        }
    }
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::String(text),
        toml::Value::Integer(number) => json!(number),
        toml::Value::Float(number) => json!(number),
        toml::Value::Boolean(flag) => Value::Bool(flag),
        toml::Value::Datetime(datetime) => Value::String(datetime.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, value)| (key, toml_to_json(value)))
                .collect(),
        ),
    }
}

/// TOML has no null, so a null is left out; `Ok(None)` is such a value.
fn json_to_toml(value: &Value) -> Result<Option<toml::Value>, CoreError> {
    Ok(Some(match value {
        Value::Null => return Ok(None),
        Value::Bool(flag) => toml::Value::Boolean(*flag),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                toml::Value::Integer(integer)
            } else if let Some(float) = number.as_f64().filter(|_| !number.is_u64()) {
                toml::Value::Float(float)
            } else {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Run configuration number does not fit in TOML",
                )
                .with_details(number.to_string()));
            }
        }
        Value::String(text) => toml::Value::String(text.clone()),
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                if let Some(item) = json_to_toml(item)? {
                    converted.push(item);
                }
            }
            toml::Value::Array(converted)
        }
        Value::Object(object) => {
            let mut table = toml::Table::new();
            for (key, item) in object {
                if let Some(item) = json_to_toml(item)? {
                    table.insert(key.clone(), item);
                }
            }
            toml::Value::Table(table)
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(version: &str, repository: Option<&str>) -> InstalledPlugin {
        InstalledPlugin {
            id: "dev.lithe.test.build".to_string(),
            display_name: Some("Test Build".to_string()),
            version: version.to_string(),
            repository: repository.map(str::to_string),
            plugin_api_version: Some(0),
            requires: Some(json!({"buildProviders": 0})),
        }
    }

    // A tool choice reset to automatic is null in JSON; TOML has no null, so
    // it must survive a write and read as the `automatic` spelling.
    #[test]
    fn rendered_documents_parse_back_to_the_same_configurations() {
        let header = refreshed_header(
            None,
            "dev.lithe.test.build",
            Some(&installed(
                "1.4.0",
                Some("https://github.com/example/build"),
            )),
        );
        let configurations = vec![json!({
            "provider": "build.provider",
            "id": "user:serve",
            "name": "serve",
            "args": ["--port", "8080"],
            "env": {"MODE": "dev"},
            "extensions": {
                "build": {"pluginID": "dev.lithe.test.build", "run": "serve"},
                "build-run": {"tools": {"dev.lithe.test.build/jdk": null}}
            }
        })];
        let text = render(&header, &configurations).unwrap();
        assert!(text.starts_with("schemaVersion = 1\n"), "{text}");
        assert!(text.contains("[[configurations]]"), "{text}");
        assert!(text.contains("\"automatic\""), "{text}");
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.configurations, configurations);
        assert_eq!(parsed.dependency.author.as_deref(), Some("example"));
        assert_eq!(
            parsed.dependency.version_requirement.as_deref(),
            Some(">=1.4.0")
        );
        assert_eq!(parsed.header["plugin"]["capabilities"]["buildProviders"], 0);
    }

    #[test]
    fn availability_compares_the_installed_plugin_with_the_record() {
        let mut dependency = PluginDependency {
            id: "dev.lithe.test.build".to_string(),
            repository: Some("https://github.com/example/build".to_string()),
            version_requirement: Some("^1.2.0".to_string()),
            ..PluginDependency::default()
        };
        let code = |plugins: &[InstalledPlugin], dependency: &PluginDependency| {
            availability(dependency, Some(plugins)).map(|availability| availability.code)
        };
        assert_eq!(availability(&dependency, None), None);
        assert_eq!(code(&[], &dependency).as_deref(), Some("pluginMissing"));
        assert_eq!(
            code(
                &[installed(
                    "1.3.0",
                    Some("https://github.com/Example/Build/")
                )],
                &dependency
            ),
            None
        );
        assert_eq!(
            code(
                &[installed("1.3.0", Some("https://github.com/other/build"))],
                &dependency
            )
            .as_deref(),
            Some("pluginRepositoryMismatch")
        );
        assert_eq!(
            code(&[installed("2.0.0", None)], &dependency).as_deref(),
            Some("pluginVersionUnsupported")
        );
        dependency.version_requirement = Some(">=one".to_string());
        assert_eq!(
            code(&[installed("1.3.0", None)], &dependency).as_deref(),
            Some("invalidConfiguration")
        );
    }

    #[test]
    fn malformed_documents_are_explained() {
        for (text, expected) in [
            ("schemaVersion = 2\n[plugin]\nid = \"a.b\"\n", "schemaVersion"),
            ("schemaVersion = 1\n", "[plugin]"),
            ("schemaVersion = 1\n[plugin]\nid = \"A B\"\n", "plugin ID"),
            (
                "schemaVersion = 1\n[plugin]\nid = \"a.b\"\nrepository = \"https://example.com/a/b\"\n",
                "repository",
            ),
            (
                "schemaVersion = 1\n[plugin]\nid = \"a.b\"\n[[configurations]]\nname = \"x\"\n",
                "id",
            ),
            ("schemaVersion = ", "TOML"),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains(expected), "{text}: {error}");
        }
    }
}
