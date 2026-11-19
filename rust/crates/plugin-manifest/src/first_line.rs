//! Bounded, data-only first-line language matching for documents with no path association.

use super::{
    declarations::is_language_id, decode_payload, invalid, CoreError, ErrorCode, Validation,
};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

const MAX_PATTERN_BYTES: usize = 512;
const MAX_FIRST_LINE_BYTES: usize = 1024;
const MAX_CANDIDATES: usize = 64;
const REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// An omitted declaration is distinct from an explicitly invalid JSON null.
pub(crate) fn present_pattern<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

/// Use the same compiled program on manifest acceptance and on repeated document opens.
/// The cache contains only declaration text and programs, never document content.
fn compiled(source: &str) -> Result<Arc<Regex>, &'static str> {
    if source.is_empty() || source.len() > MAX_PATTERN_BYTES {
        return Err("A first-line pattern must be 1 to 512 UTF-8 bytes.");
    }
    type Cache = VecDeque<(String, Arc<Regex>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    let mut entries = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some((_, regex)) = entries.iter().find(|(pattern, _)| pattern == source) {
        return Ok(Arc::clone(regex));
    }
    let regex = RegexBuilder::new(source)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|_| "Invalid first-line regular expression or compiled size limit exceeded.")?;
    if regex.is_match("") {
        return Err("A first-line pattern cannot match empty input.");
    }
    let regex = Arc::new(regex);
    if entries.len() == MAX_CANDIDATES {
        entries.pop_front();
    }
    entries.push_back((source.to_owned(), Arc::clone(&regex)));
    Ok(regex)
}

pub(crate) fn validate_pattern(source: &str, field: &str) -> Validation<()> {
    compiled(source)
        .map(|_| ())
        .map_err(|reason| invalid(field, reason))
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
        let regex = compiled(&candidate.first_line_match).map_err(invalid_request)?;
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
