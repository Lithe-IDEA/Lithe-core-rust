//! Capability versions of the declarative plugin API.
//!
//! A capability owns one manifest section (or one family of fields) and the
//! runtime protocol that serves it. A manifest lists in `requires` exactly the
//! capabilities it uses, each with the version of that capability's contract
//! it was written against, so a plugin without build systems never depends on
//! the build-system contract and a later version of one capability only
//! affects plugins that opt into it. Every capability currently supports only
//! version 0. Everything outside the capability sections, including launches,
//! tool references, and executable candidates, belongs to the base contract
//! (`pluginAPIVersion`) and is never listed in `requires`.
//!
//! Closure rule: a host declares a capability version when it accepts every
//! manifest form of that version and speaks its protocol. A feature a plugin
//! offers may go unpresented by a host without breaking that declaration, and
//! a host service a plugin needs is discovered at runtime (`getCapabilities`
//! and permissions), not through `requires`.
//!
//! A runtime protocol carries the version of the capability that owns it, so
//! the two can never disagree: `litheBuild` is `buildProviders`,
//! `litheFunctions` is `backend`, `litheVcs` is `vcsProviders`, the toolchain
//! protocol is `toolchainProviders`, the keymap protocol is
//! `keymapImportProviders`, and `litheLogging` is `pluginAPIVersion`. Use
//! `versions` and `SUPPORTED_PLUGIN_API_VERSIONS` for the versions this
//! Core build speaks.
//!
//! `requires` is checked before the sections are decoded, so a Core build
//! that does not know a capability reports which one is missing instead of
//! an unknown field. Hosts state the capability versions they implement in
//! `hostCapabilities`; validating a section is not the same as running it.
//!
//! Note: the decision is recorded in .agents/notes/implemented/architecture/2026-11-11-plugin-contract-v2-baseline.md

use crate::{PluginRejection, Validation};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// `languages[]`: file types and project markers, a TextMate `grammar`, and an
/// editing `configuration`; see `lithe_core::plugins::grammar`.
pub const LANGUAGES: &str = "languages";
/// `languageServers[]`: the launch, `workspaceConfiguration`, and the session
/// fields a plugin backend uses to talk to its own server (`requestMethods`,
/// `commands`, `serializedCommands`, `forwardNotifications`, `state`, and
/// `remote`).
pub const LANGUAGE_SERVERS: &str = "languageServers";
/// `vendor.bundledSignatureRequirement`: origin-scoped host-seal trust for a
/// package whose independently installed versions require a publisher signature.
pub const PACKAGE_TRUST: &str = "packageTrust";
/// `executionProviders[]`: run and test templates.
pub const EXECUTION_PROVIDERS: &str = "executionProviders";
/// `settings[]`: scopes, effects, settings pages, and the multiSelect, text,
/// file, and directory types; see [`crate::settings`].
pub const SETTINGS: &str = "settings";
/// `ui`: tool windows with text, web, or host-rendered native content, and
/// commands.
pub const UI: &str = "ui";
/// Declarative build detection, models, and command plans.
pub const BUILD_SYSTEMS: &str = "buildSystems";
/// `buildSystems[].provider`, whether it owns or shares a language server process.
pub const BUILD_PROVIDERS: &str = "buildProviders";
/// Optional public model snapshots between enabled build providers.
pub const BUILD_MODEL_INPUTS: &str = "buildModelInputs";
/// `vcsProviders[]`: version control systems implemented by a plugin
/// process; see `lithe_vcs`.
pub const VCS_PROVIDERS: &str = "vcsProviders";

/// `backend`: the plugin's own long-lived process; see `lithe_core::plugins::backend`. It
/// may start without a page (`onWorkspaceOpen`, a plugin command) and may be
/// granted the `workspace` host service; it speaks the plugin-function
/// protocol (`litheFunctions`).
pub const BACKEND: &str = "backend";

/// `projectGenerators[]`: entries of the New Project window. A field selects
/// installations of any plugin-defined tool with its `tool` field.
pub const PROJECT_GENERATORS: &str = "projectGenerators";

/// `debugAdapters[]`: debug adapters implemented by a plugin process, with
/// declared adapter `features`, `defaults`, and argument and environment
/// bindings; see `lithe_debug::adapters`.
pub const DEBUG_ADAPTERS: &str = "debugAdapters";

/// Independent providers that translate external shortcut settings.
pub const KEYMAP_IMPORT_PROVIDERS: &str = "keymapImportProviders";
/// `pluginFunctions`: functions exported to, and called from, other plugins;
/// see `lithe_core::plugins::plugin_functions`.
pub const PLUGIN_FUNCTIONS: &str = "pluginFunctions";
/// `toolDefinitions[]`: plugin-owned tool identities, discovery policy, and
/// executable candidates.
pub const TOOL_DEFINITIONS: &str = "toolDefinitions";
/// Backend facts that can influence the executable selected for a defined tool.
pub const TOOLCHAIN_PROVIDERS: &str = "toolchainProviders";
/// `outputLinkMatchers[]`: patterns that turn console output into links; see
/// `lithe_core::plugins::output_links`.
pub const OUTPUT_LINK_MATCHERS: &str = "outputLinkMatchers";

/// Capability versions this Core build validates and plans. A later version
/// of a capability is appended to its list.
pub const SUPPORTED_CAPABILITIES: &[(&str, &[u32])] = &[
    (BACKEND, &[0]),
    (BUILD_MODEL_INPUTS, &[0]),
    (BUILD_PROVIDERS, &[0]),
    (BUILD_SYSTEMS, &[0]),
    (DEBUG_ADAPTERS, &[0]),
    (EXECUTION_PROVIDERS, &[0]),
    (KEYMAP_IMPORT_PROVIDERS, &[0]),
    (LANGUAGE_SERVERS, &[0]),
    (LANGUAGES, &[0]),
    (OUTPUT_LINK_MATCHERS, &[0]),
    (PACKAGE_TRUST, &[0]),
    (PLUGIN_FUNCTIONS, &[0]),
    (PROJECT_GENERATORS, &[0]),
    (SETTINGS, &[0]),
    (TOOL_DEFINITIONS, &[0]),
    (TOOLCHAIN_PROVIDERS, &[0]),
    (UI, &[0]),
    (VCS_PROVIDERS, &[0]),
];

/// Longest capability ID, so a hostile index cannot grow host state.
const MAX_CAPABILITY_ID: usize = 64;

/// Capability ID to the contract version a plugin was written against.
pub type Requires = BTreeMap<String, u32>;

/// Capability ID to the versions a host implements, as the host sends them.
pub type HostCapabilities = BTreeMap<String, Vec<u32>>;

/// Checks a raw manifest's `requires` before its sections are decoded. Every
/// manifest must declare it.
pub fn check_manifest_requires(manifest: &Value) -> Validation<()> {
    let Some(requires) = manifest.get("requires") else {
        return Err(PluginRejection::at(
            "plugin.manifest.invalid",
            "requires",
            "A manifest lists the capabilities it uses in requires.",
        ));
    };
    let requires = parse_requires(requires, "requires", "plugin.manifest.invalid")?;
    match requires
        .iter()
        .find(|(capability, version)| !core_supports(capability, **version))
    {
        Some((capability, version)) => Err(PluginRejection::at(
            "plugin.api.unsupported",
            format!("requires.{capability}"),
            format!("Plugin capability {capability} {version} is not supported by this version of Lithe."),
        )),
        None => Ok(()),
    }
}

/// Reads a `requires` object. IDs are checked for shape only, so an index
/// can list a plugin for a capability this Core build does not know yet.
pub fn parse_requires(value: &Value, field: &str, code: &'static str) -> Validation<Requires> {
    let Value::Object(entries) = value else {
        return Err(PluginRejection::at(
            code,
            field,
            "requires maps capability IDs to versions.",
        ));
    };
    if entries.is_empty() {
        return Err(PluginRejection::at(
            code,
            field,
            "requires lists no capabilities.",
        ));
    }
    let mut requires = Requires::new();
    for (capability, version) in entries {
        let entry = format!("{field}.{capability}");
        if !is_capability_id(capability) {
            return Err(PluginRejection::at(
                code,
                entry,
                "A capability ID is a lowerCamelCase identifier.",
            ));
        }
        let version = version
            .as_u64()
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| {
                PluginRejection::at(
                    code,
                    entry,
                    "A capability version is a non-negative integer.",
                )
            })?;
        requires.insert(capability.clone(), version);
    }
    Ok(requires)
}

/// Checks that `requires` lists exactly the capabilities a decoded manifest
/// uses. A missing entry would let a host that lacks the capability accept
/// the plugin; an extra one would hide it from hosts it could run on.
pub fn check_used(requires: &Requires, used: &BTreeSet<&'static str>) -> Validation<()> {
    if let Some(missing) = used
        .iter()
        .find(|capability| !requires.contains_key(**capability))
    {
        return Err(PluginRejection::at(
            "plugin.manifest.invalid",
            "requires",
            format!("The manifest uses {missing}, which requires does not list."),
        ));
    }
    if let Some(unused) = requires
        .keys()
        .find(|capability| !used.contains(capability.as_str()))
    {
        return Err(PluginRejection::at(
            "plugin.manifest.invalid",
            format!("requires.{unused}"),
            format!("requires lists {unused}, which the manifest does not use."),
        ));
    }
    Ok(())
}

/// Whether a host can run every required capability. A host that sends no
/// capabilities runs no plugin, and a capability this Core build cannot
/// validate is never hosted, whatever the host claims.
pub fn hosts(requires: &Requires, host: Option<&HostCapabilities>) -> bool {
    first_unhosted(requires, host).is_none()
}

/// Rejects a manifest that requires a capability version the host lacks.
pub fn check_hosted(requires: &Requires, host: &HostCapabilities) -> Validation<()> {
    match first_unhosted(requires, Some(host)) {
        Some((capability, version)) => Err(PluginRejection::at(
            "plugin.api.unsupported",
            format!("requires.{capability}"),
            format!("This version of Lithe cannot run plugin capability {capability} {version}."),
        )),
        None => Ok(()),
    }
}

fn first_unhosted<'a>(
    requires: &'a Requires,
    host: Option<&HostCapabilities>,
) -> Option<(&'a str, u32)> {
    requires
        .iter()
        .find(|(capability, version)| {
            !core_supports(capability, **version)
                || !host
                    .and_then(|host| host.get(capability.as_str()))
                    .is_some_and(|versions| versions.contains(version))
        })
        .map(|(capability, version)| (capability.as_str(), *version))
}

/// Versions of `capability` this Core build validates and plans; empty when
/// the capability is unknown.
pub fn versions(capability: &str) -> &'static [u32] {
    SUPPORTED_CAPABILITIES
        .iter()
        .find(|(id, _)| *id == capability)
        .map(|(_, versions)| *versions)
        .unwrap_or(&[])
}

fn core_supports(capability: &str, version: u32) -> bool {
    versions(capability).contains(&version)
}

fn is_capability_id(value: &str) -> bool {
    let mut chars = value.chars();
    value.len() <= MAX_CAPABILITY_ID
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|rest| rest.is_ascii_alphanumeric())
}

/// The versions a Lithe runtime protocol offers in `initialize`. A runtime
/// protocol has no version of its own: it speaks the version of the contract
/// that owns it, the capability version of the manifest section that declares
/// the provider (or the plugin API version for plugin logging). The host
/// therefore offers exactly the versions of that contract it implements, and a
/// reply counts as negotiated only when it names one of them.
pub fn offered_versions(contract_versions: &[u32]) -> Vec<u64> {
    contract_versions
        .iter()
        .map(|version| u64::from(*version))
        .collect()
}
