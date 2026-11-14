//! Workspace traversal, file operations, search, and replacement previews.

use super::encoding::{self, DocumentEncoding};
use super::search_index::{self, UpdateOutcome, WorkspaceSearchIndex};
use super::search_paging::SearchPage;
use crate::contracts::{
    DirectoryListing, DirectoryListingResponse, FileReadResponse, FileWriteResponse,
    ReplacementPreviewResponse, SearchMatch, SearchResponse, WorkspaceNode,
    WorkspaceSnapshotResponse,
};
use lithe_core_rules::file_names::FileNameKey;
use lithe_protocol::{invalid_relative_path, CoreError, ErrorCode};
use lithe_workspace::fs;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Default workspace exclusions, also used by Git discovery for generated directories.
/// The list lives in `lithe-core-rules`, the only definition every host shares.
pub(crate) use lithe_core_rules::visibility::BUILT_IN_HIDDEN_DIRECTORIES;
use lithe_core_rules::visibility::{
    self as shared_visibility, disabled_built_ins, glob_matches, is_disabled_directory,
};

mod events;
mod matcher;
mod paths;
mod replace;
mod requests;
mod search;
mod search_everywhere;
mod snapshot;
mod text_io;
mod visibility;

pub use events::*;
pub use matcher::*;
pub(crate) use paths::*;
pub use replace::*;
pub use requests::*;
pub use search::*;
pub use search_everywhere::*;
pub use snapshot::*;
pub use text_io::*;
pub use visibility::*;

const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;

const MAX_OPEN_FILE_SIZE: u64 = 32 * 1024 * 1024;

#[cfg(test)]
mod event_filter_tests {
    use super::*;

    #[test]
    fn path_filters_match_the_displayed_relative_root_before_limits() {
        let filter =
            SearchPathFilters::new(&["src/**".into()], &["/test/".into()], "module").unwrap();
        assert!(filter.allows("src/main/a.ts"));
        assert!(!filter.allows("src/test/a.ts"));
        assert!(!filter.allows("other/a.ts"));
        let rooted = SearchPathFilters::new(&["module/src".into()], &[], "module").unwrap();
        assert!(rooted.allows("src/a.ts"));
    }

    #[test]
    fn literal_replacement_uses_original_offsets_after_unicode_case_folding() {
        let matcher = Matcher::new("x", false, false, false).unwrap();
        assert_eq!(
            matcher.replace_with_options("İx", "y", false),
            ("İy".into(), 1)
        );
        assert_eq!(
            matcher.replace_with_options("Kxİx", "y", false),
            ("Kyİy".into(), 2)
        );
        let partial = Matcher::new("i", false, false, false).unwrap();
        assert_eq!(
            partial.replace_with_options("İ", "z", false),
            ("z".into(), 1)
        );
        let combining = Matcher::new("\u{307}", false, false, false).unwrap();
        assert_eq!(
            combining.replace_with_options("İ", "z", false),
            ("z".into(), 1)
        );
    }

    #[test]
    fn search_snapshots_merge_at_100ms_and_keep_the_best_bounded_candidates() {
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let received = events.clone();
        lithe_request_context::events::with_sink(
            Arc::new(move |raw| {
                received
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(raw).unwrap());
            }),
            || {
                let make_match = |name: &str, degree| SearchMatch {
                    kind: "file".into(),
                    path: name.into(),
                    line: None,
                    preview: name.into(),
                    symbol_name: None,
                    name_match_ranges: vec![],
                    source_line: None,
                    content_match_ranges: Vec::new(),
                    matching_degree: Some(degree),
                    is_ignored: None,
                };
                let mut files = vec![make_match("First", 1)];
                let (mut types, mut symbols, mut last) = (vec![], vec![], None);
                let now = Instant::now();
                let mut flush = |files: &mut Vec<SearchMatch>, milliseconds, force| {
                    emit_search_results(
                        [(files, 1), (&mut types, 1), (&mut symbols, 1)],
                        1,
                        &mut last,
                        now + Duration::from_millis(milliseconds),
                        force,
                    );
                };
                flush(&mut files, 0, false);
                files.push(make_match("Better", 10));
                flush(&mut files, 99, false);
                assert_eq!(events.lock().unwrap().len(), 1);
                flush(&mut files, 100, false);
                files.push(make_match("Best", 20));
                flush(&mut files, 101, true);
            },
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3);
        for (event, path) in events.iter().zip(["First", "Better", "Best"]) {
            assert_eq!(event["type"], "searchResults");
            assert_eq!(event["results"]["matches"].as_array().unwrap().len(), 1);
            assert_eq!(event["results"]["matches"][0]["path"], path);
        }
    }
}
