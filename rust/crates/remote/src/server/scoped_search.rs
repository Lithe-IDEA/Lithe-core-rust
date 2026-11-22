//! One bounded disk search, with the same matching and decoding rules as
//! Core workspace search. All filesystem access is relative to an open directory.

use crate::protocol::methods::{
    FileKind, ScopedSearchMatch, ScopedSearchParams, ScopedSearchResult, MAX_SCOPED_SEARCH_RESULTS,
};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::{fs, scoped_files};
use lithe_core_rules::visibility::{file_mask_allows, parse_file_mask, VisibilityRules};
use lithe_text::search::{line_content, searchable_bytes_text, Matcher};
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::time::{Duration, Instant};

const MAX_QUERY_BYTES: usize = 4096;
const MAX_PATH_BYTES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 50_000;
const MAX_NAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOTAL_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PREVIEW_BYTES: usize = 512;
const SCAN_DEADLINE: Duration = Duration::from_secs(8);

pub(super) fn search(params: ScopedSearchParams) -> Result<ScopedSearchResult, RemoteError> {
    let options = params.search;
    if options.query.len() > MAX_QUERY_BYTES
        || options.file_mask.len() > MAX_QUERY_BYTES
        || (options.text && options.query.trim().is_empty())
    {
        return Err(RemoteError::new(
            ErrorCode::InvalidRequest,
            "Invalid workspace search query or file mask",
        ));
    }
    let matcher = Matcher::new(
        &options.query,
        options.case_sensitive,
        options.whole_words,
        options.regular_expression,
    )
    .map_err(|_| {
        RemoteError::new(
            ErrorCode::InvalidRequest,
            "Invalid workspace search expression",
        )
    })?;
    let root = fs::host_path(&params.root)?;
    let directory =
        scoped_files::open_root(&root).map_err(|error| fs::io_error(error, &params.root))?;
    let mut scan = Scan {
        matcher,
        text: options.text,
        masks: parse_file_mask(&options.file_mask),
        visibility: VisibilityRules::new(params.hidden_directories, Vec::new()),
        result: ScopedSearchResult {
            matches: Vec::new(),
            is_truncated: false,
        },
        deadline: Instant::now() + SCAN_DEADLINE,
        entries_left: MAX_ENTRIES,
        name_bytes_left: MAX_NAME_BYTES,
        bytes_left: MAX_TOTAL_FILE_BYTES,
    };
    scan.directory(&directory, &mut String::new(), 0)
        .map_err(|error| fs::io_error(error, &params.root))?;
    Ok(scan.result)
}

struct Scan {
    matcher: Matcher,
    text: bool,
    masks: Vec<String>,
    visibility: VisibilityRules,
    result: ScopedSearchResult,
    deadline: Instant,
    entries_left: usize,
    name_bytes_left: usize,
    bytes_left: usize,
}

impl Scan {
    fn exhausted(&mut self) -> bool {
        if self.bytes_left == 0 || Instant::now() >= self.deadline {
            self.result.is_truncated = true;
            return true;
        }
        false
    }

    fn directory(&mut self, directory: &File, path: &mut String, depth: usize) -> io::Result<bool> {
        let mut walk_limit_reached = false;
        let entries = match scoped_files::list_with_budget(directory.try_clone()?, |name_bytes| {
            if self.entries_left == 0
                || name_bytes > self.name_bytes_left
                || Instant::now() >= self.deadline
            {
                walk_limit_reached = true;
                return false;
            }
            self.entries_left -= 1;
            self.name_bytes_left -= name_bytes;
            true
        }) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
                self.result.is_truncated = true;
                // Discard a partial directory listing so readdir order cannot
                // influence the ordered prefix returned to the plugin.
                return Ok(!walk_limit_reached);
            }
            Err(error) => return Err(error),
        };
        let prefix = path.len();
        for entry in entries {
            if self.exhausted() {
                return Ok(false);
            }
            path.truncate(prefix);
            if prefix != 0 {
                path.push('/');
            }
            path.push_str(&entry.name);
            let is_directory = entry.kind == FileKind::Directory;
            if self.visibility.is_hidden(path, is_directory) {
                continue;
            }
            if path.len() > MAX_PATH_BYTES || (is_directory && depth >= MAX_DEPTH) {
                self.result.is_truncated = true;
                continue;
            }
            if !is_directory && !file_mask_allows(&self.masks, path) {
                continue;
            }
            let name = CString::new(entry.name).expect("readdir names cannot contain NUL");
            let next = if is_directory {
                scoped_files::open_directory_at(directory, &name)
                    .and_then(|child| self.directory(&child, path, depth + 1))
            } else if self.text {
                self.file(directory, &name, path)
            } else if self.matcher.matches(path) {
                Ok(self.push_match(path, None, None))
            } else {
                Ok(true)
            };
            match next {
                Ok(true) => {}
                Ok(false) => return Ok(false),
                // The tree can change during a scan. Refused or vanished entries
                // cannot contribute results from a replacement path.
                Err(_) => self.result.is_truncated = true,
            }
        }
        path.truncate(prefix);
        Ok(true)
    }

    fn file(&mut self, directory: &File, name: &std::ffi::CStr, path: &str) -> io::Result<bool> {
        let budget = self.bytes_left.min(MAX_FILE_BYTES);
        let (bytes, truncated) = scoped_files::read_regular_at(directory, name, budget)?;
        self.bytes_left -= bytes.len();
        if truncated {
            self.result.is_truncated = true;
            return Ok(true);
        }
        let Some(text) = searchable_bytes_text(bytes) else {
            return Ok(true);
        };
        for (index, line) in text.split('\n').map(line_content).enumerate() {
            // A single file can contain millions of short lines. Avoid reading
            // the clock on each line without dropping the scan deadline.
            if index % 256 == 0 && Instant::now() >= self.deadline {
                self.result.is_truncated = true;
                return Ok(false);
            }
            if self.matcher.matches(line) {
                let preview = line.trim();
                let end = preview.floor_char_boundary(preview.len().min(MAX_PREVIEW_BYTES));
                if !self.push_match(path, Some(index + 1), Some(&preview[..end])) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn push_match(&mut self, path: &str, line: Option<usize>, preview: Option<&str>) -> bool {
        if self.result.matches.len() == MAX_SCOPED_SEARCH_RESULTS {
            self.result.is_truncated = true;
            return false;
        }
        self.result.matches.push(ScopedSearchMatch {
            path: path.to_owned(),
            line,
            preview: preview.map(str::to_owned),
        });
        true
    }
}
