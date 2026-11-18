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
    collect_setting_references, validate_launch_value, LanguageServerLaunch, LaunchVariable,
    RuntimeTools, ServerProcess, ServerRole,
};
use lithe_plugin_model::settings::{
    DeclaredSettings, ResolvedSetting, SettingAction, SettingDeclaration, SettingLayers,
    SettingRegion, SettingsSurface, MAX_SETTINGS, MAX_SETTING_ACTIONS,
};
use lithe_plugin_model::{
    decode_payload, invalid, invalid_template, is_bare_command, is_environment_name, is_identifier,
    is_official_namespace, is_plugin_id, is_safe_relative_path, is_semantic_version, outcome,
    parse_template_with, regex_literal, sorted_unique, validate_minimum_version, PluginRejection,
    TemplatePart, UIIcon, Validation, ViewDeclaration,
};
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

mod command_installers;
mod commands;
mod declarations;
mod launch;
mod managed_runtimes;
mod manifest;
mod permissions;
mod provider_launch;
mod providers;
mod run_plans;
mod settings;
mod settings_surfaces;
mod templates;
mod ui;
mod validation;
mod views;

pub use command_installers::{
    command_installer_default_directory_command, command_installer_plan_command,
    command_installer_requirements_command,
};
pub use commands::*;
pub use launch::*;
pub use managed_runtimes::{managed_runtime_plan_command, managed_runtime_resolution_command};
pub use manifest::*;
use permissions::*;
pub use provider_launch::*;
pub use run_plans::*;
pub use settings::*;
pub use settings_surfaces::{
    settings_catalog_command, settings_delivery_command, settings_navigation_command,
};
use templates::*;
pub use validation::*;
pub use views::apply_result_command;

pub use lithe_plugin_model::capabilities::SUPPORTED_PLUGIN_API_VERSIONS;
