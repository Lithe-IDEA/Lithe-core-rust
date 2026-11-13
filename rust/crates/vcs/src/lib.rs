//! The unified VCS layer: one model of roots, file status, history, and
//! check-in that every version control system maps onto, and the
//! deterministic half of talking to a plugin VCS provider.
//!
//! A provider is a plugin process speaking the LSP base protocol, like a
//! build provider. Core validates its `vcsProviders` declaration, detects
//! roots from static markers, plans each request from an operation name and
//! checked arguments, and validates every reply before a host renders it.
//! Hosts resolve the client tool, start and stop processes, watch metadata,
//! and own every piece of UI and state such as changelists.
//!
//! Git is not part of this layer yet: Core recognizes `.git` markers so a
//! nested working copy is assigned to the right root, and the existing
//! `git.*` commands keep serving Git roots unchanged.
//!
//! Note: the design is recorded in .agents/notes/proposed/architecture/2026-10-22-unified-vcs-provider-model.md

mod commands;
mod manifest;
mod model;
mod requests;
mod roots;

pub use commands::{vcs_decode_reply_command, vcs_detect_roots_command, vcs_owner_of_command};
pub use manifest::{
    activation_event, programs, validate_declarations, DeclarationContext, VcsProviderDeclaration,
};
pub use model::{is_vcs_id, negotiated_capabilities, Capabilities};
pub use requests::{is_vcs_method, plan, Operation, PlannedRequest};

/// What `vcs.detectRoots` reads from one plugin manifest that the plugin host
/// validated; the host's manifest type implements it.
pub trait VcsManifest {
    /// The plugin ID.
    fn id(&self) -> &str;
    /// The manifest's validated `vcsProviders`, sorted by VCS ID.
    fn vcs_providers(&self) -> &[VcsProviderDeclaration];
}
