//! Translated text of build options: `build.localizeOptions`.
//!
//! A build plugin's options are named in its manifest, in the plugin's
//! language. The host shows them in the option dialogs, the run
//! configuration editor, the toolbar toggles and the tree's option nodes,
//! all read from the manifest of the detected instance. So the translation
//! is applied to the manifest itself, before detection: the host sends the
//! enabled build plugins' manifests with the interface language, Core names
//! the translation files each may read (`l10n/<language>.json`, as for
//! settings and run configuration types), the host reads them from the
//! installed package and asks again, and Core returns every manifest with
//! the option text of its `buildOptions` section replaced.
//!
//! A translation replaces only text the manifest has: an option's
//! `displayName`, its `description` and the titles of its declared choices.
//! It never renames, adds or reorders anything, and every text keeps the
//! manifest's limits, so a translated manifest validates and plans exactly
//! as the original. A file that does not fit is refused whole and reported;
//! the manifest keeps its wording.

use super::manifest::{check_description, check_text, BuildSystemDeclaration, OptionDeclaration};
use super::BuildManifest;
use lithe_plugin_model::settings::{parse_localization_file, present, LocalizationRequest};
use lithe_plugin_model::{PluginRejection, Validation};
use lithe_protocol::json_document::UniqueMap;
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Error code of a translation file Core did not apply.
const INVALID_LOCALIZATION: &str = "plugin.build.invalidLocalization";
/// Most plugins one request may name.
const MAX_PLUGINS: usize = 512;

/// The `build.localizeOptions` request: the enabled build plugins and the
/// interface language.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalizeOptionsRequest {
    /// Interface language; absent returns the manifests unchanged.
    #[serde(default)]
    language: Option<String>,
    plugins: Vec<OptionPlugin>,
}

/// One enabled plugin.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OptionPlugin {
    manifest: Value,
    /// The translation files the native adapter read, by package-relative
    /// path; absent until it has read the candidates a report names.
    #[serde(default)]
    localizations: Option<BTreeMap<String, String>>,
}

/// `build.localizeOptions`: `{manifests, localizations}`. `manifests` has
/// one entry per plugin, in request order: the manifest with its option text
/// translated, or as sent when there is nothing to translate or it does not
/// validate (detection skips such a manifest anyway). `localizations` has
/// one report per plugin that declares a build option, when a language is
/// named: `{pluginID, language, candidates, maximumBytes, readRequired,
/// applied, rejected}`, the report of `plugins.settings`.
pub fn build_localize_options_command<M: BuildManifest>(
    payload: Value,
) -> Result<Value, CoreError> {
    let request: LocalizeOptionsRequest =
        lithe_plugin_model::decode_payload(payload, "build.localizeOptions")?;
    if request.plugins.len() > MAX_PLUGINS {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many plugins in the build option localization request",
        ));
    }
    let mut manifests = Vec::with_capacity(request.plugins.len());
    let mut reports = Vec::new();
    for plugin in request.plugins {
        let localization =
            LocalizationRequest::new(request.language.as_deref(), plugin.localizations.as_ref())
                .map_err(|message| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid build option localization request",
                    )
                    .with_details(message)
                })?;
        let Some(localization) = localization else {
            manifests.push(plugin.manifest);
            continue;
        };
        let Ok(validated) = M::validate(plugin.manifest.clone()) else {
            manifests.push(plugin.manifest);
            continue;
        };
        let systems: Vec<&BuildSystemDeclaration> = validated
            .build_systems()
            .iter()
            .filter(|system| !system.options.is_empty())
            .collect();
        if systems.is_empty() {
            manifests.push(plugin.manifest);
            continue;
        }
        let (chosen, mut report) = localization.select_with(|text| decode_file(text, &systems));
        report["pluginID"] = json!(validated.id());
        reports.push(report);
        let mut manifest = plugin.manifest;
        if let Some(texts) = chosen {
            apply(&mut manifest, &texts);
        }
        manifests.push(manifest);
    }
    Ok(json!({ "manifests": manifests, "localizations": reports }))
}

/// The part of a translation file this module reads:
/// `buildOptions.<buildSystemID>.<optionID>`. Other top-level keys belong to
/// other surfaces.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationFile {
    #[serde(default)]
    build_options: UniqueMap<UniqueMap<OptionText>>,
}

/// Translated text of one option, by the manifest field it replaces.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OptionText {
    #[serde(default, deserialize_with = "present")]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<String>,
    /// Choice titles by declared choice value.
    #[serde(default, deserialize_with = "present")]
    choices: Option<UniqueMap<String>>,
}

/// Checked option text of one plugin, by build system and option ID.
type OptionTexts = BTreeMap<(String, String), OptionText>;

/// Parses one translation file and checks it against the declarations: it
/// names only declared build systems, options and choices, translates only
/// text the manifest has, and every text fits the manifest's limits.
fn decode_file(text: &str, systems: &[&BuildSystemDeclaration]) -> Validation<OptionTexts> {
    let file: LocalizationFile = parse_localization_file(text, INVALID_LOCALIZATION)?;
    let mut texts = OptionTexts::new();
    for (system_id, options) in file.build_options.0 {
        let field = format!("buildOptions.{system_id}");
        let system = systems
            .iter()
            .find(|system| system.id == system_id)
            .ok_or_else(|| {
                rejection(
                    &field,
                    "The plugin declares no build system with options under this ID.",
                )
            })?;
        for (option_id, text) in options.0 {
            let field = format!("{field}.{option_id}");
            let declaration = system.option(&option_id).ok_or_else(|| {
                rejection(&field, "The build system declares no option with this ID.")
            })?;
            check_option_text(&text, declaration, &field)?;
            texts.insert((system_id.clone(), option_id), text);
        }
    }
    Ok(texts)
}

fn check_option_text(
    text: &OptionText,
    declaration: &OptionDeclaration,
    field: &str,
) -> Validation<()> {
    if let Some(name) = &text.display_name {
        relabel(check_text(
            name,
            &format!("{field}.displayName"),
            "display name",
        ))?;
    }
    if let Some(description) = &text.description {
        let field = format!("{field}.description");
        if declaration.description.is_none() {
            return Err(rejection(
                field,
                "The manifest has no text here to translate.",
            ));
        }
        relabel(check_description(description, &field))?;
    }
    for (value, title) in text.choices.iter().flat_map(|choices| &choices.0) {
        let field = format!("{field}.choices.{value}");
        if !declaration
            .choices
            .iter()
            .any(|choice| &choice.value == value)
        {
            return Err(rejection(
                &field,
                "The option declares no choice with this value.",
            ));
        }
        relabel(check_text(title, &field, "title"))?;
    }
    Ok(())
}

/// Replaces the option text of `manifest` in place. The manifest validated,
/// so its build systems and options are the objects the checks read.
fn apply(manifest: &mut Value, texts: &OptionTexts) {
    let Some(systems) = manifest
        .get_mut("buildSystems")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for system in systems {
        let Some(system_id) = system.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let Some(options) = system.get_mut("options").and_then(Value::as_array_mut) else {
            continue;
        };
        for option in options {
            let Some(option_id) = option.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(text) = texts.get(&(system_id.clone(), option_id.to_owned())) else {
                continue;
            };
            if let Some(name) = &text.display_name {
                option["displayName"] = json!(name);
            }
            if let Some(description) = &text.description {
                option["description"] = json!(description);
            }
            let Some(titles) = &text.choices else {
                continue;
            };
            for choice in option
                .get_mut("choices")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                let title = choice
                    .get("value")
                    .and_then(Value::as_str)
                    .and_then(|value| titles.0.get(value))
                    .cloned();
                if let Some(title) = title {
                    choice["title"] = json!(title);
                }
            }
        }
    }
}

/// A manifest check's rejection, as a rejection of the translation file.
fn relabel(result: Validation<()>) -> Validation<()> {
    result.map_err(|error| PluginRejection {
        code: INVALID_LOCALIZATION,
        ..error
    })
}

fn rejection(field: impl Into<String>, message: impl Into<String>) -> PluginRejection {
    PluginRejection::at(INVALID_LOCALIZATION, field, message)
}
