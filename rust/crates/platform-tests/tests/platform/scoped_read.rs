//! No-follow reads against real OS directory and file handles.

use crate::support::TempDir;
use lithe_platform::scoped_read::{Directory, Kind};
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read};

#[test]
fn scoped_read_handles_keep_the_opened_directory_when_its_name_changes() {
    let root = TempDir::new("lithe scoped read");
    fs::create_dir(root.path().join("child")).unwrap();
    fs::write(root.path().join("child/data"), "original").unwrap();
    let handle = Directory::open_root(root.path()).unwrap();
    let child = handle.directory(OsStr::new("child")).unwrap();
    let renamed = fs::rename(root.path().join("child"), root.path().join("old"));
    // Windows denies renames while the ancestor handle excludes delete sharing.
    // Unix permits them, and descendant lookups stay relative to the held fd.
    if let Err(error) = &renamed {
        assert!(
            cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)),
            "unexpected directory rename failure: {error}"
        );
    }
    if renamed.is_ok() {
        fs::create_dir(root.path().join("child")).unwrap();
        fs::write(root.path().join("child/data"), "replacement").unwrap();
    }
    let mut text = String::new();
    child
        .file(OsStr::new("data"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "original");
    assert_eq!(
        child.child_metadata(OsStr::new("data")).unwrap().kind,
        Kind::File
    );
}

#[test]
fn scoped_read_refuses_links_and_reports_the_leaf_itself() {
    let root = TempDir::new("lithe scoped links");
    fs::create_dir(root.path().join("target")).unwrap();
    fs::write(root.path().join("target/data"), "target").unwrap();
    if let Err(error) = lithe_platform::entries::create_symlink(
        std::path::Path::new("target"),
        &root.path().join("alias"),
    ) {
        if cfg!(windows) && error.raw_os_error() == Some(1314) {
            return;
        }
        panic!("create symlink: {error}");
    }
    let handle = Directory::open_root(root.path()).unwrap();
    assert_eq!(
        handle.child_metadata(OsStr::new("alias")).unwrap().kind,
        Kind::Symlink
    );
    assert_eq!(
        handle.directory(OsStr::new("alias")).err().unwrap().kind(),
        ErrorKind::PermissionDenied
    );
    assert_eq!(
        handle.file(OsStr::new("alias")).err().unwrap().kind(),
        ErrorKind::PermissionDenied
    );
    for name in ["..", "a/b", "/absolute"] {
        assert!(handle.directory(OsStr::new(name)).is_err());
    }
}
