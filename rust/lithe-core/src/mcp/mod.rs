//! Deterministic MCP server configuration parsing and format-preserving edits.
//!
//! Hosts read and write the actual configuration files; this module only turns
//! bounded text into normalized server definitions and computes the next file
//! content. It never consults the filesystem, the environment, or the network.

mod json_edit;
mod library;
mod model;
mod parse;
mod plan;
mod registry;
mod source_hash;
mod toml_edit;
mod tools;

pub use library::{batch_plan, library_snapshot, validate_draft};
pub use model::{McpKeyValue, McpServer, McpTransport};
pub use parse::parse_configuration;
pub use plan::plan_write;
pub use registry::{featured_registry, parse_registry};
pub use tools::{command_line, tool_policy};

#[cfg(test)]
mod library_tests;
#[cfg(test)]
mod tests;
