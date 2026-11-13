//! Build systems declared by API level 3 plugins: one model for every
//! project-level build tool, and the deterministic half of running one.
//!
//! A plugin describes a build tool as data (`buildSystems` in `plugin.json`):
//! how to recognize its projects, which executable runs it, which options,
//! tasks, run entries, and tests it has, and how to read its output. Core
//! validates the declaration, reads static models during the shared project
//! walk, expands argument templates into argv plans, and decodes model, test,
//! diagnostic, and dependency output into one normalized shape. Hosts resolve
//! executables, start processes, watch files, and store user choices.
//!
//! Nothing here starts a process or writes a file. Workspace files are only
//! read, and only where a command says so (static models, test reports, and
//! sync fingerprints).
//!
//! Note: the contract is recorded in .agents/notes/implemented/architecture/2026-10-17-build-system-plugin-contract.md

mod commands;
mod configuration;
mod debug_preparation;
mod decode;
mod detect;
mod invocation;
mod manifest;
mod model;
mod model_inputs;
mod options;
mod outputs;
mod plans;
mod project_options;
mod run_preparation;
mod run_start;
mod select;
mod stored_options;
mod template;
mod test_discovery;
mod test_outcome;
mod test_outcome_merge;

pub use commands::*;
pub use manifest::{
    activation_event, programs, validate_declarations, BuildSystemDeclaration, DeclarationContext,
    ProviderProcess,
};
pub use project_options::project_options_command;
pub use run_start::run_decode_start_command;
pub use stored_options::resolve_stored_options_command;
pub use test_outcome::resolve_test_run_outcome;
pub use test_outcome_merge::merge_test_outcomes;

use lithe_plugin_model::{PluginManifest, Validation};
use serde_json::Value;
use std::collections::BTreeMap;

/// What the build commands read from a validated plugin manifest.
///
/// Every command that takes a manifest validates it again through
/// [`PluginManifest::validate`], so a host never has to trust its own cache
/// of a manifest; the plugin host implements this trait for its manifest type.
pub trait BuildManifest: PluginManifest {
    /// The build systems the plugin declares, in declaration order.
    fn build_systems(&self) -> &[BuildSystemDeclaration];

    /// The tool definition IDs the plugin requires.
    fn required_tools(&self) -> &[String];

    /// The executable path a declared executable setting selects, from the
    /// stored setting values. Strict: an invalid stored value is rejected
    /// rather than replaced by the setting's default.
    fn executable_setting_value(
        &self,
        id: &str,
        values: &BTreeMap<String, Value>,
    ) -> Validation<Option<String>>;
}
