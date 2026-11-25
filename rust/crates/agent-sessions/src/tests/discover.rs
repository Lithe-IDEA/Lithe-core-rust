//! Listing the session files a source matches.

use std::fs;
use std::path::Path;

use lithe_protocol::ErrorCode;

use super::TempDir;
use crate::discover::{discover, Discovery, SessionSource};

fn tree() -> TempDir {
    let dir = TempDir::new("discover");
    for file in ["a.jsonl", "b.txt", "sub/c.jsonl", "sub/deep/d.jsonl"] {
        let path = dir.path().join(file);
        fs::create_dir_all(path.parent().expect("a parent directory")).expect("create a directory");
        fs::write(&path, b"{}\n").expect("write a file");
    }
    dir
}

fn source(root: &Path, include: &[&str]) -> SessionSource {
    SessionSource {
        root: root.to_path_buf(),
        include: include.iter().map(|pattern| pattern.to_string()).collect(),
    }
}

/// The files `include` lists, relative to the root, with `/` separators.
fn listed(dir: &TempDir, include: &[&str]) -> Vec<String> {
    let discovery = discover(&source(dir.path(), include)).expect("list the files");
    assert!(!discovery.truncated);
    assert_eq!(discovery.unreadable_entries, 0);
    discovery
        .files
        .iter()
        .map(|path| {
            path.strip_prefix(dir.path())
                .expect("a file below the root")
                .components()
                .map(|component| component.as_os_str().to_str().expect("a Unicode name"))
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

#[test]
fn patterns_match_paths_below_the_root() {
    let dir = tree();
    assert_eq!(listed(&dir, &["*.jsonl"]), ["a.jsonl"]);
    assert_eq!(
        listed(&dir, &["**/*.jsonl"]),
        ["a.jsonl", "sub/c.jsonl", "sub/deep/d.jsonl"]
    );
    assert_eq!(listed(&dir, &["sub/*.jsonl"]), ["sub/c.jsonl"]);
    assert_eq!(
        listed(&dir, &["*.txt", "sub/deep/*"]),
        ["b.txt", "sub/deep/d.jsonl"]
    );
    // Directories are not files to read.
    assert_eq!(listed(&dir, &["*"]), ["a.jsonl", "b.txt"]);
    assert!(listed(&dir, &[]).is_empty());
}

#[test]
fn a_missing_root_lists_nothing() {
    let dir = TempDir::new("discover-missing");
    let discovery =
        discover(&source(&dir.path().join("never-written"), &["**/*"])).expect("list the files");
    assert_eq!(discovery, Discovery::default());
}

#[test]
fn an_invalid_pattern_is_refused() {
    let dir = tree();
    for root in [dir.path().to_path_buf(), dir.path().join("never-written")] {
        let error = discover(&source(&root, &["*.jsonl", "a["])).expect_err("an invalid pattern");
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
    }
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_not_followed() {
    use std::os::unix::fs::symlink;

    let dir = tree();
    let outside = TempDir::new("discover-outside");
    fs::write(outside.path().join("x.jsonl"), b"{}\n").expect("write a file");
    symlink(outside.path(), dir.path().join("linked-dir")).expect("link a directory");
    symlink(
        outside.path().join("x.jsonl"),
        dir.path().join("linked.jsonl"),
    )
    .expect("link a file");
    assert_eq!(
        listed(&dir, &["**/*.jsonl"]),
        ["a.jsonl", "sub/c.jsonl", "sub/deep/d.jsonl"]
    );
}
