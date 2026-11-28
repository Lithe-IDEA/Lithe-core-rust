//! Official SDK for Lithe plugin backends written in Rust.
//!
//! A plugin backend is the one long-lived process a plugin declares under
//! `backend` in `plugin.json`; Lithe starts it when the plugin first needs
//! it, such as when one of its web pages calls a function or one of its views
//! opens. It speaks the LSP base protocol on stdio and implements, from
//! [plugin contract v1](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/shared/plugins/contract/v1/README.md):
//!
//! - **`litheFunctions`**: plugin functions with progress, cancellation, and
//!   registration at runtime, and the [`view`] functions that draw the
//!   plugin's tool windows and settings regions;
//! - **[`Host`] services**: the plugin's own [`Credentials`] in the
//!   platform's secret store, workspace contexts, files, documents, the
//!   editor, previews, the browser, window messages, capability discovery,
//!   subscriptions, the commands and context changes the host notifies, and
//!   calls to the functions other plugins export through Core's relay
//!   ([`Functions`]) with the [`Caller`] of a function called that way;
//! - **[Agent sessions](Backend::on_agent_sessions)**: native sessions of
//!   the plugin's agent CLIs, translated in order, and the [`Agents`]
//!   service that writes to the CLI, reports session events, and reads
//!   usage totals (plan API-45);
//! - **`litheKeymapImport`**: shortcut import providers;
//! - **`litheToolchains`**: [toolchain discovery providers](toolchains);
//! - **`litheBuild`**: a build provider's typed `lithe/build/*` requests,
//!   declared with [`Backend::build_model`] and its siblings and answered
//!   with a [`BuildContext`] for progress, output, and cancellation.
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

mod agents;
mod backend;
mod build;
mod diagnostics;
mod error;
mod framing;
mod host;
mod panic_boundary;
mod protocol;
mod stdio;
pub mod toolchains;
pub mod view;

pub use agents::{AgentImage, AgentSession, AgentSessionHandle, AgentSessionStart, Agents};
pub use backend::{
    Backend, BackendHandle, Caller, Cancellation, Context, Initialize, KeymapImportRequest,
    KeymapSourceFile,
};
pub use build::BuildContext;
pub use diagnostics::Diagnostics;
pub use error::{
    sdk_codes, Cancelled, FunctionError, PluginError, SdkError, HOST_ERROR_CODES, RELAY_ERROR_CODES,
};
pub use host::{
    Browser, Clipboard, Credentials, Documents, Editor, Event, Functions, Host, HostFolder,
    HostPath, Network, Output, Preview, ProcessRoot, Progress, StatusBar, Subscription,
    Subscriptions, Terminal, Views, Window, Workspace, WorkspaceContext,
};
pub use protocol::{is_command_id, is_credential_key, is_function_id, is_plugin_error_code};
pub use serde_json::Value;
