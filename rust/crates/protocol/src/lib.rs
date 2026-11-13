//! Cross-boundary contract shared by every Lithe Core crate.
//!
//! The response envelope ([`CoreResponse`]), the stable error categories ([`ErrorCode`],
//! [`CoreError`]) and per-request cancellation live here so a domain crate can report results and
//! failures without depending on `lithe-core`. `lithe-core` re-exports these items unchanged; the
//! JSON shape and error code names are part of the C ABI contract.

pub mod cancellation;
mod error;
pub mod json_document;
mod response;

pub use error::{invalid_relative_path, CoreError, ErrorCode};
pub use response::{CoreResponse, ResponseData};
