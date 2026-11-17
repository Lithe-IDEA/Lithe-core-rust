//! Text primitives shared by the editor and LSP layers: zero-based positions
//! and ranges in UTF-16 code units, edit validation and planning, line
//! indexing, result-row line previews, and the in-process completions, hover,
//! navigation and snippets that stay available without a language server.
//!
//! The `lsp.*` and `editor.*` JSON commands that use them are dispatched by
//! `lithe-core`.

mod edits;
mod line_preview;
mod position;
mod snippets;
mod symbols;

pub use edits::*;
pub use line_preview::*;
pub use position::*;
pub use snippets::*;
pub use symbols::*;

#[cfg(test)]
mod tests;
