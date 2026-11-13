//! The vocabulary every part of a declarative plugin manifest shares.
//!
//! Language servers, build systems, VCS providers, debug adapters, tool
//! definitions and project generators each validate their own manifest
//! section, but they refuse content the same way ([`PluginRejection`]), check
//! the same identifier alphabets ([`is_plugin_id`], [`is_safe_relative_path`]),
//! parse the same `${...}` templates ([`parse_template_with`]), and declare
//! the same [`settings`], [`executables`] and server [`process`]es, and are
//! gated by the same [`capabilities`]. This crate holds that vocabulary so
//! each domain can live in its own crate; assembling a whole manifest stays
//! with the plugin host in `lithe-core`.
//!
//! Note: the contract is `shared/plugins/contract/v1/`; both products must
//! produce the same outcome for its fixtures.

pub mod capabilities;
pub mod executables;
mod icon;
mod manifest;
mod names;
pub mod process;
mod rejection;
pub mod settings;
mod template;
mod version;

pub use icon::UIIcon;
pub use manifest::PluginManifest;
pub use names::{
    is_bare_command, is_function_id, is_identifier, is_local_tool_id, is_official_namespace,
    is_plugin_id, is_safe_relative_path, is_semantic_version, is_tool_id, is_tool_namespace,
    sorted_unique, MAX_FUNCTION_ID_BYTES, OFFICIAL_PLUGIN_NAMESPACE,
};
pub use rejection::{
    decode_payload, invalid, invalid_template, outcome, PluginRejection, Validation,
};
pub use template::{parse_template_with, regex_literal, TemplatePart};
pub use version::{reported_version, validate_minimum_version, version_components};
