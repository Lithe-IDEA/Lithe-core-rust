//! Search Everywhere: ranked name and text matches across the workspace.

use super::*;

/// Combines file and content results for Search Everywhere.
///
/// The `type` and `symbol` buckets stay in the paging contract but Core no
/// longer fills them: symbol knowledge belongs to language servers and
/// plugins, so a source-specific regex here would favour one language.
pub fn search_everywhere(request: SearchRequest) -> Result<SearchResponse, CoreError> {
    search_everywhere_with_page(request, None)
}

/// Searches one bounded page when a continuation state is supplied.
pub(crate) fn search_everywhere_with_page(
    request: SearchRequest,
    mut page: Option<&mut SearchPage>,
) -> Result<SearchResponse, CoreError> {
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
    .with_workspace_roots(&root, &request.workspace_roots);
    let matcher = Matcher::new(
        &query,
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
    let file_limit = if page.as_ref().is_some_and(|page| page.exhausted("file")) {
        0
    } else {
        request.max_file_results.unwrap_or(limit).min(limit)
    };
    let symbol_limit = if page
        .as_ref()
        .is_some_and(|page| page.exhausted("type") && page.exhausted("symbol"))
    {
        0
    } else {
        request.max_symbol_results.unwrap_or(limit).min(limit)
    };
    let content_limit = if page.as_ref().is_some_and(|page| page.exhausted("content")) {
        0
    } else {
        request.max_content_results.unwrap_or(limit).min(limit)
    };
    let context_index = request
        .context
        .as_ref()
        .map(crate::name_ranking::ContextIndex::new);
    let mut files = Vec::new();
    let mut types = Vec::new();
    let mut symbols = Vec::new();
    let mut last_flush = None;
    if file_limit > 0 {
        let name_rules = VisibilityRules {
            // Generated output is a file-name source, not a content/symbol index.
            hidden_directories: rules
                .hidden_directories
                .iter()
                .filter(|name| {
                    !request.include_generated_file_names
                        || !BUILT_IN_HIDDEN_DIRECTORIES.contains(&name.as_str())
                        || [".git", ".svn", ".worktree", ".worktrees"].contains(&name.as_str())
                })
                .cloned()
                .collect(),
            hidden_file_patterns: rules.hidden_file_patterns.clone(),
            delegated_roots: rules.delegated_roots.clone(),
        };
        let names = search_index::get_names(&root, &name_rules)?;
        let names = names
            .read()
            .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))?;
        for id in names.all_file_ids() {
            lithe_protocol::cancellation::check()?;
            let Some(file) = names.file(id) else { continue };
            if !scope_allows(&request.scope_paths, &file.path)
                || !file_mask_allows(&masks, &file.path)
                || !path_filters.allows(&file.path)
            {
                continue;
            }
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            let mut result = SearchMatch {
                kind: "file".to_string(),
                path: file.path.clone(),
                line: None,
                preview: file.path.clone(),
                symbol_name: None,
                name_match_ranges: Vec::new(),
                source_line: None,
                content_match_ranges: Vec::new(),
                matching_degree: None,
                is_ignored: None,
            };
            if match_everywhere_name(&request, &matcher, name, &mut result) {
                result.matching_degree.get_or_insert(0);
            } else if match_everywhere_name(&request, &matcher, &file.path, &mut result) {
                // Directory-only matches remain candidates (including Agent mentions), below every basename hit.
                result.matching_degree = Some(-1);
                result.name_match_ranges.clear();
            } else {
                continue;
            }
            if let Some(index) = &context_index {
                result.matching_degree = result
                    .matching_degree
                    .map(|degree| index.contextual_degree(degree, &file.path));
            }
            if let Some(page) = page.as_ref() {
                page.push(&mut files, result, file_limit);
            } else {
                files.push(result);
            }
            emit_search_results(
                [
                    (&mut files, file_limit),
                    (&mut types, symbol_limit),
                    (&mut symbols, symbol_limit),
                ],
                limit,
                &mut last_flush,
                std::time::Instant::now(),
                false,
            );
        }
    }
    emit_search_results(
        [
            (&mut files, file_limit),
            (&mut types, symbol_limit),
            (&mut symbols, symbol_limit),
        ],
        limit,
        &mut last_flush,
        std::time::Instant::now(),
        true,
    );
    // Publish names before content parsing starts.
    // Zero source caps are execution boundaries, not just post-scan truncation.
    let shared_index = if content_limit > 0 {
        Some(search_index::get_for_text(&root, &rules)?)
    } else {
        None
    };
    let index = shared_index
        .as_ref()
        .map(|index| {
            index
                .read()
                .map_err(|_| CoreError::new(ErrorCode::Unknown, "Search index lock was poisoned"))
        })
        .transpose()?;
    if let Some(page) = page.as_mut() {
        page.finish("file", &mut files, file_limit);
        page.finish("type", &mut types, symbol_limit);
        page.finish("symbol", &mut symbols, symbol_limit);
    } else {
        for (bucket, cap) in [
            (&mut files, file_limit),
            (&mut types, symbol_limit),
            (&mut symbols, symbol_limit),
        ] {
            rank_search_bucket(bucket, cap);
        }
    }
    let mut content_request = request.clone();
    content_request.max_file_results = Some(0);
    if page.is_some() {
        content_request.max_results = content_limit + 1;
        content_request.max_content_results = Some(content_limit + 1);
    }
    let mut content = if content_limit > 0 {
        search_with_index(
            &root,
            &content_request,
            &query,
            index
                .as_ref()
                .expect("content source requires content index"),
            page.as_deref(),
        )?
        .matches
    } else {
        Vec::new()
    };
    if let Some(page) = page.as_mut() {
        page.finish("content", &mut content, content_limit);
    }
    let mut matches = files;
    matches.extend(types);
    matches.extend(symbols);
    matches.extend(content);
    matches.truncate(limit);
    drop(index);
    // Ask Git once for the bounded result set, not once per row or indexed file.
    let paths: Vec<String> = matches.iter().map(|item| item.path.clone()).collect();
    if let Some(ignored) = lithe_git::ignored_paths(&root, &paths)? {
        for item in &mut matches {
            item.is_ignored = Some(ignored.contains(&item.path));
        }
    }
    if let Some(policy) = &indexing_policy {
        policy.check()?;
    }
    Ok(SearchResponse { matches })
}

/// Stable total ordering shared by bounded collection and cursor continuation.
pub(crate) fn compare_search_matches(a: &SearchMatch, b: &SearchMatch) -> std::cmp::Ordering {
    b.matching_degree
        .cmp(&a.matching_degree)
        .then_with(|| a.path.cmp(&b.path))
        .then_with(|| a.line.cmp(&b.line))
        .then_with(|| a.symbol_name.cmp(&b.symbol_name))
}

fn rank_search_bucket(bucket: &mut Vec<SearchMatch>, cap: usize) {
    bucket.sort_by(compare_search_matches);
    bucket.truncate(cap);
}

// Replaceable snapshots let slow hosts keep only the newest batch. Ranking
// before each cap preserves the final top matches even when better hits arrive late.
pub(crate) fn emit_search_results(
    buckets: [(&mut Vec<SearchMatch>, usize); 3],
    limit: usize,
    last_flush: &mut Option<std::time::Instant>,
    now: std::time::Instant,
    force: bool,
) {
    if !lithe_request_context::events::has_sink()
        || buckets.iter().all(|(bucket, _)| bucket.is_empty())
        || (!force
            && last_flush.is_some_and(|last| {
                now.duration_since(last) < std::time::Duration::from_millis(100)
            }))
    {
        return;
    }
    let mut matches = Vec::new();
    for (bucket, cap) in buckets {
        rank_search_bucket(bucket, cap + 1);
        matches.extend(
            bucket
                .iter()
                .take(cap.min(limit.saturating_sub(matches.len())))
                .cloned(),
        );
    }
    *last_flush = Some(now);
    lithe_request_context::events::emit(serde_json::json!({
        "type": "searchResults", "results": SearchResponse { matches }
    }));
}

fn match_everywhere_name(
    request: &SearchRequest,
    literal: &Matcher,
    name: &str,
    result: &mut SearchMatch,
) -> bool {
    if request.regular_expression || request.whole_words || request.case_sensitive {
        return literal.matches(name);
    }
    let Some(hit) = crate::name_matching::match_name(request.query.trim(), name) else {
        return false;
    };
    result.name_match_ranges = hit.ranges;
    result.matching_degree = Some(hit.degree);
    true
}
