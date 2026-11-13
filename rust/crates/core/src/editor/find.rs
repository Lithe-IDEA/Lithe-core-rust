//! Bounded, Unicode-aware matching and replacement plans over borrowed editor text.
//!
//! The host owns the immutable document snapshot. Core borrows it for one worker request,
//! keeps no text cache, and returns only byte ranges and requested replacements.

use crate::protocol::{cancellation, CoreError, ErrorCode};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_QUERY_BYTES: usize = 4096;
const MAX_MATCHES: usize = 10_000;
const MAX_REPLACEMENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    query: String,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    whole_word: bool,
    #[serde(default)]
    regex: bool,
    range: Option<Range>,
    replacement: Option<String>,
    /// When absent, a replacement request plans every match atomically.
    replace_start: Option<usize>,
    #[serde(default)]
    caret: usize,
    #[serde(default)]
    backwards: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Range {
    start: usize,
    end: usize,
}

fn invalid(reason: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Editor search request was rejected",
    )
    .with_details(reason)
}

pub(crate) fn command(payload: Value, source: Option<&str>) -> Result<Value, CoreError> {
    let request: Request =
        serde_json::from_value(payload).map_err(|_| invalid("invalid_request"))?;
    let source = source.ok_or_else(|| invalid("missing_document"))?;
    if source.len() > MAX_SOURCE_BYTES {
        return Err(invalid("source_too_large"));
    }
    if request.query.len() > MAX_QUERY_BYTES {
        return Err(invalid("query_too_large"));
    }
    let scoped = request.range.is_some();
    let (start, end) = request
        .range
        .map_or((0, source.len()), |r| (r.start, r.end));
    if start > end
        || end > source.len()
        || !source.is_char_boundary(start)
        || !source.is_char_boundary(end)
    {
        return Err(invalid("invalid_range"));
    }
    if request.query.is_empty() {
        return Ok(json!({"matches":[], "edits":[], "selected":null, "truncated":false}));
    }
    let mut pattern = if request.regex {
        request.query.clone()
    } else {
        regex::escape(&request.query)
    };
    if request.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    let matcher = RegexBuilder::new(&pattern)
        .case_insensitive(!request.case_sensitive)
        .multi_line(true)
        .nest_limit(64)
        .size_limit(1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .build()
        .map_err(|_| invalid("invalid_pattern"))?;
    let text = &source[start..end];
    let mut matches = Vec::new();
    let mut edits = Vec::new();
    let mut truncated = false;
    let mut replacement_bytes = 0;
    let mut removed_bytes = 0;
    for captures in matcher.captures_iter(text) {
        cancellation::check()?;
        if matches.len() == MAX_MATCHES {
            truncated = true;
            break;
        }
        let m = captures
            .get(0)
            .expect("a successful capture has a whole match");
        let at = start + m.start();
        matches.push(json!({"start":at, "end":start + m.end()}));
        if let Some(replacement) = &request.replacement {
            if request.replace_start.is_some_and(|wanted| wanted != at) {
                continue;
            }
            let mut inserted = String::new();
            if request.regex {
                captures.expand(replacement, &mut inserted);
            } else {
                inserted.push_str(replacement);
            }
            replacement_bytes += inserted.len();
            removed_bytes += m.len();
            if replacement_bytes > MAX_REPLACEMENT_BYTES {
                return Err(invalid("replacement_too_large"));
            }
            edits.push(json!({"start":at, "end":start + m.end(), "text":inserted}));
        }
    }
    if truncated && request.replacement.is_some() && request.replace_start.is_none() {
        return Err(invalid("too_many_matches"));
    }
    let selected = if request.backwards {
        matches
            .iter()
            .rposition(|m| m["start"].as_u64().unwrap_or(0) < request.caret as u64)
            .or_else(|| matches.len().checked_sub(1))
    } else {
        matches
            .iter()
            .position(|m| m["start"].as_u64().unwrap_or(0) >= request.caret as u64)
            .or_else(|| (!matches.is_empty()).then_some(0))
    };
    let mut result =
        json!({"matches":matches, "edits":edits, "selected":selected, "truncated":truncated});
    if scoped {
        result["range"] = json!({"start":start,"end":end-removed_bytes+replacement_bytes});
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_case_folding_preserves_original_byte_offsets() {
        let result = command(json!({"query":"k", "caret":2}), Some("😀Kk")).unwrap();
        assert_eq!(
            result["matches"],
            json!([{"start":4,"end":7},{"start":7,"end":8}])
        );
        assert_eq!(result["selected"], 0);
    }

    #[test]
    fn regex_captures_and_zero_width_matches_have_atomic_replacement_plans() {
        let result = command(
            json!({"query":"(?m)^([a-z]+)","regex":true,"replacement":"${1}!"}),
            Some("a\nb"),
        )
        .unwrap();
        assert_eq!(
            result["edits"],
            json!([{"start":0,"end":1,"text":"a!"},{"start":2,"end":3,"text":"b!"}])
        );
        let empty = command(json!({"query":"^|$", "regex":true}), Some("😀")).unwrap();
        assert_eq!(
            empty["matches"],
            json!([{"start":0,"end":0},{"start":4,"end":4}])
        );
    }

    #[test]
    fn word_selection_scope_and_single_replace_do_not_touch_other_matches() {
        let result = command(json!({"query":"cat","wholeWord":true,"range":{"start":4,"end":18},"replacement":"$1","replaceStart":15}), Some("cat scatter x cat! cat")).unwrap();
        assert_eq!(result["edits"], json!([]));
        let result = command(
            json!({"query":"cat","wholeWord":true,"replacement":"$1","replaceStart":14}),
            Some("cat scatter x cat! cat"),
        )
        .unwrap();
        assert_eq!(result["edits"], json!([{"start":14,"end":17,"text":"$1"}]));
    }

    #[test]
    fn invalid_regex_is_reported_without_echoing_document_or_pattern() {
        let err = command(json!({"query":"private[", "regex":true}), Some("secret")).unwrap_err();
        assert_eq!(err.details.as_deref(), Some("invalid_pattern"));
        assert!(!err.message.contains("private"));
        assert!(command(
            json!({"query":"a", "range":{"start":1,"end":4}}),
            Some("😀a")
        )
        .is_err());
    }

    #[test]
    fn replace_all_refuses_truncation_instead_of_applying_a_partial_plan() {
        let text = "a".repeat(MAX_MATCHES + 1);
        assert_eq!(
            command(json!({"query":"a"}), Some(&text)).unwrap()["truncated"],
            true
        );
        assert_eq!(
            command(json!({"query":"a","replacement":"b"}), Some(&text))
                .unwrap_err()
                .details
                .as_deref(),
            Some("too_many_matches")
        );
    }
}
