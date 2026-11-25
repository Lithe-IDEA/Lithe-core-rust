//! Splits indexed text and search queries into the same terms.
//!
//! SQLite's tokenizers split on spaces, which Chinese, Japanese and Korean
//! text rarely has. Text is therefore split here, before it reaches the index:
//!
//! - a run of letters and digits of other scripts is one lowercase word;
//! - a run of characters of the scripts above becomes every pair of adjacent
//!   characters, plus its last character alone. Every character then starts
//!   a term, so a one-character query, which matches as a prefix, finds the
//!   run wherever the character sits.
//!
//! The terms contain only letters and digits and are joined by spaces, and
//! the index uses the `ascii` tokenizer, which splits on ASCII punctuation and
//! spaces only. A term therefore stays one index token, and a quoted query
//! term can never become a phrase, which an index without positions rejects.

/// Longest term, in bytes; longer words are cut at a character boundary, the
/// same way in the index and in queries, so a long identifier still matches
/// by its start, and an encoded blob does not bloat the index.
const MAX_TERM_BYTES: usize = 64;

/// Most terms of one query; the rest of a longer query is ignored.
pub(crate) const MAX_QUERY_TERMS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermKind {
    /// Letters and digits of a spaced script; queried as a prefix.
    Word,
    /// Two adjacent characters of an unspaced run; queried exactly.
    Pair,
    /// A run of one unspaced character; queried as a prefix of pairs too.
    Single,
    /// The last character of a longer unspaced run. Only indexed: a query run
    /// of two or more characters is covered by its pairs.
    Tail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Term {
    text: String,
    kind: TermKind,
}

/// The index terms of `text`, joined by spaces.
pub(crate) fn index_terms(text: &str) -> String {
    let mut joined = String::new();
    for term in terms(text) {
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(&term.text);
    }
    joined
}

/// An FTS5 query that matches text containing every term of `query`, or
/// `None` when the query has no terms.
pub(crate) fn match_expression(query: &str) -> Option<String> {
    let mut expression = String::new();
    let mut seen: Vec<String> = Vec::new();
    for term in terms(query) {
        if term.kind == TermKind::Tail || seen.contains(&term.text) {
            continue;
        }
        if seen.len() == MAX_QUERY_TERMS {
            break;
        }
        if !expression.is_empty() {
            expression.push(' ');
        }
        expression.push('"');
        expression.push_str(&term.text);
        expression.push('"');
        if term.kind != TermKind::Pair {
            expression.push('*');
        }
        seen.push(term.text);
    }
    (!expression.is_empty()).then_some(expression)
}

fn terms(text: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut word = String::new();
    let mut run: Vec<char> = Vec::new();
    for character in text.chars() {
        if is_unspaced(character) {
            flush_word(&mut word, &mut terms);
            run.push(character);
        } else if character.is_alphanumeric() {
            flush_run(&mut run, &mut terms);
            for lower in character.to_lowercase() {
                if word.len() + lower.len_utf8() <= MAX_TERM_BYTES {
                    word.push(lower);
                }
            }
        } else {
            flush_word(&mut word, &mut terms);
            flush_run(&mut run, &mut terms);
        }
    }
    flush_word(&mut word, &mut terms);
    flush_run(&mut run, &mut terms);
    terms
}

fn flush_word(word: &mut String, terms: &mut Vec<Term>) {
    if !word.is_empty() {
        terms.push(Term {
            text: std::mem::take(word),
            kind: TermKind::Word,
        });
    }
}

fn flush_run(run: &mut Vec<char>, terms: &mut Vec<Term>) {
    for pair in run.windows(2) {
        terms.push(Term {
            text: pair.iter().collect(),
            kind: TermKind::Pair,
        });
    }
    if let Some(last) = run.last() {
        terms.push(Term {
            text: last.to_string(),
            kind: if run.len() == 1 {
                TermKind::Single
            } else {
                TermKind::Tail
            },
        });
    }
    run.clear();
}

/// Characters of scripts written without spaces between words: Han
/// ideographs, kana and Hangul syllables.
fn is_unspaced(character: char) -> bool {
    matches!(
        character as u32,
        // Hiragana and Katakana
        0x3040..=0x30FF
            // CJK Unified Ideographs Extension A
            | 0x3400..=0x4DBF
            // CJK Unified Ideographs
            | 0x4E00..=0x9FFF
            // Hangul Syllables
            | 0xAC00..=0xD7AF
            // CJK Compatibility Ideographs
            | 0xF900..=0xFAFF
            // CJK Unified Ideographs Extensions B to H
            | 0x20000..=0x323AF
    )
}
