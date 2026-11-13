//! The `provider` server process: its declaration and validation.

use super::*;

/// A server process that answers `lithe/build/*` requests over the LSP base
/// protocol. It is a language-server process: either one of its own, with
/// the same launch fields as `languageServers[]`, or the plugin's language
/// server of one language, which then answers build requests besides its
/// language features. The form is decided by `languageServer`, so a mistake
/// is reported against that form's fields.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)] // Decoded once per manifest validation.
pub enum ProviderDeclaration {
    /// The plugin's language server of one language answers build requests.
    LanguageServer(SharedProvider),
    /// A provider process of its own.
    Process(ProcessProvider),
}

/// Build requests go to the language server of `languageServer`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedProvider {
    #[serde(rename = "languageServer")]
    pub(crate) language_id: String,
    pub(crate) executes_project_code: bool,
    pub(crate) may_use_network: bool,
}

/// A provider process of its own.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessProvider {
    #[serde(flatten)]
    pub(crate) process: ServerProcess,
    pub(crate) executes_project_code: bool,
    pub(crate) may_use_network: bool,
}

impl<'de> Deserialize<'de> for ProviderDeclaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if value.get("languageServer").is_some() {
            SharedProvider::deserialize(value).map(Self::LanguageServer)
        } else {
            ProcessProvider::deserialize(value).map(Self::Process)
        }
        .map_err(serde::de::Error::custom)
    }
}

/// Which process a provider's requests go to.
pub enum ProviderProcess<'a> {
    /// The language server of this language ID.
    LanguageServer(&'a str),
    /// A process the build system declares for itself.
    Own(&'a ServerProcess),
}

impl ProviderDeclaration {
    /// The process the provider's requests go to.
    pub fn process(&self) -> ProviderProcess<'_> {
        match self {
            Self::LanguageServer(shared) => ProviderProcess::LanguageServer(&shared.language_id),
            Self::Process(own) => ProviderProcess::Own(&own.process),
        }
    }

    /// Shown to the user; whether a provider may run is decided by trust.
    pub fn executes_project_code(&self) -> bool {
        match self {
            Self::LanguageServer(shared) => shared.executes_project_code,
            Self::Process(own) => own.executes_project_code,
        }
    }

    /// Whether the provider may use the network; shown to the user.
    pub fn may_use_network(&self) -> bool {
        match self {
            Self::LanguageServer(shared) => shared.may_use_network,
            Self::Process(own) => own.may_use_network,
        }
    }
}

impl BuildSystemDeclaration {
    /// Whether the build system starts a provider process of its own.
    pub fn starts_own_provider(&self) -> bool {
        matches!(self.provider, Some(ProviderDeclaration::Process(_)))
    }
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_integration(&self, field: &str) -> Validation<()> {
        let Some(integration) = &self.host_integration else {
            return Ok(());
        };
        let message = match integration.as_str() {
            Some(name) => format!("The host has no build integration named {name}."),
            None => "The host has no build integration.".to_owned(),
        };
        Err(PluginRejection::at(
            "plugin.integration.unsupported",
            format!("{field}.hostIntegration"),
            message,
        ))
    }

    pub(crate) fn validate_provider(
        &self,
        field: &str,
        context: &DeclarationContext<'_>,
    ) -> Validation<()> {
        let provider_runs = self
            .run
            .iter()
            .any(|run| run.source == TaskSource::Provider);
        let provider_tests = self
            .test
            .as_ref()
            .is_some_and(|test| test.source == TaskSource::Provider);
        let uses = self.configuration.is_some()
            || matches!(self.model, Some(ModelSource::Provider))
            || provider_runs
            || matches!(self.dependencies, Some(DependenciesSource::Provider { .. }))
            || provider_tests
            || self
                .tasks
                .iter()
                .any(|task| task.source == TaskSource::Provider);
        let field = format!("{field}.provider");
        let Some(provider) = &self.provider else {
            if uses {
                return Err(invalid(field, "A provider source requires provider."));
            }
            return Ok(());
        };
        if self.configuration.is_some() && !matches!(provider, ProviderDeclaration::Process(_)) {
            return Err(invalid(
                &field,
                "Native configuration requires an owned provider process.",
            ));
        }
        if !uses {
            return Err(invalid(
                field,
                "No model, dependencies, task or test source uses the provider.",
            ));
        }
        for run in &self.run {
            if let Some(debug) = &run.debug {
                match (&debug.adapter, &debug.configuration_type) {
                    (Some(_), None) => {
                        crate::debug_preparation::adapter(run, context.debug_adapters)?;
                    }
                    (None, Some(kind))
                        if crate::run_preparation::valid_configuration_type(kind)
                            && run.execution == Some(BuildExecution::Service) => {}
                    _ => return Err(invalid(&field,
                        "Debug requires a same-package native adapter, or one registry configurationType for a service.")),
                }
            }
        }
        if (provider_runs
            || provider_tests
            || self
                .tasks
                .iter()
                .any(|task| task.source == TaskSource::Provider))
            && !matches!(provider, ProviderDeclaration::Process(_))
        {
            return Err(invalid(
                &field,
                "Provider execution requires a plugin-owned provider process.",
            ));
        }
        match provider {
            ProviderDeclaration::LanguageServer(shared) => {
                if !context.language_servers.contains(&shared.language_id) {
                    return Err(invalid(
                        format!("{field}.languageServer"),
                        "The provider names a language the plugin declares no language server for.",
                    ));
                }
                Ok(())
            }
            ProviderDeclaration::Process(own) => own.process.validate(
                &field,
                context.declared_settings,
                ServerRole::BuildProvider,
                context.runtime_tools,
            ),
        }
    }
}
