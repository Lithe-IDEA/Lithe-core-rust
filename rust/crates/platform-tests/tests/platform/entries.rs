//! No-replace renames and symbolic links through `lithe_platform::entries`
//! on the host's file system.

use crate::support::TempDir;
use lithe_platform::entries::{create_symlink, rename_no_replace};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

#[test]
fn a_rename_never_replaces_an_existing_entry() {
    let directory = TempDir::new("lithe rename");
    let from = directory.path().join("from.txt");
    let to = directory.path().join("to.txt");
    fs::write(&from, "from").expect("from");
    fs::write(&to, "to").expect("to");

    let error = rename_no_replace(&from, &to).expect_err("target exists");
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&to).expect("to"), "to");

    fs::remove_file(&to).expect("remove target");
    rename_no_replace(&from, &to).expect("rename");
    assert_eq!(fs::read_to_string(&to).expect("to"), "from");
    assert!(!from.exists());
}

#[test]
fn a_directory_renames_without_replacing_one() {
    let directory = TempDir::new("lithe rename directory");
    let from = directory.path().join("from");
    let to = directory.path().join("to");
    fs::create_dir(&from).expect("from");
    fs::write(from.join("inner.txt"), "inner").expect("inner");
    fs::create_dir(&to).expect("to");

    let error = rename_no_replace(&from, &to).expect_err("target exists");
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);

    fs::remove_dir(&to).expect("remove target");
    rename_no_replace(&from, &to).expect("rename");
    assert_eq!(
        fs::read_to_string(to.join("inner.txt")).expect("inner"),
        "inner"
    );
}

/// Windows creates links only with Developer Mode or elevation; build
/// machines run elevated, but a developer's shell may not.
fn links_refused(error: &std::io::Error) -> bool {
    cfg!(windows) && error.raw_os_error() == Some(1314)
}

#[test]
fn a_symlink_stores_its_target_and_never_replaces_an_entry() {
    let directory = TempDir::new("lithe symlink");
    let link = directory.path().join("link");
    match create_symlink(Path::new("missing/target"), &link) {
        Ok(()) => {}
        Err(error) if links_refused(&error) => return,
        Err(error) => panic!("link: {error}"),
    }
    let stored = fs::read_link(&link).expect("read link");
    let expected = if cfg!(windows) {
        Path::new("missing\\target")
    } else {
        Path::new("missing/target")
    };
    assert_eq!(stored, expected);
    assert!(fs::symlink_metadata(&link)
        .expect("link metadata")
        .file_type()
        .is_symlink());

    let error = create_symlink(Path::new("other"), &link).expect_err("link exists");
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read_link(&link).expect("read link"), expected);
}

#[test]
fn a_symlink_to_a_directory_lists_the_directory() {
    let directory = TempDir::new("lithe symlink directory");
    let target = directory.path().join("shared");
    fs::create_dir(&target).expect("target");
    fs::write(target.join("inner.txt"), "inner").expect("inner");
    let link = directory.path().join("link");
    match create_symlink(Path::new("shared"), &link) {
        Ok(()) => {}
        Err(error) if links_refused(&error) => return,
        Err(error) => panic!("link: {error}"),
    }
    assert!(fs::metadata(&link).expect("followed").is_dir());
    assert_eq!(
        fs::read_to_string(link.join("inner.txt")).expect("through link"),
        "inner"
    );
}
