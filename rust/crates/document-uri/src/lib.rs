//! Conversions between native filesystem paths and LSP document URIs.
//!
//! Paths are parsed lexically for the requested platform syntax, never through the build host's
//! filesystem, so macOS and Windows hosts get byte-identical URIs for the same input.

mod decode;
mod encode;

pub use decode::{file_path_for_uri, file_path_from_uri, hex_value, native_path, percent_decode};
pub use encode::{
    document_uri_command, resolve_request_paths, DocumentPathPlatform, MAX_PATH_BYTES,
};
