//! Structured diagnostics shared by every host: the generated event catalog,
//! log channels and records, filters, queries, formatting, retention and
//! collection plans, diagnostic-bundle redaction and manifests, and the bounded
//! queue that hands Core-owned facts to the host writer.
//!
//! Everything here is pure and deterministic. The queue owns observation
//! timestamps but no file writer; hosts gather log files, panic reports and
//! environment facts natively, per `develop-lithe`'s platform-adapter boundary.
//! The `diagnostics.*` JSON commands are dispatched by `lithe-core`.

pub mod catalog;
pub mod channel;
pub mod collection;
pub mod core_events;
pub mod filter;
pub mod format;
pub mod logging;
pub mod manifest;
pub mod plugin_operations;
pub mod query_v2;
pub mod record;
pub mod redaction;
pub mod retention;
