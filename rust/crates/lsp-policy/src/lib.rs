//! Deterministic language-tooling rules that both platform hosts apply around
//! their native language-server sessions and editor presentation.
//!
//! Every command here is a pure function of the facts the platform submits:
//! this crate owns no process, document or clock. Platforms keep the native
//! I/O, the per-session storage of returned state, and localization.

pub mod activation;
pub mod completion_merge;
pub mod declaration_or_usages;
pub mod document_identity;
pub mod navigation_target;
pub mod problems;
pub mod workspace_edit_plan;
mod workspace_scope;
