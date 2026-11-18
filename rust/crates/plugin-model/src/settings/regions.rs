//! Plugin-owned regions of a settings page: search keywords for content the
//! host cannot read, and an optional view the plugin draws there.
//!
//! A region sits on one page: the plugin's own Settings tab when it names no
//! page, or the language, build-system or compiler page it names. The host
//! draws the declared rows and actions as before; a region adds the plugin's
//! view between the page header and those rows, and its keywords make the page
//! findable by words that appear only inside the view (canvas text is never
//! indexed). A region never stores values: controls it binds edit the declared
//! settings through the host's ordinary draft and Apply.
//!
//! Note: the decision is recorded in .agents/notes/proposed/architecture/2026-11-12-plugin-authored-settings-content.md

use super::{check_page, is_setting_identifier, SettingPage};
use crate::rejection::{invalid, Validation};
use crate::view::ViewDeclaration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Most regions one plugin may declare.
pub const MAX_SETTING_REGIONS: usize = 16;
/// Most search keywords one region may declare, and most one translation may add.
pub const MAX_REGION_KEYWORDS: usize = 32;
/// Longest search keyword, in characters.
pub const MAX_KEYWORD_CHARS: usize = 64;

/// A plugin-owned part of one settings page.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingRegion {
    /// Identifier, unique among the plugin's regions; view events name it.
    pub id: String,
    /// Page the region appears on; the plugin's own Settings tab when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<SettingPage>,
    /// Words that find the page in settings search, in addition to the page
    /// title and the declared rows. Translations may add more.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Function that draws the region; keywords alone need no backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewDeclaration>,
}

impl SettingRegion {
    /// Checks the region as the manifest entry at `field`.
    pub fn validate(&self, field: &str) -> Validation<()> {
        if !is_setting_identifier(&self.id) {
            return Err(invalid(
                format!("{field}.id"),
                "A region ID starts with a lowercase letter and contains only letters and digits.",
            ));
        }
        if let Some(page) = &self.page {
            check_page(page, field)?;
        }
        if self.keywords.is_empty() && self.view.is_none() {
            return Err(invalid(
                field,
                "A region declares search keywords, a view, or both.",
            ));
        }
        check_keywords(&self.keywords)
            .map_err(|message| invalid(format!("{field}.keywords"), message))?;
        if let Some(view) = &self.view {
            view.validate(&format!("{field}.view"))?;
        }
        Ok(())
    }

    /// The region as a page projection shows it: manifest keywords followed by
    /// the translated ones that differ ignoring case, so both languages find
    /// the page.
    pub(super) fn shown(&self, translated: Option<&Vec<String>>) -> Value {
        let mut keywords = self.keywords.clone();
        let mut seen: BTreeSet<String> = keywords.iter().map(|k| k.to_lowercase()).collect();
        for keyword in translated.into_iter().flatten() {
            if seen.insert(keyword.to_lowercase()) {
                keywords.push(keyword.clone());
            }
        }
        let mut shown = json!({"id": self.id, "keywords": keywords});
        if let Some(view) = &self.view {
            shown["view"] = json!(view);
        }
        shown
    }
}

/// Checks the manifest's whole region list: bounded, unique IDs and at most
/// one region per page, so a page has one plugin view slot per plugin.
pub fn validate_regions(regions: &[SettingRegion], field: &str) -> Validation<()> {
    if regions.len() > MAX_SETTING_REGIONS {
        return Err(invalid(
            field,
            "A plugin declares at most 16 setting regions.",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut pages = BTreeSet::new();
    for (index, region) in regions.iter().enumerate() {
        let entry = format!("{field}[{index}]");
        region.validate(&entry)?;
        if !ids.insert(region.id.as_str()) {
            return Err(invalid(format!("{entry}.id"), "Region IDs are unique."));
        }
        if !pages.insert(region.page.clone()) {
            return Err(invalid(
                format!("{entry}.page"),
                "A plugin declares at most one region per page.",
            ));
        }
    }
    Ok(())
}

/// Keywords are short, distinct, single-line words a user might type.
pub(super) fn check_keywords(keywords: &[String]) -> Result<(), &'static str> {
    if keywords.len() > MAX_REGION_KEYWORDS {
        return Err("A region declares at most 32 keywords.");
    }
    let mut seen = BTreeSet::new();
    for keyword in keywords {
        if keyword.trim() != keyword
            || keyword.is_empty()
            || keyword.chars().count() > MAX_KEYWORD_CHARS
            || keyword.chars().any(char::is_control)
        {
            return Err(
                "A keyword is 1 to 64 characters of single-line text without surrounding spaces.",
            );
        }
        if !seen.insert(keyword.to_lowercase()) {
            return Err("Keywords are distinct, ignoring case.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions(value: Value) -> Vec<SettingRegion> {
        serde_json::from_value(value).expect("regions")
    }

    #[test]
    fn a_region_needs_keywords_or_a_view() {
        let empty = regions(json!([{"id":"overview"}]));
        assert!(validate_regions(&empty, "settingRegions").is_err());
        let keywords = regions(json!([{"id":"overview","keywords":["jdk","sdk"]}]));
        assert!(validate_regions(&keywords, "settingRegions").is_ok());
        let view = regions(json!([{"id":"overview","view":{"function":"settings.overview"}}]));
        assert!(validate_regions(&view, "settingRegions").is_ok());
    }

    #[test]
    fn one_region_per_page_and_unique_ids() {
        let same_page = regions(json!([
            {"id":"a","keywords":["x"]},
            {"id":"b","keywords":["y"]}
        ]));
        let error = validate_regions(&same_page, "settingRegions").expect_err("same page");
        assert_eq!(error.field.as_deref(), Some("settingRegions[1].page"));
        let pages = regions(json!([
            {"id":"a","keywords":["x"]},
            {"id":"b","page":{"buildSystemID":"maven"},"keywords":["y"]}
        ]));
        assert!(validate_regions(&pages, "settingRegions").is_ok());
        let ids = regions(json!([
            {"id":"a","keywords":["x"]},
            {"id":"a","page":{"buildSystemID":"maven"},"keywords":["y"]}
        ]));
        assert!(validate_regions(&ids, "settingRegions").is_err());
    }

    #[test]
    fn keywords_are_short_distinct_single_line_words() {
        for bad in [
            json!([" padded"]),
            json!([""]),
            json!(["line\nbreak"]),
            json!(["Maven", "maven"]),
            json!(["x".repeat(65)]),
        ] {
            let region = regions(json!([{"id":"a","keywords":bad}]));
            assert!(
                validate_regions(&region, "settingRegions").is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn shown_keywords_append_distinct_translations() {
        let region = &regions(json!([{"id":"a","keywords":["JDK","toolchain"]}]))[0];
        let shown = region.shown(Some(&vec!["工具链".to_owned(), "jdk".to_owned()]));
        assert_eq!(
            shown,
            json!({"id":"a","keywords":["JDK","toolchain","工具链"]})
        );
    }
}
