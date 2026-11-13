//! Search path filters and the text matcher.

use super::*;

/// Include/exclude fragments are compiled once and applied before page caps.
pub(crate) struct SearchPathFilters {
    includes: Vec<Regex>,
    excludes: Vec<Regex>,
    prefix: String,
}

impl SearchPathFilters {
    pub(crate) fn new(
        includes: &[String],
        excludes: &[String],
        prefix: &str,
    ) -> Result<Self, CoreError> {
        fn compile(patterns: &[String]) -> Result<Vec<Regex>, CoreError> {
            patterns
                .iter()
                .filter(|pattern| !pattern.trim().is_empty())
                .map(|pattern| {
                    let mut source = String::new();
                    let mut chars = pattern.trim().chars().peekable();
                    while let Some(character) = chars.next() {
                        match character {
                            '*' if chars.peek() == Some(&'*') => {
                                chars.next();
                                source.push_str(".*");
                            }
                            '*' => source.push_str("[^/]*"),
                            '?' => source.push_str("[^/]"),
                            other => source.push_str(&regex::escape(&other.to_string())),
                        }
                    }
                    RegexBuilder::new(&source)
                        .case_insensitive(true)
                        .build()
                        .map_err(|error| {
                            CoreError::new(ErrorCode::InvalidRequest, "Invalid search path filter")
                                .with_details(error.to_string())
                        })
                })
                .collect()
        }
        Ok(Self {
            includes: compile(includes)?,
            excludes: compile(excludes)?,
            prefix: prefix.trim_end_matches('/').to_string(),
        })
    }

    pub(crate) fn allows(&self, path: &str) -> bool {
        if self.includes.is_empty() && self.excludes.is_empty() {
            return true;
        }
        let path = if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{}", self.prefix, path)
        };
        (self.includes.is_empty() || self.includes.iter().any(|filter| filter.is_match(&path)))
            && !self.excludes.iter().any(|filter| filter.is_match(&path))
    }
}

/// Splits a mask list such as `*.md, *.txt`; an empty list disables filtering.
pub(crate) fn parse_file_mask(mask: &str) -> Vec<String> {
    mask.split(',')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

/// Matches masks against the file name only and accepts any matching pattern.
pub(crate) fn file_mask_allows(masks: &[String], path: &str) -> bool {
    if masks.is_empty() {
        return true;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    masks.iter().any(|mask| glob_matches(mask, name))
}

/// Literal or regular-expression search semantics compiled for repeated matches.
pub(crate) struct Matcher {
    plain_query: String,
    regex: Option<Regex>,
    case_sensitive: bool,
    whole_words: bool,
}

impl Matcher {
    /// Projects original byte ranges into the editor's UTF-16 coordinate space.
    pub(crate) fn content_ranges(&self, text: &str) -> Vec<crate::contracts::SearchNameRange> {
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
                crate::contracts::SearchNameRange {
                    start: first,
                    length: units - first,
                }
            })
            .collect()
    }

    pub(crate) fn new(
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

    pub(crate) fn matches(&self, text: &str) -> bool {
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
    pub(crate) fn replace_with_options(
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
    pub(crate) fn replace_first_from(
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

pub(crate) fn read_searchable_text(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_SIZE {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    is_plain_text(&text).then_some(text)
}

/// Whether `text` has no NUL or other control characters besides tab, line feed, vertical tab, form feed and carriage return.
pub fn is_plain_text(text: &str) -> bool {
    text.chars().all(|character| {
        let value = character as u32;
        value != 0 && !(value < 0x09 || (value > 0x0D && value < 0x20) || value == 0x7F)
    })
}

fn is_word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '$'
}
