//! Declarative plugin contract: manifest validation and deterministic
//! expansion of run/test command templates and language-server launch plans.
//!
//! A declarative plugin carries no host-loaded code. The host reads its
//! `plugin.json`, asks this module to validate and normalize it, and launches
//! the declared external processes itself. Both products must produce the
//! same outcome for the fixtures in `shared/plugins/contract/v1/`.
//!
//! Content problems in a manifest or request are reported as `rejected`
//! outcomes with a stable `plugin.*` code instead of Core failures, so hosts
//! can show them next to the affected plugin. Only a malformed command
//! payload is a Core `invalid_request` failure.

use lithe_plugin_model::capabilities::{self, HostCapabilities, Requires};
use lithe_plugin_model::process::{
    collect_setting_references, is_packaged_runtime_path, validate_launch_value,
    LanguageServerLaunch, LaunchVariable, RuntimeTools, ServerProcess, ServerRole,
};
use lithe_plugin_model::settings::{
    DeclaredSettings, ResolvedSetting, SettingAction, SettingDeclaration, SettingLayers,
    MAX_SETTINGS, MAX_SETTING_ACTIONS,
};
use lithe_plugin_model::{
    decode_payload, invalid, invalid_template, is_bare_command, is_identifier,
    is_official_namespace, is_plugin_id, is_safe_relative_path, is_semantic_version, outcome,
    parse_template_with, regex_literal, sorted_unique, PluginRejection, TemplatePart, UIIcon,
    Validation,
};
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// `plugin.json` schema version that identifies a declarative manifest.
pub const DECLARATIVE_MANIFEST_SCHEMA_VERSION: u32 = 0;
/// Base-contract versions (`pluginAPIVersion`) this Core build can host. The
/// version is independent of the Lithe application version, and a later one
/// is appended to this list.
pub const SUPPORTED_PLUGIN_API_VERSIONS: &[u32] = &[0];

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
const PERMISSION_CREDENTIALS: &str = "credentials.store";
/// Letting the backend list the open workspaces and read their folders and
/// its per-workspace state directory.
const PERMISSION_WORKSPACE_METADATA: &str = "workspace.metadata";
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
/// Most tool definitions one plugin may require.
const MAX_REQUIRED_TOOLS: usize = 32;
/// Native host integrations a language server may ask for. The Core
/// implements none: a language server's run, test and debug support comes
/// from the plugin's own backend and providers, so every name is rejected
/// with `plugin.integration.unsupported`. The declaration stays in the
/// contract so a manifest naming one fails validation instead of parsing.
const KNOWN_HOST_INTEGRATIONS: &[&str] = &[];
/// Host capability: running a program of the language.
const HOST_CAPABILITY_RUN: &str = "run";
/// Host capability: discovering and running tests of the language.
const HOST_CAPABILITY_TESTING: &str = "testing";
/// CPU architectures a host may report for `${arch}`.
const KNOWN_ARCHITECTURES: &[&str] = &["arm64", "x86_64"];

/// Root of a declarative `plugin.json`. Unknown fields are refused so a newer
/// manifest feature cannot be silently ignored by an older host.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclarativePluginManifest {
    schema_version: u32,
    id: String,
    display_name: String,
    version: String,
    #[serde(rename = "pluginAPIVersion")]
    plugin_api_version: u32,
    /// Capability versions the manifest was written against; see
    /// `lithe_plugin_model::capabilities`.
    requires: Requires,
    vendor: PluginVendor,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    activation_events: Vec<String>,
    #[serde(default)]
    languages: Vec<LanguageDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ui: Option<UIContributions>,
    #[serde(default)]
    language_servers: Vec<LanguageServerDeclaration>,
    #[serde(default)]
    execution_providers: Vec<ExecutionProviderDeclaration>,
    /// User-adjustable options of the plugin; see `lithe_plugin_model::settings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    settings: Vec<SettingDeclaration>,
    /// Buttons on settings pages that invoke the plugin's functions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    setting_actions: Vec<SettingAction>,
    /// Project-level build tools the plugin describes. The shape and its
    /// validation live in `lithe_build`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    build_systems: Vec<lithe_build::BuildSystemDeclaration>,
    /// Version control systems the plugin implements. The shape and its
    /// validation live in `lithe_vcs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    vcs_providers: Vec<lithe_vcs::VcsProviderDeclaration>,
    /// Debug adapters the plugin implements. The shape and its validation
    /// live in `lithe_debug::adapters`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    debug_adapters: Vec<lithe_debug::adapters::DebugAdapterDeclaration>,
    /// Entries of the New Project window. The shape and its validation live
    /// in `lithe_project_generators`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    project_generators: Vec<lithe_project_generators::ProjectGeneratorDeclaration>,
    /// The plugin's own process serving its pages; see `lithe_plugin_backend`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend: Option<lithe_plugin_backend::BackendDeclaration>,
    /// Tool policies owned by plugins, never a host-maintained definition table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_definitions: Vec<Value>,
    /// Fact providers served by the plugin's existing backend process.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    toolchain_providers: Vec<Value>,
    /// Independent shortcut importers implemented by this plugin's backend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keymap_import_providers: Vec<lithe_plugin_keymap_import::ProviderDeclaration>,
    /// Tool-definition IDs the plugin's processes and Run requests name;
    /// hosts resolve them through plugin-owned toolchains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires_tools: Vec<String>,
    /// Patterns that turn console output into links; see
    /// `lithe_plugin_output_links`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    output_link_matchers: Vec<lithe_plugin_output_links::OutputLinkMatcher>,
    /// Functions this plugin exports to other plugins and the exported
    /// functions it calls; see `lithe_plugin_functions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_functions: Option<lithe_plugin_functions::PluginFunctionsDeclaration>,
}

/// UI supplied by plugins. The host owns every window shell: docking, title,
/// activity bar, order, visibility, layout and lifecycle. A window body is
/// text, web content that the host loads from the package into a sandboxed
/// web view inside that shell, or host-rendered native content.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UIContributions {
    #[serde(default)]
    tool_windows: Vec<UIToolWindow>,
    #[serde(default)]
    commands: Vec<UICommand>,
}

/// A stable tool-window identity and plain-text content rendered by the host.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UIToolWindow {
    id: String,
    title: String,
    /// Initial dock only; saved user layout takes precedence.
    placement: UIDock,
    /// Sort rank within the dock; ties fall back to the identifier.
    #[serde(default)]
    order: i32,
    /// Plain text shown in the window body; the host never interprets markup.
    /// Exactly one of `text` and `web`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// The plugin's own page shown in the window body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    web: Option<UIWebContent>,
    /// Host-rendered tree/list content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native: Option<lithe_plugin_native_ui::NativeUI>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<UIIcon>,
}

/// A page from the plugin package. The host serves only package files to it,
/// grants no network, navigation or storage, and talks to it through the
/// plugin web bridge, which reaches only this plugin's own declarations.
///
/// Note: the decision is recorded in .agents/notes/implemented/architecture/2026-10-18-plugin-web-content-in-host-windows.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UIWebContent {
    /// Package-relative HTML entry, e.g. `ui/index.html`.
    entry: String,
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
struct UICommand {
    id: String,
    title: String,
    /// Absent for a command the plugin backend handles.
    #[serde(
        default,
        rename = "toolWindowID",
        skip_serializing_if = "Option::is_none"
    )]
    tool_window_id: Option<String>,
    #[serde(default)]
    order: i32,
    placements: Vec<UICommandPlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<UIIcon>,
}

/// Host surface that may list a plugin command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum UICommandPlacement {
    CommandPalette,
    Toolbar,
    Menu,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginVendor {
    id: String,
    display_name: String,
    signature_requirement: SignatureRequirement,
    /// Optional trust for the host's sealed built-in copy only. Installation
    /// from every other origin still requires the primary publisher signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bundled_signature_requirement: Option<SignatureRequirement>,
}

/// How a package proves its origin. `sameTeamAsHost` is only honored by a
/// host for packages shipped inside its own signed installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum SignatureRequirement {
    SameTeamAsHost,
    PublisherPackage,
    /// Community package without a Lithe signature. Its integrity comes from
    /// the marketplace index checksum and its trust from the user's explicit
    /// confirmation, so the official namespace can never use it.
    None,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LanguageDeclaration {
    id: String,
    display_name: String,
    #[serde(default)]
    file_extensions: Vec<String>,
    #[serde(default)]
    file_names: Vec<String>,
    #[serde(default)]
    project_file_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dependencies: Option<DependencyDeclaration>,
    /// Syntax highlighting from a package file; see `lithe_plugin_grammar`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grammar: Option<lithe_plugin_grammar::GrammarDeclaration>,
    /// Comment and bracket behavior of the editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    configuration: Option<lithe_plugin_grammar::LanguageConfiguration>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DependencyDeclaration {
    #[serde(default)]
    management_file_names: Vec<String>,
    #[serde(default)]
    project_dependency_paths: Vec<String>,
    #[serde(default)]
    project_binary_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LanguageServerDeclaration {
    #[serde(rename = "languageID")]
    language_id: String,
    /// LSP `languageId` sent for documents; defaults to the language ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language_identifier: Option<String>,
    #[serde(flatten)]
    process: ServerProcess,
    /// Native host integration this server needs, with the package files it
    /// consumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_integration: Option<HostIntegrationDeclaration>,
    /// Settings the server reads through `workspace/configuration` and
    /// receives again in `workspace/didChangeConfiguration` whenever one of
    /// the settings it names changes. String values are templates like
    /// `initializationOptions`, and may also name settings whose `effect`
    /// is `immediate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_configuration: Option<Value>,
    /// Fields that govern what the plugin backend may send to its own server
    /// and how the host keeps the server's state.
    #[serde(flatten)]
    session: lithe_plugin_language_servers::SessionDeclaration,
}

/// A named piece of host code and the package resources it reads. The plugin
/// owns the files; the host owns the behavior and is told where they are.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostIntegrationDeclaration {
    id: String,
    /// Process-backed features of the server's language that the host's own
    /// implementation provides through this integration. A language owned by
    /// a plugin gets none of them unless the plugin names it here or supplies
    /// an execution provider for it. Sorted and unique once validated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    capabilities: Vec<String>,
    /// Resource name to package-relative path templates.
    #[serde(default)]
    resources: BTreeMap<String, ResourcePaths>,
}

/// One package path, or an ordered list of them.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum ResourcePaths {
    One(String),
    Many(Vec<String>),
}

impl ResourcePaths {
    fn paths(&self) -> &[String] {
        match self {
            Self::One(path) => std::slice::from_ref(path),
            Self::Many(paths) => paths,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutionProviderDeclaration {
    #[serde(rename = "languageID")]
    language_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<RunTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    test: Option<TestTemplate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunTemplate {
    executable: ExecutableDeclaration,
    arguments: Vec<ArgumentTemplate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestTemplate {
    #[serde(rename = "frameworkID")]
    framework_id: String,
    workspace_label: String,
    project_markers: Vec<String>,
    test_file_patterns: Vec<String>,
    executable: ExecutableDeclaration,
    workspace_arguments: Vec<ArgumentTemplate>,
    file_arguments: Vec<ArgumentTemplate>,
    test_case_arguments: Vec<ArgumentTemplate>,
}

/// Exactly one of a bare command resolved on the host's tool path or a
/// host-owned project toolchain ID such as `project-go`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutableDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    toolchain: Option<String>,
}

/// One argv element. A conditional element is emitted only when the request
/// selects (or does not select) a file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum ArgumentTemplate {
    Literal(String),
    Conditional {
        value: String,
        when: ArgumentCondition,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum ArgumentCondition {
    FileSelected,
    NoFileSelected,
}

/// Template variables. `${userArguments}` expands to zero or more elements;
/// the others substitute into a single element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateVariable {
    File,
    FileDirectory,
    TestName,
    TestNameRegex,
    UserArguments,
}

impl TemplateVariable {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "file" => Some(Self::File),
            "fileDirectory" => Some(Self::FileDirectory),
            "testName" => Some(Self::TestName),
            "testName:regex" => Some(Self::TestNameRegex),
            "userArguments" => Some(Self::UserArguments),
            _ => None,
        }
    }

    fn needs_file(self) -> bool {
        matches!(self, Self::File | Self::FileDirectory)
    }
}

/// Which command template an argument list belongs to; each allows a
/// different set of variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateContext {
    Run,
    TestWorkspace,
    TestFile,
    TestCase,
}

impl TemplateContext {
    fn allows(self, variable: TemplateVariable) -> bool {
        use TemplateVariable::*;
        match self {
            Self::Run => matches!(variable, File | FileDirectory | UserArguments),
            Self::TestWorkspace => false,
            Self::TestFile => matches!(variable, File | FileDirectory),
            Self::TestCase => matches!(variable, File | FileDirectory | TestName | TestNameRegex),
        }
    }
}

fn parse_template(value: &str) -> Option<Vec<TemplatePart<TemplateVariable>>> {
    parse_template_with(value, TemplateVariable::parse)
}

impl lithe_plugin_model::PluginManifest for DeclarativePluginManifest {
    fn validate(value: Value) -> Validation<Self> {
        validate_manifest_value(value)
    }

    fn id(&self) -> &str {
        &self.id
    }
}

impl lithe_plugin_keymap_import::KeymapImportManifest for DeclarativePluginManifest {
    fn keymap_import_providers(&self) -> &[lithe_plugin_keymap_import::ProviderDeclaration] {
        &self.keymap_import_providers
    }
}

impl lithe_vcs::VcsManifest for DeclarativePluginManifest {
    fn vcs_providers(&self) -> &[lithe_vcs::VcsProviderDeclaration] {
        &self.vcs_providers
    }
}

impl lithe_build::BuildManifest for DeclarativePluginManifest {
    /// Validated build systems, sorted by ID.
    fn build_systems(&self) -> &[lithe_build::BuildSystemDeclaration] {
        &self.build_systems
    }

    /// Tool-definition IDs the plugin may start processes with, sorted.
    fn required_tools(&self) -> &[String] {
        &self.requires_tools
    }

    /// Strict executable selection preserves invalid stored values instead of
    /// applying the UI settings resolver's tolerant fallback-to-default rule.
    fn executable_setting_value(
        &self,
        id: &str,
        values: &BTreeMap<String, Value>,
    ) -> Validation<Option<String>> {
        let setting = self
            .settings
            .iter()
            .find(|setting| setting.id == id)
            .ok_or_else(|| {
                invalid(
                    "executables.settingID",
                    "The executable setting is not declared.",
                )
            })?;
        setting.executable_value(values.get(id))
    }
}

impl lithe_debug::DebugManifest for DeclarativePluginManifest {
    fn debug_adapters(&self) -> &[lithe_debug::adapters::DebugAdapterDeclaration] {
        &self.debug_adapters
    }
}

impl lithe_project_generators::GeneratorManifest for DeclarativePluginManifest {
    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn requires(&self) -> &Requires {
        &self.requires
    }

    fn project_generators(&self) -> &[lithe_project_generators::ProjectGeneratorDeclaration] {
        &self.project_generators
    }

    fn declares_language(&self, language_id: &str) -> bool {
        self.languages
            .iter()
            .any(|language| language.id == language_id)
    }

    fn declares_build_system(&self, build_system_id: &str) -> bool {
        self.build_systems
            .iter()
            .any(|system| system.id == build_system_id)
    }

    /// Whether the plugin declares a definition that `tool` names. A
    /// definition is addressed by its short ID or as `<pluginID>:<id>`; both
    /// spellings count as a declaration here. Whether the host adopts the
    /// short ID is its registry's decision: when several enabled plugins define
    /// the same short ID, it belongs to none of them.
    fn declares_tool(&self, tool: &str) -> bool {
        // `Some(local)` when `tool` is spelled `<this plugin>:<local>`.
        let local_name = tool
            .strip_prefix(self.id.as_str())
            .and_then(|rest| rest.strip_prefix(':'));
        self.tool_definitions
            .iter()
            .filter_map(|definition| definition["id"].as_str())
            .any(|id| tool == id || local_name == Some(id))
    }
}

/// Validates a raw manifest value and returns its normalized form as JSON, the
/// shape [`lithe_toolchain::ManifestValidator`] gives toolchain policy.
pub fn validated_manifest_json(value: Value) -> Validation<Value> {
    let manifest = validate_manifest_value(value)?;
    serde_json::to_value(manifest)
        .map_err(|_| invalid("manifest", "The validated manifest cannot be encoded."))
}

/// Validates a raw manifest value and returns its normalized form.
pub fn validate_manifest_value(value: Value) -> Validation<DeclarativePluginManifest> {
    // Check the version gates first so a future schema reports the precise
    // reason instead of an unknown-field decoding failure.
    if let Some(version) = value.get("schemaVersion").and_then(Value::as_u64) {
        if version != u64::from(DECLARATIVE_MANIFEST_SCHEMA_VERSION) {
            return Err(PluginRejection::at(
                "plugin.schema.unsupported",
                "schemaVersion",
                format!("Plugin manifest schema {version} is not supported."),
            ));
        }
    }
    if let Some(version) = value.get("pluginAPIVersion").and_then(Value::as_u64) {
        if !SUPPORTED_PLUGIN_API_VERSIONS
            .iter()
            .any(|supported| u64::from(*supported) == version)
        {
            return Err(PluginRejection::at(
                "plugin.api.unsupported",
                "pluginAPIVersion",
                format!("Plugin API {version} is not supported by this version of Lithe."),
            ));
        }
        capabilities::check_manifest_requires(&value)?;
    }
    for (field, limit) in [
        ("toolDefinitions", 32),
        ("toolchainProviders", 8),
        (
            "keymapImportProviders",
            lithe_plugin_keymap_import::MAX_PROVIDERS,
        ),
    ] {
        if value.get(field).is_some_and(|section| {
            !section
                .as_array()
                .is_some_and(|items| !items.is_empty() && items.len() <= limit)
        }) {
            return Err(invalid(
                field,
                "A declared provider section must be nonempty and bounded.",
            ));
        }
    }
    let manifest: DeclarativePluginManifest = serde_json::from_value(value).map_err(|error| {
        PluginRejection::new(
            "plugin.manifest.invalid",
            format!("The plugin manifest is invalid: {error}"),
        )
    })?;
    manifest.validated()
}

impl DeclarativePluginManifest {
    fn validated(mut self) -> Validation<Self> {
        if !is_plugin_id(&self.id) {
            return Err(invalid(
                "id",
                "The plugin ID must use lowercase letters, digits, dots, and hyphens.",
            ));
        }
        if !is_semantic_version(&self.version) {
            return Err(invalid(
                "version",
                "The plugin version must use major.minor.patch.",
            ));
        }
        if self.display_name.trim().is_empty() {
            return Err(invalid("displayName", "The plugin display name is empty."));
        }
        if self.vendor.id.trim().is_empty() || self.vendor.display_name.trim().is_empty() {
            return Err(invalid("vendor", "The plugin vendor is incomplete."));
        }
        if self.vendor.signature_requirement == SignatureRequirement::None
            && is_official_namespace(&self.id)
        {
            return Err(invalid(
                "vendor.signatureRequirement",
                "Official plugin IDs must carry a Lithe signature.",
            ));
        }
        if let Some(bundled) = self.vendor.bundled_signature_requirement {
            if self.vendor.signature_requirement != SignatureRequirement::PublisherPackage
                || bundled != SignatureRequirement::SameTeamAsHost
                || !is_official_namespace(&self.id)
            {
                return Err(invalid(
                    "vendor.bundledSignatureRequirement",
                    "Bundled host-seal trust requires an official plugin ID and publisherPackage for installed copies.",
                ));
            }
        }
        capabilities::check_used(&self.requires, &self.used_capabilities())?;
        if self.languages.is_empty()
            && self.ui.is_none()
            && self.build_systems.is_empty()
            && self.vcs_providers.is_empty()
            && self.debug_adapters.is_empty()
            && self.project_generators.is_empty()
            && self.tool_definitions.is_empty()
            && self.toolchain_providers.is_empty()
            && self.keymap_import_providers.is_empty()
            && self.output_link_matchers.is_empty()
            // A backend may be the whole plugin: it starts on a workspace.
            && self.backend.is_none()
        {
            return Err(invalid("languages", "The plugin declares no languages."));
        }

        let mut language_ids = BTreeSet::new();
        for (index, language) in self.languages.iter_mut().enumerate() {
            language.normalize(index)?;
            if !language_ids.insert(language.id.clone()) {
                return Err(invalid(
                    format!("languages[{index}].id"),
                    "A language is declared more than once.",
                ));
            }
        }
        self.languages.sort_by(|left, right| left.id.cmp(&right.id));

        let declared_settings = self.validate_settings()?;
        let required_tools = self.validate_required_tools()?;
        let runtime_tools = RuntimeTools::new(&required_tools);
        lithe_plugin_output_links::validate_declarations(
            &mut self.output_link_matchers,
            self.backend.is_some(),
        )?;

        let mut server_languages = BTreeSet::new();
        let mut referenced_immediate_settings = BTreeSet::new();
        for (index, server) in self.language_servers.iter_mut().enumerate() {
            let field = format!("languageServers[{index}]");
            if let Some(integration) = &mut server.host_integration {
                integration.capabilities =
                    sorted_unique(std::mem::take(&mut integration.capabilities));
            }
            if !language_ids.contains(&server.language_id) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "The language server references an undeclared language.",
                ));
            }
            if !server_languages.insert(server.language_id.clone()) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "A language declares more than one language server.",
                ));
            }
            if let Some(configuration) = &server.workspace_configuration {
                collect_setting_references(configuration, &mut referenced_immediate_settings);
            }
            server.validate(&field, &declared_settings, runtime_tools)?;
        }
        if let Some(id) =
            declared_settings.first_unreferenced_immediate(&referenced_immediate_settings)
        {
            return Err(invalid(
                "settings",
                format!("The immediate setting {id} must be used by workspaceConfiguration."),
            ));
        }
        self.language_servers
            .sort_by(|left, right| left.language_id.cmp(&right.language_id));

        let mut execution_languages = BTreeSet::new();
        for (index, provider) in self.execution_providers.iter_mut().enumerate() {
            let field = format!("executionProviders[{index}]");
            if !language_ids.contains(&provider.language_id) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "The execution provider references an undeclared language.",
                ));
            }
            if !execution_languages.insert(provider.language_id.clone()) {
                return Err(invalid(
                    format!("{field}.languageID"),
                    "A language declares more than one execution provider.",
                ));
            }
            provider.normalize(&field)?;
        }
        self.execution_providers
            .sort_by(|left, right| left.language_id.cmp(&right.language_id));

        // One feature of a language has one owner. Two would leave it to
        // host wiring order whether the plugin's command or the host's
        // implementation runs.
        for (index, server) in self.language_servers.iter().enumerate() {
            let Some(integration) = &server.host_integration else {
                continue;
            };
            let Some(provider) = self
                .execution_providers
                .iter()
                .find(|provider| provider.language_id == server.language_id)
            else {
                continue;
            };
            let declared_twice = [
                (HOST_CAPABILITY_RUN, provider.run.is_some()),
                (HOST_CAPABILITY_TESTING, provider.test.is_some()),
            ]
            .into_iter()
            .find(|(capability, provided)| {
                *provided
                    && integration
                        .capabilities
                        .iter()
                        .any(|name| name == capability)
            });
            if let Some((capability, _)) = declared_twice {
                return Err(invalid(
                    format!("languageServers[{index}].hostIntegration.capabilities"),
                    format!(
                        "The {capability} capability is already provided by the execution provider of {}.",
                        server.language_id
                    ),
                ));
            }
        }

        if self.language_servers.is_empty()
            && self.execution_providers.is_empty()
            && self.ui.is_none()
            && self.build_systems.is_empty()
            && self.vcs_providers.is_empty()
            && self.debug_adapters.is_empty()
            && self.project_generators.is_empty()
            && self.tool_definitions.is_empty()
            && self.toolchain_providers.is_empty()
            && self.keymap_import_providers.is_empty()
            // Console links alone are a complete plugin: Core matches them.
            && self.output_link_matchers.is_empty()
            && self.backend.is_none()
            // A grammar alone is a complete highlighting plugin.
            && !self
                .languages
                .iter()
                .any(|language| language.grammar.is_some() || language.configuration.is_some())
        {
            return Err(invalid(
                "languages",
                "The plugin declares no language server or execution provider.",
            ));
        }
        self.validate_ui()?;
        lithe_project_generators::validate_declarations(&self.project_generators)?;
        lithe_build::validate_declarations(
            &mut self.build_systems,
            &lithe_build::DeclarationContext {
                debug_adapters: &self.debug_adapters,
                official: is_official_namespace(&self.id),
                declared_settings: &declared_settings,
                language_servers: &server_languages,
                runtime_tools,
            },
        )?;
        lithe_vcs::validate_declarations(
            &mut self.vcs_providers,
            &lithe_vcs::DeclarationContext {
                declared_settings: &declared_settings,
                runtime_tools,
            },
        )?;
        self.validate_debug_adapters(&declared_settings, runtime_tools)?;
        self.validate_backend_launches()?;
        self.validate_toolchains()?;
        if !self.keymap_import_providers.is_empty() {
            lithe_plugin_keymap_import::validate_declarations(&mut self.keymap_import_providers)?;
            self.validate_packaged_provider_backend()?;
        }
        self.validate_plugin_functions()?;
        if let Some(backend) = &self.backend {
            backend.validate(&lithe_plugin_backend::DeclarationContext {
                declared_settings: &declared_settings,
                has_web_window: self
                    .ui
                    .as_ref()
                    .is_some_and(|ui| ui.tool_windows.iter().any(|window| window.web.is_some())),
                has_command: self.ui.as_ref().is_some_and(|ui| !ui.commands.is_empty()),
                activates_on_workspace_open: self
                    .activation_events
                    .iter()
                    .any(|event| event == lithe_plugin_backend::ON_WORKSPACE_OPEN),
                // A language server the backend launches starts the backend
                // when the host asks it for the server's command line; an
                // exported function starts it when another plugin calls it.
                // Neither needs a page, command or workspace event.
                provider_query_trigger: !self.toolchain_providers.is_empty()
                    || !self.keymap_import_providers.is_empty()
                    || self
                        .language_servers
                        .iter()
                        .any(|server| server.process.is_backend_launch())
                    || self.plugin_functions.as_ref().is_some_and(
                        lithe_plugin_functions::PluginFunctionsDeclaration::exports_any,
                    ),
                runtime_tools,
            })?;
        }
        self.validate_permissions()?;
        self.validate_activation_events()?;
        Ok(self)
    }

    /// Capabilities whose sections the manifest declares. Launches, tool
    /// references, and executable candidates belong to the base contract and
    /// are never listed.
    fn used_capabilities(&self) -> BTreeSet<&'static str> {
        let sections = [
            (
                capabilities::PACKAGE_TRUST,
                self.vendor.bundled_signature_requirement.is_some(),
            ),
            (capabilities::LANGUAGES, !self.languages.is_empty()),
            (
                capabilities::LANGUAGE_SERVERS,
                !self.language_servers.is_empty(),
            ),
            (
                capabilities::EXECUTION_PROVIDERS,
                !self.execution_providers.is_empty(),
            ),
            (
                capabilities::SETTINGS,
                !self.settings.is_empty() || !self.setting_actions.is_empty(),
            ),
            (capabilities::UI, self.ui.is_some()),
            (capabilities::BUILD_SYSTEMS, !self.build_systems.is_empty()),
            (
                capabilities::PROJECT_GENERATORS,
                !self.project_generators.is_empty(),
            ),
            (
                capabilities::BUILD_PROVIDERS,
                self.build_systems
                    .iter()
                    .any(|system| system.provider.is_some()),
            ),
            (
                capabilities::BUILD_MODEL_INPUTS,
                self.build_systems.iter().any(|system| {
                    system.model_sharing.is_some() || !system.model_inputs.is_empty()
                }),
            ),
            (capabilities::VCS_PROVIDERS, !self.vcs_providers.is_empty()),
            (
                capabilities::DEBUG_ADAPTERS,
                !self.debug_adapters.is_empty(),
            ),
            (capabilities::BACKEND, self.backend.is_some()),
            (
                capabilities::TOOL_DEFINITIONS,
                !self.tool_definitions.is_empty(),
            ),
            (
                capabilities::TOOLCHAIN_PROVIDERS,
                !self.toolchain_providers.is_empty(),
            ),
            (
                capabilities::KEYMAP_IMPORT_PROVIDERS,
                !self.keymap_import_providers.is_empty(),
            ),
            (
                capabilities::OUTPUT_LINK_MATCHERS,
                !self.output_link_matchers.is_empty(),
            ),
            (
                capabilities::PLUGIN_FUNCTIONS,
                self.plugin_functions.is_some(),
            ),
        ];
        sections
            .into_iter()
            .filter_map(|(capability, used)| used.then_some(capability))
            .collect()
    }

    /// Checks the debug adapters against the plugin's settings, language
    /// servers, and `requiresTools`.
    fn validate_debug_adapters(
        &mut self,
        declared_settings: &DeclaredSettings,
        runtime_tools: RuntimeTools<'_>,
    ) -> Validation<()> {
        let language_servers: BTreeSet<String> = self
            .language_servers
            .iter()
            .map(|server| server.language_id.clone())
            .collect();
        lithe_debug::adapters::validate_declarations(
            &mut self.debug_adapters,
            &lithe_debug::adapters::DeclarationContext {
                declared_settings,
                language_servers: &language_servers,
                runtime_tools,
            },
        )
    }

    /// A language server the plugin backend launches asks the backend for its
    /// command line, so the manifest must declare that backend.
    fn validate_backend_launches(&self) -> Validation<()> {
        if self.backend.is_none()
            && self
                .language_servers
                .iter()
                .any(|server| server.process.is_backend_launch())
        {
            return Err(invalid(
                "backend",
                "A language server launched by the plugin backend requires the backend section.",
            ));
        }
        Ok(())
    }

    fn validate_toolchains(&self) -> Validation<()> {
        if self.tool_definitions.is_empty() && self.toolchain_providers.is_empty() {
            return Ok(());
        }
        lithe_toolchain::definitions::validate_declarations(
            &self.id,
            &self.tool_definitions,
            &self.toolchain_providers,
        )?;
        if !self.toolchain_providers.is_empty() {
            self.validate_packaged_provider_backend()?;
        }
        Ok(())
    }

    /// Checks `pluginFunctions`: only the plugin backend registers functions
    /// for the protocol that relays calls.
    fn validate_plugin_functions(&mut self) -> Validation<()> {
        let Some(section) = self.plugin_functions.as_mut() else {
            return Ok(());
        };
        if self.backend.is_none() {
            return Err(invalid(
                "pluginFunctions",
                "Exported and called plugin functions need the plugin backend.",
            ));
        }
        lithe_plugin_functions::validate(section)
    }

    /// Host-queried providers must start without relying on the discovery they provide.
    fn validate_packaged_provider_backend(&self) -> Validation<()> {
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| invalid("backend", "A provider requires the plugin backend."))?;
        let value = serde_json::to_value(backend)
            .map_err(|_| invalid("backend", "The backend cannot be encoded."))?;
        let launch = &value["launch"];
        let packaged = launch["kind"] == "packaged"
            || (launch["kind"] == "runtime" && launch["packagedRuntime"].as_str().is_some());
        if !packaged
            || value["runtimeDependencies"]
                .as_array()
                .is_some_and(|values| !values.is_empty())
        {
            return Err(invalid("backend.launch", "Provider backends must bootstrap from packaged executables or packaged runtimes without discovered dependencies."));
        }
        Ok(())
    }

    fn validate_ui(&mut self) -> Validation<()> {
        let Some(ui) = &mut self.ui else {
            return Ok(());
        };
        if (ui.tool_windows.is_empty() && ui.commands.is_empty())
            || ui.tool_windows.len() > 64
            || ui.commands.len() > 128
        {
            return Err(invalid(
                "ui",
                "UI declares a tool window or a command, with at most 64 tool windows and 128 commands.",
            ));
        }
        let prefix = format!("{}.", self.id);
        let mut ids = BTreeSet::new();
        let mut windows = BTreeSet::new();
        let mut action_ids = BTreeSet::new();
        for (index, window) in ui.tool_windows.iter().enumerate() {
            if let Some(icon) = &window.icon {
                icon.validate()?;
            }
            validate_window_body(window, index)?;
            if let Some(source) = window
                .native
                .as_ref()
                .and_then(|native| native.source.as_ref())
            {
                if !self
                    .build_systems
                    .iter()
                    .any(|system| system.id == source.build_system_id)
                {
                    return Err(invalid(
                        "ui.toolWindows.native.source",
                        "The native source must belong to this plugin's build systems.",
                    ));
                }
            }
            if !is_plugin_id(&window.id)
                || !window.id.starts_with(&prefix)
                || window.title.trim().is_empty()
                || window.title.len() > 256
                || !ids.insert(window.id.clone())
            {
                return Err(invalid(
                    "ui.toolWindows",
                    "Invalid, duplicate or unowned tool-window ID, title or text.",
                ));
            }
            windows.insert(window.id.clone());
            action_ids.insert(format!("{}.toggle", window.id));
        }
        for command in &mut ui.commands {
            if let Some(icon) = &command.icon {
                icon.validate()?;
            }
            if !is_plugin_id(&command.id)
                || !command.id.starts_with(&prefix)
                || command.title.trim().is_empty()
                || command.title.len() > 256
                || !ids.insert(command.id.clone())
                || !action_ids.insert(command.id.clone())
                || match &command.tool_window_id {
                    Some(id) => !windows.contains(id),
                    // A command without a window runs on the plugin backend.
                    None => self.backend.is_none(),
                }
                || command.placements.is_empty()
            {
                return Err(invalid(
                    "ui.commands",
                    "Invalid or duplicate command, placement or tool-window reference.",
                ));
            }
            command.placements.sort();
            command.placements.dedup();
        }
        // Placement entries share one command action, but their contribution
        // IDs must not shadow an explicitly declared window or another action.
        for command in &ui.commands {
            for placement in &command.placements {
                let suffix = match placement {
                    UICommandPlacement::CommandPalette => "commandPalette",
                    UICommandPlacement::Toolbar => "toolbar",
                    UICommandPlacement::Menu => "menu",
                };
                if !ids.insert(format!("{}.{suffix}", command.id)) {
                    return Err(invalid(
                        "ui.commands",
                        "A command placement shadows another UI identity.",
                    ));
                }
            }
        }
        ui.tool_windows
            .sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        ui.commands
            .sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        Ok(())
    }

    fn validate_permissions(&mut self) -> Validation<()> {
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

    fn validate_activation_events(&mut self) -> Validation<()> {
        let mut expected = BTreeSet::new();
        for server in &self.language_servers {
            expected.insert(format!("onLanguage:{}", server.language_id));
        }
        for provider in &self.execution_providers {
            if provider.run.is_some() {
                expected.insert(format!("onRun:{}", provider.language_id));
            }
            if provider.test.is_some() {
                expected.insert(format!("onTest:{}", provider.language_id));
            }
        }
        for build_system in &self.build_systems {
            expected.insert(lithe_build::activation_event(build_system));
        }
        for provider in &self.vcs_providers {
            expected.insert(lithe_vcs::activation_event(provider));
        }
        for adapter in &self.debug_adapters {
            expected.insert(lithe_debug::adapters::activation_event(adapter));
        }
        if let Some(ui) = &self.ui {
            for window in &ui.tool_windows {
                expected.insert(format!("onView:{}", window.id));
            }
            for command in &ui.commands {
                expected.insert(format!("onCommand:{}", command.id));
            }
        }
        self.activation_events = sorted_unique(std::mem::take(&mut self.activation_events));
        let mut declared: BTreeSet<String> = self.activation_events.iter().cloned().collect();
        // The backend may start once a workspace opens. The event is optional
        // because pages and commands can start it too, so it is checked here
        // and kept out of the exact provider match below.
        if self.backend.is_some() {
            declared.remove(lithe_plugin_backend::ON_WORKSPACE_OPEN);
        }
        // Every provider needs an event, and every event needs a provider:
        // an event without one would promise activation the host cannot do.
        if let Some(missing) = expected.difference(&declared).next() {
            return Err(PluginRejection::at(
                "plugin.activation.invalid",
                "activationEvents",
                format!("The plugin must declare the activation event {missing}."),
            ));
        }
        if let Some(extra) = declared.difference(&expected).next() {
            return Err(PluginRejection::at(
                "plugin.activation.invalid",
                "activationEvents",
                format!("The activation event {extra} has no matching provider."),
            ));
        }
        Ok(())
    }

    fn execution_provider(&self, language_id: &str) -> Option<&ExecutionProviderDeclaration> {
        self.execution_providers
            .iter()
            .find(|provider| provider.language_id == language_id)
    }
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

    /// Checks `requiresTools` and returns its IDs. Every tool a process names
    /// is one of them.
    fn validate_required_tools(&mut self) -> Validation<BTreeSet<String>> {
        if self.requires_tools.is_empty() {
            return Ok(BTreeSet::new());
        }
        if self.requires_tools.len() > MAX_REQUIRED_TOOLS {
            return Err(invalid(
                "requiresTools",
                format!("A plugin requires at most {MAX_REQUIRED_TOOLS} tools."),
            ));
        }
        let mut tools = BTreeSet::new();
        for (index, tool) in self.requires_tools.iter().enumerate() {
            if !lithe_plugin_model::is_tool_id(tool) {
                return Err(invalid(
                    format!("requiresTools[{index}]"),
                    "A required tool is a short or plugin-qualified tool-definition ID.",
                ));
            }
            if !tools.insert(tool.clone()) {
                return Err(invalid(
                    format!("requiresTools[{index}]"),
                    "A tool is required more than once.",
                ));
            }
        }
        self.requires_tools = tools.iter().cloned().collect();
        Ok(tools)
    }

    /// Checks the declared settings, sorts them by ID, and returns the IDs
    /// launch templates may refer to.
    fn validate_settings(&mut self) -> Validation<DeclaredSettings> {
        self.validate_setting_actions()?;
        if self.settings.is_empty() {
            return Ok(DeclaredSettings::default());
        }
        if self.settings.len() > MAX_SETTINGS {
            return Err(invalid(
                "settings",
                "The plugin declares too many settings.",
            ));
        }
        let mut ids = BTreeSet::new();
        for (index, setting) in self.settings.iter().enumerate() {
            let field = format!("settings[{index}]");
            setting.validate(&field)?;
            if !ids.insert(setting.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "A setting is declared more than once.",
                ));
            }
        }
        self.settings.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(lithe_plugin_model::settings::declared(&self.settings))
    }

    /// Checks the settings-page actions and sorts them by ID.
    fn validate_setting_actions(&mut self) -> Validation<()> {
        if self.setting_actions.is_empty() {
            return Ok(());
        }
        if self.setting_actions.len() > MAX_SETTING_ACTIONS {
            return Err(invalid(
                "settingActions",
                "The plugin declares too many settings actions.",
            ));
        }
        let mut ids = BTreeSet::new();
        for (index, action) in self.setting_actions.iter().enumerate() {
            let field = format!("settingActions[{index}]");
            action.validate(&field)?;
            if !ids.insert(action.id.clone()) {
                return Err(invalid(
                    format!("{field}.id"),
                    "An action is declared more than once.",
                ));
            }
        }
        self.setting_actions
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }

    /// Effective value of every declared setting and the layer it came from.
    pub(crate) fn resolve_settings(&self, layers: SettingLayers<'_>) -> Vec<ResolvedSetting> {
        lithe_plugin_model::settings::resolve(&self.settings, layers)
    }

    /// Value of every declared setting for one launch; see
    /// `lithe_plugin_model::settings::resolve` for the fallback order.
    fn effective_settings(&self, layers: SettingLayers<'_>) -> BTreeMap<String, Value> {
        lithe_plugin_model::settings::effective_values(&self.settings, layers)
    }

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
            // Integration resources are files host code loads or runs for
            // the plugin, so repointing one is a new program as well.
            if let Some(integration) = &server.host_integration {
                program["hostIntegration"] = json!(integration);
            }
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

impl LanguageDeclaration {
    fn normalize(&mut self, index: usize) -> Validation<()> {
        let field = format!("languages[{index}]");
        if let Some(grammar) = &self.grammar {
            grammar.validate(&format!("{field}.grammar"))?;
        }
        if let Some(configuration) = &self.configuration {
            configuration.validate(&format!("{field}.configuration"))?;
        }
        let id = self.id.trim().to_lowercase();
        if id.is_empty()
            || id != self.id
            || !id.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || "+-_".contains(character)
            })
        {
            return Err(invalid(
                format!("{field}.id"),
                "The language ID must be lowercase.",
            ));
        }
        if self.display_name.trim().is_empty() {
            return Err(invalid(
                format!("{field}.displayName"),
                "The language display name is empty.",
            ));
        }
        self.file_extensions = normalized_extensions(&self.file_extensions);
        self.file_names = normalized_names(&self.file_names);
        self.project_file_names = normalized_names(&self.project_file_names);
        if self.file_extensions.is_empty()
            && self.file_names.is_empty()
            && self.project_file_names.is_empty()
        {
            return Err(invalid(
                field,
                "The language declares no file extensions or file names.",
            ));
        }
        if self
            .file_extensions
            .iter()
            .chain(&self.file_names)
            .chain(&self.project_file_names)
            .any(|name| name.contains('/') || name.contains('\\'))
        {
            return Err(invalid(
                field,
                "Language file names must not contain path separators.",
            ));
        }
        if let Some(dependencies) = &mut self.dependencies {
            dependencies.management_file_names =
                normalized_names(&dependencies.management_file_names);
            dependencies.project_dependency_paths =
                sorted_unique(dependencies.project_dependency_paths.clone());
            dependencies.project_binary_paths =
                sorted_unique(dependencies.project_binary_paths.clone());
            if dependencies
                .management_file_names
                .iter()
                .chain(&dependencies.project_dependency_paths)
                .chain(&dependencies.project_binary_paths)
                .any(|path| !is_safe_relative_path(path))
            {
                return Err(invalid(
                    format!("{field}.dependencies"),
                    "Dependency paths must stay inside the project.",
                ));
            }
        }
        Ok(())
    }
}

impl LanguageServerDeclaration {
    fn validate(
        &self,
        field: &str,
        declared_settings: &DeclaredSettings,
        runtime_tools: RuntimeTools<'_>,
    ) -> Validation<()> {
        self.process.validate(
            field,
            declared_settings,
            ServerRole::LanguageServer,
            runtime_tools,
        )?;
        self.session.validate(field)?;
        if self.process.is_backend_launch() && self.host_integration.is_some() {
            return Err(invalid(
                format!("{field}.hostIntegration"),
                "A language server started through the plugin backend has no host integration.",
            ));
        }
        if let Some(configuration) = &self.workspace_configuration {
            let field = format!("{field}.workspaceConfiguration");
            if !configuration.is_object() {
                return Err(invalid(field, "workspaceConfiguration is a JSON object."));
            }
            validate_launch_value(
                configuration,
                &declared_settings.for_configuration(),
                &field,
            )?;
            // The host re-expands it on every change without a launch, so
            // only setting values may vary.
            if mentions_launch_directory(configuration) {
                return Err(invalid_template(
                    field,
                    "workspaceConfiguration may only use ${setting:<id>} variables.",
                ));
            }
        }
        if let Some(identifier) = &self.language_identifier {
            if identifier.trim().is_empty() {
                return Err(invalid(
                    format!("{field}.languageIdentifier"),
                    "The LSP language identifier is empty.",
                ));
            }
        }
        if let Some(integration) = &self.host_integration {
            self.validate_host_integration(integration, field)?;
        }
        Ok(())
    }

    fn validate_host_integration(
        &self,
        integration: &HostIntegrationDeclaration,
        field: &str,
    ) -> Validation<()> {
        let field = format!("{field}.hostIntegration");
        if !KNOWN_HOST_INTEGRATIONS.contains(&integration.id.as_str()) {
            return Err(PluginRejection::at(
                "plugin.integration.unsupported",
                format!("{field}.id"),
                format!("The host has no integration named {}.", integration.id),
            ));
        }
        // (capability, resources the host code behind it reads); empty
        // because no integration exists.
        let capabilities: &[(&str, &[&str])] = &[];
        for name in &integration.capabilities {
            let capabilities_field = format!("{field}.capabilities");
            let Some((_, needed)) = capabilities.iter().find(|(known, _)| known == name) else {
                return Err(invalid(
                    capabilities_field,
                    format!("The integration does not provide the capability {name}."),
                ));
            };
            if let Some(missing) = needed
                .iter()
                .find(|resource| !integration.resources.contains_key(**resource))
            {
                return Err(invalid(
                    capabilities_field,
                    format!("The capability {name} requires the resource {missing}."),
                ));
            }
        }
        // (name, required, accepts a list); empty because no integration exists.
        let known: &[(&str, bool, bool)] = &[];
        for (name, paths) in &integration.resources {
            let resource_field = format!("{field}.resources.{name}");
            let Some((_, _, accepts_list)) = known.iter().find(|(known, _, _)| known == name)
            else {
                return Err(invalid(
                    resource_field,
                    "The integration does not use this resource.",
                ));
            };
            if matches!(paths, ResourcePaths::Many(_)) != *accepts_list {
                return Err(invalid(
                    resource_field,
                    "The resource has the wrong shape for this integration.",
                ));
            }
            if paths
                .paths()
                .iter()
                .any(|path| !is_packaged_runtime_path(path))
            {
                return Err(invalid(
                    resource_field,
                    "Integration resources must be paths inside the plugin package that vary only by ${arch}.",
                ));
            }
        }
        if let Some((name, _, _)) = known
            .iter()
            .find(|(name, required, _)| *required && !integration.resources.contains_key(*name))
        {
            return Err(invalid(
                format!("{field}.resources"),
                format!("The integration requires the resource {name}."),
            ));
        }
        Ok(())
    }
}

/// A window body is literal text, one package HTML page, or host-rendered
/// native content.
fn validate_window_body(window: &UIToolWindow, index: usize) -> Validation<()> {
    let field = format!("ui.toolWindows[{index}]");
    if let Some(native) = &window.native {
        if window.text.is_some() || window.web.is_some() {
            return Err(invalid(
                &field,
                "Native content cannot coexist with text or web.",
            ));
        }
        return native
            .validate()
            .map_err(|message| invalid(&field, message));
    }
    match (&window.text, &window.web) {
        (Some(text), None) if text.len() <= 65536 => Ok(()),
        (Some(_), None) => Err(invalid(
            format!("{field}.text"),
            "The window text is too long.",
        )),
        (None, Some(web)) => {
            let entry = &web.entry;
            let valid = is_safe_relative_path(entry)
                && entry.len() <= 256
                && !entry.contains(':')
                && !entry.chars().any(char::is_control)
                && entry.split('/').all(|part| !part.is_empty() && part != ".")
                && entry.ends_with(".html");
            if valid {
                Ok(())
            } else {
                Err(invalid(
                    format!("{field}.web.entry"),
                    "The web entry must be a package-relative .html file.",
                ))
            }
        }
        _ => Err(invalid(
            field,
            "A tool window shows exactly one of text and web content.",
        )),
    }
}

impl ExecutionProviderDeclaration {
    fn normalize(&mut self, field: &str) -> Validation<()> {
        if self.run.is_none() && self.test.is_none() {
            return Err(invalid(
                field.to_string(),
                "The execution provider declares neither run nor test.",
            ));
        }
        if let Some(run) = &self.run {
            validate_executable(&run.executable, &format!("{field}.run.executable"))?;
            validate_arguments(
                &run.arguments,
                TemplateContext::Run,
                &format!("{field}.run.arguments"),
            )?;
        }
        if let Some(test) = &mut self.test {
            let test_field = format!("{field}.test");
            if !is_identifier(&test.framework_id) {
                return Err(invalid(
                    format!("{test_field}.frameworkID"),
                    "The test framework ID is invalid.",
                ));
            }
            if test.workspace_label.trim().is_empty() {
                return Err(invalid(
                    format!("{test_field}.workspaceLabel"),
                    "The workspace test label is empty.",
                ));
            }
            test.project_markers = normalized_names(&test.project_markers);
            test.test_file_patterns = normalized_names(&test.test_file_patterns);
            if test.project_markers.is_empty()
                || test.project_markers.iter().any(|name| name.contains('/'))
            {
                return Err(invalid(
                    format!("{test_field}.projectMarkers"),
                    "Project markers must be file names.",
                ));
            }
            if test.test_file_patterns.is_empty()
                || test.test_file_patterns.iter().any(|pattern| {
                    pattern.contains('/') || pattern.contains('?') || pattern.contains('[')
                })
            {
                return Err(invalid(
                    format!("{test_field}.testFilePatterns"),
                    "Test file patterns must be file names using only *.",
                ));
            }
            validate_executable(&test.executable, &format!("{test_field}.executable"))?;
            validate_arguments(
                &test.workspace_arguments,
                TemplateContext::TestWorkspace,
                &format!("{test_field}.workspaceArguments"),
            )?;
            validate_arguments(
                &test.file_arguments,
                TemplateContext::TestFile,
                &format!("{test_field}.fileArguments"),
            )?;
            validate_arguments(
                &test.test_case_arguments,
                TemplateContext::TestCase,
                &format!("{test_field}.testCaseArguments"),
            )?;
            let names_test = test.test_case_arguments.iter().any(|argument| {
                parse_template(argument.value()).is_some_and(|parts| {
                    parts.iter().any(|part| {
                        matches!(
                            part,
                            TemplatePart::Variable(
                                TemplateVariable::TestName | TemplateVariable::TestNameRegex
                            )
                        )
                    })
                })
            });
            if !names_test {
                return Err(invalid_template(
                    format!("{test_field}.testCaseArguments"),
                    "The test case command must reference the test name.",
                ));
            }
        }
        Ok(())
    }
}

impl ArgumentTemplate {
    fn value(&self) -> &str {
        match self {
            Self::Literal(value) | Self::Conditional { value, .. } => value,
        }
    }

    fn condition(&self) -> Option<ArgumentCondition> {
        match self {
            Self::Literal(_) => None,
            Self::Conditional { when, .. } => Some(*when),
        }
    }
}

fn validate_executable(executable: &ExecutableDeclaration, field: &str) -> Validation<()> {
    match (&executable.command, &executable.toolchain) {
        (Some(command), None) if is_bare_command(command) => Ok(()),
        (None, Some(toolchain)) if is_identifier(toolchain) => Ok(()),
        _ => Err(invalid(
            field.to_string(),
            "The executable must be exactly one bare command or toolchain ID.",
        )),
    }
}

fn validate_arguments(
    arguments: &[ArgumentTemplate],
    context: TemplateContext,
    field: &str,
) -> Validation<()> {
    for (index, argument) in arguments.iter().enumerate() {
        let element = format!("{field}[{index}]");
        let value = argument.value();
        if value.contains('\0') {
            return Err(invalid_template(element, "Arguments must not contain NUL."));
        }
        let Some(parts) = parse_template(value) else {
            return Err(invalid_template(
                element,
                "The argument uses an unknown or unterminated template variable.",
            ));
        };
        let condition = argument.condition();
        if condition.is_some() && context != TemplateContext::TestCase {
            return Err(invalid_template(
                element,
                "Conditional arguments are only allowed in test case commands.",
            ));
        }
        for part in &parts {
            let TemplatePart::Variable(variable) = part else {
                continue;
            };
            if !context.allows(*variable) {
                return Err(invalid_template(
                    element,
                    "The argument uses a variable that is not available in this command.",
                ));
            }
            if *variable == TemplateVariable::UserArguments && parts.len() != 1 {
                return Err(invalid_template(
                    element,
                    "${userArguments} must be a whole argument.",
                ));
            }
            // A test case may run without a selected file, so file variables
            // must be guarded instead of expanding to an empty argument.
            if context == TemplateContext::TestCase
                && variable.needs_file()
                && condition != Some(ArgumentCondition::FileSelected)
            {
                return Err(invalid_template(
                    element,
                    "File variables in a test case command require \"when\": \"fileSelected\".",
                ));
            }
        }
    }
    Ok(())
}

// MARK: Plans

/// A run request for one workspace-relative file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPlanRequest {
    relative_file_path: String,
    #[serde(default)]
    arguments: Vec<String>,
}

/// A test discovery request over the workspace's relative file list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestDiscoveryRequest {
    relative_project_file_paths: Vec<String>,
}

/// A test launch request for a workspace, file, or single test case.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestPlanRequest {
    scope: TestScope,
    relative_project_file_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum TestScope {
    Workspace,
    File {
        #[serde(rename = "relativeFilePath")]
        relative_file_path: String,
    },
    TestCase {
        identifier: String,
        #[serde(default, rename = "relativeFilePath")]
        relative_file_path: Option<String>,
    },
}

/// Values substituted into one command template.
struct ExpansionValues<'a> {
    file: Option<&'a str>,
    test_name: Option<&'a str>,
    user_arguments: &'a [String],
}

fn run_plan(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &RunPlanRequest,
) -> Validation<Value> {
    let run = manifest
        .execution_provider(language_id)
        .and_then(|provider| provider.run.as_ref())
        .ok_or_else(|| unavailable(language_id, "run"))?;
    let path = checked_path(&request.relative_file_path)?;
    let arguments = expand(
        &run.arguments,
        &ExpansionValues {
            file: Some(&path),
            test_name: None,
            user_arguments: &request.arguments,
        },
    );
    Ok(json!({
        "executable": run.executable,
        "arguments": arguments,
        "workingDirectory": "."
    }))
}

fn discover_tests(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &TestDiscoveryRequest,
) -> Validation<Value> {
    let test = test_template(manifest, language_id)?;
    let paths = checked_paths(&request.relative_project_file_paths)?;
    if !supports_project(test, &paths) {
        return Ok(json!([]));
    }
    let mut files: Vec<&String> = paths
        .iter()
        .filter(|path| {
            let name = file_name(path).to_lowercase();
            test.test_file_patterns
                .iter()
                .any(|pattern| matches_pattern(pattern, &name))
        })
        .collect();
    files.sort();
    files.dedup();
    let mut items = vec![json!({
        "id": format!("{language_id}:workspace"),
        "label": test.workspace_label,
        "kind": "workspace"
    })];
    items.extend(files.into_iter().map(|path| {
        json!({
            "id": format!("{language_id}:file:{path}"),
            "label": path,
            "kind": "file",
            "relativeFilePath": path
        })
    }));
    Ok(Value::Array(items))
}

fn test_plan(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &TestPlanRequest,
) -> Validation<Value> {
    let test = test_template(manifest, language_id)?;
    let paths = checked_paths(&request.relative_project_file_paths)?;
    if !supports_project(test, &paths) {
        return Err(PluginRejection::new(
            "plugin.test.unsupportedProject",
            format!("The workspace is not a supported {language_id} test project."),
        ));
    }
    let (label, arguments) = match &request.scope {
        TestScope::Workspace => (
            test.workspace_label.clone(),
            expand(
                &test.workspace_arguments,
                &ExpansionValues {
                    file: None,
                    test_name: None,
                    user_arguments: &[],
                },
            ),
        ),
        TestScope::File { relative_file_path } => {
            let path = checked_path(relative_file_path)?;
            let arguments = expand(
                &test.file_arguments,
                &ExpansionValues {
                    file: Some(&path),
                    test_name: None,
                    user_arguments: &[],
                },
            );
            (file_name(&path).to_string(), arguments)
        }
        TestScope::TestCase {
            identifier,
            relative_file_path,
        } => {
            let name = identifier.trim();
            if name.is_empty() || name.contains('\n') || name.contains('\r') {
                return Err(PluginRejection::new(
                    "plugin.test.invalidIdentifier",
                    "The selected test identifier is invalid.",
                ));
            }
            let path = relative_file_path
                .as_deref()
                .map(checked_path)
                .transpose()?;
            let arguments = expand(
                &test.test_case_arguments,
                &ExpansionValues {
                    file: path.as_deref(),
                    test_name: Some(name),
                    user_arguments: &[],
                },
            );
            (name.to_string(), arguments)
        }
    };
    Ok(json!({
        "label": label,
        "frameworkID": test.framework_id,
        "executable": test.executable,
        "arguments": arguments,
        "workingDirectory": "."
    }))
}

/// Host facts substituted into a language server's launch templates. The
/// host owns every directory; Core only joins text and never touches disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageServerLaunchPlanRequest {
    /// Absolute root of the installed plugin package.
    plugin_directory: String,
    /// Absolute writable directory the host keeps for this plugin's server.
    state_directory: String,
    /// Absolute writable directory for the server's state of this workspace.
    /// A host without a workspace, such as one starting a plugin backend,
    /// omits it; a launch that names it then has no usable plan.
    #[serde(default)]
    workspace_state_directory: Option<String>,
    /// Absolute current-session `plugins/<id>/files` directory. The host creates it
    /// outside installed resources; Core never infers a writable path.
    #[serde(default)]
    log_directory: Option<String>,
    /// Absolute writable scratch directory for one start of the server. Only
    /// a launch the backend resolves needs it.
    #[serde(default)]
    temporary_directory: Option<String>,
    /// Host CPU architecture, one of `KNOWN_ARCHITECTURES`.
    architecture: String,
    /// Values the user stored for the plugin's settings, by setting ID. A
    /// missing or no longer acceptable value falls back to the default.
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    /// The current workspace's overrides of project-scoped settings.
    #[serde(default)]
    project_settings: BTreeMap<String, Value>,
}

impl LanguageServerLaunchPlanRequest {
    fn layers(&self) -> SettingLayers<'_> {
        SettingLayers {
            global: &self.settings,
            project: &self.project_settings,
        }
    }

    /// The four writable directories a backend-resolved launch receives.
    /// Each is required, because the answer may name paths in any of them.
    fn backend_launch_directories(
        &self,
    ) -> Validation<lithe_plugin_language_servers::LaunchDirectories<'_>> {
        fn required<'a>(value: Option<&'a String>, name: &str) -> Validation<&'a str> {
            match value {
                Some(directory) => launch_directory(directory, name),
                None => Err(PluginRejection::new(
                    "plugin.launch.invalidRequest",
                    format!("The launch request has no usable {name}."),
                )),
            }
        }
        Ok(lithe_plugin_language_servers::LaunchDirectories {
            state: launch_directory(&self.state_directory, "stateDirectory")?,
            workspace_state: required(
                self.workspace_state_directory.as_ref(),
                "workspaceStateDirectory",
            )?,
            log: required(self.log_directory.as_ref(), "logDirectory")?,
            temporary: required(self.temporary_directory.as_ref(), "temporaryDirectory")?,
        })
    }
}

/// Validated substitution values of one launch plan.
struct LaunchValues<'a> {
    plugin_directory: &'a str,
    state_directory: &'a str,
    workspace_state_directory: &'a str,
    log_directory: &'a str,
    architecture: &'a str,
    /// Effective value of every declared setting.
    settings: &'a BTreeMap<String, Value>,
}

/// What a launch plan needs from the host besides the command line.
#[derive(Default)]
struct LaunchRequirements {
    /// Package-relative files or directories the templates point at.
    package_paths: BTreeSet<String>,
    uses_state_directory: bool,
    uses_workspace_state_directory: bool,
    uses_log_directory: bool,
}

fn language_server_launch_plan(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let server = manifest
        .language_servers
        .iter()
        .find(|server| server.language_id == language_id)
        .ok_or_else(|| {
            PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The plugin does not provide a language server for {language_id}."),
            )
        })?;
    let mut plan = match &server.process.launch {
        LanguageServerLaunch::Backend {
            function,
            timeout_milliseconds,
        } => backend_resolved_launch_plan(
            manifest,
            &server.language_id,
            function,
            *timeout_milliseconds,
            request,
        )?,
        _ => server_launch_plan(
            manifest,
            &server.process,
            server.host_integration.as_ref(),
            request,
        )?,
    };
    if let Some(configuration) = &server.workspace_configuration {
        plan["workspaceConfiguration"] = expand_configuration(
            configuration,
            &manifest.effective_settings(request.layers()),
        );
    }
    // The engine enforces the session fields.
    plan["sessionPolicy"] = server.session.policy(&server.language_id);
    Ok(plan)
}

/// The first step of a language server whose backend resolves its launch:
/// the function to call and its params. The host calls
/// the function and passes the answer to `plugins.languageServers.decodeLaunch`,
/// whose plan it then starts like any other.
fn backend_resolved_launch_plan(
    manifest: &DeclarativePluginManifest,
    server_id: &str,
    function: &str,
    timeout_milliseconds: Option<u64>,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let directories = request.backend_launch_directories()?;
    if !KNOWN_ARCHITECTURES.contains(&request.architecture.as_str()) {
        return Err(PluginRejection::new(
            "plugin.launch.invalidRequest",
            format!(
                "The architecture {} is not supported.",
                request.architecture
            ),
        ));
    }
    let settings = manifest.effective_settings(request.layers());
    let mut plan = json!({
        "owner": {"pluginID":manifest.id,"version":manifest.version},
        "launch": {
            "kind": "backend",
            "function": function,
            "timeoutMilliseconds": timeout_milliseconds
                .unwrap_or(lithe_plugin_model::process::RESOLVE_TIMEOUT_MILLISECONDS),
        },
        "resolveParams": lithe_plugin_language_servers::resolve_params(
            server_id,
            &request.architecture,
            &directories,
            &settings,
        ),
        "arguments": [],
        "validationArguments": [],
        "runtimeRequirements": [],
        "requiredDirectories": ["state", "workspaceState", "log", "temporary"],
        "requiredPackagePaths": [],
    });
    if !settings.is_empty() {
        plan["settings"] = json!(settings);
    }
    Ok(plan)
}

/// `plugins.languageServers.decodeLaunch`: checks a launch function's answer
/// against the server's declaration and the directories of this start, and
/// returns the plan the host starts. A server whose launch is not resolved
/// by the backend is refused, so a host never runs an answer it did not ask
/// for.
pub fn decode_language_server_launch_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "plugins.languageServers.decodeLaunch";
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        manifest: Value,
        #[serde(rename = "languageID")]
        language_id: String,
        request: LanguageServerLaunchPlanRequest,
        result: Value,
    }
    let payload: Payload = decode_payload(payload, COMMAND)?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            let server = manifest
                .language_servers
                .iter()
                .find(|server| server.language_id == payload.language_id)
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.launch.unavailable",
                        format!(
                            "The plugin does not provide a language server for {}.",
                            payload.language_id
                        ),
                    )
                })?;
            if !server.process.is_backend_launch() {
                return Err(PluginRejection::new(
                    "plugin.launch.notBackend",
                    format!(
                        "The language server for {} is not launched by the plugin backend.",
                        payload.language_id
                    ),
                ));
            }
            let directories = payload.request.backend_launch_directories()?;
            let tool_ids: BTreeSet<&str> = manifest
                .tool_definitions
                .iter()
                .filter_map(|definition| definition["id"].as_str())
                .collect();
            let (mut plan, gates) = lithe_plugin_language_servers::decode_answer(
                payload.result,
                &directories,
                &tool_ids,
            )?;
            let settings = manifest.effective_settings(payload.request.layers());
            if !settings.is_empty() {
                plan["settings"] = json!(settings);
            }
            if plan.get("workspaceConfiguration").is_none() {
                if let Some(configuration) = &server.workspace_configuration {
                    plan["workspaceConfiguration"] = expand_configuration(configuration, &settings);
                }
            }
            let mut policy = server.session.policy(&server.language_id);
            policy["gates"] = json!(gates);
            plan["sessionPolicy"] = policy;
            Ok(("plan", plan))
        },
    )))
}

/// Whether any string in a JSON value names a variable other than a setting.
fn mentions_launch_directory(value: &Value) -> bool {
    match value {
        Value::String(text) => parse_template_with(text, LaunchVariable::parse)
            .unwrap_or_default()
            .iter()
            .any(|part| {
                matches!(part, TemplatePart::Variable(variable) if !matches!(variable, LaunchVariable::Setting(_)))
            }),
        Value::Array(items) => items.iter().any(mentions_launch_directory),
        Value::Object(members) => members.values().any(mentions_launch_directory),
        _ => false,
    }
}

/// Expands a validated `workspaceConfiguration` with effective setting
/// values. A string that is exactly one setting keeps the setting's JSON type.
fn expand_configuration(value: &Value, settings: &BTreeMap<String, Value>) -> Value {
    match value {
        Value::String(text) => {
            if let Some(id) = sole_setting_reference(text) {
                return settings.get(&id).cloned().unwrap_or(Value::Null);
            }
            let parts = parse_template_with(text, LaunchVariable::parse).unwrap_or_default();
            Value::String(
                parts
                    .into_iter()
                    .map(|part| match part {
                        TemplatePart::Text(text) => text,
                        TemplatePart::Variable(LaunchVariable::Setting(id)) => {
                            lithe_plugin_model::settings::setting_text(settings.get(&id))
                        }
                        TemplatePart::Variable(_) => String::new(),
                    })
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| expand_configuration(item, settings))
                .collect(),
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, item)| (key.clone(), expand_configuration(item, settings)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The launch plan of a build system's provider. A provider that shares a
/// language server gets that server's plan; `buildProvider.languageServer`
/// then tells the host to send build requests to the session it already
/// keeps for the language instead of starting a second process.
fn build_provider_launch_plan(
    manifest: &DeclarativePluginManifest,
    build_system_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let unavailable = |message: String| PluginRejection::new("plugin.launch.unavailable", message);
    let build_system = manifest
        .build_systems
        .iter()
        .find(|build_system| build_system.id == build_system_id)
        .ok_or_else(|| {
            unavailable(format!(
                "The plugin declares no build system {build_system_id}."
            ))
        })?;
    let provider = build_system.provider.as_ref().ok_or_else(|| {
        unavailable(format!(
            "The build system {build_system_id} has no provider process."
        ))
    })?;
    let mut description = json!({
        "buildSystemID": build_system.id,
        "executesProjectCode": provider.executes_project_code(),
        "mayUseNetwork": provider.may_use_network(),
    });
    let mut plan = match provider.process() {
        lithe_build::ProviderProcess::LanguageServer(language_id) => {
            description["languageServer"] = json!(language_id);
            language_server_launch_plan(manifest, language_id, request)?
        }
        lithe_build::ProviderProcess::Own(process) => {
            server_launch_plan(manifest, process, None, request)?
        }
    };
    plan["buildProvider"] = description;
    Ok(plan)
}

/// The launch plan of a VCS provider. `vcsProvider` repeats what the host
/// needs to resolve the client tool and to check the provider's answer in
/// `initialize`, which must name the same VCS.
fn vcs_provider_launch_plan(
    manifest: &DeclarativePluginManifest,
    vcs_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let provider = manifest
        .vcs_providers
        .iter()
        .find(|provider| provider.id == vcs_id)
        .ok_or_else(|| {
            PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The plugin declares no VCS provider {vcs_id}."),
            )
        })?;
    let mut plan = server_launch_plan(manifest, &provider.process, None, request)?;
    plan["vcsProvider"] = provider.descriptor(&manifest.id);
    Ok(plan)
}

/// The launch plan of the plugin's backend. `backend.hostServices` lists the
/// host services its permissions grant, which the host passes to
/// `lsp.startServer` so Core admits only those requests.
fn backend_launch_plan(
    manifest: &DeclarativePluginManifest,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let backend = manifest.backend.as_ref().ok_or_else(|| {
        PluginRejection::new(
            "plugin.launch.unavailable",
            "The plugin declares no backend.",
        )
    })?;
    let mut plan = server_launch_plan(manifest, &backend.process, None, request)?;
    plan["backend"] = lithe_plugin_backend::plan_descriptor(
        manifest.capability_version(capabilities::BACKEND),
        &manifest.backend_host_services(),
    );
    Ok(plan)
}

/// The launch plan of a debug adapter. `debugAdapter` repeats what the host
/// needs besides the process: the transport and what to show. An adapter on
/// the runtime Run selected has no process of its own to plan yet. An adapter
/// inside the plugin's language server plans that server, as a build provider
/// on a shared server does.
fn debug_adapter_launch_plan(
    manifest: &DeclarativePluginManifest,
    adapter_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    use lithe_debug::adapters::DebugProcess;
    let adapter = manifest
        .debug_adapters
        .iter()
        .find(|adapter| adapter.id == adapter_id)
        .ok_or_else(|| {
            PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The plugin declares no debug adapter {adapter_id}."),
            )
        })?;
    let mut plan = match &adapter.launch.process {
        Some(DebugProcess::Server(process)) => {
            server_launch_plan(manifest, process, None, request)?
        }
        Some(DebugProcess::SelectedRuntime(_)) => {
            return Err(PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The debug adapter {adapter_id} starts on the selected runtime."),
            ));
        }
        None => {
            let language_id = adapter
                .launch
                .endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.language_id.as_deref())
                .unwrap_or_default();
            hosting_language_server_plan(manifest, language_id, request)?
        }
    };
    plan["debugAdapter"] = adapter.plan_descriptor(&manifest.id);
    Ok(plan)
}

/// The plan of the language server a `tcpLanguageServer` adapter runs in.
/// The server's own session starts it; this plan only names the server the
/// adapter belongs to. A server the backend launches is
/// asked by that session with the scratch directories of its own start, so
/// here its plan names the function and demands no directory.
fn hosting_language_server_plan(
    manifest: &DeclarativePluginManifest,
    language_id: &str,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let server = manifest
        .language_servers
        .iter()
        .find(|server| server.language_id == language_id)
        .ok_or_else(|| {
            PluginRejection::new(
                "plugin.launch.unavailable",
                format!("The plugin does not provide a language server for {language_id}."),
            )
        })?;
    match &server.process.launch {
        LanguageServerLaunch::Backend {
            function,
            timeout_milliseconds,
        } => Ok(json!({
            "launch": {
                "kind": "backend",
                "function": function,
                "timeoutMilliseconds": timeout_milliseconds
                    .unwrap_or(lithe_plugin_model::process::RESOLVE_TIMEOUT_MILLISECONDS),
            },
            "arguments": [],
            "validationArguments": [],
            "runtimeRequirements": [],
            "requiredDirectories": [],
            "requiredPackagePaths": [],
        })),
        _ => language_server_launch_plan(manifest, language_id, request),
    }
}

/// `debug.adapters.startArguments`: checks one start against the adapter's
/// declaration and returns the DAP arguments its configuration defaults and
/// target bindings give.
pub fn debug_start_arguments_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "debug.adapters.startArguments";
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        manifest: Value,
        #[serde(rename = "adapterID")]
        adapter_id: String,
        #[serde(flatten)]
        start: lithe_debug::adapters::StartRequest,
    }
    let payload: Payload = decode_payload(payload, COMMAND)?;
    let started = validate_manifest_value(payload.manifest).and_then(|manifest| {
        let adapter = manifest
            .debug_adapters
            .iter()
            .find(|adapter| adapter.id == payload.adapter_id)
            .ok_or_else(|| {
                PluginRejection::new(
                    "plugin.launch.unavailable",
                    format!(
                        "The plugin declares no debug adapter {}.",
                        payload.adapter_id
                    ),
                )
            })?;
        lithe_debug::adapters::start_arguments(adapter, &payload.start)
    });
    // `connection` is present only for a remote target, so a host that never
    // attaches to one can ignore it.
    let connection = started
        .as_ref()
        .ok()
        .and_then(|started| started.connection.clone());
    let mut result =
        outcome(started.map(|started| ("arguments", Value::Object(started.arguments))));
    if let Some(connection) = connection {
        result["connection"] = json!(connection);
    }
    Ok(result)
}

/// `debug.adapters.executablePlan`: where a host looks for the program a
/// launch debugs, when the adapter's configuration binds `program` to
/// `target.executable`. `plan` is `null` for an adapter with no such lookup.
/// The host owns the file system, so it checks `configured` and then each
/// candidate; Core only says what to look at and in which order.
pub fn debug_executable_plan_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "debug.adapters.executablePlan";
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        manifest: Value,
        #[serde(rename = "adapterID")]
        adapter_id: String,
        #[serde(default)]
        settings: BTreeMap<String, Value>,
        #[serde(default)]
        project_settings: BTreeMap<String, Value>,
        /// Absolute root of the open project.
        #[serde(default)]
        workspace: Option<String>,
        /// Absolute path of the source file being debugged, when there is one.
        #[serde(default)]
        target_path: Option<String>,
    }
    let payload: Payload = decode_payload(payload, COMMAND)?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            let adapter = manifest
                .debug_adapters
                .iter()
                .find(|adapter| adapter.id == payload.adapter_id)
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.launch.unavailable",
                        format!(
                            "The plugin declares no debug adapter {}.",
                            payload.adapter_id
                        ),
                    )
                })?;
            let Some(lookup) = adapter
                .configurations
                .as_ref()
                .and_then(|configurations| configurations.launch.as_ref())
                .and_then(|configuration| configuration.executable.as_ref())
            else {
                return Ok(("plan", Value::Null));
            };
            let layers = SettingLayers {
                global: &payload.settings,
                project: &payload.project_settings,
            };
            let configured = manifest
                .resolve_settings(layers)
                .into_iter()
                .find(|setting| setting.id == lookup.setting)
                .and_then(|setting| match &setting.value {
                    Value::String(path) if !path.is_empty() => {
                        Some(json!({"path": path, "source": setting.source}))
                    }
                    _ => None,
                });
            Ok((
                "plan",
                json!({
                    "setting": lookup.setting,
                    "configured": configured,
                    "candidates": lookup.expanded_candidates(
                        payload.workspace.as_deref(),
                        payload.target_path.as_deref(),
                    ),
                }),
            ))
        },
    )))
}

fn server_launch_plan(
    manifest: &DeclarativePluginManifest,
    process: &ServerProcess,
    host_integration: Option<&HostIntegrationDeclaration>,
    request: &LanguageServerLaunchPlanRequest,
) -> Validation<Value> {
    let effective_settings = manifest.effective_settings(request.layers());
    let values = LaunchValues {
        plugin_directory: launch_directory(&request.plugin_directory, "pluginDirectory")?,
        state_directory: launch_directory(&request.state_directory, "stateDirectory")?,
        workspace_state_directory: match &request.workspace_state_directory {
            Some(directory) => launch_directory(directory, "workspaceStateDirectory")?,
            None if process.uses_workspace_state_directory() => {
                return Err(PluginRejection::new(
                    "plugin.launch.invalidRequest",
                    "The launch request has no usable workspaceStateDirectory.",
                ))
            }
            None => "",
        },
        log_directory: match request.log_directory.as_deref() {
            Some(directory) => {
                let directory = launch_directory(directory, "logDirectory")?;
                // Recognize absolute native paths on both platforms without using
                // the build machine's Path::is_absolute semantics for the other OS.
                let absolute = directory.starts_with('/')
                    || directory.starts_with("\\\\")
                    || (directory.as_bytes().get(1) == Some(&b':')
                        && directory
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphabetic)
                        && directory
                            .as_bytes()
                            .get(2)
                            .is_some_and(|b| b"/\\".contains(b)));
                if !absolute {
                    return Err(PluginRejection::new(
                        "plugin.launch.invalidRequest",
                        "The launch request has no usable absolute logDirectory.",
                    ));
                }
                directory
            }
            None => "",
        },
        architecture: &request.architecture,
        settings: &effective_settings,
    };
    if !KNOWN_ARCHITECTURES.contains(&values.architecture) {
        return Err(PluginRejection::new(
            "plugin.launch.invalidRequest",
            format!("The architecture {} is not supported.", values.architecture),
        ));
    }

    let mut requirements = LaunchRequirements::default();
    let expand_all = |templates: &[String], requirements: &mut LaunchRequirements| {
        templates
            .iter()
            .map(|value| expand_launch_template(value, &values, requirements))
            .collect::<Vec<String>>()
    };

    let launch = match &process.launch {
        // A tool definition owns how its executable starts an entry, so the
        // plan hands the host the parts for `toolchains.instantiateLaunch`.
        LanguageServerLaunch::Runtime {
            tool,
            minimum_version,
            packaged_runtime,
            runtime_arguments,
            entry,
        } => {
            requirements.package_paths.insert(entry.clone());
            let mut launch = json!({
                "kind": "tool",
                "tool": tool,
                "entry": format!("{}/{entry}", values.plugin_directory),
                "runtimeArguments": expand_all(runtime_arguments, &mut requirements),
            });
            if let Some(version) = minimum_version {
                launch["minimumVersion"] = json!(version);
            }
            if let Some(path) = packaged_runtime {
                let path = path.replace("${arch}", values.architecture);
                requirements.package_paths.insert(path.clone());
                launch["packagedRuntime"] = json!(path);
            }
            launch
        }
        LanguageServerLaunch::Packaged { path } => {
            requirements.package_paths.insert(path.clone());
            json!({ "kind": "packaged", "path": path })
        }
        LanguageServerLaunch::SystemCommand { commands } => {
            json!({ "kind": "systemCommand", "commands": commands })
        }
        // Validation admits a backend launch only for a language server,
        // whose plan `backend_resolved_launch_plan` builds instead.
        LanguageServerLaunch::Backend { .. } => {
            return Err(PluginRejection::new(
                "plugin.launch.unavailable",
                "This process is launched by the plugin backend.",
            ));
        }
    };
    let arguments = expand_all(&process.arguments, &mut requirements);
    let validation_arguments = expand_all(&process.validation_arguments, &mut requirements);

    let mut plan = json!({
        "owner": {"pluginID":manifest.id,"version":manifest.version},
        "launch": launch,
        "arguments": arguments,
        "validationArguments": validation_arguments,
        "runtimeRequirements": process.runtime_requirements(),
    });
    if let Some(options) = &process.initialization_options {
        plan["initializationOptions"] = expand_launch_value(options, &values, &mut requirements);
    }
    if let Some(integration) = host_integration {
        let mut resources = serde_json::Map::new();
        for (name, paths) in &integration.resources {
            let expanded: Vec<String> = paths
                .paths()
                .iter()
                .map(|path| path.replace("${arch}", values.architecture))
                .collect();
            requirements.package_paths.extend(expanded.iter().cloned());
            resources.insert(
                name.clone(),
                match paths {
                    ResourcePaths::One(_) => json!(expanded[0]),
                    ResourcePaths::Many(_) => json!(expanded),
                },
            );
        }
        plan["hostIntegration"] = json!({ "id": integration.id, "resources": resources });
    }
    let mut directories = Vec::new();
    if requirements.uses_state_directory {
        directories.push("state");
    }
    if requirements.uses_workspace_state_directory {
        directories.push("workspaceState");
    }
    if !effective_settings.is_empty() {
        plan["settings"] = json!(effective_settings);
    }
    if requirements.uses_log_directory {
        if values.log_directory.is_empty() {
            return Err(PluginRejection::new(
                "plugin.launch.invalidRequest",
                "The launch request has no usable logDirectory.",
            ));
        }
        directories.push("log");
    }
    plan["requiredDirectories"] = json!(directories);
    plan["requiredPackagePaths"] = json!(requirements.package_paths);
    Ok(plan)
}

/// Accepts a host directory as substitution text. A trailing separator is
/// dropped so `${pluginDir}/x` never yields a doubled separator.
fn launch_directory<'a>(value: &'a str, name: &str) -> Validation<&'a str> {
    let trimmed = value.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(PluginRejection::new(
            "plugin.launch.invalidRequest",
            format!("The launch request has no usable {name}."),
        ));
    }
    Ok(trimmed)
}

/// The setting ID when the whole template is exactly one `${setting:<id>}`.
/// Such a value keeps the setting's JSON type, so an LSP option can receive a
/// number or a boolean rather than its text.
fn sole_setting_reference(template: &str) -> Option<String> {
    match parse_template_with(template, LaunchVariable::parse)?.as_slice() {
        [TemplatePart::Variable(LaunchVariable::Setting(id))] => Some(id.clone()),
        _ => None,
    }
}

fn expand_launch_value(
    value: &Value,
    values: &LaunchValues<'_>,
    requirements: &mut LaunchRequirements,
) -> Value {
    match value {
        Value::String(text) => sole_setting_reference(text)
            .and_then(|id| values.settings.get(&id).cloned())
            .unwrap_or_else(|| Value::String(expand_launch_template(text, values, requirements))),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| expand_launch_value(item, values, requirements))
                .collect(),
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, item)| (key.clone(), expand_launch_value(item, values, requirements)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn expand_launch_template(
    value: &str,
    values: &LaunchValues<'_>,
    requirements: &mut LaunchRequirements,
) -> String {
    // Validation guarantees the template parses and that the text after
    // `${pluginDir}` is a `/`-rooted path inside the package.
    let parts = parse_template_with(value, LaunchVariable::parse).unwrap_or_default();
    let mut expanded = String::new();
    let mut package_path_start = None;
    for part in parts {
        match part {
            TemplatePart::Text(text) => expanded.push_str(&text),
            TemplatePart::Variable(LaunchVariable::PluginDir) => {
                expanded.push_str(values.plugin_directory);
                package_path_start = Some(expanded.len() + 1);
            }
            TemplatePart::Variable(LaunchVariable::StateDir) => {
                requirements.uses_state_directory = true;
                expanded.push_str(values.state_directory);
            }
            TemplatePart::Variable(LaunchVariable::WorkspaceStateDir) => {
                requirements.uses_workspace_state_directory = true;
                expanded.push_str(values.workspace_state_directory);
            }
            TemplatePart::Variable(LaunchVariable::LogDir) => {
                requirements.uses_log_directory = true;
                expanded.push_str(values.log_directory);
            }
            TemplatePart::Variable(LaunchVariable::Arch) => expanded.push_str(values.architecture),
            TemplatePart::Variable(LaunchVariable::Setting(id)) => {
                expanded.push_str(&lithe_plugin_model::settings::setting_text(
                    values.settings.get(&id),
                ));
            }
        }
    }
    if let Some(start) = package_path_start {
        requirements
            .package_paths
            .insert(expanded[start..].trim_end_matches('/').to_string());
    }
    expanded
}

fn test_template<'a>(
    manifest: &'a DeclarativePluginManifest,
    language_id: &str,
) -> Validation<&'a TestTemplate> {
    manifest
        .execution_provider(language_id)
        .and_then(|provider| provider.test.as_ref())
        .ok_or_else(|| unavailable(language_id, "test"))
}

fn supports_project(test: &TestTemplate, paths: &[String]) -> bool {
    paths.iter().any(|path| {
        test.project_markers
            .contains(&file_name(path).to_lowercase())
    })
}

fn expand(arguments: &[ArgumentTemplate], values: &ExpansionValues<'_>) -> Vec<String> {
    let mut result = Vec::new();
    for argument in arguments {
        match argument.condition() {
            Some(ArgumentCondition::FileSelected) if values.file.is_none() => continue,
            Some(ArgumentCondition::NoFileSelected) if values.file.is_some() => continue,
            _ => {}
        }
        // Validation guarantees the template parses and that every variable
        // it uses has a value in this context.
        let parts = parse_template(argument.value()).unwrap_or_default();
        if parts == [TemplatePart::Variable(TemplateVariable::UserArguments)] {
            result.extend(values.user_arguments.iter().cloned());
            continue;
        }
        let mut element = String::new();
        for part in parts {
            match part {
                TemplatePart::Text(text) => element.push_str(&text),
                TemplatePart::Variable(TemplateVariable::File) => {
                    element.push_str(&command_path(values.file.unwrap_or_default()))
                }
                TemplatePart::Variable(TemplateVariable::FileDirectory) => {
                    element.push_str(&directory_argument(values.file.unwrap_or_default()))
                }
                TemplatePart::Variable(TemplateVariable::TestName) => {
                    element.push_str(values.test_name.unwrap_or_default())
                }
                TemplatePart::Variable(TemplateVariable::TestNameRegex) => {
                    element.push_str(&regex_literal(values.test_name.unwrap_or_default()))
                }
                TemplatePart::Variable(TemplateVariable::UserArguments) => {}
            }
        }
        result.push(element);
    }
    result
}

/// A file name beginning with `-` would be parsed as an option by most
/// tools, so it is passed as an explicit relative path.
fn command_path(path: &str) -> String {
    if path.starts_with('-') {
        format!("./{path}")
    } else {
        path.to_string()
    }
}

/// The selected file's directory as a `./`-relative argument; `.` for a file
/// at the workspace root.
fn directory_argument(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((directory, _)) if !directory.is_empty() => format!("./{directory}"),
        _ => ".".to_string(),
    }
}

fn matches_pattern(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || !name[first.len()..].ends_with(last) {
        return false;
    }
    let mut remainder = &name[first.len()..name.len() - last.len()];
    for piece in &pieces[1..pieces.len() - 1] {
        match remainder.find(piece) {
            Some(index) => remainder = &remainder[index + piece.len()..],
            None => return false,
        }
    }
    true
}

fn checked_path(value: &str) -> Validation<String> {
    // Paths are not trimmed: surrounding spaces are part of a real file name.
    if is_safe_relative_path(value) {
        Ok(value.to_string())
    } else {
        Err(PluginRejection::new(
            "plugin.path.invalid",
            "A path must stay inside the current workspace.",
        ))
    }
}

fn checked_paths(values: &[String]) -> Validation<Vec<String>> {
    values.iter().map(|value| checked_path(value)).collect()
}

fn unavailable(language_id: &str, operation: &str) -> PluginRejection {
    PluginRejection::new(
        "plugin.execution.unavailable",
        format!("The plugin does not provide {operation} support for {language_id}."),
    )
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Trims and lowercases complete file names and markers. A leading `.` is
/// part of the name (`.toolrc`, `.env.example`) and is kept.
fn normalized_names(values: &[String]) -> Vec<String> {
    sorted_unique(
        values
            .iter()
            .filter_map(|value| {
                let name = value.trim().to_lowercase();
                (!name.is_empty()).then_some(name)
            })
            .collect(),
    )
}

/// Normalizes file extensions, where a leading `.` is optional notation:
/// `.php` and `php` both declare the `php` extension.
fn normalized_extensions(values: &[String]) -> Vec<String> {
    let names = normalized_names(values);
    sorted_unique(
        names
            .iter()
            .filter_map(|name| {
                let extension = name.strip_prefix('.').unwrap_or(name);
                (!extension.is_empty()).then(|| extension.to_string())
            })
            .collect(),
    )
}

// MARK: Command entry points

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidatePayload {
    manifest: Value,
    /// Capability versions the host runs. When sent, a manifest that
    /// requires anything else is refused.
    #[serde(default)]
    host_capabilities: Option<HostCapabilities>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanPayload<Request> {
    manifest: Value,
    #[serde(rename = "languageID")]
    language_id: String,
    request: Request,
}

/// A launch plan names the process by the language whose server it is, by
/// the build system whose provider it is, or by the VCS it provides.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchPlanPayload {
    manifest: Value,
    #[serde(default, rename = "languageID")]
    language_id: Option<String>,
    #[serde(default, rename = "buildSystemID")]
    build_system_id: Option<String>,
    #[serde(default, rename = "vcsProviderID")]
    vcs_provider_id: Option<String>,
    /// Selects the plugin's backend.
    #[serde(default)]
    backend: bool,
    #[serde(default, rename = "debugAdapterID")]
    debug_adapter_id: Option<String>,
    request: LanguageServerLaunchPlanRequest,
}

enum LaunchSelection<'a> {
    Language(&'a str),
    BuildSystem(&'a str),
    VcsProvider(&'a str),
    Backend,
    DebugAdapter(&'a str),
}

/// `plugins.declarative.validate`: returns `{status: "ok", plugin}` with the
/// normalized manifest, or `{status: "rejected", error}`.
pub fn validate_command(payload: Value) -> Result<Value, CoreError> {
    let payload: ValidatePayload = decode_payload(payload, "plugins.declarative.validate")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            if let Some(host) = &payload.host_capabilities {
                capabilities::check_hosted(manifest.requires(), host)?;
            }
            serde_json::to_value(manifest)
                .map(|value| ("plugin", value))
                .map_err(|error| PluginRejection::new("plugin.manifest.invalid", error.to_string()))
        },
    )))
}

/// `plugins.declarative.runPlan`: expands the run template for one file.
pub fn run_plan_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PlanPayload<RunPlanRequest> =
        decode_payload(payload, "plugins.declarative.runPlan")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            run_plan(&manifest, &payload.language_id, &payload.request).map(|plan| ("plan", plan))
        },
    )))
}

/// `plugins.declarative.discoverTests`: lists workspace and file test items.
pub fn discover_tests_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PlanPayload<TestDiscoveryRequest> =
        decode_payload(payload, "plugins.declarative.discoverTests")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            discover_tests(&manifest, &payload.language_id, &payload.request)
                .map(|items| ("items", items))
        },
    )))
}

/// `plugins.declarative.testPlan`: expands the test template for one scope.
pub fn test_plan_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PlanPayload<TestPlanRequest> =
        decode_payload(payload, "plugins.declarative.testPlan")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            test_plan(&manifest, &payload.language_id, &payload.request).map(|plan| ("plan", plan))
        },
    )))
}

/// `plugins.declarative.languageServerLaunchPlan`: expands a language
/// server's launch declaration with the host's directories and architecture.
pub fn language_server_launch_plan_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "plugins.declarative.languageServerLaunchPlan";
    let payload: LaunchPlanPayload = decode_payload(payload, COMMAND)?;
    let selected = match (
        &payload.language_id,
        &payload.build_system_id,
        &payload.vcs_provider_id,
        payload.backend,
        &payload.debug_adapter_id,
    ) {
        (Some(language_id), None, None, false, None) => LaunchSelection::Language(language_id),
        (None, Some(build_system_id), None, false, None) => {
            LaunchSelection::BuildSystem(build_system_id)
        }
        (None, None, Some(vcs_id), false, None) => LaunchSelection::VcsProvider(vcs_id),
        (None, None, None, true, None) => LaunchSelection::Backend,
        (None, None, None, false, Some(adapter_id)) => LaunchSelection::DebugAdapter(adapter_id),
        _ => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!("Invalid {COMMAND} payload"),
            )
            .with_details(
                "Name exactly one of languageID, buildSystemID, vcsProviderID, backend, and debugAdapterID.",
            ))
        }
    };
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            match selected {
                LaunchSelection::Language(language_id) => {
                    language_server_launch_plan(&manifest, language_id, &payload.request)
                }
                LaunchSelection::BuildSystem(build_system_id) => {
                    build_provider_launch_plan(&manifest, build_system_id, &payload.request)
                }
                LaunchSelection::VcsProvider(vcs_id) => {
                    vcs_provider_launch_plan(&manifest, vcs_id, &payload.request)
                }
                LaunchSelection::Backend => backend_launch_plan(&manifest, &payload.request),
                LaunchSelection::DebugAdapter(adapter_id) => {
                    debug_adapter_launch_plan(&manifest, adapter_id, &payload.request)
                }
            }
            .map(|plan| ("plan", plan))
        },
    )))
}

/// Stored setting layers of one plugin, as a host sends them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsPayload {
    manifest: Value,
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    #[serde(default)]
    project_settings: BTreeMap<String, Value>,
    /// Names the language server whose `workspaceConfiguration` to expand.
    #[serde(default, rename = "languageID")]
    language_id: Option<String>,
}

impl SettingsPayload {
    fn layers(&self) -> SettingLayers<'_> {
        SettingLayers {
            global: &self.settings,
            project: &self.project_settings,
        }
    }
}

/// `plugins.declarative.resolveSettings`: the effective value of every
/// declared setting and the layer it came from (`default`, `global`, or
/// `project`), so hosts show and launch with the same value.
pub fn resolve_settings_command(payload: Value) -> Result<Value, CoreError> {
    let payload: SettingsPayload = decode_payload(payload, "plugins.declarative.resolveSettings")?;
    Ok(outcome(
        validate_manifest_value(payload.manifest.clone()).and_then(|manifest| {
            serde_json::to_value(manifest.resolve_settings(payload.layers()))
                .map(|settings| ("settings", settings))
                .map_err(|error| PluginRejection::new("plugin.manifest.invalid", error.to_string()))
        }),
    ))
}

mod settings_surfaces;
pub use settings_surfaces::{
    settings_catalog_command, settings_delivery_command, settings_navigation_command,
};

/// Installed package identity captured by the native registration adapter.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsOwner {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    version: String,
}

/// Complete stored setting documents, captured by the native storage adapter.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsStatePayload {
    manifest: Value,
    /// Optional native registration facts; this is identity binding, not admission.
    owner: Option<SettingsOwner>,
    /// Missing files are null; an existing empty or malformed file is an error.
    global_document: Option<String>,
    project_document: Option<String>,
}

/// `plugins.declarative.settingsState`: checked UI rows and raw declared launch
/// layers. Reading a state neither edits storage nor admits plugin capabilities.
pub fn settings_state_command(payload: Value) -> Result<Value, CoreError> {
    let payload: SettingsStatePayload =
        decode_payload(payload, "plugins.declarative.settingsState")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            if payload.owner.as_ref().is_some_and(|owner| {
                owner.plugin_id != manifest.id || owner.version != manifest.version
            }) {
                return Err(PluginRejection::at(
                    "plugin.settings.ownerChanged",
                    "owner",
                    "The manifest does not match the captured installed package identity.",
                ));
            }
            lithe_plugin_model::settings::document_state(
                &manifest.id,
                &manifest.settings,
                &manifest.setting_actions,
                payload.global_document.as_deref(),
                payload.project_document.as_deref(),
            )
            .map(|mut state| {
                state["metadata"] = json!({"displayName": manifest.display_name, "version":manifest.version, "vendor":manifest.vendor.display_name});
                ("state", state)
            })
        },
    )))
}

/// Native facts for one explicitly selected installed setting action.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsActionPayload {
    manifest: Value,
    owner: SettingsOwner,
    #[serde(rename = "actionID")]
    action_id: String,
    /// Captured by the native confirmation UI, not inferred from saved preferences.
    confirmed: bool,
    /// Current native enablement choice; a stored draft cannot activate a plugin.
    enabled: bool,
    /// Describes the requesting window; runtime ownership is checked on invocation.
    workspace_available: bool,
}

/// Plans a declared settings action without starting or admitting its provider.
/// Native callers retain package/project ownership and confirmation checks.
pub fn settings_action_plan_command(payload: Value) -> Result<Value, CoreError> {
    let payload: SettingsActionPayload =
        decode_payload(payload, "plugins.declarative.settingsActionPlan")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            if payload.owner.plugin_id != manifest.id || payload.owner.version != manifest.version {
                return Err(PluginRejection::at(
                    "plugin.settings.ownerChanged",
                    "owner",
                    "The installed package identity changed before the action was planned.",
                ));
            }
            lithe_plugin_model::settings::plan_action(
                &manifest.id,
                &manifest.setting_actions,
                &payload.action_id,
                payload.confirmed,
                payload.workspace_available,
                payload.enabled,
            )
            .map(|mut plan| {
                plan["version"] = json!(manifest.version);
                ("plan", plan)
            })
        },
    )))
}

/// One layer and one atomic edit batch; the host owns its captured byte baseline.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsEditPayload {
    manifest: Value,
    scope: lithe_plugin_model::settings::SettingScope,
    document: Option<String>,
    edits: Vec<lithe_plugin_model::settings::SettingsEdit>,
}

/// `plugins.declarative.editSettings`: validates all edits and returns complete
/// text, deletion eligibility and required effects, without performing any I/O.
pub fn edit_settings_command(payload: Value) -> Result<Value, CoreError> {
    let payload: SettingsEditPayload = decode_payload(payload, "plugins.declarative.editSettings")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            lithe_plugin_model::settings::edit_document(
                &manifest.id,
                &manifest.settings,
                payload.scope,
                payload.document.as_deref(),
                payload.edits,
            )
            .map(|edit| ("edit", edit))
        },
    )))
}

/// `plugins.declarative.workspaceConfiguration`: a language server's
/// `workspaceConfiguration` expanded with the current values, which the host
/// sends to the running server after an `immediate` setting changed.
/// `configuration` is `null` when the server declares none.
pub fn workspace_configuration_command(payload: Value) -> Result<Value, CoreError> {
    const COMMAND: &str = "plugins.declarative.workspaceConfiguration";
    let payload: SettingsPayload = decode_payload(payload, COMMAND)?;
    let Some(language_id) = payload.language_id.clone() else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("Invalid {COMMAND} payload"),
        )
        .with_details("languageID is required."));
    };
    Ok(outcome(
        validate_manifest_value(payload.manifest.clone()).and_then(|manifest| {
            let server = manifest
                .language_servers
                .iter()
                .find(|server| server.language_id == language_id)
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.launch.unavailable",
                        format!("The plugin does not provide a language server for {language_id}."),
                    )
                })?;
            let configuration = server
                .workspace_configuration
                .as_ref()
                .map(|value| {
                    expand_configuration(value, &manifest.effective_settings(payload.layers()))
                })
                .unwrap_or(Value::Null);
            Ok(("configuration", configuration))
        }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrammarPayload {
    manifest: Value,
    #[serde(rename = "languageID")]
    language_id: String,
    /// Text of the grammar file the host read from the package.
    grammar: String,
}

/// `plugins.declarative.decodeGrammar`: validates the grammar file of one
/// language and returns what the editor registers for it.
pub fn decode_grammar_command(payload: Value) -> Result<Value, CoreError> {
    let payload: GrammarPayload = decode_payload(payload, "plugins.declarative.decodeGrammar")?;
    Ok(outcome(validate_manifest_value(payload.manifest).and_then(
        |manifest| {
            let language = manifest
                .languages
                .iter()
                .find(|language| language.id == payload.language_id)
                .filter(|language| language.grammar.is_some() || language.configuration.is_some())
                .ok_or_else(|| {
                    PluginRejection::new(
                        "plugin.grammar.unavailable",
                        format!(
                            "The plugin declares no grammar for {}.",
                            payload.language_id
                        ),
                    )
                })?;
            let grammar = language
                .grammar
                .as_ref()
                .map(|grammar| match grammar.kind {
                    // The editor's engine parses the original text; Core only
                    // proves that it can and names the scope it registers.
                    lithe_plugin_grammar::GrammarKind::Textmate => {
                        lithe_plugin_grammar::decode_textmate(&payload.grammar).map(|scope_name| {
                            json!({
                                "kind": grammar.kind,
                                "scopeName": scope_name,
                                "source": payload.grammar,
                            })
                        })
                    }
                })
                .transpose()?;
            Ok((
                "language",
                json!({
                    "id": language.id,
                    "fileExtensions": language.file_extensions,
                    "fileNames": language.file_names,
                    "grammar": grammar,
                    "configuration": language.configuration,
                }),
            ))
        },
    )))
}
