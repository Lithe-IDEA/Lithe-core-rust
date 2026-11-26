//! Bounded, Unicode-aware matching and replacement plans over borrowed editor text.
//!
//! The host owns the immutable document snapshot. Core borrows it for one worker request,
//! keeps no text cache, and returns only byte ranges and requested replacements.

use lithe_protocol::{cancellation, CoreError, ErrorCode};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::LazyLock;

const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_QUERY_BYTES: usize = 4096;
const MAX_MATCHES: usize = 10_000;
const MAX_REPLACEMENT_BYTES: usize = 8 * 1024 * 1024;
/// Bound rejected candidates that rescan already-searched bytes. Identifier-only
/// queries skip each rejected identifier run once and do not spend this budget.
const MAX_IDENTIFIER_RETRIES: usize = 100_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    query: String,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    whole_word: bool,
    /// For literal searches, treat Unicode ID_Continue, join controls and dollar signs as identifier continuations.
    #[serde(default)]
    whole_identifier: bool,
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

fn identifier_continues(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_alphanumeric() || matches!(character, '_' | '$');
    }
    static NON_ASCII_CONTINUATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A[\p{ID_Continue}\p{Join_Control}]\z")
            .expect("identifier continuation uses supported Unicode properties")
    });
    NON_ASCII_CONTINUATION.is_match(character.encode_utf8(&mut [0; 4]))
}

/// Punctuation at a match edge never joins an adjacent identifier. Only an
/// identifier character at that edge needs a boundary in the original document.
fn identifier_bounded(source: &str, at: usize, through: usize) -> bool {
    let matched = &source[at..through];
    let joins = |edge: Option<char>, beside: Option<char>| {
        edge.is_some_and(identifier_continues) && beside.is_some_and(identifier_continues)
    };
    !joins(matched.chars().next(), source[..at].chars().next_back())
        && !joins(
            matched.chars().next_back(),
            source[through..].chars().next(),
        )
}

/// Skip the identifier run containing the rejected start: no later start in
/// that run can have a left boundary. A punctuation start advances one rune.
fn next_identifier_candidate(text: &str, at: usize) -> usize {
    for (offset, character) in text[at..].char_indices() {
        if !identifier_continues(character) {
            return at
                + if offset == 0 {
                    character.len_utf8()
                } else {
                    offset
                };
        }
    }
    text.len()
}

/// Finds matches in the borrowed document `source` and, when a replacement is
/// requested, plans the edits (`editor.find`).
///
/// Returns `{ matches, edits, selected, truncated }` with byte ranges into
/// `source`. Malformed requests, a missing document, oversized inputs and
/// ranges off character boundaries return `invalidRequest` with the reason in
/// the details.
pub fn command(payload: Value, source: Option<&str>) -> Result<Value, CoreError> {
    let request: Request =
        serde_json::from_value(payload).map_err(|_| invalid("invalid_request"))?;
    if request.whole_identifier && request.regex {
        return Err(invalid("regex_with_whole_identifier"));
    }
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
    if request.whole_word && !request.whole_identifier {
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
    let mut record = |at: usize,
                      through: usize,
                      captures: Option<&regex::Captures<'_>>|
     -> Result<bool, CoreError> {
        if matches.len() == MAX_MATCHES {
            return Ok(false);
        }
        matches.push(json!({"start":at, "end":through}));
        if let Some(replacement) = &request.replacement {
            if request.replace_start.is_some_and(|wanted| wanted != at) {
                return Ok(true);
            }
            let mut inserted = String::new();
            if let Some(captures) = captures.filter(|_| request.regex) {
                captures.expand(replacement, &mut inserted);
            } else {
                inserted.push_str(replacement);
            }
            replacement_bytes += inserted.len();
            removed_bytes += through - at;
            if replacement_bytes > MAX_REPLACEMENT_BYTES {
                return Err(invalid("replacement_too_large"));
            }
            edits.push(json!({"start":at, "end":through, "text":inserted}));
        }
        Ok(true)
    };
    if request.whole_identifier {
        let mut from = 0;
        let mut retries = 0;
        while let Some(m) = matcher.find_at(text, from) {
            cancellation::check()?;
            let (at, through) = (start + m.start(), start + m.end());
            if identifier_bounded(source, at, through) {
                if !record(at, through, None)? {
                    truncated = true;
                    break;
                }
                from = m.end();
                continue;
            }
            from = next_identifier_candidate(text, m.start());
            if from < m.end() {
                retries += 1;
                if retries > MAX_IDENTIFIER_RETRIES {
                    truncated = true;
                    break;
                }
            }
        }
    } else {
        for captures in matcher.captures_iter(text) {
            cancellation::check()?;
            let m = captures
                .get(0)
                .expect("a successful capture has a whole match");
            if !record(start + m.start(), start + m.end(), Some(&captures))? {
                truncated = true;
                break;
            }
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
    fn whole_identifier_retries_overlapping_candidates_and_respects_punctuation_edges() {
        for (query, source, expected) in [
            ("a-a", "xa-a-a", json!([{"start":3,"end":6}])),
            ("obj.obj", "aobj.obj.obj", json!([{"start":5,"end":12}])),
            ("ab", "abab ab", json!([{"start":5,"end":7}])),
            ("(foo", "x(foo", json!([{"start":1,"end":5}])),
            ("foo(", "call foo(bar)", json!([{"start":5,"end":9}])),
            (".x", "a.x", json!([{"start":1,"end":3}])),
        ] {
            let result =
                command(json!({"query":query,"wholeIdentifier":true}), Some(source)).unwrap();
            assert_eq!(result["matches"], expected, "query {query}");
            assert_eq!(result["truncated"], false);
        }
    }

    #[test]
    fn whole_identifier_skips_identifier_runs_without_spending_retry_budget() {
        let source = format!("{} a", "a".repeat(400_000));
        let result = command(json!({"query":"a","wholeIdentifier":true}), Some(&source)).unwrap();
        assert_eq!(result["matches"], json!([{"start":400001,"end":400002}]));
        assert_eq!(result["truncated"], false);
    }

    #[test]
    fn whole_identifier_bounds_repeated_candidate_rescans_and_rejects_partial_bulk_edits() {
        let source = "xa-a ".repeat(100_001);
        let result = command(json!({"query":"a-a","wholeIdentifier":true}), Some(&source)).unwrap();
        assert_eq!(result["matches"], json!([]));
        assert_eq!(result["truncated"], true);
        let error = command(
            json!({"query":"a-a","wholeIdentifier":true,"replacement":"b"}),
            Some(&source),
        )
        .unwrap_err();
        assert_eq!(error.details.as_deref(), Some("too_many_matches"));
    }

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
    fn whole_identifier_treats_dollar_as_continuation() {
        let text = "$name $$name $name$ $nameSuffix name name$ $name";
        for (query, ranges) in [
            ("$name", json!([{"start":0,"end":5},{"start":43,"end":48}])),
            ("name", json!([{"start":32,"end":36}])),
        ] {
            let result = command(
                json!({"query":query,"caseSensitive":true,"wholeIdentifier":true}),
                Some(text),
            )
            .unwrap();
            assert_eq!(result["matches"], ranges);
        }
    }

    #[test]
    fn whole_identifier_uses_unicode_identifier_continuations() {
        let source = "foo ℘foo a·b ℘foo a·b b foo";
        for (query, ranges) in [
            ("foo", json!([{"start":0,"end":3},{"start":30,"end":33}])),
            ("℘foo", json!([{"start":4,"end":10},{"start":16,"end":22}])),
            ("a·b", json!([{"start":11,"end":15},{"start":23,"end":27}])),
            ("b", json!([{"start":28,"end":29}])),
        ] {
            let result = command(
                json!({"query":query,"caseSensitive":true,"wholeIdentifier":true}),
                Some(source),
            )
            .unwrap();
            assert_eq!(result["matches"], ranges, "query {query}");
        }
        let scoped = command(
            json!({"query":"foo","wholeIdentifier":true,"range":{"start":7,"end":10}}),
            Some(source),
        )
        .unwrap();
        assert_eq!(scoped["matches"], json!([]));
    }

    #[test]
    fn whole_identifier_rejects_regex() {
        let error = command(
            json!({"query":"foo(?:-bar)?","regex":true,"wholeIdentifier":true}),
            Some("foo-barSuffix"),
        )
        .unwrap_err();
        assert_eq!(
            error.details.as_deref(),
            Some("regex_with_whole_identifier")
        );
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
