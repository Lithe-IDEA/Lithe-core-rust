//! Placement projections retain indices into the complete owner-bound editor.

use super::{SettingPage, SettingsLocalization, SettingsSurface};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Row and action indices placed in one section.
type SectionIndices = (Vec<usize>, Vec<usize>);

/// Groups only explicitly placed rows, actions and regions. Unplaced
/// declarations remain on the plugin's own page; an action or a region alone
/// is sufficient to create a page. Sections are grouped and ordered by their
/// manifest heading; `localization` changes only the heading and adds region
/// keywords.
pub fn page_sections(
    surface: SettingsSurface<'_>,
    localization: Option<&SettingsLocalization>,
) -> Vec<Value> {
    let mut pages: BTreeMap<SettingPage, BTreeMap<String, SectionIndices>> = BTreeMap::new();
    for (index, row) in surface.settings.iter().enumerate() {
        if let Some(page) = &row.page {
            pages
                .entry(page.clone())
                .or_default()
                .entry(row.section.clone().unwrap_or_default())
                .or_default()
                .0
                .push(index);
        }
    }
    for (index, action) in surface.actions.iter().enumerate() {
        if let Some(page) = &action.page {
            pages
                .entry(page.clone())
                .or_default()
                .entry(action.section.clone().unwrap_or_default())
                .or_default()
                .1
                .push(index);
        }
    }
    for region in surface.regions {
        if let Some(page) = &region.page {
            pages.entry(page.clone()).or_default();
        }
    }
    pages.into_iter().map(|(page, sections)| {
        let (kind, id) = match &page {
            SettingPage::Language(id) => ("language", id),
            SettingPage::BuildSystem(id) => ("buildSystem", id),
            SettingPage::Compiler(id) => ("compiler", id),
        };
        let sections: Vec<_> = sections.into_iter().map(|(title, (rows, actions))| {
            json!({"title":super::localization::shown_section(localization, title),"rows":rows,"actions":actions})
        }).collect();
        let mut shown = json!({"key":format!("{kind}:{id}"),"kind":kind,"id":id,"sections":sections});
        if let Some(region) = shown_region(surface, Some(&page), localization) {
            shown["region"] = region;
        }
        shown
    }).collect()
}

/// The region the plugin placed on `page` (`None` is its own Settings tab),
/// with translated keywords appended.
pub(super) fn shown_region(
    surface: SettingsSurface<'_>,
    page: Option<&SettingPage>,
    localization: Option<&SettingsLocalization>,
) -> Option<Value> {
    surface
        .regions
        .iter()
        .find(|region| region.page.as_ref() == page)
        .map(|region| region.shown(localization.and_then(|l| l.region_keywords(&region.id))))
}
