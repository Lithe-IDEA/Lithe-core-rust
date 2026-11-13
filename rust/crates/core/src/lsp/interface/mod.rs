//! Standard LSP contracts and the stateful client/session implementation.

mod build_provider;
mod client;
mod engine;
mod engine_logging;
mod function_relay;
mod keymap_provider;
pub(crate) mod plugin_function;
mod plugin_logging;
mod plugin_server;
mod toolchain_provider;
mod types;
mod vcs_provider;

pub(crate) use client::*;
pub(crate) use engine::*;
pub(crate) use engine_logging::{resolve_engine_log_subject, ResolveEngineLogSubjectRequest};
pub(crate) use function_relay::RelaySnapshot;
pub(crate) use lithe_document_uri::{document_uri_command, resolve_request_paths, MAX_PATH_BYTES};
pub(crate) use types::*;

/// The versions a Lithe runtime protocol offers in `initialize`. A runtime
/// protocol has no version of its own: it speaks the version of the contract
/// that owns it, the capability version of the manifest section that declares
/// the provider (or the plugin API version for plugin logging). The host
/// therefore offers exactly the versions of that contract it implements, and a
/// reply counts as negotiated only when it names one of them.
fn offered_versions(contract_versions: &[u32]) -> Vec<u64> {
    contract_versions
        .iter()
        .map(|version| u64::from(*version))
        .collect()
}
