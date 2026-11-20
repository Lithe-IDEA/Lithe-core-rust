//! Bounded, data-only first-line language matching for documents with no path association.

use super::{declarations::is_language_id, decode_payload, CoreError, ErrorCode};
use lithe_plugin_model::{PluginRejection, Validation};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

const MAX_PATTERN_BYTES: usize = 512;
const MAX_FIRST_LINE_BYTES: usize = 1024;
const MAX_CANDIDATES: usize = 64;
const CACHE_CAPACITY: usize = 64;
const REGEX_SIZE_LIMIT: usize = 256 * 1024;
const MAX_METADATA_BYTES: usize = 4096;

type PatternCache = VecDeque<(String, Arc<Regex>)>;

#[derive(Clone, Copy)]
enum CacheAccess {
    Register,
    Match,
}

fn compiled_in(
    cache: &Mutex<PatternCache>,
    source: &str,
    access: CacheAccess,
) -> Result<Arc<Regex>, &'static str> {
    if source.is_empty() || source.len() > MAX_PATTERN_BYTES {
        return Err("A first-line pattern must be 1 to 512 UTF-8 bytes.");
    }
    let entries = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some((_, regex)) = entries.iter().find(|(pattern, _)| pattern == source) {
        return Ok(Arc::clone(regex));
    }
    drop(entries);

    // A cold compilation should not block other workers' cache hits.
    let regex = RegexBuilder::new(source)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|_| "Invalid first-line regular expression or compiled size limit exceeded.")?;
    if regex.is_match("") {
        return Err("A first-line pattern cannot match empty input.");
    }
    let regex = Arc::new(regex);
    let mut entries = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some((_, existing)) = entries.iter().find(|(pattern, _)| pattern == source) {
        return Ok(Arc::clone(existing));
    }
    if entries.len() == CACHE_CAPACITY {
        // Document matching can encounter more than one batch. Filling the
        // cache is useful, but evicting warm patterns during a scan makes the
        // next opening repeat the same compilations.
        if matches!(access, CacheAccess::Match) {
            return Ok(regex);
        }
        entries.pop_front();
    }
    entries.push_back((source.to_owned(), Arc::clone(&regex)));
    Ok(regex)
}

/// Use the same compiled program in grammar decoding and on repeated document opens.
/// The cache contains only declaration text and programs, never document content.
fn compiled(source: &str, access: CacheAccess) -> Result<Arc<Regex>, &'static str> {
    static CACHE: OnceLock<Mutex<PatternCache>> = OnceLock::new();
    compiled_in(CACHE.get_or_init(Mutex::default), source, access)
}

/// Whether a grammar hint uses the syntax and budget of the first-line matcher.
pub(crate) fn supports_pattern(source: &str) -> bool {
    compiled(source, CacheAccess::Register).is_ok()
}

/// Reads only the hint from a standalone TextMate metadata file. An absent or
/// unsupported hint leaves the language's existing highlighting intact.
pub(crate) fn decode_metadata(source: &str) -> Validation<Option<String>> {
    let invalid =
        |message| PluginRejection::at("plugin.grammar.invalid", "firstLineMatch", message);
    if source.len() > MAX_METADATA_BYTES {
        return Err(invalid(
            "The first-line metadata file is larger than 4096 bytes.",
        ));
    }
    let metadata = serde_json::from_str::<serde_json::Map<String, Value>>(source)
        .map_err(|_| invalid("The first-line metadata file must be a JSON object."))?;
    Ok(metadata
        .get("firstLineMatch")
        .and_then(Value::as_str)
        .filter(|pattern| supports_pattern(pattern))
        .map(str::to_owned))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Candidate {
    #[serde(rename = "languageID")]
    language_id: String,
    first_line_match: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    candidates: Vec<Candidate>,
    first_line: String,
}

/// `languages.matchFirstLine` selects the first matching registered ID in
/// host-supplied precedence order. The host sends only a complete decoded first
/// line after path and explicit user associations have failed.
pub fn match_first_line(payload: Value) -> Result<Value, CoreError> {
    let request: Request = decode_payload(payload, "languages.matchFirstLine")?;
    let invalid_request = |message| CoreError::new(ErrorCode::InvalidRequest, message);
    if request.candidates.len() > MAX_CANDIDATES {
        return Err(invalid_request(
            "A first-line request accepts up to 64 candidates.",
        ));
    }
    let line = request.first_line;
    let eligible = !line.is_empty()
        && line.len() <= MAX_FIRST_LINE_BYTES
        && !line.contains(['\0', '\r', '\n']);
    let mut selected = None;
    for candidate in request.candidates {
        if !is_language_id(&candidate.language_id) {
            return Err(invalid_request("Invalid first-line candidate language ID."));
        }
        let regex =
            compiled(&candidate.first_line_match, CacheAccess::Match).map_err(invalid_request)?;
        if eligible
            && selected.is_none()
            && regex
                .find(&line)
                .is_some_and(|hit| hit.start() == 0 && !hit.is_empty())
        {
            selected = Some(candidate.language_id);
        }
    }
    Ok(json!({"languageID": selected}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanning_more_than_one_batch_keeps_cached_patterns() {
        let cache = Mutex::default();
        let initial = compiled_in(&cache, "^sample-0$", CacheAccess::Register).unwrap();
        for i in 1..=MAX_CANDIDATES {
            compiled_in(&cache, &format!("^sample-{i}$"), CacheAccess::Match).unwrap();
        }
        let repeated = compiled_in(&cache, "^sample-0$", CacheAccess::Match).unwrap();
        assert!(Arc::ptr_eq(&initial, &repeated));
        assert_eq!(cache.lock().unwrap().len(), CACHE_CAPACITY);
    }
}
