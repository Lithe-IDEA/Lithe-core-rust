//! Editor commands owned by Rust Core: deterministic line edits, find,
//! plugin text edit plans, navigation input and history, editor
//! preferences, rainbow coloring, recent files and crash-recovery decisions.
//!
//! Hosts apply the returned replacements and states through their own text
//! engine. The `editor.*` JSON commands are dispatched by `lithe-core`.

pub mod doc_render;
pub mod file_type_mappings;
pub mod find;
pub mod line_edit;
pub mod navigation_history;
pub mod navigation_input;
pub mod preferences;
pub mod rainbow_brackets;
pub mod rainbow_variables;
pub mod recent;
pub mod recovery;
pub mod reference_location;
pub mod text_edit_plan;

pub use line_edit::{line_edit, LineEditRequest};

#[cfg(test)]
mod tests;
