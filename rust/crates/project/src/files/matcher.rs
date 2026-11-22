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

/// Parses a request-local scope; path ownership still comes from the project policy.
pub(crate) fn parse_search_scope(
    expression: &str,
    paths: &mut Vec<String>,
    includes: &mut Vec<String>,
    excludes: &mut Vec<String>,
) -> Result<(), CoreError> {
    for part in expression
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let (excluded, value) = part
            .strip_prefix('!')
            .map_or((false, part), |value| (true, value));
        if value.is_empty() || value.starts_with('/') || value.split('/').any(|part| part == "..") {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid project search scope",
            ));
        }
        if excluded {
            excludes.push(value.to_string());
        } else if value.contains(['*', '?']) {
            includes.push(value.to_string());
        } else {
            paths.push(safe_relative_path_string(value.trim_end_matches('/'))?);
        }
    }
    Ok(())
}

/// Reads a single file using the user-selected search budget.
pub(crate) fn read_searchable_text_limit(path: &Path, limit: u64) -> Option<String> {
    read_searchable_bytes(path, limit).and_then(searchable_bytes_text)
}

/// Captures the exact bounded bytes used for a replacement preview identity.
pub(crate) fn read_searchable_bytes(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    use std::io::Read;
    let mut bytes = Vec::with_capacity(metadata.len().min(limit) as usize);
    fs::File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > limit {
        return None;
    }
    // Outside UTF-16 a zero byte decodes to U+0000, which plain text excludes,
    // so binary files are rejected before any decoding is attempted.
    if !encoding::starts_with_utf16_bom(&bytes) && bytes.contains(&0) {
        return None;
    }
    Some(bytes)
}
