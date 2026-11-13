//! `buildSystems` declarations and their validation.
//!
//! A declaration is data only. Everything a plugin needs is
//! expressible here: any command or packaged script as the executable, any
//! runtime it depends on (described by how to find and probe it), and any
//! output, either through a selector map over a public format or by printing
//! one of the canonical `lithe*` formats. What no command can print is
//! answered by a `provider`: a server process of the plugin, declared with
//! the language-server launch fields or shared with one of its language
//! servers. The host never special-cases a build tool by name:
//! `hostIntegration` is reserved and refused whatever it names.

use super::select::{self, FieldSpec, Predicate};
use super::template::{self, Allowed, Availability, BuildArgument, Variable};
use lithe_plugin_model::process::{ServerProcess, ServerRole};
use lithe_plugin_model::settings::is_setting_identifier;
use lithe_plugin_model::{
    invalid, invalid_template as template_error, is_bare_command, is_identifier,
    is_safe_relative_path, sorted_unique, validate_minimum_version, PluginRejection, UIIcon,
    Validation,
};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

mod checks;
mod dependencies;
mod detection;
mod diagnostics;
mod invocation;
mod model;
mod options;
mod provider;
mod run;
mod tasks;
mod testing;

pub use checks::*;
pub(crate) use dependencies::*;
pub(crate) use detection::*;
pub(crate) use diagnostics::*;
pub(crate) use invocation::*;
pub(crate) use model::*;
pub(crate) use options::*;
pub use provider::*;
pub(crate) use run::*;
pub(crate) use tasks::*;
pub(crate) use testing::*;

const MAX_BUILD_SYSTEMS: usize = 16;
const MAX_LIST: usize = 32;
const MAX_TASKS: usize = 128;
const MAX_CHOICES: usize = 64;
const MAX_ARGUMENTS: usize = 64;
const MAX_TEXT: usize = 120;
const MAX_OPTION_VALUE: usize = 256;
const MAX_REGEX: usize = 512;
const MAX_IDENTIFIER: usize = 64;
const MAX_PRIORITY: u8 = 100;
const DEFAULT_PRIORITY: u8 = 50;
const DEFAULT_SEPARATOR: &str = ",";

/// Deepest heading path a task can be filed under, e.g. `Plugins › compiler`.
pub(crate) const MAX_CATEGORY_DEPTH: usize = 4;

/// One project-level build tool a plugin describes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildSystemDeclaration {
    /// The build-system ID, unique within the plugin.
    pub id: String,
    pub(crate) display_name: String,
    /// Optional, explicit read-only publication of the normalized model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_sharing: Option<super::model_inputs::ModelSharing>,
    /// Exact optional producers; these are not installation dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_inputs: Vec<super::model_inputs::ModelInput>,
    /// Icon of the build system's tool window; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<UIIcon>,
    pub(crate) detection: Detection,
    pub(crate) invocation: Invocation,
    /// Reserved. Core implements no build-system host integration: every
    /// build tool is described by its plugin's data and backend, so any
    /// value is refused with `plugin.integration.unsupported`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host_integration: Option<Value>,
    /// The process that answers the `provider` sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) options: Vec<OptionDeclaration>,
    /// Native non-sensitive settings and generic write bindings from the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) configuration: Option<super::configuration::ConfigurationDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<ModelSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tasks: Vec<TaskDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) run: Vec<RunDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) test: Option<TestDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostics: Option<DiagnosticsDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dependencies: Option<DependenciesSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sync: Option<SyncDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) layout: Option<LayoutDeclaration>,
}

/// Validates and normalizes every build system of a manifest.
/// Normalizes the tags a selected target must carry; they only make sense
/// when the command can run on a target.
fn check_target_tags(tags: &mut Vec<String>, scopes: &[Scope], field: &str) -> Validation<()> {
    if !tags.is_empty() && !scopes.contains(&Scope::Target) {
        return Err(invalid(
            format!("{field}.targetTags"),
            "targetTags need the target scope.",
        ));
    }
    if tags.iter().any(|tag| !is_tag(tag)) {
        return Err(invalid(
            format!("{field}.targetTags"),
            "Tags are identifiers.",
        ));
    }
    *tags = sorted_unique(std::mem::take(tags));
    Ok(())
}

/// Facts of the rest of the manifest a build-system declaration is checked
/// against.
pub struct DeclarationContext<'a> {
    /// Debug adapters the plugin declares; a debug run names one of them.
    pub debug_adapters: &'a [lithe_debug::adapters::DebugAdapterDeclaration],
    /// Setting IDs a provider's launch templates may name.
    pub declared_settings: &'a lithe_plugin_model::settings::DeclaredSettings,
    /// Language IDs the plugin declares a language server for.
    pub language_servers: &'a BTreeSet<String>,
    /// Whether tool names are host runtimes or tool-definition IDs.
    pub runtime_tools: lithe_plugin_model::process::RuntimeTools<'a>,
}

/// Validates every build-system declaration of a manifest against the rest
/// of it, and sorts the declarations by ID.
pub fn validate_declarations(
    build_systems: &mut [BuildSystemDeclaration],
    context: &DeclarationContext<'_>,
) -> Validation<()> {
    if build_systems.is_empty() {
        return Ok(());
    }
    if build_systems.len() > MAX_BUILD_SYSTEMS {
        return Err(invalid(
            "buildSystems",
            "The plugin declares too many build systems.",
        ));
    }
    let mut ids = BTreeSet::new();
    for (index, build_system) in build_systems.iter_mut().enumerate() {
        let field = format!("buildSystems[{index}]");
        build_system.validate(&field, context)?;
        if !ids.insert(build_system.id.clone()) {
            return Err(invalid(
                format!("{field}.id"),
                "A build system is declared more than once.",
            ));
        }
    }
    build_systems.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

impl BuildSystemDeclaration {
    fn validate(&mut self, field: &str, context: &DeclarationContext<'_>) -> Validation<()> {
        if !is_entry_identifier(&self.id) {
            return Err(invalid(
                format!("{field}.id"),
                "A build-system ID starts with a letter and uses lowercase letters, digits, and hyphens.",
            ));
        }
        check_text(
            &self.display_name,
            &format!("{field}.displayName"),
            "display name",
        )?;
        if let Some(icon) = &self.icon {
            icon.validate_at(&format!("{field}.icon"))?;
        }
        self.validate_integration(field)?;
        self.validate_provider(field, context)?;
        self.validate_detection(field)?;
        let option_ids = self.validate_options(field)?;
        super::model_inputs::validate_declaration(self)?;
        self.validate_invocation(field, &option_ids, context)?;
        let model_templates = self.validate_tasks(field, &option_ids)?;
        self.validate_model(field, &option_ids)?;
        self.validate_run(field, &option_ids, &model_templates)?;
        self.validate_test(field, &option_ids)?;
        self.validate_diagnostics(field)?;
        self.validate_dependencies(field, &option_ids)?;
        if let Some(sync) = &mut self.sync {
            check_paths(&mut sync.watch, &format!("{field}.sync.watch"), false)?;
        }
        if let Some(layout) = &mut self.layout {
            check_paths(
                &mut layout.excluded,
                &format!("{field}.layout.excluded"),
                false,
            )?;
        }
        self.validate_option_emission(field)
    }
}
