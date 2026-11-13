//! `backend` of a plugin manifest: one long-lived process a plugin starts for
//! itself, independent of any build system, language, or workspace.
//!
//! A build provider or VCS provider exists for one domain contract and one
//! workspace. A backend serves only the plugin itself: its web pages call the
//! backend's [plugin functions](crate::lsp::interface::plugin_function), and
//! the backend may ask the host for the primitive services its permissions
//! grant, such as private credential storage or workspace contexts. The host
//! keeps one process per plugin on the machine that runs Lithe, and ends it
//! when the plugin is disabled, updated, or uninstalled.
//!
//! The process starts on the first call from one of the plugin's pages, when
//! a workspace opens (`onWorkspaceOpen`), when one of the plugin's commands
//! runs, or when a provider query or exported function needs it, so the
//! manifest declares at least one of those triggers. Every workspace reaches
//! the one process as an explicit context in each call, never as process
//! state, so `${workspaceStateDir}` is refused; the per-workspace state
//! directory travels in the context instead.
//!
//! The process shares the launch fields, templates, and install review of
//! every plugin server process.
//!
//! Note: the decisions are recorded in .agents/notes/implemented/architecture/2026-10-24-database-provider-plugin.md
//! and .agents/notes/implemented/architecture/2026-10-30-plugin-backend-workspace-context.md

use lithe_plugin_model::process::{ServerProcess, ServerRole};
use lithe_plugin_model::settings::DeclaredSettings;
use lithe_plugin_model::{invalid, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The plugin's own backend process.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BackendDeclaration {
    #[serde(flatten)]
    pub(crate) process: ServerProcess,
}

/// Primitive host services a backend may request, each granted by one
/// manifest permission.
pub(crate) const HOST_SERVICE_CREDENTIALS: &str = "credentials";
/// Workspace contexts and their folders (`workspace.metadata`).
pub(crate) const HOST_SERVICE_WORKSPACE: &str = "workspace";

/// Activation event of a backend that starts once a workspace is open. It
/// names no ID: the backend is one process for every workspace.
pub(crate) const ON_WORKSPACE_OPEN: &str = "onWorkspaceOpen";

/// What backend validation needs from the rest of the manifest.
pub(crate) struct DeclarationContext<'a> {
    pub(crate) declared_settings: &'a DeclaredSettings,
    /// Whether a tool window of the plugin shows web content, whose pages
    /// call the backend.
    pub(crate) has_web_window: bool,
    /// Whether the plugin declares a UI command, whose `onCommand` event
    /// starts the backend.
    pub(crate) has_command: bool,
    /// Whether the manifest lists `onWorkspaceOpen`.
    pub(crate) activates_on_workspace_open: bool,
    /// A provider capability may start this backend for a host-owned query.
    pub(crate) provider_query_trigger: bool,
    /// The tool-definition IDs from `requiresTools`.
    pub(crate) runtime_tools: lithe_plugin_model::process::RuntimeTools<'a>,
}

impl BackendDeclaration {
    pub(crate) fn validate(&self, context: &DeclarationContext<'_>) -> Validation<()> {
        // A backend nothing can start would never run: it needs a page, a
        // command, a workspace event, or a provider query trigger.
        if !context.has_web_window
            && !context.has_command
            && !context.activates_on_workspace_open
            && !context.provider_query_trigger
        {
            return Err(invalid(
                "backend",
                "A plugin backend needs an admitted page, command, workspace or provider query trigger.",
            ));
        }
        self.process.validate(
            "backend",
            context.declared_settings,
            ServerRole::Backend,
            context.runtime_tools,
        )?;
        // One process serves every workspace, so a launch argument cannot
        // name one of them; the directory arrives with each context.
        if self.process.uses_workspace_state_directory() {
            return Err(invalid(
                "backend",
                "A plugin backend is not bound to a workspace and cannot use ${workspaceStateDir}.",
            ));
        }
        Ok(())
    }
}

/// Install-review entries: the backend process with the runtimes it needs.
pub(crate) fn programs(backend: &BackendDeclaration) -> Vec<Value> {
    let mut programs = vec![backend.process.program(ServerRole::Backend)];
    programs.extend(backend.process.runtime_programs());
    programs
}

/// The `backend` member of a launch plan: the `backend` capability version
/// the manifest requires and the host services the session may request,
/// which the host passes to `lsp.startServer` unchanged.
pub(crate) fn plan_descriptor(version: u32, host_services: &[&str]) -> Value {
    json!({ "hostServices": host_services, "version": version })
}
