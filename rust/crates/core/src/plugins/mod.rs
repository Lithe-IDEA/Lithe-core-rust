//! Plugin manifests, capabilities, installation and the services plugins contribute.

pub use lithe_plugin_model::capabilities;
pub mod declarative;
pub mod marketplace;
/// Plugin pages in host tool windows: loading rules and the web bridge.
pub(crate) mod web_content;
