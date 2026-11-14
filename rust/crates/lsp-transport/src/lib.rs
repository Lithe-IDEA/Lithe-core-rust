//! The language-server process boundary and its wire framing.
//!
//! The language-server engine reaches a server's process and stdio only through
//! the traits in [`process`]: [`process::SystemProcessLauncher`] starts a local
//! child, or a backend on the active remote workspace host through
//! [`remote_process`]. [`framing`] encodes and incrementally parses the LSP
//! `Content-Length` frames carried over those streams. With the
//! `test-support` feature, `scripted` provides a scripted server so the
//! engine's timing and crash rules can be asserted without a real one.

pub mod framing;
pub mod process;
pub mod remote_process;
#[cfg(any(test, feature = "test-support"))]
pub mod scripted;

pub use framing::*;
