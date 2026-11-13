//! Plugin manifests, capabilities, installation and the services plugins contribute.

pub(crate) mod backend;
pub use lithe_plugin_model::capabilities;
pub mod declarative;
/// Public host-method catalog and authorization for platform dispatchers.
pub(crate) mod host_api;
pub(crate) mod keymap_import;
pub mod marketplace;
pub(crate) mod plugin_functions;
/// Plugin pages in host tool windows: loading rules and the web bridge.
pub(crate) mod web_content;
