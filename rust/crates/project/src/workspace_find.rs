//! Bounded plugin disk searches, reusing workspace matching without retaining an index.
//!
//! Only the best page plus a sentinel is retained. Traversal and text reads use
//! held no-follow handles; a continuation binds the query and visibility generation.

use crate::files::{
    file_mask_allows, parse_file_mask, searchable_bytes_text, Matcher, VisibilityRules,
};
use crate::workspace_read::{check, decode, failure, invalid, root, validate_path};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use lithe_platform::scoped_read::{Directory, Kind};
use lithe_protocol::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_RESULTS: usize = 200;
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 128;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FindRequest {
    root: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    file_mask: String,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    whole_words: bool,
    #[serde(default)]
    regular_expression: bool,
    cursor: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    hidden_directory_names: Vec<String>,
    #[serde(default)]
    hidden_file_patterns: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    scope: String,
    after: (String, usize),
    returned: usize,
}

/// Executes API-08 file-name searches with a root/query-bound continuation.
pub fn find_files_command(payload: Value) -> Result<Value, CoreError> {
    find(decode(payload)?, false)
}

/// Executes API-08 line searches against disk, never an unsaved editor buffer.
pub fn find_text_command(payload: Value) -> Result<Value, CoreError> {
    find(decode(payload)?, true)
}

struct Search {
    matcher: Matcher,
    masks: Vec<String>,
    rules: VisibilityRules,
    policy: Option<std::sync::Arc<crate::indexing::IndexingContext>>,
    after: (String, usize),
    cap: usize,
    candidates: BTreeMap<(String, usize), Value>,
    text: bool,
    incomplete: bool,
    deadline: Instant,
}

impl Search {
    fn push(&mut self, path: &str, line: usize, value: Value) {
        let key = (path.to_string(), line);
        if key <= self.after {
            return;
        }
        self.candidates.insert(key, value);
        if self.candidates.len() > self.cap + 1 {
            self.candidates.pop_last();
        }
    }

    fn walk(&mut self, directory: Directory, prefix: &str, depth: usize) -> Result<(), CoreError> {
        check(self.deadline)?;
        if depth > MAX_DEPTH {
            self.incomplete = true;
            return Ok(());
        }
        let mut refused = None;
        let result = directory.visit(|name, info| {
            match self.entry(&directory, name, info.kind, info.size, prefix, depth) {
                Ok(()) => Ok(()),
                Err(error) => {
                    refused = Some(error);
                    Err(std::io::Error::other("Workspace search stopped"))
                }
            }
        });
        if let Some(error) = refused {
            Err(error)
        } else {
            result.map_err(failure)
        }
    }

    fn entry(
        &mut self,
        directory: &Directory,
        name: &OsStr,
        kind: Kind,
        size: u64,
        prefix: &str,
        depth: usize,
    ) -> Result<(), CoreError> {
        check(self.deadline)?;
        let Some(name_text) = name.to_str() else {
            return Ok(());
        };
        let path = if prefix.is_empty() {
            name_text.to_string()
        } else {
            format!("{prefix}/{name_text}")
        };
        if !matches!(kind, Kind::Directory | Kind::File)
            || validate_path(&path).is_err()
            || self.rules.is_hidden(&path, kind == Kind::Directory)
        {
            return Ok(());
        }
        if let Some(policy) = &self.policy {
            if policy
                .search_reason(&path, kind == Kind::Directory, false)?
                .is_some()
            {
                return Ok(());
            }
        }
        if kind == Kind::Directory {
            return match directory.directory(name) {
                Ok(child) => self.walk(child, &path, depth + 1),
                // A concurrently removed entry contributes no result.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(failure(error)),
            };
        }
        if !file_mask_allows(&self.masks, &path) {
            return Ok(());
        }
        if !self.text {
            if self.matcher.matches(&path) {
                self.push(&path, 0, json!({"path":path}));
            }
            return Ok(());
        }
        if size > MAX_TEXT_BYTES as u64 {
            self.incomplete = true;
            return Ok(());
        }
        let mut file = match directory.file(name) {
            Ok(file) => file.take((MAX_TEXT_BYTES + 1) as u64),
            // A concurrently removed entry contributes no result.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(failure(error)),
        };
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            check(self.deadline)?;
            let count = file.read(&mut chunk).map_err(failure)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        if bytes.len() > MAX_TEXT_BYTES {
            self.incomplete = true;
            return Ok(());
        }
        let Some(text) = searchable_bytes_text(bytes) else {
            return Ok(());
        };
        for (index, line) in text.split('\n').enumerate() {
            check(self.deadline)?;
            let line = line.strip_suffix('\r').unwrap_or(line);
            if self.matcher.matches(line) {
                let preview: String = line.trim().chars().take(500).collect();
                self.push(
                    &path,
                    index + 1,
                    json!({"path":path,"line":index+1,"preview":preview,"range":null}),
                );
            }
        }
        Ok(())
    }
}

fn find(mut request: FindRequest, text: bool) -> Result<Value, CoreError> {
    let query = if text {
        request.pattern.as_deref()
    } else {
        request.query.as_deref()
    }
    .ok_or_else(|| invalid("The search query is required"))?;
    if query.is_empty()
        || query.len() > 4096
        || request.file_mask.len() > 4096
        || (text && request.query.is_some())
        || (!text
            && (request.pattern.is_some()
                || request.case_sensitive
                || request.whole_words
                || request.regular_expression))
    {
        return Err(invalid("Invalid search query or options"));
    }
    let limit = request.limit.unwrap_or(100);
    if !(1..=MAX_RESULTS).contains(&limit) {
        return Err(invalid("Search limit must be 1-200"));
    }
    let directory = root(&request.root)?;
    let canonical = Path::new(&request.root).canonicalize().map_err(failure)?;
    let policy = crate::indexing::context(&canonical);
    let rules = VisibilityRules::new(
        request.hidden_directory_names.clone(),
        request.hidden_file_patterns.clone(),
    );
    let cursor = request.cursor.take();
    // Page size may change between requests, but query, root and effective policy may not.
    request.limit = None;
    let scope = crate::encoding::bytes_identity(
        &serde_json::to_vec(&json!([
            text,
            request,
            canonical,
            rules.hidden_directories,
            rules.hidden_file_patterns,
            policy.as_ref().map(|policy| policy.revision)
        ]))
        .expect("search scope encodes"),
    );
    let cursor: Option<Cursor> = cursor
        .map(|value| {
            if value.len() > 8192 {
                return Err(invalid("Invalid search cursor"));
            }
            URL_SAFE_NO_PAD
                .decode(value)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .ok_or_else(|| invalid("Invalid search cursor"))
        })
        .transpose()?;
    if cursor.as_ref().is_some_and(|cursor| {
        cursor.scope != scope
            || cursor.returned == 0
            || cursor.returned >= MAX_RESULTS
            || cursor.after.0.is_empty()
            || validate_path(&cursor.after.0).is_err()
    }) {
        return Err(invalid("Search cursor does not belong to this request"));
    }
    let returned = cursor.as_ref().map_or(0, |cursor| cursor.returned);
    let mut search = Search {
        matcher: Matcher::new(
            query,
            request.case_sensitive,
            request.whole_words,
            request.regular_expression,
        )?,
        masks: parse_file_mask(&request.file_mask),
        rules,
        policy,
        after: cursor.map_or((String::new(), 0), |cursor| cursor.after),
        cap: limit.min(MAX_RESULTS - returned),
        candidates: BTreeMap::new(),
        text,
        incomplete: false,
        deadline: Instant::now() + Duration::from_secs(30),
    };
    search.walk(directory, "", 0)?;
    if let Some(policy) = &search.policy {
        policy.check()?
    }
    let mut more = search.candidates.len() > search.cap;
    let mut matches = Vec::new();
    let mut last = None;
    let mut page_bytes = 0;
    for (key, value) in search.candidates.into_iter().take(search.cap) {
        let bytes = serde_json::to_vec(&value)
            .expect("search result encodes")
            .len()
            + 256;
        if page_bytes + bytes > 900 * 1024 {
            more = true;
            break;
        }
        page_bytes += bytes;
        last = Some(key);
        matches.push(value);
    }
    let total = returned + matches.len();
    let next = if more && total < MAX_RESULTS {
        Some(
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&Cursor {
                    scope,
                    after: last.expect("a search page has a result"),
                    returned: total,
                })
                .expect("search cursor encodes"),
            ),
        )
    } else {
        None
    };
    Ok(
        json!({"matches":matches,"nextCursor":next,"isTruncated":search.incomplete || (more && total >= MAX_RESULTS),"source":"disk"}),
    )
}
