//! Plugin manifests, capabilities, installation and the services plugins contribute.

pub(crate) mod backend;
pub use lithe_plugin_model::capabilities;
/// Read-only documents a plugin backend serves as a URI scheme.
pub(crate) mod content_providers;
pub mod declarative;
/// Independent, host-rendered editor actions of ordinary tool plugins.
pub mod editor_actions;
pub mod editor_providers;
pub(crate) mod grammar;
/// Public host-method catalog and authorization for platform dispatchers.
pub(crate) mod host_api;
/// Numeric version selection over bounded native installation facts.
pub mod installed;
pub(crate) mod keymap_import;
/// Session fields of plugin-owned language servers and the launch their
/// backend resolves.
pub(crate) mod language_server_sessions;
pub mod marketplace;
pub(crate) mod native_ui;
/// Plugin-declared console link patterns and their resolution.
pub(crate) mod output_links;
pub(crate) mod plugin_functions;
/// Plugin pages in host tool windows: loading rules and the web bridge.
pub(crate) mod web_content;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Strict three-component semantic version used for compatibility comparisons.
pub struct PluginVersion {
    /// Breaking-change component.
    pub major: u32,
    /// Backward-compatible feature component.
    pub minor: u32,
    /// Backward-compatible fix component.
    pub patch: u32,
}

impl PluginVersion {
    /// Parses exactly `major.minor.patch`; prerelease tags and missing parts are
    /// rejected because the manifest contract does not define their ordering.
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('.');
        let version = Self {
            major: parts.next()?.parse().ok()?,
            minor: parts.next()?.parse().ok()?,
            patch: parts.next()?.parse().ok()?,
        };
        parts.next().is_none().then_some(version)
    }
}
