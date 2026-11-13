//! The stateful LSP client Lithe runs for every language server and plugin
//! backend: initialization and capability negotiation, open documents and
//! their versions, feature requests and the host models their responses are
//! shaped into, and the experimental provider protocols (build, VCS,
//! toolchain, keymap import, plugin logging) a server can offer.
//!
//! Everything here is a pure function of the client state and one message.
//! Processes, threads and timeouts belong to `lithe-lsp-engine`.

pub mod build_provider;
mod client;
pub mod keymap_provider;
pub mod plugin_logging;
pub mod toolchain_provider;
mod types;
pub mod vcs_provider;

pub use client::*;
use lithe_plugin_model::capabilities::offered_versions;
pub use types::*;

#[cfg(test)]
mod tests;
