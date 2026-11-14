//! Trait implementations through which the domain crates read a validated manifest.

use super::*;

impl lithe_plugin_model::PluginManifest for DeclarativePluginManifest {
    fn validate(value: Value) -> Validation<Self> {
        validate_manifest_value(value)
    }

    fn id(&self) -> &str {
        &self.id
    }
}

impl lithe_plugin_keymap_import::KeymapImportManifest for DeclarativePluginManifest {
    fn keymap_import_providers(&self) -> &[lithe_plugin_keymap_import::ProviderDeclaration] {
        &self.keymap_import_providers
    }
}

impl lithe_vcs::VcsManifest for DeclarativePluginManifest {
    fn vcs_providers(&self) -> &[lithe_vcs::VcsProviderDeclaration] {
        &self.vcs_providers
    }
}

impl lithe_build::BuildManifest for DeclarativePluginManifest {
    /// Validated build systems, sorted by ID.
    fn build_systems(&self) -> &[lithe_build::BuildSystemDeclaration] {
        &self.build_systems
    }

    /// Tool-definition IDs the plugin may start processes with, sorted.
    fn required_tools(&self) -> &[String] {
        &self.requires_tools
    }

    /// Strict executable selection preserves invalid stored values instead of
    /// applying the UI settings resolver's tolerant fallback-to-default rule.
    fn executable_setting_value(
        &self,
        id: &str,
        values: &BTreeMap<String, Value>,
    ) -> Validation<Option<String>> {
        let setting = self
            .settings
            .iter()
            .find(|setting| setting.id == id)
            .ok_or_else(|| {
                invalid(
                    "executables.settingID",
                    "The executable setting is not declared.",
                )
            })?;
        setting.executable_value(values.get(id))
    }
}

impl lithe_debug::DebugManifest for DeclarativePluginManifest {
    fn debug_adapters(&self) -> &[lithe_debug::adapters::DebugAdapterDeclaration] {
        &self.debug_adapters
    }
}

impl lithe_project_generators::GeneratorManifest for DeclarativePluginManifest {
    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn requires(&self) -> &Requires {
        &self.requires
    }

    fn project_generators(&self) -> &[lithe_project_generators::ProjectGeneratorDeclaration] {
        &self.project_generators
    }

    fn declares_language(&self, language_id: &str) -> bool {
        self.languages
            .iter()
            .any(|language| language.id == language_id)
    }

    fn declares_build_system(&self, build_system_id: &str) -> bool {
        self.build_systems
            .iter()
            .any(|system| system.id == build_system_id)
    }

    /// Whether the plugin declares a definition that `tool` names. A
    /// definition is addressed by its short ID or as `<pluginID>:<id>`; both
    /// spellings count as a declaration here. Whether the host adopts the
    /// short ID is its registry's decision: when several enabled plugins define
    /// the same short ID, it belongs to none of them.
    fn declares_tool(&self, tool: &str) -> bool {
        // `Some(local)` when `tool` is spelled `<this plugin>:<local>`.
        let local_name = tool
            .strip_prefix(self.id.as_str())
            .and_then(|rest| rest.strip_prefix(':'));
        self.tool_definitions
            .iter()
            .filter_map(|definition| definition["id"].as_str())
            .any(|id| tool == id || local_name == Some(id))
    }
}
