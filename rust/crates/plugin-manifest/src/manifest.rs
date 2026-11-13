//! The manifest model: `DeclarativePluginManifest`, the declarations it carries and
//! its accessors.

use super::*;

/// `plugin.json` schema version that identifies a declarative manifest.
pub const DECLARATIVE_MANIFEST_SCHEMA_VERSION: u32 = 0;

/// Most tool definitions one plugin may require.
pub(crate) const MAX_REQUIRED_TOOLS: usize = 32;

/// CPU architectures a host may report for `${arch}`.
pub(crate) const KNOWN_ARCHITECTURES: &[&str] = &["arm64", "x86_64"];

/// Root of a declarative `plugin.json`. Unknown fields are refused so a newer
/// manifest feature cannot be silently ignored by an older host.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclarativePluginManifest {
    schema_version: u32,
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) version: String,
    #[serde(rename = "pluginAPIVersion")]
    plugin_api_version: u32,
    /// Capability versions the manifest was written against; see
    /// `lithe_plugin_model::capabilities`.
    pub(crate) requires: Requires,
    pub(crate) vendor: PluginVendor,
    #[serde(default)]
    pub(crate) permissions: Vec<String>,
    #[serde(default)]
    pub(crate) activation_events: Vec<String>,
    #[serde(default)]
    pub(crate) languages: Vec<LanguageDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ui: Option<UIContributions>,
    #[serde(default)]
    pub(crate) language_servers: Vec<LanguageServerDeclaration>,
    #[serde(default)]
    pub(crate) execution_providers: Vec<ExecutionProviderDeclaration>,
    /// User-adjustable options of the plugin; see `lithe_plugin_model::settings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) settings: Vec<SettingDeclaration>,
    /// Buttons on settings pages that invoke the plugin's functions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) setting_actions: Vec<SettingAction>,
    /// Project-level build tools the plugin describes. The shape and its
    /// validation live in `lithe_build`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) build_systems: Vec<lithe_build::BuildSystemDeclaration>,
    /// Version control systems the plugin implements. The shape and its
    /// validation live in `lithe_vcs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) vcs_providers: Vec<lithe_vcs::VcsProviderDeclaration>,
    /// Debug adapters the plugin implements. The shape and its validation
    /// live in `lithe_debug::adapters`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) debug_adapters: Vec<lithe_debug::adapters::DebugAdapterDeclaration>,
    /// Entries of the New Project window. The shape and its validation live
    /// in `lithe_project_generators`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) project_generators: Vec<lithe_project_generators::ProjectGeneratorDeclaration>,
    /// The plugin's own process serving its pages; see `lithe_plugin_backend`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) backend: Option<lithe_plugin_backend::BackendDeclaration>,
    /// Tool policies owned by plugins, never a host-maintained definition table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tool_definitions: Vec<Value>,
    /// Fact providers served by the plugin's existing backend process.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) toolchain_providers: Vec<Value>,
    /// Independent shortcut importers implemented by this plugin's backend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) keymap_import_providers: Vec<lithe_plugin_keymap_import::ProviderDeclaration>,
    /// Tool-definition IDs the plugin's processes and Run requests name;
    /// hosts resolve them through plugin-owned toolchains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) requires_tools: Vec<String>,
    /// Patterns that turn console output into links; see
    /// `lithe_plugin_output_links`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) output_link_matchers: Vec<lithe_plugin_output_links::OutputLinkMatcher>,
    /// Functions this plugin exports to other plugins and the exported
    /// functions it calls; see `lithe_plugin_functions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plugin_functions: Option<lithe_plugin_functions::PluginFunctionsDeclaration>,
}

/// UI supplied by plugins. The host owns every window shell: docking, title,
/// activity bar, order, visibility, layout and lifecycle. A window body is
/// text, web content that the host loads from the package into a sandboxed
/// web view inside that shell, or host-rendered native content.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UIContributions {
    #[serde(default)]
    pub(crate) tool_windows: Vec<UIToolWindow>,
    #[serde(default)]
    pub(crate) commands: Vec<UICommand>,
}

/// A stable tool-window identity and plain-text content rendered by the host.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UIToolWindow {
    pub(crate) id: String,
    pub(crate) title: String,
    /// Initial dock only; saved user layout takes precedence.
    placement: UIDock,
    /// Sort rank within the dock; ties fall back to the identifier.
    #[serde(default)]
    pub(crate) order: i32,
    /// Plain text shown in the window body; the host never interprets markup.
    /// Exactly one of `text` and `web`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    /// The plugin's own page shown in the window body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) web: Option<UIWebContent>,
    /// Host-rendered tree/list content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native: Option<lithe_plugin_native_ui::NativeUI>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<UIIcon>,
}

/// A page from the plugin package. The host serves only package files to it,
/// grants no network, navigation or storage, and talks to it through the
/// plugin web bridge, which reaches only this plugin's own declarations.
///
/// Note: the decision is recorded in .agents/notes/implemented/architecture/2026-10-18-plugin-web-content-in-host-windows.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UIWebContent {
    /// Package-relative HTML entry, e.g. `ui/index.html`.
    pub(crate) entry: String,
}

/// Edge of the workbench where a tool window first docks.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum UIDock {
    Left,
    Bottom,
    Right,
}

/// The initial controlled action opens one of this plugin's own windows.
/// Commands cannot invoke arbitrary host actions or execute a process.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UICommand {
    pub(crate) id: String,
    pub(crate) title: String,
    /// Absent for a command the plugin backend handles.
    #[serde(
        default,
        rename = "toolWindowID",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) tool_window_id: Option<String>,
    #[serde(default)]
    pub(crate) order: i32,
    pub(crate) placements: Vec<UICommandPlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) icon: Option<UIIcon>,
}

/// Host surface that may list a plugin command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum UICommandPlacement {
    CommandPalette,
    Toolbar,
    Menu,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PluginVendor {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) signature_requirement: SignatureRequirement,
    /// Optional trust for the host's sealed built-in copy only. Installation
    /// from every other origin still requires the primary publisher signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bundled_signature_requirement: Option<SignatureRequirement>,
}

/// How a package proves its origin. `sameTeamAsHost` is only honored by a
/// host for packages shipped inside its own signed installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SignatureRequirement {
    SameTeamAsHost,
    PublisherPackage,
    /// Community package without a Lithe signature. Its integrity comes from
    /// the marketplace index checksum and its trust from the user's explicit
    /// confirmation, so the official namespace can never use it.
    None,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LanguageDeclaration {
    pub(crate) id: String,
    pub(crate) display_name: String,
    #[serde(default)]
    pub(crate) file_extensions: Vec<String>,
    #[serde(default)]
    pub(crate) file_names: Vec<String>,
    #[serde(default)]
    pub(crate) project_file_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dependencies: Option<DependencyDeclaration>,
    /// Syntax highlighting from a package file; see `lithe_plugin_grammar`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) grammar: Option<lithe_plugin_grammar::GrammarDeclaration>,
    /// Comment and bracket behavior of the editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) configuration: Option<lithe_plugin_grammar::LanguageConfiguration>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DependencyDeclaration {
    #[serde(default)]
    pub(crate) management_file_names: Vec<String>,
    #[serde(default)]
    pub(crate) project_dependency_paths: Vec<String>,
    #[serde(default)]
    pub(crate) project_binary_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LanguageServerDeclaration {
    #[serde(rename = "languageID")]
    pub(crate) language_id: String,
    /// LSP `languageId` sent for documents; defaults to the language ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) language_identifier: Option<String>,
    #[serde(flatten)]
    pub(crate) process: ServerProcess,
    /// Reserved. Core implements no host integration: a language server's
    /// run, test and debug support comes from the plugin's own backend and
    /// providers, so any value is refused with `plugin.integration.unsupported`
    /// instead of loading without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host_integration: Option<Value>,
    /// Settings the server reads through `workspace/configuration` and
    /// receives again in `workspace/didChangeConfiguration` whenever one of
    /// the settings it names changes. String values are templates like
    /// `initializationOptions`, and may also name settings whose `effect`
    /// is `immediate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_configuration: Option<Value>,
    /// Fields that govern what the plugin backend may send to its own server
    /// and how the host keeps the server's state.
    #[serde(flatten)]
    pub(crate) session: lithe_plugin_language_servers::SessionDeclaration,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExecutionProviderDeclaration {
    #[serde(rename = "languageID")]
    pub(crate) language_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run: Option<RunTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) test: Option<TestTemplate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunTemplate {
    pub(crate) executable: ExecutableDeclaration,
    pub(crate) arguments: Vec<ArgumentTemplate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TestTemplate {
    #[serde(rename = "frameworkID")]
    pub(crate) framework_id: String,
    pub(crate) workspace_label: String,
    pub(crate) project_markers: Vec<String>,
    pub(crate) test_file_patterns: Vec<String>,
    pub(crate) executable: ExecutableDeclaration,
    pub(crate) workspace_arguments: Vec<ArgumentTemplate>,
    pub(crate) file_arguments: Vec<ArgumentTemplate>,
    pub(crate) test_case_arguments: Vec<ArgumentTemplate>,
}

/// Exactly one of a bare command resolved on the host's tool path or a
/// host-owned project toolchain ID such as `project-go`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExecutableDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) toolchain: Option<String>,
}

/// One argv element. A conditional element is emitted only when the request
/// selects (or does not select) a file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum ArgumentTemplate {
    Literal(String),
    Conditional {
        value: String,
        when: ArgumentCondition,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ArgumentCondition {
    FileSelected,
    NoFileSelected,
}

impl DeclarativePluginManifest {
    /// Stable plugin ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Package-relative HTML entry of this plugin's web tool window
    /// `window_id`, or `None` when the plugin declares no such web window.
    pub fn web_window_entry(&self, window_id: &str) -> Option<&str> {
        self.ui
            .as_ref()?
            .tool_windows
            .iter()
            .find(|window| window.id == window_id)?
            .web
            .as_ref()
            .map(|web| web.entry.as_str())
    }

    /// Plugin version, `major.minor.patch`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The version of `capability` the manifest requires. Validation has
    /// already checked that every used capability is listed.
    pub(crate) fn capability_version(&self, capability: &str) -> u32 {
        self.requires.get(capability).copied().unwrap_or_default()
    }

    /// Normalized, sorted permission list.
    pub fn permissions(&self) -> &[String] {
        &self.permissions
    }

    /// Capability versions the manifest requires.
    pub fn requires(&self) -> &Requires {
        &self.requires
    }

    /// Host services the plugin's backend may request, sorted; empty without
    /// a backend. Each is granted by one permission.
    pub(crate) fn backend_host_services(&self) -> Vec<&'static str> {
        let mut services = Vec::new();
        if self.backend.is_none() {
            return services;
        }
        if self.permissions.iter().any(|p| p == PERMISSION_CREDENTIALS) {
            services.push(lithe_plugin_backend::HOST_SERVICE_CREDENTIALS);
        }
        if self
            .permissions
            .iter()
            .any(|p| p == PERMISSION_WORKSPACE_METADATA)
        {
            services.push(lithe_plugin_backend::HOST_SERVICE_WORKSPACE);
        }
        for permission in &self.permissions {
            services.extend(lithe_plugin_rpc::methods_for_permission(permission));
        }
        // Calling another plugin's exported function takes no permission: the
        // declaration, and the receiver's own export, are the consent. The
        // service lets the backend ask Core to find and call a provider.
        if self
            .plugin_functions
            .as_ref()
            .is_some_and(lithe_plugin_functions::PluginFunctionsDeclaration::calls_any)
        {
            services.push(lithe_plugin_rpc::HOST_SERVICE_FUNCTIONS);
        }
        services.sort_unstable();
        services.dedup();
        services
    }
}

impl DeclarativePluginManifest {
    /// Every program the host may start on this plugin's behalf, in a stable
    /// order. Each entry carries everything that decides what actually runs:
    /// the executable plus its arguments, the language server's validation
    /// probe, and the run/test argument templates. An update that changes any
    /// of these, even with the same executable, is a new program and must be
    /// approved again.
    pub fn programs(&self) -> Vec<Value> {
        let mut programs = Vec::new();
        for server in &self.language_servers {
            let mut program = server.process.program(ServerRole::LanguageServer);
            program["languageID"] = json!(server.language_id);
            programs.push(program);
            programs.extend(server.process.runtime_programs());
        }
        for provider in &self.execution_providers {
            if let Some(run) = &provider.run {
                programs.push(json!({
                    "kind": "run",
                    "languageID": provider.language_id,
                    "executable": run.executable,
                    "arguments": run.arguments,
                }));
            }
            if let Some(test) = &provider.test {
                programs.push(json!({
                    "kind": "test",
                    "languageID": provider.language_id,
                    "executable": test.executable,
                    "workspaceArguments": test.workspace_arguments,
                    "fileArguments": test.file_arguments,
                    "testCaseArguments": test.test_case_arguments,
                }));
            }
        }
        for build_system in &self.build_systems {
            programs.extend(lithe_build::programs(build_system));
        }
        for provider in &self.vcs_providers {
            programs.extend(lithe_vcs::programs(provider));
        }
        for adapter in &self.debug_adapters {
            programs.extend(lithe_debug::adapters::programs(adapter));
        }
        for generator in &self.project_generators {
            programs.extend(lithe_project_generators::programs(generator));
        }
        if let Some(backend) = &self.backend {
            programs.extend(lithe_plugin_backend::programs(backend));
        }
        // Order by kind, then language or tool, then the full serialized
        // value as a total tie-breaker; duplicates (two servers needing the
        // same runtime) collapse to one entry.
        programs.sort_by_key(|program| {
            let text = |key: &str| program[key].as_str().unwrap_or_default().to_string();
            (
                text("kind"),
                text("languageID"),
                text("tool"),
                program.to_string(),
            )
        });
        programs.dedup();
        programs
    }
}
