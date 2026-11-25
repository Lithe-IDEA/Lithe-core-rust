//! File permissions through `lithe_platform::fs` on the host's file system.

#[cfg(unix)]
mod unix {
    use crate::support::TempDir;
    use lithe_platform::fs::{
        add_execute_bits, directory_mode, has_execute_bit, is_launchable_file, owner_only_file,
        restrict_directory_to_owner, unix_mode, OWNER_ONLY_DIRECTORY_MODE, OWNER_ONLY_FILE_MODE,
    };
    use std::fs::{DirBuilder, OpenOptions, Permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn permission_bits(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn private_files_and_directories_are_readable_by_their_owner_only() {
        let directory = TempDir::new("lithe owner only");
        let file = directory.path().join("token");
        owner_only_file(OpenOptions::new().write(true).create_new(true))
            .open(&file)
            .expect("private file");
        let created = directory.path().join("created");
        directory_mode(&mut DirBuilder::new(), OWNER_ONLY_DIRECTORY_MODE)
            .create(&created)
            .expect("private directory");
        let existing = directory.path().join("existing");
        std::fs::create_dir(&existing).expect("directory");
        std::fs::set_permissions(&existing, Permissions::from_mode(0o755)).expect("chmod");

        restrict_directory_to_owner(&existing).expect("restrict");

        assert_eq!(permission_bits(&file), OWNER_ONLY_FILE_MODE);
        assert_eq!(permission_bits(&created), OWNER_ONLY_DIRECTORY_MODE);
        assert_eq!(permission_bits(&existing), OWNER_ONLY_DIRECTORY_MODE);
    }

    #[test]
    fn execute_bits_make_a_regular_file_launchable() {
        let directory = TempDir::new("lithe execute bits");
        let file = directory.path().join("mvnw");
        std::fs::write(&file, b"#!/bin/sh\n").expect("script");
        std::fs::set_permissions(&file, Permissions::from_mode(0o644)).expect("chmod");
        let before = std::fs::metadata(&file).expect("metadata");
        assert_eq!(unix_mode(&before).map(|mode| mode & 0o777), Some(0o644));
        assert!(!has_execute_bit(&before));
        assert!(!is_launchable_file(&before));

        add_execute_bits(&file).expect("add execute bits");

        let after = std::fs::metadata(&file).expect("metadata");
        assert_eq!(permission_bits(&file), 0o755);
        assert!(has_execute_bit(&after));
        assert!(is_launchable_file(&after));
        let folder = std::fs::metadata(directory.path()).expect("metadata");
        assert!(
            has_execute_bit(&folder) && !is_launchable_file(&folder),
            "a searchable directory is not a program"
        );
    }
}

#[cfg(windows)]
mod windows {
    use crate::support::TempDir;
    use lithe_git_host::private_directory::PrivateDirectory;
    use lithe_platform::fs::{add_execute_bits, has_execute_bit, is_launchable_file, unix_mode};
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    #[test]
    fn windows_stores_no_mode_and_launches_any_regular_file() {
        let directory = TempDir::new("lithe execute bits");
        let file = directory.path().join("tool.exe");
        std::fs::write(&file, b"").expect("placeholder executable");

        add_execute_bits(&file).expect("adding execute bits does nothing");

        let metadata = std::fs::metadata(&file).expect("metadata");
        assert_eq!(unix_mode(&metadata), None);
        assert!(!has_execute_bit(&metadata));
        assert!(is_launchable_file(&metadata));
        let folder = std::fs::metadata(directory.path()).expect("metadata");
        assert!(!is_launchable_file(&folder));
    }

    // Mirrors Win32-OpenSSH opening `-E <path>` without FILE_SHARE_WRITE while
    // the host reads the evidence log concurrently after closing its create handle.
    #[test]
    fn private_directory_evidence_log_allows_read_only_sharing_with_single_writer() {
        let directory = PrivateDirectory::new().expect("private directory");
        let path = directory.path().join("ssh.log");
        drop(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .expect("pre-create evidence log"),
        );

        let mut writer = OpenOptions::new()
            .append(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .expect("open log without FILE_SHARE_WRITE");
        writer.write_all(b"debug1: connected\n").expect("write log");
        writer.flush().expect("flush log");

        let mut reader = OpenOptions::new()
            .read(true)
            .open(&path)
            .expect("read log while writer is open");
        let mut text = String::new();
        reader.read_to_string(&mut text).expect("read contents");
        assert_eq!(text, "debug1: connected\n");
    }
}
