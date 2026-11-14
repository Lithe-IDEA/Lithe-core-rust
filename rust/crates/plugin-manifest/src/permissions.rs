//! Permission names, the set the contract knows, and the checks that tie each
//! declared contribution to its permission.

use super::*;

const PERMISSION_LANGUAGE_SERVER: &str = "languageServer.launch";

const PERMISSION_RUN: &str = "process.run";

const PERMISSION_TEST: &str = "process.test";

const PERMISSION_UI: &str = "ui.contribute";

/// Running the plugin's own scripts in a host-managed web view.
const PERMISSION_UI_WEB: &str = "ui.web";

/// Running a declared build system's executable: model, task, test, and
/// dependency commands.
const PERMISSION_BUILD: &str = "process.build";

/// Letting a VCS provider change the working copy and contact its server.
const PERMISSION_VCS: &str = "process.vcs";

/// Creating projects: writing template files, running scaffolding tools,
/// and downloading project archives.
const PERMISSION_PROJECT_GENERATE: &str = "project.generate";

/// Starting the plugin's own backend process.
const PERMISSION_BACKEND: &str = "backend.launch";

/// Supplying facts that can change the executable selected for a defined tool.
const PERMISSION_TOOLCHAINS: &str = "toolchains.provide";

/// Providing normalized shortcuts during an explicit import.
const PERMISSION_KEYMAPS: &str = "keymaps.provide";

/// Reading declared files from the explicitly selected import source.
const PERMISSION_KEYMAP_SOURCES: &str = "keymaps.readSelectedSources";

/// Letting the backend keep secrets in the host's credential store, private
/// to the plugin.
pub(crate) const PERMISSION_CREDENTIALS: &str = "credentials.store";

/// Letting the backend list the open workspaces and read their folders and
/// its per-workspace state directory.
pub(crate) const PERMISSION_WORKSPACE_METADATA: &str = "workspace.metadata";

/// Starting a debug session the plugin's adapter serves.
const PERMISSION_DEBUG_LAUNCH: &str = "debug.launch";

/// Attaching the plugin's adapter to a running process.
const PERMISSION_DEBUG_ATTACH: &str = "debug.attach";

/// Letting the backend start a debug session on one of its own adapters
/// (`lithe/host/debug/start`), a host-method permission of the backend.
const PERMISSION_DEBUG_START: &str = "debug.start";

const KNOWN_PERMISSIONS: &[&str] = &[
    PERMISSION_BACKEND,
    PERMISSION_BUILD,
    PERMISSION_CREDENTIALS,
    PERMISSION_DEBUG_ATTACH,
    PERMISSION_DEBUG_LAUNCH,
    PERMISSION_LANGUAGE_SERVER,
    PERMISSION_KEYMAPS,
    PERMISSION_KEYMAP_SOURCES,
    PERMISSION_PROJECT_GENERATE,
    PERMISSION_RUN,
    PERMISSION_TEST,
    PERMISSION_TOOLCHAINS,
    PERMISSION_UI,
    PERMISSION_UI_WEB,
    PERMISSION_VCS,
    PERMISSION_WORKSPACE_METADATA,
];

impl DeclarativePluginManifest {
    pub(crate) fn validate_permissions(&mut self) -> Validation<()> {
        for permission in &self.permissions {
            if !KNOWN_PERMISSIONS.contains(&permission.as_str())
                && !lithe_plugin_rpc::HOST_PERMISSIONS.contains(&permission.as_str())
            {
                return Err(PluginRejection::at(
                    "plugin.permission.unknown",
                    "permissions",
                    format!("The plugin requests an unknown permission: {permission}."),
                ));
            }
        }
        self.permissions = sorted_unique(std::mem::take(&mut self.permissions));
        let mut required = BTreeSet::new();
        if let Some(ui) = &self.ui {
            required.insert(PERMISSION_UI);
            if ui.tool_windows.iter().any(|window| window.web.is_some()) {
                required.insert(PERMISSION_UI_WEB);
            }
        }
        // A provider process of its own is a server process like a language
        // server, so it needs the same consent.
        if !self.language_servers.is_empty()
            || !self.vcs_providers.is_empty()
            || self
                .build_systems
                .iter()
                .any(lithe_build::BuildSystemDeclaration::starts_own_provider)
        {
            required.insert(PERMISSION_LANGUAGE_SERVER);
        }
        for provider in &self.execution_providers {
            if provider.run.is_some() {
                required.insert(PERMISSION_RUN);
            }
            if provider.test.is_some() {
                required.insert(PERMISSION_TEST);
            }
        }
        if !self.build_systems.is_empty() {
            required.insert(PERMISSION_BUILD);
        }
        if !self.toolchain_providers.is_empty() {
            required.insert(PERMISSION_TOOLCHAINS);
        } else if self
            .permissions
            .iter()
            .any(|permission| permission == PERMISSION_TOOLCHAINS)
        {
            return Err(invalid(
                "permissions",
                "toolchains.provide requires a toolchain provider.",
            ));
        }
        if !self.keymap_import_providers.is_empty() {
            required.insert(PERMISSION_KEYMAPS);
            required.insert(PERMISSION_KEYMAP_SOURCES);
        } else if self.permissions.iter().any(|permission| {
            permission == PERMISSION_KEYMAPS || permission == PERMISSION_KEYMAP_SOURCES
        }) {
            return Err(invalid(
                "permissions",
                "Shortcut import permissions require a shortcut import provider.",
            ));
        }
        if !self.vcs_providers.is_empty() {
            required.insert(PERMISSION_VCS);
        }
        if !self.debug_adapters.is_empty() {
            required.insert(PERMISSION_DEBUG_LAUNCH);
        }
        if lithe_debug::adapters::serves_attach(&self.debug_adapters) {
            required.insert(PERMISSION_DEBUG_ATTACH);
        } else if self
            .permissions
            .iter()
            .any(|p| p == PERMISSION_DEBUG_ATTACH)
        {
            return Err(invalid(
                "permissions",
                "debug.attach is used by an adapter that serves attach; declare one or remove the permission.",
            ));
        }
        if !self.project_generators.is_empty() {
            required.insert(PERMISSION_PROJECT_GENERATE);
        }
        if self.backend.is_some() {
            required.insert(PERMISSION_BACKEND);
        } else if self.permissions.iter().any(|p| p == PERMISSION_CREDENTIALS) {
            // Only a backend can use the store; listing the permission
            // without one would show the user a grant nothing exercises.
            return Err(invalid(
                "permissions",
                "credentials.store is used by a plugin backend; declare one or remove the permission.",
            ));
        }
        // Only the plugin backend receives workspace contexts, so the grant
        // the user reviewed would otherwise never be exercised.
        if self
            .permissions
            .iter()
            .any(|p| p == PERMISSION_WORKSPACE_METADATA)
            && self.backend.is_none()
        {
            return Err(invalid(
                "permissions",
                "workspace.metadata is used by the plugin backend; declare one or remove the permission.",
            ));
        }
        if self
            .permissions
            .iter()
            .any(|p| lithe_plugin_rpc::HOST_PERMISSIONS.contains(&p.as_str()))
            && self.backend.is_none()
        {
            return Err(invalid(
                "permissions",
                "Host API permissions require the plugin backend.",
            ));
        }
        // Actions and providers both read the live document they annotate.
        if self
            .permissions
            .iter()
            .any(|permission| permission == "editor.actions" || permission == "editor.providers")
            && !self
                .permissions
                .iter()
                .any(|permission| permission == "documents.read")
        {
            return Err(invalid(
                "permissions",
                "Editor actions and providers require documents.read for live snapshots.",
            ));
        }
        // `lithe/host/debug/start` names one of the plugin's own adapters, so
        // the grant is useless without one.
        if self
            .permissions
            .iter()
            .any(|permission| permission == PERMISSION_DEBUG_START)
            && self.debug_adapters.is_empty()
        {
            return Err(invalid(
                "permissions",
                "debug.start starts the plugin's own debug adapters; declare one or remove the permission.",
            ));
        }
        // The backend reaches only the servers whose session fields expose
        // them; the grant without one would show the user a capability
        // nothing exercises.
        let language_server_request = lithe_plugin_rpc::PERMISSION_LANGUAGE_SERVER_REQUEST;
        if self
            .language_servers
            .iter()
            .any(|server| server.session.exposes_server())
        {
            required.insert(language_server_request);
        } else if self
            .permissions
            .iter()
            .any(|permission| permission == language_server_request)
        {
            return Err(invalid(
                "permissions",
                "languageServer.request is used by a server that lists requestMethods or forwardNotifications; declare one or remove the permission.",
            ));
        }
        for permission in required {
            if !self
                .permissions
                .iter()
                .any(|declared| declared == permission)
            {
                return Err(PluginRejection::at(
                    "plugin.permission.undeclared",
                    "permissions",
                    format!("The plugin uses {permission} without declaring the permission."),
                ));
            }
        }
        Ok(())
    }
}
