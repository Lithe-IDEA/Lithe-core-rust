//! Manifest validation: the outcome entry points and the checks that span the whole
//! manifest.

use super::*;

/// Validates a raw manifest value and returns its normalized form as JSON, the
/// shape [`lithe_toolchain::ManifestValidator`] gives toolchain policy.
pub fn validated_manifest_json(value: Value) -> Validation<Value> {
    let manifest = validate_manifest_value(value)?;
    serde_json::to_value(manifest)
        .map_err(|_| invalid("manifest", "The validated manifest cannot be encoded."))
}

/// Validates a raw manifest value and returns its normalized form.
pub fn validate_manifest_value(value: Value) -> Validation<DeclarativePluginManifest> {
    // Check the version gates first so a future schema reports the precise
    // reason instead of an unknown-field decoding failure.
    if let Some(version) = value.get("schemaVersion").and_then(Value::as_u64) {
        if version != u64::from(DECLARATIVE_MANIFEST_SCHEMA_VERSION) {
            return Err(PluginRejection::at(
                "plugin.schema.unsupported",
                "schemaVersion",
                format!("Plugin manifest schema {version} is not supported."),
            ));
        }
    }
    if let Some(version) = value.get("pluginAPIVersion").and_then(Value::as_u64) {
        if !SUPPORTED_PLUGIN_API_VERSIONS
            .iter()
            .any(|supported| u64::from(*supported) == version)
        {
            return Err(PluginRejection::at(
                "plugin.api.unsupported",
                "pluginAPIVersion",
                format!("Plugin API {version} is not supported by this version of Lithe."),
            ));
        }
        capabilities::check_manifest_requires(&value)?;
    }
    for (field, limit) in [
        ("toolDefinitions", 32),
        ("toolchainProviders", 8),
        (
            "keymapImportProviders",
            lithe_plugin_keymap_import::MAX_PROVIDERS,
        ),
    ] {
        if value.get(field).is_some_and(|section| {
            !section
                .as_array()
                .is_some_and(|items| !items.is_empty() && items.len() <= limit)
        }) {
            return Err(invalid(
                field,
                "A declared provider section must be nonempty and bounded.",
            ));
        }
    }
    let manifest: DeclarativePluginManifest = serde_json::from_value(value).map_err(|error| {
        PluginRejection::new(
            "plugin.manifest.invalid",
            format!("The plugin manifest is invalid: {error}"),
        )
    })?;
    manifest.validated()
}

impl DeclarativePluginManifest {
    fn validated(mut self) -> Validation<Self> {
        if !is_plugin_id(&self.id) {
            return Err(invalid(
                "id",
                "The plugin ID must use lowercase letters, digits, dots, and hyphens.",
            ));
        }
        if !is_semantic_version(&self.version) {
            return Err(invalid(
                "version",
                "The plugin version must use major.minor.patch.",
            ));
        }
        if self.display_name.trim().is_empty() {
            return Err(invalid("displayName", "The plugin display name is empty."));
        }
        if self.vendor.id.trim().is_empty() || self.vendor.display_name.trim().is_empty() {
            return Err(invalid("vendor", "The plugin vendor is incomplete."));
        }
        if self.vendor.signature_requirement == SignatureRequirement::None
            && is_official_namespace(&self.id)
        {
            return Err(invalid(
                "vendor.signatureRequirement",
                "Official plugin IDs must carry a Lithe signature.",
            ));
        }
        if let Some(bundled) = self.vendor.bundled_signature_requirement {
            if self.vendor.signature_requirement != SignatureRequirement::PublisherPackage
                || bundled != SignatureRequirement::SameTeamAsHost
                || !is_official_namespace(&self.id)
            {
                return Err(invalid(
                    "vendor.bundledSignatureRequirement",
                    "Bundled host-seal trust requires an official plugin ID and publisherPackage for installed copies.",
                ));
            }
        }
        capabilities::check_used(&self.requires, &self.used_capabilities())?;
        if self.languages.is_empty()
            && self.ui.is_none()
            && self.build_systems.is_empty()
            && self.vcs_providers.is_empty()
            && self.debug_adapters.is_empty()
            && self.project_generators.is_empty()
            && self.tool_definitions.is_empty()
            && self.toolchain_providers.is_empty()
            && self.keymap_import_providers.is_empty()
            && self.output_link_matchers.is_empty()
            // A backend may be the whole plugin: it starts on a workspace.
            && self.backend.is_none()
        {
            return Err(invalid("languages", "The plugin declares no languages."));
        }

        let mut language_ids = BTreeSet::new();
        for (index, language) in self.languages.iter_mut().enumerate() {
            language.normalize(index)?;
            if !language_ids.insert(language.id.clone()) {
                return Err(invalid(
                    format!("languages[{index}].id"),
                    "A language is declared more than once.",
                ));
            }
        }
        self.languages.sort_by(|left, right| left.id.cmp(&right.id));

        let declared_settings = self.validate_settings()?;
        let required_tools = self.validate_required_tools()?;
        let runtime_tools = RuntimeTools::new(&required_tools);
        lithe_plugin_output_links::validate_declarations(
            &mut self.output_link_matchers,
            self.backend.is_some(),
        )?;

        let mut server_languages = BTreeSet::new();
        let mut referenced_immediate_settings = BTreeSet::new();
        for (index, server) in self.language_servers.iter_mut().enumerate() {
            let field = format!("languageServers[{index}]");
            if !language_ids.contains(&server.language_id) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "The language server references an undeclared language.",
                ));
            }
            if !server_languages.insert(server.language_id.clone()) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "A language declares more than one language server.",
                ));
            }
            if let Some(configuration) = &server.workspace_configuration {
                collect_setting_references(configuration, &mut referenced_immediate_settings);
            }
            server.validate(&field, &declared_settings, runtime_tools)?;
        }
        if let Some(id) =
            declared_settings.first_unreferenced_immediate(&referenced_immediate_settings)
        {
            return Err(invalid(
                "settings",
                format!("The immediate setting {id} must be used by workspaceConfiguration."),
            ));
        }
        self.language_servers
            .sort_by(|left, right| left.language_id.cmp(&right.language_id));

        let mut execution_languages = BTreeSet::new();
        for (index, provider) in self.execution_providers.iter_mut().enumerate() {
            let field = format!("executionProviders[{index}]");
            if !language_ids.contains(&provider.language_id) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "The execution provider references an undeclared language.",
                ));
            }
            if !execution_languages.insert(provider.language_id.clone()) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "A language declares more than one execution provider.",
                ));
            }
            provider.normalize(&field)?;
        }
        self.execution_providers
            .sort_by(|left, right| left.language_id.cmp(&right.language_id));

        if self.language_servers.is_empty()
            && self.execution_providers.is_empty()
            && self.ui.is_none()
            && self.build_systems.is_empty()
            && self.vcs_providers.is_empty()
            && self.debug_adapters.is_empty()
            && self.project_generators.is_empty()
            && self.tool_definitions.is_empty()
            && self.toolchain_providers.is_empty()
            && self.keymap_import_providers.is_empty()
            // Console links alone are a complete plugin: Core matches them.
            && self.output_link_matchers.is_empty()
            && self.backend.is_none()
            // A grammar alone is a complete highlighting plugin.
            && !self
                .languages
                .iter()
                .any(|language| language.grammar.is_some() || language.configuration.is_some())
        {
            return Err(invalid(
                "languages",
                "The plugin declares no language server or execution provider.",
            ));
        }
        self.validate_ui()?;
        lithe_project_generators::validate_declarations(&self.project_generators)?;
        lithe_build::validate_declarations(
            &mut self.build_systems,
            &lithe_build::DeclarationContext {
                debug_adapters: &self.debug_adapters,
                declared_settings: &declared_settings,
                language_servers: &server_languages,
                runtime_tools,
            },
        )?;
        lithe_vcs::validate_declarations(
            &mut self.vcs_providers,
            &lithe_vcs::DeclarationContext {
                declared_settings: &declared_settings,
                runtime_tools,
            },
        )?;
        self.validate_debug_adapters(&declared_settings, runtime_tools)?;
        self.validate_backend_launches()?;
        self.validate_toolchains()?;
        if !self.keymap_import_providers.is_empty() {
            lithe_plugin_keymap_import::validate_declarations(&mut self.keymap_import_providers)?;
            self.validate_packaged_provider_backend()?;
        }
        self.validate_plugin_functions()?;
        if let Some(backend) = &self.backend {
            backend.validate(&lithe_plugin_backend::DeclarationContext {
                declared_settings: &declared_settings,
                has_web_window: self
                    .ui
                    .as_ref()
                    .is_some_and(|ui| ui.tool_windows.iter().any(|window| window.web.is_some())),
                has_command: self.ui.as_ref().is_some_and(|ui| !ui.commands.is_empty()),
                activates_on_workspace_open: self
                    .activation_events
                    .iter()
                    .any(|event| event == lithe_plugin_backend::ON_WORKSPACE_OPEN),
                // A language server the backend launches starts the backend
                // when the host asks it for the server's command line; an
                // exported function starts it when another plugin calls it.
                // Neither needs a page, command or workspace event.
                provider_query_trigger: !self.toolchain_providers.is_empty()
                    || !self.keymap_import_providers.is_empty()
                    || self
                        .language_servers
                        .iter()
                        .any(|server| server.process.is_backend_launch())
                    || self.plugin_functions.as_ref().is_some_and(
                        lithe_plugin_functions::PluginFunctionsDeclaration::exports_any,
                    ),
                runtime_tools,
            })?;
        }
        self.validate_permissions()?;
        self.validate_activation_events()?;
        Ok(self)
    }

    /// Capabilities whose sections the manifest declares. Launches, tool
    /// references, and executable candidates belong to the base contract and
    /// are never listed.
    fn used_capabilities(&self) -> BTreeSet<&'static str> {
        let sections = [
            (
                capabilities::PACKAGE_TRUST,
                self.vendor.bundled_signature_requirement.is_some(),
            ),
            (capabilities::LANGUAGES, !self.languages.is_empty()),
            (
                capabilities::LANGUAGE_SERVERS,
                !self.language_servers.is_empty(),
            ),
            (
                capabilities::EXECUTION_PROVIDERS,
                !self.execution_providers.is_empty(),
            ),
            (
                capabilities::SETTINGS,
                !self.settings.is_empty() || !self.setting_actions.is_empty(),
            ),
            (capabilities::UI, self.ui.is_some()),
            (capabilities::BUILD_SYSTEMS, !self.build_systems.is_empty()),
            (
                capabilities::PROJECT_GENERATORS,
                !self.project_generators.is_empty(),
            ),
            (
                capabilities::BUILD_PROVIDERS,
                self.build_systems
                    .iter()
                    .any(|system| system.provider.is_some()),
            ),
            (
                capabilities::BUILD_MODEL_INPUTS,
                self.build_systems.iter().any(|system| {
                    system.model_sharing.is_some() || !system.model_inputs.is_empty()
                }),
            ),
            (capabilities::VCS_PROVIDERS, !self.vcs_providers.is_empty()),
            (
                capabilities::DEBUG_ADAPTERS,
                !self.debug_adapters.is_empty(),
            ),
            (capabilities::BACKEND, self.backend.is_some()),
            (
                capabilities::TOOL_DEFINITIONS,
                !self.tool_definitions.is_empty(),
            ),
            (
                capabilities::TOOLCHAIN_PROVIDERS,
                !self.toolchain_providers.is_empty(),
            ),
            (
                capabilities::KEYMAP_IMPORT_PROVIDERS,
                !self.keymap_import_providers.is_empty(),
            ),
            (
                capabilities::OUTPUT_LINK_MATCHERS,
                !self.output_link_matchers.is_empty(),
            ),
            (
                capabilities::PLUGIN_FUNCTIONS,
                self.plugin_functions.is_some(),
            ),
        ];
        sections
            .into_iter()
            .filter_map(|(capability, used)| used.then_some(capability))
            .collect()
    }

    /// Checks the debug adapters against the plugin's settings, language
    /// servers, and `requiresTools`.
    fn validate_debug_adapters(
        &mut self,
        declared_settings: &DeclaredSettings,
        runtime_tools: RuntimeTools<'_>,
    ) -> Validation<()> {
        let language_servers: BTreeSet<String> = self
            .language_servers
            .iter()
            .map(|server| server.language_id.clone())
            .collect();
        lithe_debug::adapters::validate_declarations(
            &mut self.debug_adapters,
            &lithe_debug::adapters::DeclarationContext {
                declared_settings,
                language_servers: &language_servers,
                runtime_tools,
            },
        )
    }

    /// A language server the plugin backend launches asks the backend for its
    /// command line, so the manifest must declare that backend.
    fn validate_backend_launches(&self) -> Validation<()> {
        if self.backend.is_none()
            && self
                .language_servers
                .iter()
                .any(|server| server.process.is_backend_launch())
        {
            return Err(invalid(
                "backend",
                "A language server launched by the plugin backend requires the backend section.",
            ));
        }
        Ok(())
    }

    fn validate_toolchains(&self) -> Validation<()> {
        if self.tool_definitions.is_empty() && self.toolchain_providers.is_empty() {
            return Ok(());
        }
        lithe_toolchain::definitions::validate_declarations(
            &self.id,
            &self.tool_definitions,
            &self.toolchain_providers,
        )?;
        if !self.toolchain_providers.is_empty() {
            self.validate_packaged_provider_backend()?;
        }
        Ok(())
    }

    /// Checks `pluginFunctions`: only the plugin backend registers functions
    /// for the protocol that relays calls.
    fn validate_plugin_functions(&mut self) -> Validation<()> {
        let Some(section) = self.plugin_functions.as_mut() else {
            return Ok(());
        };
        if self.backend.is_none() {
            return Err(invalid(
                "pluginFunctions",
                "Exported and called plugin functions need the plugin backend.",
            ));
        }
        lithe_plugin_functions::validate(section)
    }

    /// Host-queried providers must start without relying on the discovery they provide.
    fn validate_packaged_provider_backend(&self) -> Validation<()> {
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| invalid("backend", "A provider requires the plugin backend."))?;
        let value = serde_json::to_value(backend)
            .map_err(|_| invalid("backend", "The backend cannot be encoded."))?;
        let launch = &value["launch"];
        let packaged = launch["kind"] == "packaged"
            || (launch["kind"] == "runtime" && launch["packagedRuntime"].as_str().is_some());
        if !packaged
            || value["runtimeDependencies"]
                .as_array()
                .is_some_and(|values| !values.is_empty())
        {
            return Err(invalid("backend.launch", "Provider backends must bootstrap from packaged executables or packaged runtimes without discovered dependencies."));
        }
        Ok(())
    }
}

impl DeclarativePluginManifest {
    fn validate_activation_events(&mut self) -> Validation<()> {
        let mut expected = BTreeSet::new();
        for server in &self.language_servers {
            expected.insert(format!("onLanguage:{}", server.language_id));
        }
        for provider in &self.execution_providers {
            if provider.run.is_some() {
                expected.insert(format!("onRun:{}", provider.language_id));
            }
            if provider.test.is_some() {
                expected.insert(format!("onTest:{}", provider.language_id));
            }
        }
        for build_system in &self.build_systems {
            expected.insert(lithe_build::activation_event(build_system));
        }
        for provider in &self.vcs_providers {
            expected.insert(lithe_vcs::activation_event(provider));
        }
        for adapter in &self.debug_adapters {
            expected.insert(lithe_debug::adapters::activation_event(adapter));
        }
        if let Some(ui) = &self.ui {
            for window in &ui.tool_windows {
                expected.insert(format!("onView:{}", window.id));
            }
            for command in &ui.commands {
                expected.insert(format!("onCommand:{}", command.id));
            }
        }
        self.activation_events = sorted_unique(std::mem::take(&mut self.activation_events));
        let mut declared: BTreeSet<String> = self.activation_events.iter().cloned().collect();
        // The backend may start once a workspace opens. The event is optional
        // because pages and commands can start it too, so it is checked here
        // and kept out of the exact provider match below.
        if self.backend.is_some() {
            declared.remove(lithe_plugin_backend::ON_WORKSPACE_OPEN);
        }
        // Every provider needs an event, and every event needs a provider:
        // an event without one would promise activation the host cannot do.
        if let Some(missing) = expected.difference(&declared).next() {
            return Err(PluginRejection::at(
                "plugin.activation.invalid",
                "activationEvents",
                format!("The plugin must declare the activation event {missing}."),
            ));
        }
        if let Some(extra) = declared.difference(&expected).next() {
            return Err(PluginRejection::at(
                "plugin.activation.invalid",
                "activationEvents",
                format!("The activation event {extra} has no matching provider."),
            ));
        }
        Ok(())
    }

    pub(crate) fn execution_provider(
        &self,
        language_id: &str,
    ) -> Option<&ExecutionProviderDeclaration> {
        self.execution_providers
            .iter()
            .find(|provider| provider.language_id == language_id)
    }
}

impl DeclarativePluginManifest {
    /// Checks `requiresTools` and returns its IDs. Every tool a process names
    /// is one of them.
    fn validate_required_tools(&mut self) -> Validation<BTreeSet<String>> {
        if self.requires_tools.is_empty() {
            return Ok(BTreeSet::new());
        }
        if self.requires_tools.len() > MAX_REQUIRED_TOOLS {
            return Err(invalid(
                "requiresTools",
                format!("A plugin requires at most {MAX_REQUIRED_TOOLS} tools."),
            ));
        }
        let mut tools = BTreeSet::new();
        for (index, tool) in self.requires_tools.iter().enumerate() {
            if !lithe_plugin_model::is_tool_id(tool) {
                return Err(invalid(
                    format!("requiresTools[{index}]"),
                    "A required tool is a short or plugin-qualified tool-definition ID.",
                ));
            }
            if !tools.insert(tool.clone()) {
                return Err(invalid(
                    format!("requiresTools[{index}]"),
                    "A tool is required more than once.",
                ));
            }
        }
        self.requires_tools = tools.iter().cloned().collect();
        Ok(tools)
    }
}
