//! Shared text search and replacement semantics.

use crate::{encoding, Utf16Span};
use lithe_protocol::{CoreError, ErrorCode};
use regex::{Regex, RegexBuilder};

/// Literal or regular-expression search semantics compiled for repeated matches.
pub struct Matcher {
    plain_query: String,
    regex: Option<Regex>,
    case_sensitive: bool,
    whole_words: bool,
}

impl Matcher {
    /// Counts occurrences without constructing replacement text.
    pub fn occurrence_count(&self, text: &str) -> usize {
        if let Some(regex) = &self.regex {
            regex.find_iter(text).count()
        } else {
            let haystack = if self.case_sensitive {
                std::borrow::Cow::Borrowed(text)
            } else {
                std::borrow::Cow::Owned(text.to_lowercase())
            };
            let needle = if self.case_sensitive {
                std::borrow::Cow::Borrowed(self.plain_query.as_str())
            } else {
                std::borrow::Cow::Owned(self.plain_query.to_lowercase())
            };
            if needle.is_empty() {
                return 0;
            }
            haystack
                .match_indices(needle.as_ref())
                .filter(|(start, matched)| {
                    !self.whole_words
                        || (!haystack[..*start]
                            .chars()
                            .next_back()
                            .is_some_and(is_word_character)
                            && !haystack[*start + matched.len()..]
                                .chars()
                                .next()
                                .is_some_and(is_word_character))
                })
                .count()
        }
    }
    /// Projects original byte ranges into the editor's UTF-16 coordinate space.
    pub fn content_ranges(&self, text: &str) -> Vec<Utf16Span> {
        let ranges = if let Some(regex) = &self.regex {
            regex
                .find_iter(text)
                .map(|item| (item.start(), item.end()))
                .collect::<Vec<_>>()
        } else {
            self.literal_match_ranges(text)
        };
        let (mut offset, mut units) = (0, 0);
        ranges
            .into_iter()
            .map(|(start, end)| {
                units += text[offset..start].encode_utf16().count();
                let first = units;
                units += text[start..end].encode_utf16().count();
                offset = end;
                Utf16Span {
                    start: first,
                    length: units - first,
                }
            })
            .collect()
    }

    /// Compiles one query for reuse over paths or lines.
    pub fn new(
        query: &str,
        case_sensitive: bool,
        whole_words: bool,
        regular_expression: bool,
    ) -> Result<Self, CoreError> {
        if regular_expression {
            let pattern = query.to_string();
            let regex = RegexBuilder::new(&pattern)
                .case_insensitive(!case_sensitive)
                .build()
                .map_err(|error| {
                    CoreError::new(ErrorCode::ParseFailed, "Invalid search expression")
                        .with_details(error.to_string())
                })?;
            Ok(Self {
                plain_query: query.to_string(),
                regex: Some(regex),
                case_sensitive,
                whole_words: false,
            })
        } else {
            Ok(Self {
                plain_query: query.to_string(),
                regex: None,
                case_sensitive,
                whole_words,
            })
        }
    }

    /// Reports whether a path or line contains a match.
    pub fn matches(&self, text: &str) -> bool {
        if let Some(regex) = &self.regex {
            return regex.is_match(text);
        }
        let (haystack, needle) = if self.case_sensitive {
            (text.to_string(), self.plain_query.clone())
        } else {
            (text.to_lowercase(), self.plain_query.to_lowercase())
        };
        if !self.whole_words {
            return haystack.contains(&needle);
        }
        haystack.match_indices(&needle).any(|(start, _)| {
            let end = start + needle.len();
            let before = haystack[..start].chars().next_back();
            let after = haystack[end..].chars().next();
            !before.is_some_and(is_word_character) && !after.is_some_and(is_word_character)
        })
    }

    /// Finds literal matches in the original line, preserving UTF-8 boundaries.
    fn literal_match_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        let haystack = if self.case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        };
        let needle = if self.case_sensitive {
            self.plain_query.clone()
        } else {
            self.plain_query.to_lowercase()
        };
        if needle.is_empty() {
            return Vec::new();
        }
        let ranges = haystack
            .match_indices(&needle)
            .filter_map(|(start, matched)| {
                let end = start + matched.len();
                if self.whole_words {
                    let before = haystack[..start].chars().next_back();
                    let after = haystack[end..].chars().next();
                    if before.is_some_and(is_word_character) || after.is_some_and(is_word_character)
                    {
                        return None;
                    }
                }
                Some((start, end))
            })
            .collect::<Vec<_>>();
        // Lowercase expansions can change byte offsets relative to the original.
        if self.case_sensitive {
            ranges
        } else {
            original_ranges_for_lowercase_matches(text, &ranges)
        }
    }

    /// Preserve Case applies only to literal replacements; regex keeps captures.
    pub fn replace_with_options(
        &self,
        text: &str,
        replacement: &str,
        preserve_case: bool,
    ) -> (String, usize) {
        if let Some(regex) = &self.regex {
            let count = regex.find_iter(text).count();
            return (regex.replace_all(text, replacement).into_owned(), count);
        }
        let ranges = self.literal_match_ranges(text);
        let mut result = String::with_capacity(text.len());
        let mut cursor = 0;
        for (start, end) in &ranges {
            result.push_str(&text[cursor..*start]);
            if preserve_case {
                result.push_str(&apply_case_pattern(&text[*start..*end], replacement));
            } else {
                result.push_str(replacement);
            }
            cursor = *end;
        }
        result.push_str(&text[cursor..]);
        (result, ranges.len())
    }

    /// Finds the first replacement touching or following a UTF-16 column.
    pub fn replace_first_from(
        &self,
        text: &str,
        replacement: &str,
        column: usize,
    ) -> Option<String> {
        let mut utf16 = 0usize;
        let mut column_byte = text.len();
        for (index, character) in text.char_indices() {
            if utf16 + character.len_utf16() > column {
                column_byte = index;
                break;
            }
            utf16 += character.len_utf16();
        }
        let follows = |start: usize, end: usize| {
            start >= column_byte || (column_byte >= start && column_byte < end)
        };
        let (start, end, value) = if let Some(regex) = &self.regex {
            let captures = regex.captures_iter(text).find(|capture| {
                capture
                    .get(0)
                    .is_some_and(|matched| follows(matched.start(), matched.end()))
            })?;
            let matched = captures.get(0)?;
            let mut value = String::new();
            captures.expand(replacement, &mut value);
            (matched.start(), matched.end(), value)
        } else {
            let (start, end) = self
                .literal_match_ranges(text)
                .into_iter()
                .find(|(start, end)| follows(*start, *end))?;
            (start, end, replacement.to_string())
        };
        Some(format!("{}{}{}", &text[..start], value, &text[end..]))
    }
}

/// Maps ordered lowercase-match byte ranges to complete original characters.
fn original_ranges_for_lowercase_matches(
    text: &str,
    ranges: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let mut chars = text.char_indices();
    let (mut source_start, mut source_end, mut folded_end) = (0, 0, 0);
    let mut result: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for &(start, end) in ranges {
        while folded_end <= start {
            let Some((index, character)) = chars.next() else {
                break;
            };
            source_start = index;
            source_end = index + character.len_utf8();
            folded_end += character.to_lowercase().map(char::len_utf8).sum::<usize>();
        }
        let original_start = source_start;
        while folded_end < end {
            let Some((index, character)) = chars.next() else {
                break;
            };
            source_end = index + character.len_utf8();
            folded_end += character.to_lowercase().map(char::len_utf8).sum::<usize>();
        }
        if let Some(previous) = result
            .last_mut()
            .filter(|previous| original_start < previous.1)
        {
            previous.1 = previous.1.max(source_end);
        } else {
            result.push((original_start, source_end));
        }
    }
    result
}

/// Decodes one captured source under the same text policy as the editor.
pub fn searchable_bytes_text(bytes: Vec<u8>) -> Option<String> {
    // NUL cannot appear in searchable text. Only detected UTF-16 can
    // contain zero bytes in an otherwise ordinary source document.
    if !encoding::starts_with_utf16_bom(&bytes) && bytes.contains(&0) {
        return None;
    }
    // log-exempt: expectedAbsence — non-text bytes are outside the search corpus.
    let text = encoding::decode_detected_text(bytes).ok()?;
    is_plain_text(&text).then_some(text)
}

/// `line` without the carriage return that ends a CRLF line, so `$`, word
/// boundaries and match ranges are the same on CRLF and LF files.
pub fn line_content(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// Whether `text` has no NUL or other control characters besides tab, line feed, vertical tab, form feed, carriage return and ANSI escape.
pub fn is_plain_text(text: &str) -> bool {
    text.chars().all(|character| {
        let value = character as u32;
        value != 0
            && !(value < 0x09 || (value > 0x0D && value < 0x20 && value != 0x1B) || value == 0x7F)
    })
}

fn is_word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '$'
}

// Preserve casing for uppercase, lowercase, and initial-uppercase matches.
fn apply_case_pattern(matched: &str, replacement: &str) -> String {
    let letters = matched.chars().filter(|value| value.is_alphabetic());
    let mut has_lower = false;
    let mut has_upper = false;
    for letter in letters {
        if letter.is_lowercase() {
            has_lower = true;
        } else if letter.is_uppercase() {
            has_upper = true;
        }
    }
    // With no letters there is no case shape to preserve.
    if !has_lower && !has_upper {
        return replacement.to_string();
    }
    // Require multiple letters so a single `F` is not mistaken for SCREAMING_CASE.
    let letter_count = matched
        .chars()
        .filter(|value| value.is_alphabetic())
        .count();
    if has_upper && !has_lower && letter_count > 1 {
        return replacement.to_uppercase();
    }
    let first_is_upper = matched
        .chars()
        .find(|value| value.is_alphabetic())
        .is_some_and(char::is_uppercase);
    if first_is_upper {
        let mut characters = replacement.chars();
        return match characters.next() {
            Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
            None => String::new(),
        };
    }
    replacement.to_string()
}
