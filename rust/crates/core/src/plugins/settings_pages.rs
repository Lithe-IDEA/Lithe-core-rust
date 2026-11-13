//! Placement projections retain indices into the complete owner-bound editor.

use super::{SettingAction, SettingDeclaration, SettingPage};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Groups only explicitly placed rows and actions. Unplaced declarations remain
/// on the plugin's own page; an action alone is sufficient to create a page.
pub(crate) fn page_sections(rows: &[SettingDeclaration], actions: &[SettingAction]) -> Vec<Value> {
    let mut pages: BTreeMap<SettingPage, BTreeMap<String, (Vec<usize>, Vec<usize>)>> =
        BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
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
    for (index, action) in actions.iter().enumerate() {
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
    pages.into_iter().map(|(page, sections)| {
        let (kind, id) = match &page {
            SettingPage::Language(id) => ("language", id),
            SettingPage::BuildSystem(id) => ("buildSystem", id),
        };
        json!({"key":format!("{kind}:{id}"),"kind":kind,"id":id,"sections":sections.into_iter().map(|(title,(rows,actions))| json!({"title":title,"rows":rows,"actions":actions})).collect::<Vec<_>>()})
    }).collect()
}
