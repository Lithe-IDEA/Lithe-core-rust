//! Text search, with and without the search index.

use super::*;
use crate::contracts::SearchNameRange;
use lithe_platform::paths::relative_path_contains;
use lithe_platform::Platform;
use lithe_text::{line_preview, Utf16Span};

/// Applies workspace matching to provider snapshots in one bounded request.
pub fn search_text_batch(request: SearchTextBatchRequest) -> Result<serde_json::Value, CoreError> {
    if request.files.len() > 32
        || request
            .files
            .iter()
            .any(|file| file.text.len() > MAX_FILE_SIZE as usize)
        || request
            .files
            .iter()
            .map(|file| file.text.len())
            .sum::<usize>()
            > 16 * 1024 * 1024
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Text search batch is too large",
        ));
    }
    let query = request.query.trim();
    if query.is_empty() {
        return Ok(serde_json::json!({"files": []}));
    }
    let matcher = Matcher::new(
        query,
        request.case_sensitive,
        request.whole_words,
        request.regular_expression,
    )?;
    let context = request.context_lines.min(20);
    let mut files = Vec::new();
    for file in request.files {
        lithe_protocol::cancellation::check()?;
        if file.text.contains('\0') {
            continue;
        }
        let lines: Vec<_> = file.text.split('\n').map(line_content).collect();
        let mut matches = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if !matcher.matches(line) {
                continue;
            }
            let ranges = matcher.content_ranges(line);
            matches.push(
                serde_json::json!({"line": index + 1, "lineText": line, "ranges": ranges,
                "contextBefore": &lines[index.saturating_sub(context)..index],
                "contextAfter": &lines[index + 1..(index + 1 + context).min(lines.len())]}),
            );
        }
        if !matches.is_empty() {
            files.push(serde_json::json!({"path": file.path, "matches": matches}));
        }
    }
    Ok(serde_json::json!({"files": files}))
}

/// Searches visible file paths and current file contents using the shared index.
pub fn search(request: SearchRequest) -> Result<SearchResponse, CoreError> {
    search_with_page(request, None)
}

/// Counts every matching occurrence without retaining previews or delaying a result page.
pub fn search_counts(
    mut request: SearchRequest,
    count_paths: &[String],
) -> Result<serde_json::Value, CoreError> {
    parse_search_scope(
        &request.scope_expression,
        &mut request.scope_paths,
        &mut request.include_path_patterns,
        &mut request.exclude_path_patterns,
    )?;
    let root = existing_root(&request.root)?;
    let rules = VisibilityRules::new(
        request.hidden_directory_names.clone(),
        request.hidden_file_patterns.clone(),
    )
    .with_workspace_roots(&root, &request.workspace_roots)
    .with_search_options(request.max_file_bytes, request.include_ignored)?
    .with_search_visibility(
        request.include_generated,
        request.follow_symlinks,
        &request.hidden_directory_names,
    );
    let index = search_index::get_for_text(&root, &rules)?;
    let index = index
        .read()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))?;
    let matcher = Matcher::new(
        request.query.trim(),
        request.case_sensitive,
        request.whole_words,
        request.regular_expression,
    )?;
    let masks = parse_file_mask(&request.file_mask);
    let filters = SearchPathFilters::new(
        &request.include_path_patterns,
        &request.exclude_path_patterns,
        &request.path_filter_prefix,
    )?;
    let mut file_occurrences = std::collections::BTreeMap::new();
    let aliases = canonical_text_overrides(&root, &request.text_overrides, request.follow_symlinks);
    let (mut files, mut lines, mut occurrences) = (0usize, 0usize, 0usize);
    if count_paths.len() > 200 {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many file count paths",
        ));
    }
    if !request.query.trim().is_empty() {
        for id in index.all_file_ids() {
            lithe_protocol::cancellation::check()?;
            let Some(file) = index.file(id) else { continue };
            let path = &file.path;
            if !scope_allows(&request.scope_paths, path)
                || !file_mask_allows(&masks, path)
                || !filters.allows(path)
            {
                continue;
            }
            let Ok(absolute) = safe_relative_path(&root, path) else {
                continue;
            };
            let text = request
                .text_overrides
                .get(path)
                .map(String::as_str)
                .or_else(|| aliases.get(&absolute).copied())
                .map(str::to_owned)
                .or_else(|| read_searchable_text_limit(&absolute, rules.max_file_bytes));
            let Some(text) = text else { continue };
            let mut count = 0;
            for line in text.split('\n').map(line_content) {
                lithe_protocol::cancellation::check()?;
                let n = matcher.occurrence_count(line);
                lines += usize::from(n > 0);
                count += n;
            }
            if count > 0 {
                files += 1;
                occurrences += count;
                if count_paths.contains(path) {
                    file_occurrences.insert(path.clone(), count);
                }
            }
        }
    }
    if let Some(policy) = crate::indexing::context(&root) {
        policy.check()?;
    }
    Ok(
        serde_json::json!({"files":files, "lines":lines, "occurrences":occurrences, "fileOccurrences":file_occurrences}),
    )
}

/// Continues a content query inside the matcher, before applying its page budget.
pub(crate) fn search_with_page(
    mut request: SearchRequest,
    page: Option<&SearchPage>,
) -> Result<SearchResponse, CoreError> {
    parse_search_scope(
        &request.scope_expression,
        &mut request.scope_paths,
        &mut request.include_path_patterns,
        &mut request.exclude_path_patterns,
    )?;
    let root = existing_root(&request.root)?;
    let indexing_policy = crate::indexing::context(&root);
    let query = request.query.trim().to_string();
    if query.is_empty() {
        return Ok(SearchResponse {
            matches: Vec::new(),
        });
    }

    let rules = VisibilityRules::new(
        request.hidden_directory_names.clone(),
        request.hidden_file_patterns.clone(),
    )
    .with_workspace_roots(&root, &request.workspace_roots)
    .with_search_options(request.max_file_bytes, request.include_ignored)?
    .with_search_visibility(
        request.include_generated,
        request.follow_symlinks,
        &request.hidden_directory_names,
    );
    // Interactive pages must not build trigrams for every file before the first
    // result. Reuse warmed postings, or read candidates from the names inventory.
    let index = if page.is_some() {
        search_index::get_for_text(&root, &rules)?
    } else {
        search_index::get_or_build(&root, &rules)?
    };
    let index = index
        .read()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))?;
    let result = search_with_index(&root, &request, &query, &index, page)?;
    if let Some(policy) = &indexing_policy {
        policy.check()?;
    }
    Ok(result)
}

pub(crate) fn scope_allows(scopes: &[String], path: &str) -> bool {
    scopes.is_empty()
        || scopes
            .iter()
            .any(|scope| scope.is_empty() || relative_path_contains(scope, path, Platform::HOST))
}

pub(crate) fn search_with_index(
    root: &Path,
    request: &SearchRequest,
    query: &str,
    index: &WorkspaceSearchIndex,
    page: Option<&SearchPage>,
) -> Result<SearchResponse, CoreError> {
    let matcher = Matcher::new(
        query,
        request.case_sensitive,
        request.whole_words,
        request.regular_expression,
    )?;
    let masks = parse_file_mask(&request.file_mask);
    let path_filters = SearchPathFilters::new(
        &request.include_path_patterns,
        &request.exclude_path_patterns,
        &request.path_filter_prefix,
    )?;
    let limit = request.max_results.clamp(1, 10_000);
    let file_limit = request.max_file_results.unwrap_or(limit).min(limit);
    let content_limit = request.max_content_results.unwrap_or(limit).min(limit);
    let mut matches = Vec::new();
    let mut file_matches = 0;

    for id in index.all_file_ids() {
        lithe_protocol::cancellation::check()?;
        if matches.len() >= limit || file_matches >= file_limit {
            break;
        }
        let Some(indexed_file) = index.file(id) else {
            continue;
        };
        let path = &indexed_file.path;
        if !scope_allows(&request.scope_paths, path)
            || !file_mask_allows(&masks, path)
            || !path_filters.allows(path)
        {
            continue;
        }
        if matcher.matches(path) {
            matches.push(SearchMatch {
                kind: "file".to_string(),
                path: path.clone(),
                line: None,
                preview: path.clone(),
                symbol_name: None,
                name_match_ranges: Vec::new(),
                source_line: None,
                content_match_ranges: Vec::new(),
                preview_ranges: Vec::new(),
                matching_degree: None,
                is_ignored: None,
            });
            file_matches += 1;
        }
    }

    let mut content_matches = 0;
    let aliases = canonical_text_overrides(root, &request.text_overrides, request.follow_symlinks);
    let mut candidates = if aliases.is_empty() {
        index.candidate_ids(query, request.regular_expression)
    } else {
        // An alias may have no disk posting for a word present only in its editor snapshot.
        index.all_file_ids()
    };
    for path in request.text_overrides.keys() {
        if let Some(id) = index.id_for_path(path) {
            if !candidates.contains(&id) {
                candidates.push(id);
            }
        }
    }
    if page.is_some() {
        candidates.sort_by_key(|id| index.file(*id).map(|file| file.path.as_str()));
    }
    for id in candidates {
        lithe_protocol::cancellation::check()?;
        if matches.len() >= limit || content_matches >= content_limit {
            break;
        }
        let Some(indexed_file) = index.file(id) else {
            continue;
        };
        let path = &indexed_file.path;
        if page.is_some_and(|page| !page.accepts_path(path))
            || !scope_allows(&request.scope_paths, path)
            || !file_mask_allows(&masks, path)
            || !path_filters.allows(path)
        {
            continue;
        }
        let Ok(file) = safe_relative_path(root, path) else {
            continue;
        };
        let text = request
            .text_overrides
            .get(path)
            .map(String::as_str)
            .or_else(|| aliases.get(&file).copied())
            .map(str::to_owned)
            .or_else(|| {
                read_searchable_text_limit(
                    &file,
                    request.max_file_bytes.unwrap_or(MAX_SEARCH_FILE_SIZE),
                )
            });
        let Some(text) = text else { continue };
        for (index, line) in text.split('\n').map(line_content).enumerate() {
            lithe_protocol::cancellation::check()?;
            if matcher.matches(line) {
                let (preview, content_match_ranges, preview_ranges) =
                    if request.include_content_ranges {
                        content_preview(line, matcher.content_ranges(line))
                    } else {
                        (line.trim().to_string(), Vec::new(), Vec::new())
                    };
                let result = SearchMatch {
                    kind: "content".to_string(),
                    path: path.clone(),
                    line: Some(index + 1),
                    preview,
                    symbol_name: None,
                    name_match_ranges: Vec::new(),
                    source_line: request.include_content_ranges.then(|| line.to_string()),
                    content_match_ranges,
                    preview_ranges,
                    matching_degree: None,
                    is_ignored: None,
                };
                if page.is_some_and(|page| !page.accepts(&result)) {
                    continue;
                }
                matches.push(result);
                content_matches += 1;
                if matches.len() >= limit || content_matches >= content_limit {
                    break;
                }
            }
        }
    }

    Ok(SearchResponse { matches })
}

/// The preview of a content match's line and its matches in it, as every
/// result row shows a line ([`line_preview`]), next to the matches in the line.
fn content_preview(
    line: &str,
    ranges: Vec<SearchNameRange>,
) -> (String, Vec<SearchNameRange>, Vec<SearchNameRange>) {
    let highlights: Vec<Utf16Span> = ranges
        .iter()
        .map(|range| Utf16Span {
            start: range.start,
            length: range.length,
        })
        .collect();
    let shown = line_preview(line, &highlights, None);
    let preview_ranges = shown
        .ranges
        .iter()
        .map(|span| SearchNameRange {
            start: span.start,
            length: span.length,
        })
        .collect();
    (shown.text, ranges, preview_ranges)
}

/// Builds or reuses the search index and reports its current size.
pub fn warm_search_index(
    request: SearchIndexRequest,
) -> Result<SearchIndexStatusResponse, CoreError> {
    let root = existing_root(&request.root)?;
    let rules = VisibilityRules::new(request.hidden_directory_names, request.hidden_file_patterns)
        .with_workspace_roots(&root, &request.workspace_roots)
        .with_search_options(request.max_file_bytes, request.include_ignored)?;
    let index = search_index::get_or_build(&root, &rules)?;
    let index = index
        .read()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))?;
    let stats = index.stats();
    Ok(SearchIndexStatusResponse {
        file_count: stats.file_count,
        symbol_count: 0,
        posting_count: stats.posting_count,
        rebuilt: false,
    })
}

/// Applies changed paths to a cached index, rebuilding when its rules changed.
/// `update_providers` reconciles the root's running providers once Git
/// decisions are rebuilt, as for [`crate::indexing::configure`].
pub fn update_search_index(
    request: SearchIndexUpdateRequest,
    mut update_providers: impl FnMut(&Path) -> Result<(), CoreError>,
) -> Result<SearchIndexStatusResponse, CoreError> {
    let root = existing_root(&request.root)?;
    let rules = VisibilityRules::new(request.hidden_directory_names, request.hidden_file_patterns)
        .with_workspace_roots(&root, &request.workspace_roots)
        .with_search_options(request.max_file_bytes, request.include_ignored)?;
    if request.paths.iter().any(|path| {
        path.ends_with(".gitignore")
            || path.contains(".git/")
            || path.ends_with(".git")
            || root.join(path).is_dir()
    }) {
        crate::indexing::refresh(&root)?;
    }
    let outcome = search_index::update_paths(&root, &rules, &request.paths);
    let rebuilt = matches!(outcome, UpdateOutcome::RequiresRebuild);
    if rebuilt {
        search_index::invalidate_root(&root);
    }
    let index = search_index::get_or_build(&root, &rules)?;
    // Refresh starts with no Git decisions. Publish only after traversal has
    // resolved them, otherwise providers briefly (or permanently) lose exclusions.
    update_providers(&root)?;
    let index = index
        .read()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))?;
    let stats = index.stats();
    Ok(SearchIndexStatusResponse {
        file_count: stats.file_count,
        symbol_count: 0,
        posting_count: stats.posting_count,
        rebuilt: rebuilt || matches!(outcome, UpdateOutcome::NotIndexed),
    })
}

/// Drops the cached index for one validated workspace root.
pub fn invalidate_search_index(request: SearchIndexRequest) -> Result<(), CoreError> {
    let requested_root = PathBuf::from(&request.root);
    let root = search_index::canonicalize_with_missing_components(&requested_root)
        .unwrap_or(requested_root);
    search_index::invalidate_root(&root);
    Ok(())
}
