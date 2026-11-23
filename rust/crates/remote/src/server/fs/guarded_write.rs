//! Directory-handle saves. A scoped path never follows a descendant symlink,
//! and cooperating daemons serialize saves before checking the expected revision.
//! External writers do not take this lock: POSIX rename has no revision CAS,
//! so the final recheck reduces, but cannot eliminate, their last syscall window.

use super::{describe, host_path, io_error, too_large, TEMPORARY_FILE_COUNTER};
use crate::protocol::methods::{
    decode_bytes, FileMetadata, FileVersion, WriteFileParams, MAX_TRANSFER_BYTES,
};
use crate::protocol::{ErrorCode, RemoteError};
use std::ffi::{CStr, CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

pub(super) fn write(params: WriteFileParams) -> Result<FileMetadata, RemoteError> {
    write_before_commit(params, || {})
}

fn write_before_commit(
    params: WriteFileParams,
    before_commit: impl FnOnce(),
) -> Result<FileMetadata, RemoteError> {
    let data = decode_bytes(&params.data)
        .map_err(|error| RemoteError::new(ErrorCode::InvalidRequest, error.to_string()))?;
    if data.len() as u64 > MAX_TRANSFER_BYTES {
        return Err(too_large(data.len() as u64));
    }
    let requested = host_path(&params.path)?;
    let boundary = params.root.as_deref().map(host_path).transpose()?;
    let (parent, name) = if let Some(root) = &boundary {
        let relative = requested.strip_prefix(root).map_err(|_| denied())?;
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(denied());
        }
        let name = relative.file_name().ok_or_else(denied)?;
        let root = fs::canonicalize(root).map_err(|error| io_error(error, &params.path))?;
        let mut parent = open_directory(&root).map_err(|error| io_error(error, &params.path))?;
        for part in relative.parent().unwrap_or(Path::new("")).components() {
            let Component::Normal(part) = part else {
                return Err(denied());
            };
            let name = c_name(part)?;
            let opened = open_at(
                &parent,
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                0,
            );
            parent = match opened {
                Err(error) if params.create_parents && error.kind() == io::ErrorKind::NotFound => {
                    // SAFETY: parent is open and name is one NUL-terminated component.
                    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) } != 0 {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::AlreadyExists {
                            return Err(io_error(error, &params.path));
                        }
                    }
                    // Another creator may have won: re-open without following links.
                    open_at(
                        &parent,
                        &name,
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                        0,
                    )
                }
                result => result,
            }
            .map_err(|error| scoped_error(error, &params.path))?;
        }
        (parent, c_name(name)?)
    } else {
        // General host operations retain their explicit ability to write outside
        // a workspace and through an existing leaf link.
        let target = if fs::symlink_metadata(&requested).is_ok_and(|metadata| metadata.is_symlink())
        {
            fs::canonicalize(&requested).map_err(|error| io_error(error, &params.path))?
        } else {
            requested.clone()
        };
        let parent = target.parent().ok_or_else(denied)?;
        if params.create_parents {
            fs::create_dir_all(parent).map_err(|error| io_error(error, &params.path))?;
        }
        let parent = fs::canonicalize(parent).map_err(|error| io_error(error, &params.path))?;
        (
            open_directory(&parent).map_err(|error| io_error(error, &params.path))?,
            c_name(target.file_name().ok_or_else(denied)?)?,
        )
    };
    lock_parent(&parent).map_err(|error| io_error(error, &params.path))?;
    let current = metadata_at(&parent, &name).map_err(|error| scoped_error(error, &params.path))?;
    check_expected(&params, current.as_ref())?;
    let temporary = CString::new(format!(
        ".lithe-{}-{}.tmp",
        std::process::id(),
        TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
    .expect("temporary name has no NUL");
    let mut file = create_temporary(&parent, &temporary, current.as_ref())
        .map_err(|error| io_error(error, &params.path))?;
    let cleanup = Temporary {
        parent: &parent,
        name: &temporary,
    };
    file.write_all(&data)
        .and_then(|()| {
            if let Some(metadata) = &current {
                // SAFETY: file owns a live fd; mode_t retains each host's native width.
                if unsafe { libc::fchmod(file.as_raw_fd(), metadata.mode & 0o7777) } != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            file.sync_all()
        })
        .map_err(|error| io_error(error, &params.path))?;
    before_commit();
    let latest = metadata_at(&parent, &name).map_err(|error| scoped_error(error, &params.path))?;
    check_expected(&params, latest.as_ref())?;
    if current.as_ref().map(|stamp| &stamp.revision) != latest.as_ref().map(|stamp| &stamp.revision)
    {
        return Err(conflict());
    }
    let create_only = params.expected.is_none() && !params.overwrite;
    if create_only {
        // A hard-link publication is atomic no-replace on both host platforms.
        // Both names refer to the same synchronized inode until cleanup unlinks
        // the temporary name; no check-then-rename fallback can overwrite it.
        // SAFETY: parent owns a live directory fd and both C strings outlive the call.
        let status = unsafe {
            libc::linkat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                parent.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        };
        if status != 0 {
            return Err(io_error(io::Error::last_os_error(), &params.path));
        }
    } else {
        // SAFETY: both names are single components relative to the live parent fd.
        let status = unsafe {
            libc::renameat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                parent.as_raw_fd(),
                name.as_ptr(),
            )
        };
        if status != 0 {
            return Err(io_error(io::Error::last_os_error(), &params.path));
        }
    }
    drop(cleanup);
    let saved = file
        .metadata()
        .map_err(|error| io_error(error, &params.path))?;
    Ok(describe(&requested, &saved))
}

fn check_expected(params: &WriteFileParams, found: Option<&Stamp>) -> Result<(), RemoteError> {
    if found.is_some_and(|metadata| metadata.mode & libc::S_IFMT == libc::S_IFDIR) {
        return Err(RemoteError::new(
            ErrorCode::IsADirectory,
            "cannot replace a directory",
        ));
    }
    match (&params.expected, found) {
        (Some(expected), Some(metadata)) if matches_version(expected, metadata) => Ok(()),
        (Some(_), _) => Err(conflict()),
        (None, Some(_)) if !params.overwrite => Err(RemoteError::new(
            ErrorCode::AlreadyExists,
            "the file already exists",
        )),
        _ => Ok(()),
    }
}

fn matches_version(expected: &FileVersion, observed: &Stamp) -> bool {
    expected.size == observed.size
        && expected.modified_ms == observed.modified_ms
        && expected
            .revision
            .as_ref()
            .is_none_or(|expected| expected == &observed.revision)
}

fn conflict() -> RemoteError {
    RemoteError::new(ErrorCode::Conflict, "the file changed since it was read")
}
fn denied() -> RemoteError {
    RemoteError::new(
        ErrorCode::PermissionDenied,
        "the save path crosses the workspace boundary or a symbolic link",
    )
}
fn scoped_error(error: io::Error, path: &str) -> RemoteError {
    if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) {
        denied()
    } else {
        io_error(error, path)
    }
}

fn c_name(name: &OsStr) -> Result<CString, RemoteError> {
    CString::new(name.as_bytes()).map_err(|_| denied())
}

fn open_directory(path: &Path) -> io::Result<File> {
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                let name = CString::new(name.as_bytes()).map_err(io::Error::other)?;
                directory = open_at(
                    &directory,
                    &name,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                    0,
                )?;
            }
            _ => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
        }
    }
    Ok(directory)
}

fn open_at(parent: &File, name: &CStr, flags: i32, mode: libc::mode_t) -> io::Result<File> {
    // SAFETY: name is NUL-terminated, parent remains open, and a successful fd
    // is transferred exactly once into File. CLOEXEC prevents child inheritance.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned fd.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Metadata read without requiring read access or opening a FIFO/device.
struct Stamp {
    size: u64,
    modified_ms: Option<i64>,
    revision: String,
    mode: libc::mode_t,
}

fn metadata_at(parent: &File, name: &CStr) -> io::Result<Option<Stamp>> {
    let mut raw = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: raw is writable storage for a stat, name is NUL-terminated, and
    // parent stays live. AT_SYMLINK_NOFOLLOW observes the entry itself.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            raw.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error)
        };
    }
    // SAFETY: fstatat initialized raw after returning success.
    let raw = unsafe { raw.assume_init() };
    if raw.st_mode & libc::S_IFMT == libc::S_IFLNK {
        return Err(io::Error::from_raw_os_error(libc::ELOOP));
    }
    Ok(Some(Stamp {
        size: raw.st_size as u64,
        modified_ms: raw
            .st_mtime
            .checked_mul(1000)
            .and_then(|seconds| seconds.checked_add(raw.st_mtime_nsec / 1_000_000))
            .filter(|milliseconds| *milliseconds >= 0),
        revision: format!(
            "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
            raw.st_dev,
            raw.st_ino,
            raw.st_size,
            raw.st_mtime,
            raw.st_mtime_nsec,
            raw.st_ctime,
            raw.st_ctime_nsec
        ),
        mode: raw.st_mode,
    }))
}

fn create_temporary(parent: &File, name: &CStr, replaced: Option<&Stamp>) -> io::Result<File> {
    let mode = replaced.map_or(0o666, |metadata| metadata.mode & 0o777);
    open_at(
        parent,
        name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
        mode,
    )
}

fn lock_parent(parent: &File) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: flock operates on the live directory fd. Closing it releases
        // the lock, including on every error and unwinding path.
        if unsafe { libc::flock(parent.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock || Instant::now() >= deadline {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Removes unpublished data, or the extra link of a newly published file.
struct Temporary<'a> {
    parent: &'a File,
    name: &'a CStr,
}
impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        // SAFETY: both borrowed values outlive this guard; unlinkat follows no links.
        if unsafe { libc::unlinkat(self.parent.as_raw_fd(), self.name.as_ptr(), 0) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!(
                    "lithe-remote-server: temporary save cleanup failed: {:?}",
                    error.kind()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::methods::encode_bytes;
    use serde_json::json;

    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("test directory cleanup failed: {error}");
            }
        }
    }

    #[test]
    fn save_rechecks_changes_after_staging_and_reclaims_its_temporary() {
        let directory = std::env::temp_dir().join(format!(
            "lithe-save-commit-{}-{}",
            std::process::id(),
            TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let _cleanup = RemoveOnDrop(directory.clone());
        let path = directory.join("file");
        fs::write(&path, b"old").unwrap();
        let observed = describe(&path, &fs::metadata(&path).unwrap());
        let params = serde_json::from_value(json!({"path": path, "data": encode_bytes(b"mine"),
            "expected": {"size": observed.size, "modifiedMs": observed.modified_ms, "revision": observed.revision}})).unwrap();
        let result = write_before_commit(params, || {
            fs::write(&path, b"the external writer wins").unwrap();
        });
        assert_eq!(result.unwrap_err().code, ErrorCode::Conflict);
        assert_eq!(fs::read(&path).unwrap(), b"the external writer wins");
        assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
    }

    #[test]
    fn temporary_file_for_owner_only_content_is_private_at_creation() {
        let directory = std::env::temp_dir().join(format!(
            "lithe-save-mode-{}-{}",
            std::process::id(),
            TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let _cleanup = RemoveOnDrop(directory.clone());
        let parent = open_directory(&fs::canonicalize(&directory).unwrap()).unwrap();
        let stamp = Stamp {
            size: 0,
            modified_ms: None,
            revision: String::new(),
            mode: 0o600,
        };
        let file = create_temporary(&parent, c"temporary", Some(&stamp)).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o600);
    }
}
