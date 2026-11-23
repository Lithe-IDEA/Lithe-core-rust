//! File-association catalog and conflict-checked plans; native hosts own registration I/O.

use crate::{RuleError, RuleErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Projects the host's built-in syntax metadata into registrable text suffixes.
pub const CATALOG: &str = "application.fileAssociations.catalog";
/// Validates an explicit selection against fresh native observations before publication.
pub const PLAN: &str = "application.fileAssociations.plan";
const MAX_FORMATS: usize = 512;

fn invalid(reason: &str) -> RuleError {
    RuleError::new(RuleErrorCode::InvalidRequest, reason)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Language {
    id: String,
    highlighted: bool,
    extensions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogRequest {
    languages: Vec<Language>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Format {
    extension: String,
    language: String,
}

// Viva package declarations accept one suffix of at most 16 ASCII characters.
fn safe_extension(extension: &str) -> bool {
    extension.len() > 1
        && extension.len() <= 17
        && extension.starts_with('.')
        && extension.as_bytes()[1].is_ascii_alphanumeric()
        && extension[1..]
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_' || c == b'+')
        // Legacy syntax metadata also recognizes compiled/compressed containers.
        // Those are not safe default text-editor associations.
        && !matches!(extension, ".rlib" | ".svgz")
}

fn valid_language(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// Sorts and deduplicates metadata without loading grammars or inspecting user files.
pub fn catalog_json(payload: Value) -> Result<Value, RuleError> {
    let request: CatalogRequest = serde_json::from_value(payload)
        .map_err(|_| invalid("Invalid built-in language catalog"))?;
    if request.languages.len() > 128 {
        return Err(invalid("Too many built-in languages"));
    }
    let mut formats = BTreeMap::new();
    let mut count = 0;
    for language in request.languages {
        if !valid_language(&language.id) {
            return Err(invalid("Invalid built-in language identity"));
        }
        count += language.extensions.len();
        if count > MAX_FORMATS {
            return Err(invalid("Too many built-in file extensions"));
        }
        if !language.highlighted && language.id != "plaintext" {
            continue;
        }
        for extension in language.extensions {
            if safe_extension(&extension) {
                formats
                    .entry(extension)
                    .or_insert_with(|| language.id.clone());
            }
        }
    }
    let formats: Vec<Format> = formats
        .into_iter()
        .map(|(extension, language)| Format {
            extension,
            language,
        })
        .collect();
    Ok(json!({"formats": formats}))
}

/// Display names are excluded: only native identities and owned registration revisions conflict.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Observation {
    extension: String,
    default_id: String,
    registration: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanRequest {
    formats: Vec<Format>,
    selected: Vec<String>,
    baseline: Vec<Observation>,
    current: Vec<Observation>,
}

/// Refuses stale selections and returns a deterministic additive registration plan.
///
/// Hosts supply revisions of their owned native registration, re-read before applying the plan.
/// Only selected suffixes enter the plan; native content-type aliases may share defaults.
/// Windows UserChoice is never part of a write plan.
pub fn plan_json(payload: Value) -> Result<Value, RuleError> {
    let request: PlanRequest =
        serde_json::from_value(payload).map_err(|_| invalid("Invalid file-association plan"))?;
    if request.formats.len() > MAX_FORMATS
        || request.selected.is_empty()
        || request.selected.len() > MAX_FORMATS
        || request.baseline.len() > MAX_FORMATS
        || request.current.len() > MAX_FORMATS
    {
        return Err(invalid("File-association plan exceeds its bounds"));
    }
    let allowed: BTreeSet<&str> = request
        .formats
        .iter()
        .map(|f| f.extension.as_str())
        .collect();
    if allowed.len() != request.formats.len()
        || allowed.iter().any(|e| !safe_extension(e))
        || request.formats.iter().any(|f| !valid_language(&f.language))
    {
        return Err(invalid("Invalid file-association formats"));
    }
    let selected: BTreeSet<&str> = request.selected.iter().map(String::as_str).collect();
    if selected.len() != request.selected.len() || !selected.is_subset(&allowed) {
        return Err(invalid("Selection is not in the built-in text catalog"));
    }
    let observations =
        |items: Vec<Observation>| -> Result<BTreeMap<String, Observation>, RuleError> {
            let mut out = BTreeMap::new();
            for item in items {
                if !allowed.contains(item.extension.as_str())
                    || item.default_id.len() > 32768
                    || item.registration.len() > 128
                {
                    return Err(invalid("Invalid native association observation"));
                }
                if out.insert(item.extension.clone(), item).is_some() {
                    return Err(invalid("Duplicate native association observation"));
                }
            }
            Ok(out)
        };
    let baseline = observations(request.baseline)?;
    let current = observations(request.current)?;
    for extension in &selected {
        if !baseline.contains_key(*extension) || !current.contains_key(*extension) {
            return Err(invalid("Missing native association observation"));
        }
        if baseline.get(*extension) != current.get(*extension) {
            return Ok(json!({"conflict": true, "extensions": []}));
        }
    }
    Ok(json!({"conflict": false, "extensions": selected}))
}
