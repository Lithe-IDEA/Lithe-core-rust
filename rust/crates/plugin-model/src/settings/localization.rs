//! Plugin-supplied translations of the text a settings page shows.
//!
//! A package may carry `l10n/<language>.json`. Core names the files that may
//! serve the interface language, the native adapter reads the ones that exist
//! from the installed package, and Core applies the most specific file that
//! is valid. A translation replaces only text the manifest already has and
//! never renames, adds, or reorders anything, so text a file omits keeps the
//! manifest's wording and a partial translation never hides a setting.
//!
//! Note: the decision is recorded in .agents/notes/implemented/architecture/2026-10-21-plugin-settings-surfaces.md

use super::{
    check_label, SettingAction, SettingDeclaration, MAX_SETTING_TEXT, SETTING_ENUM,
    SETTING_MULTI_SELECT,
};
use crate::names::is_language_tag;
use crate::rejection::PluginRejection;
use lithe_protocol::json_document::{parse_unique_json, UniqueMap};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Package directory that holds translation files.
const LOCALIZATION_DIRECTORY: &str = "l10n";

/// Most UTF-8 bytes one translation file may contain.
pub const MAX_LOCALIZATION_BYTES: usize = 1024 * 1024;

/// Error code of a translation file Core did not apply.
const INVALID_LOCALIZATION: &str = "plugin.settings.invalidLocalization";

/// The parts of a translation file this module reads. Other top-level keys
/// are left for later surfaces, so a file written for a newer host still
/// translates settings on this one.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationFile {
    #[serde(default)]
    settings: UniqueMap<SettingText>,
    #[serde(default)]
    sections: UniqueMap<String>,
    #[serde(default)]
    setting_actions: UniqueMap<ActionText>,
}

/// Translated text of one setting, by the manifest field it replaces.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingText {
    #[serde(default, deserialize_with = "present")]
    title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    placeholder: Option<String>,
    #[serde(default, deserialize_with = "present")]
    unit: Option<String>,
    /// Option titles by option value.
    #[serde(default, deserialize_with = "present")]
    options: Option<UniqueMap<String>>,
}

/// Translated text of one settings-page action.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActionText {
    #[serde(default, deserialize_with = "present")]
    title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<String>,
}

/// A written key always carries a value: `null` is rejected rather than
/// read as "not translated", so a broken export cannot pass silently.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// A checked translation for one plugin's declarations.
#[derive(Debug, Default)]
pub struct SettingsLocalization {
    settings: BTreeMap<String, CheckedSettingText>,
    sections: BTreeMap<String, String>,
    actions: BTreeMap<String, (Option<String>, Option<String>)>,
}

#[derive(Debug, Default)]
struct CheckedSettingText {
    title: Option<String>,
    description: Option<String>,
    placeholder: Option<String>,
    unit: Option<String>,
    options: BTreeMap<String, String>,
}

impl SettingsLocalization {
    /// The declaration with its shown text translated. The section keeps its
    /// manifest heading because rows are grouped by it; [`Self::section`]
    /// translates the heading where it is shown.
    pub fn declaration(&self, declaration: &SettingDeclaration) -> SettingDeclaration {
        let mut shown = declaration.clone();
        let Some(text) = self.settings.get(&declaration.id) else {
            return shown;
        };
        if let Some(title) = &text.title {
            shown.title.clone_from(title);
        }
        replace(&mut shown.description, &text.description);
        replace(&mut shown.placeholder, &text.placeholder);
        replace(&mut shown.unit, &text.unit);
        for option in &mut shown.options {
            if let Some(title) = text.options.get(&option.value) {
                option.title.clone_from(title);
            }
        }
        shown
    }

    /// The action with its shown text translated.
    pub fn action(&self, action: &SettingAction) -> SettingAction {
        let mut shown = action.clone();
        if let Some((title, description)) = self.actions.get(&action.id) {
            if let Some(title) = title {
                shown.title.clone_from(title);
            }
            replace(&mut shown.description, description);
        }
        shown
    }

    /// The heading shown for the manifest section heading `title`.
    pub fn section(&self, title: &str) -> String {
        self.sections
            .get(title)
            .cloned()
            .unwrap_or_else(|| title.to_owned())
    }
}

/// The heading shown for a section grouped under the manifest heading `title`.
pub(super) fn shown_section(localization: Option<&SettingsLocalization>, title: String) -> String {
    match localization {
        Some(localization) => localization.section(&title),
        None => title,
    }
}

fn replace(field: &mut Option<String>, translated: &Option<String>) {
    if field.is_some() {
        if let Some(text) = translated {
            *field = Some(text.clone());
        }
    }
}

/// The translation file paths for `language`, most specific first: the tag,
/// then each shorter tag left after removing its last subtag
/// (`zh-Hans-CN`, `zh-Hans`, `zh`).
pub fn localization_candidates(language: &str) -> Vec<String> {
    let mut tag = language;
    let mut candidates = vec![format!("{LOCALIZATION_DIRECTORY}/{tag}.json")];
    while let Some((shorter, _)) = tag.rsplit_once('-') {
        tag = shorter;
        candidates.push(format!("{LOCALIZATION_DIRECTORY}/{tag}.json"));
    }
    candidates
}

/// A host's checked request for the translation of one interface language.
#[derive(Debug)]
pub struct LocalizationRequest<'a> {
    language: &'a str,
    candidates: Vec<String>,
    /// The candidates the native adapter found, by path; `None` until it has
    /// read them.
    files: Option<&'a BTreeMap<String, String>>,
}

impl<'a> LocalizationRequest<'a> {
    /// Checks what the host sent: `Ok(None)` when it names no language, and
    /// `Err` with the reason when the request is malformed, which is a host
    /// fault rather than a plugin rejection.
    pub fn new(
        language: Option<&'a str>,
        files: Option<&'a BTreeMap<String, String>>,
    ) -> Result<Option<Self>, String> {
        let Some(language) = language else {
            return match files {
                Some(_) => Err("localizations require language.".to_owned()),
                None => Ok(None),
            };
        };
        if !is_language_tag(language) {
            return Err("language must be a tag such as zh-Hans or en.".to_owned());
        }
        let candidates = localization_candidates(language);
        if let Some(path) = files
            .into_iter()
            .flat_map(BTreeMap::keys)
            .find(|path| !candidates.contains(path))
        {
            return Err(format!(
                "localizations names {path}, which is not a candidate for {language}."
            ));
        }
        Ok(Some(Self {
            language,
            candidates,
            files,
        }))
    }

    /// Chooses the translation and reports the choice. The most specific valid
    /// file wins; every invalid file is reported with its rejection and never
    /// applied in part. Before the adapter has read the candidates the report
    /// asks it to, and nothing is translated.
    pub fn select(
        &self,
        declarations: &[SettingDeclaration],
        actions: &[SettingAction],
    ) -> (Option<SettingsLocalization>, Value) {
        let mut chosen = None;
        let mut applied = Value::Null;
        let mut rejected = Vec::new();
        for path in &self.candidates {
            let Some(text) = self.files.and_then(|files| files.get(path)) else {
                continue;
            };
            match decode(text, declarations, actions) {
                Ok(localization) if chosen.is_none() => {
                    chosen = Some(localization);
                    applied = json!(path);
                }
                Ok(_) => {}
                Err(error) => rejected.push(json!({"path": path, "error": error})),
            }
        }
        let report = json!({
            "language": self.language,
            "candidates": self.candidates,
            "maximumBytes": MAX_LOCALIZATION_BYTES,
            "readRequired": self.files.is_none(),
            "applied": applied,
            "rejected": rejected,
        });
        (chosen, report)
    }
}

fn rejection(field: impl Into<String>, message: impl Into<String>) -> PluginRejection {
    PluginRejection::at(INVALID_LOCALIZATION, field, message)
}

/// A rejection of the file as a whole.
fn file_rejection(message: impl Into<String>) -> PluginRejection {
    PluginRejection::new(INVALID_LOCALIZATION, message)
}

/// Parses and checks one translation file against the declarations it
/// translates.
fn decode(
    text: &str,
    declarations: &[SettingDeclaration],
    actions: &[SettingAction],
) -> Result<SettingsLocalization, PluginRejection> {
    if text.len() > MAX_LOCALIZATION_BYTES {
        return Err(file_rejection("The translation file is larger than 1 MiB."));
    }
    let checked = parse_unique_json(text)
        .map_err(|_| file_rejection("The translation file is not JSON with unique keys."))?;
    if !checked.is_object() {
        return Err(file_rejection("The translation file is not a JSON object."));
    }
    let file: LocalizationFile = serde_json::from_value(checked).map_err(|error| {
        file_rejection(format!(
            "The translation file does not match the l10n format: {error}."
        ))
    })?;
    let mut localization = SettingsLocalization::default();
    for (id, text) in file.settings.0 {
        let field = format!("settings.{id}");
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.id == id)
            .ok_or_else(|| rejection(&field, "The plugin declares no setting with this ID."))?;
        localization
            .settings
            .insert(id, check_setting(text, declaration, &field)?);
    }
    let headings: BTreeSet<&str> = declarations
        .iter()
        .filter_map(|declaration| declaration.section.as_deref())
        .chain(
            actions
                .iter()
                .filter_map(|action| action.section.as_deref()),
        )
        .collect();
    for (heading, text) in file.sections.0 {
        let field = format!("sections.{heading}");
        if !headings.contains(heading.as_str()) {
            return Err(rejection(
                &field,
                "No setting or action uses this section heading.",
            ));
        }
        label(&text, &field)?;
        localization.sections.insert(heading, text);
    }
    for (id, text) in file.setting_actions.0 {
        let field = format!("settingActions.{id}");
        let action = actions
            .iter()
            .find(|action| action.id == id)
            .ok_or_else(|| rejection(&field, "The plugin declares no action with this ID."))?;
        if let Some(title) = &text.title {
            label(title, &format!("{field}.title"))?;
        }
        let description = translated(
            text.description,
            action.description.is_some(),
            &format!("{field}.description"),
            description_text,
        )?;
        localization.actions.insert(id, (text.title, description));
    }
    Ok(localization)
}

fn check_setting(
    text: SettingText,
    declaration: &SettingDeclaration,
    field: &str,
) -> Result<CheckedSettingText, PluginRejection> {
    if let Some(title) = &text.title {
        label(title, &format!("{field}.title"))?;
    }
    let mut checked = CheckedSettingText {
        title: text.title,
        description: translated(
            text.description,
            declaration.description.is_some(),
            &format!("{field}.description"),
            description_text,
        )?,
        placeholder: translated(
            text.placeholder,
            declaration.placeholder.is_some(),
            &format!("{field}.placeholder"),
            label,
        )?,
        unit: translated(
            text.unit,
            declaration.unit.is_some(),
            &format!("{field}.unit"),
            label,
        )?,
        options: BTreeMap::new(),
    };
    if let Some(options) = text.options {
        if !matches!(
            declaration.kind.as_str(),
            SETTING_ENUM | SETTING_MULTI_SELECT
        ) {
            return Err(rejection(
                format!("{field}.options"),
                "Only enum and multiSelect settings have options to translate.",
            ));
        }
        for (value, title) in options.0 {
            let option_field = format!("{field}.options.{value}");
            if !declaration.offers(&value) {
                return Err(rejection(
                    &option_field,
                    "The setting declares no option with this value.",
                ));
            }
            label(&title, &option_field)?;
            checked.options.insert(value, title);
        }
    }
    Ok(checked)
}

/// A translation of a field the manifest leaves out would show text the
/// plugin never declared, so it is rejected.
fn translated(
    text: Option<String>,
    declared: bool,
    field: &str,
    check: fn(&str, &str) -> Result<(), PluginRejection>,
) -> Result<Option<String>, PluginRejection> {
    let Some(text) = text else {
        return Ok(None);
    };
    if !declared {
        return Err(rejection(
            field,
            "The manifest has no text here to translate.",
        ));
    }
    check(&text, field)?;
    Ok(Some(text))
}

/// Titles, headings, units, placeholders and option titles follow the
/// manifest's length rule.
fn label(text: &str, field: &str) -> Result<(), PluginRejection> {
    check_label(text, field, "A translation is short, non-empty text.")
        .map_err(|error| rejection(field, error.message))
}

fn description_text(text: &str, field: &str) -> Result<(), PluginRejection> {
    if text.trim().is_empty() || text.chars().count() > MAX_SETTING_TEXT * 4 {
        return Err(rejection(
            field,
            "A translated description is non-empty and at most 480 characters.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declarations() -> Vec<SettingDeclaration> {
        serde_json::from_value(json!([
            {"id":"checks","type":"multiSelect","title":"Checks","default":[],"section":"Server",
             "options":[{"value":"format","title":"Format"},{"value":"credo","title":"Credo"}]},
            {"id":"mixEnv","type":"text","title":"MIX_ENV","description":"Environment.","default":"dev",
             "pattern":"[a-z]+","placeholder":"dev","section":"Server"},
            {"id":"dialyzer","type":"boolean","title":"Dialyzer","default":false}
        ]))
        .expect("declarations")
    }

    fn actions() -> Vec<SettingAction> {
        serde_json::from_value(json!([
            {"id":"rebuildPlt","title":"Rebuild PLT","function":"dev.lithe.fixture/rebuild","confirm":true}
        ]))
        .expect("actions")
    }

    fn select_text(
        language: Option<&str>,
        files: Option<&BTreeMap<String, String>>,
    ) -> (Option<SettingsLocalization>, Value) {
        LocalizationRequest::new(language, files)
            .expect("well-formed request")
            .map_or((None, Value::Null), |request| {
                request.select(&declarations(), &actions())
            })
    }

    fn select(language: &str, files: &[(&str, Value)]) -> (Option<SettingsLocalization>, Value) {
        let files = files
            .iter()
            .map(|(path, file)| (path.to_string(), file.to_string()))
            .collect::<BTreeMap<_, _>>();
        select_text(Some(language), Some(&files))
    }

    #[test]
    fn candidates_drop_one_subtag_at_a_time() {
        assert_eq!(
            localization_candidates("zh-Hans-CN"),
            ["l10n/zh-Hans-CN.json", "l10n/zh-Hans.json", "l10n/zh.json"]
        );
        assert_eq!(localization_candidates("en"), ["l10n/en.json"]);
    }

    #[test]
    fn translates_only_declared_text_and_keeps_the_rest() {
        let (localization, report) = select(
            "zh-Hans",
            &[(
                "l10n/zh-Hans.json",
                json!({
                    "settings": {
                        "checks": {"title": "检查", "options": {"credo": "Credo 检查"}},
                        "mixEnv": {"description": "环境。", "placeholder": "开发"}
                    },
                    "sections": {"Server": "服务器"},
                    "settingActions": {"rebuildPlt": {"title": "重建 PLT"}},
                    "commands": {"later": "ignored"}
                }),
            )],
        );
        let localization = localization.expect("applied");
        assert_eq!(report["applied"], "l10n/zh-Hans.json");
        let declarations = declarations();
        let checks = serde_json::to_value(localization.declaration(&declarations[0])).unwrap();
        assert_eq!(checks["title"], "检查");
        assert_eq!(checks["options"][0]["title"], "Format");
        assert_eq!(checks["options"][1]["title"], "Credo 检查");
        assert_eq!(checks["section"], "Server");
        let mix_env = serde_json::to_value(localization.declaration(&declarations[1])).unwrap();
        assert_eq!(mix_env["title"], "MIX_ENV");
        assert_eq!(mix_env["placeholder"], "开发");
        assert_eq!(localization.section("Server"), "服务器");
        assert_eq!(localization.section("Other"), "Other");
        let action = serde_json::to_value(localization.action(&actions()[0])).unwrap();
        assert_eq!(action["title"], "重建 PLT");
    }

    #[test]
    fn the_most_specific_valid_file_wins_and_invalid_files_are_reported() {
        let (localization, report) = select(
            "zh-Hans-CN",
            &[
                (
                    "l10n/zh-Hans-CN.json",
                    json!({"settings": {"missing": {"title": "无"}}}),
                ),
                (
                    "l10n/zh-Hans.json",
                    json!({"settings": {"dialyzer": {"title": "简体"}}}),
                ),
                (
                    "l10n/zh.json",
                    json!({"settings": {"dialyzer": {"title": "中文"}}}),
                ),
            ],
        );
        let shown = localization
            .expect("fallback")
            .declaration(&declarations()[2]);
        assert_eq!(serde_json::to_value(shown).unwrap()["title"], "简体");
        assert_eq!(report["applied"], "l10n/zh-Hans.json");
        assert_eq!(report["rejected"][0]["path"], "l10n/zh-Hans-CN.json");
        assert_eq!(report["rejected"][0]["error"]["code"], INVALID_LOCALIZATION);
        assert_eq!(report["rejected"][0]["error"]["field"], "settings.missing");
    }

    #[test]
    fn rejects_files_that_do_more_than_translate() {
        for (file, field) in [
            (
                json!({"settings": {"dialyzer": {"description": "无原文"}}}),
                "settings.dialyzer.description",
            ),
            (
                json!({"settings": {"dialyzer": {"options": {}}}}),
                "settings.dialyzer.options",
            ),
            (
                json!({"settings": {"checks": {"options": {"lint": "检查"}}}}),
                "settings.checks.options.lint",
            ),
            (
                json!({"settings": {"checks": {"title": " "}}}),
                "settings.checks.title",
            ),
            (
                json!({"settings": {"checks": {"title": "x".repeat(121)}}}),
                "settings.checks.title",
            ),
            (json!({"sections": {"Client": "客户端"}}), "sections.Client"),
            (
                json!({"settingActions": {"rebuildPlt": {"description": "无原文"}}}),
                "settingActions.rebuildPlt.description",
            ),
            (
                json!({"settingActions": {"clean": {"title": "清理"}}}),
                "settingActions.clean",
            ),
            (json!({"settings": {"checks": {"title": null}}}), ""),
            (json!({"settings": {"checks": {"label": "检查"}}}), ""),
            (json!(["not", "an", "object"]), ""),
        ] {
            let (localization, report) = select("zh", &[("l10n/zh.json", file.clone())]);
            assert!(localization.is_none(), "{file}");
            assert_eq!(report["applied"], Value::Null);
            assert_eq!(
                report["rejected"][0]["error"]["field"]
                    .as_str()
                    .unwrap_or(""),
                field,
                "{file}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_keys_and_oversized_files() {
        let duplicate = r#"{"sections":{"Server":"一","Server":"二"}}"#.to_owned();
        let oversized = format!(r#"{{"padding":"{}"}}"#, "x".repeat(MAX_LOCALIZATION_BYTES));
        for text in [duplicate, oversized] {
            let files = BTreeMap::from([("l10n/zh.json".to_owned(), text)]);
            let (localization, report) = select_text(Some("zh"), Some(&files));
            assert!(localization.is_none());
            assert_eq!(report["rejected"][0]["error"]["code"], INVALID_LOCALIZATION);
        }
    }

    #[test]
    fn reports_what_to_read_and_rejects_malformed_requests() {
        let (localization, report) = select_text(Some("zh-Hans"), None);
        assert!(localization.is_none());
        assert_eq!(report["readRequired"], true);
        assert_eq!(
            report["candidates"],
            json!(["l10n/zh-Hans.json", "l10n/zh.json"])
        );
        let (_, read) = select("zh-Hans", &[]);
        assert_eq!(read["readRequired"], false);
        assert_eq!(read["applied"], Value::Null);
        assert_eq!(select_text(None, None).1, Value::Null);
        let files = BTreeMap::from([("l10n/en.json".to_owned(), "{}".to_owned())]);
        for (language, files) in [
            (Some("zh_Hans"), None),
            (Some("zh-Hans"), Some(&files)),
            (None, Some(&files)),
        ] {
            assert!(LocalizationRequest::new(language, files).is_err());
        }
    }
}
