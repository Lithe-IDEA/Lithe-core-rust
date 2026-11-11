//! Official SDK for Lithe plugin backends written in Rust.
//!
//! A plugin backend is the one long-lived process a plugin declares under
//! `backend` in `plugin.json`; Lithe starts it when one of the plugin's own
//! web pages first calls a function. It speaks the LSP base protocol on
//! stdio and implements, from
//! [plugin contract v2](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/shared/plugins/contract/v1/README.md):
//!
//! - **`litheFunctions`**: plugin functions with progress, cancellation, and
//!   registration at runtime;
//! - **[`Host`] services**: the plugin's own [`Credentials`] in the
//!   platform's secret store, workspace contexts, files, documents, the
//!   editor, previews, the browser, window messages, capability discovery,
//!   subscriptions, the commands and context changes the host notifies, and
//!   calls to the functions other plugins export through Core's relay
//!   ([`Functions`]) with the [`Caller`] of a function called that way;
//! - **`litheKeymapImport`**: shortcut import providers.
//!
//! The SDK requires `litheLogging` at initialization.
//!
//! The SDK owns the wire format, negotiation, cancellation, and error
//! mapping; plugin code supplies handlers. Each call runs on a thread of its
//! own, so handlers may block. A handler returns `Ok(value)` with a JSON
//! result, a [`PluginError`] the plugin's UI can tell apart by its code, or
//! any other error, which reaches the host only as a generic failure while
//! automatic diagnostics omit its contents. A panic ends only its call;
//! SDK catch boundaries suppress raw panic payload printing.
//!
//! See [`Backend`] for a minimal backend.

mod backend;
mod diagnostics;
mod error;
mod framing;
mod host;
mod panic_boundary;
mod protocol;
mod stdio;

pub use backend::{
    Backend, BackendHandle, Caller, Cancellation, Context, Initialize, KeymapImportRequest,
    KeymapSourceFile,
};
pub use diagnostics::Diagnostics;
pub use error::{
    sdk_codes, Cancelled, FunctionError, PluginError, SdkError, HOST_ERROR_CODES, RELAY_ERROR_CODES,
};
pub use host::{
    Browser, Credentials, Documents, Editor, Event, Functions, Host, Preview, Subscription,
    Subscriptions, Terminal, Window, Workspace, WorkspaceContext,
};
pub use protocol::{is_command_id, is_credential_key, is_function_id, is_plugin_error_code};
pub use serde_json::Value;
