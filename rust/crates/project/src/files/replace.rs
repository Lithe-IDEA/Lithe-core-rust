//! Replacing text and previewing replacements.

use super::*;

/// Computes replacement text without touching storage or editor state.
pub fn replace_text(request: ReplacementTextRequest) -> Result<serde_json::Value, CoreError> {
    let query = request.query.trim();
    if query.is_empty() {
        return Ok(serde_json::json!({"text": request.text, "count": 0}));
    }
    let matcher = Matcher::new(
        query,
        request.case_sensitive,
        request.whole_words,
        request.regular_expression,
    )?;
    let mut count = 0usize;
    let mut lines = Vec::new();
    for (index, line) in request.text.split('\n').enumerate() {
        lithe_protocol::cancellation::check()?;
        if let Some(position) = &request.from {
            if count == 0 && index + 1 >= position.line.max(1) {
                let column = if index + 1 == position.line.max(1) {
                    position.column.saturating_sub(1)
                } else {
                    0
                };
                if let Some(replaced) =
                    matcher.replace_first_from(line, &request.replacement, column)
                {
                    lines.push(replaced);
                    count = 1;
                    continue;
                }
            }
            lines.push(line.to_string());
        } else {
            let (replaced, occurrences) =
                matcher.replace_with_options(line, &request.replacement, false);
            count += occurrences;
            lines.push(replaced);
        }
    }
    Ok(serde_json::json!({"text": lines.join("\n"), "count": count}))
}

/// Computes per-file replacements without writing any workspace content.
pub fn replace_preview(
    request: ReplacementPreviewRequest,
) -> Result<ReplacementPreviewResponse, CoreError> {
    let root = existing_root(&request.root)?;
    let indexing_policy = crate::indexing::context(&root);
    let query = request.query.trim().to_string();
    if query.is_empty() {
        return Ok(ReplacementPreviewResponse { files: Vec::new() });
    }
    let rules = VisibilityRules::new(
        request.hidden_directory_names.clone(),
        request.hidden_file_patterns.clone(),
    )
    .with_workspace_roots(&root, &request.workspace_roots);
    let shared_index = search_index::get_or_build(&root, &rules)?;
    let index = shared_index
        .read()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))?;
    let matcher = Matcher::new(
        &query,
        request.case_sensitive,
        request.whole_words,
        request.regular_expression,
    )?;
    let candidate_ids = index.candidate_ids(&query, request.regular_expression);
    let candidate_set = candidate_ids.iter().copied().collect::<HashSet<_>>();
    let paths = if request.paths.is_empty() {
        candidate_ids
            .into_iter()
            .filter_map(|id| index.file(id).map(|file| file.path.clone()))
            .collect::<Vec<_>>()
    } else {
        let mut paths = Vec::new();
        for value in &request.paths {
            let relative = safe_relative_path_string(value)?;
            let include = request.text_overrides.contains_key(&relative)
                || index
                    .id_for_path(&relative)
                    .map(|id| candidate_set.contains(&id))
                    .unwrap_or(true);
            if include {
                paths.push(relative);
            }
        }
        paths
    };
    drop(index);
    let masks = parse_file_mask(&request.file_mask);
    let mut files = Vec::new();
    for path in paths {
        lithe_protocol::cancellation::check()?;
        let relative = safe_relative_path_string(&path)?;
        if !file_mask_allows(&masks, &relative) {
            continue;
        }
        let file = root.join(&relative);
        if rules.is_delegated(&file) || crate::indexing::is_excluded(&root, &file, false)? {
            continue;
        }
        let text = request
            .text_overrides
            .get(&relative)
            .cloned()
            .or_else(|| read_searchable_text(&file));
        let Some(text) = text else { continue };
        let mut matches = Vec::new();
        let mut replaced_lines = Vec::new();
        for (index, line) in text.split('\n').enumerate() {
            lithe_protocol::cancellation::check()?;
            let (after, occurrence_count) =
                matcher.replace_with_options(line, &request.replacement, request.preserve_case);
            replaced_lines.push(after.clone());
            if occurrence_count > 0 {
                matches.push(crate::contracts::ReplacementMatch {
                    line: index + 1,
                    before: line.to_string(),
                    after,
                    occurrence_count,
                });
            }
        }
        if !matches.is_empty() {
            files.push(crate::contracts::ReplacementFile {
                path: relative,
                matches,
                replacement_text: replaced_lines.join("\n"),
            });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if let Some(policy) = &indexing_policy {
        policy.check()?;
    }
    Ok(ReplacementPreviewResponse { files })
}

/// Matches IDEA's Preserve Case behavior for literal replacement text.
///
/// All-uppercase matches uppercase the replacement, initial-uppercase matches
/// capitalize it, and camelCase or mixed-case matches leave it unchanged.
pub(crate) fn apply_case_pattern(matched: &str, replacement: &str) -> String {
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
