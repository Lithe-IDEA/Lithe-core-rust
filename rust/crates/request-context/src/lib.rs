//! State scoped to one Core request and shared by every domain that runs inside
//! it: where the request came from ([`Source`]), the operation it belongs to
//! ([`trace`]), and the ordered event channel that streams diagnostics to the
//! host while the request runs ([`events`]).
//!
//! All state is thread-local and restored when the scope that set it ends, so
//! nested requests and callbacks never see or overwrite the outer request's
//! state. The dispatcher in `lithe-core` opens these scopes; domains only read
//! them and emit events.

pub mod events;
mod source;
pub mod trace;

pub use source::{source_token, Source};
