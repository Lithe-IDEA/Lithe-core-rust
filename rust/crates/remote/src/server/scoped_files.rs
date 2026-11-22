//! Descriptor-relative file reads for plugin workspace resources.
//!
//! Only the root is opened by its absolute path. Every descendant uses the
//! descriptor of its parent and disables symlink following in the syscall.
//! A separate realpath check would leave a gap before the actual read.

use crate::protocol::methods::{
    encode_bytes, FileKind, ScopedDirectoryEntry, ScopedFileAccess, ScopedFileParams,
    ScopedFileRead, ScopedFileStat, ScopedWorkingDirectory, MAX_SCOPED_CONFIGURATION_BYTES,
    MAX_SCOPED_READ_BYTES, MAX_TRANSFER_BYTES,
};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::{fs, platform, to_value};
use serde_json::Value;
use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::ptr::NonNull;

const DIRECTORY_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY;
const MAX_DIRECTORY_ENTRIES: usize = 20_000;
// Even worst-case JSON escaping leaves room for metadata in a 16 MiB frame.
const MAX_DIRECTORY_NAME_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn handle(params: ScopedFileParams) -> Result<Value, RemoteError> {
    let path = components(&params.path)?;
    let budget = match params.access {
        ScopedFileAccess::Read { length } => Some((length, MAX_SCOPED_READ_BYTES)),
        ScopedFileAccess::ReadConfiguration { length } => {
            Some((length, MAX_SCOPED_CONFIGURATION_BYTES))
        }
        ScopedFileAccess::ReadDocument { length } => Some((length, MAX_TRANSFER_BYTES as usize)),
        _ => None,
    };
    if let Some((length, limit)) = budget {
        if !(1..=limit).contains(&length) {
            return Err(RemoteError::new(
                ErrorCode::TooLarge,
                format!("A scoped read must request between 1 and {limit} bytes"),
            ));
        }
    }
    let root = fs::host_path(&params.root)?;
    // The trusted workspace selector may point at a symlink. The opened
    // directory becomes the boundary, even if that root name later changes.
    let result = (|| -> io::Result<_> {
        let root = open_root(&root)?;
        match params.access {
            ScopedFileAccess::List => Ok(to_value(list(directory(root, &path)?)?)),
            ScopedFileAccess::Stat => {
                let status = match path.split_last() {
                    Some((name, parents)) => {
                        status(directory(root, parents)?.as_raw_fd(), Some(name))?
                    }
                    None => status(root.as_raw_fd(), None)?,
                };
                Ok(to_value(ScopedFileStat {
                    kind: kind(&status),
                    size: u64::try_from(status.st_size).unwrap_or_default(),
                    modified_milliseconds: status
                        .st_mtime
                        .saturating_mul(1000)
                        .saturating_add(status.st_mtime_nsec / 1_000_000),
                }))
            }
            ScopedFileAccess::Read { length }
            | ScopedFileAccess::ReadConfiguration { length }
            | ScopedFileAccess::ReadDocument { length } => Ok(to_value(read(root, &path, length)?)),
        }
    })();
    match result {
        Err(error) if error.kind() == io::ErrorKind::FileTooLarge => {
            Err(RemoteError::new(ErrorCode::TooLarge, error.to_string()))
        }
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => Err(RemoteError::new(
            ErrorCode::PermissionDenied,
            "Scoped reads cannot follow symbolic links",
        )),
        Err(error) => Err(fs::io_error(error, &params.path)),
        Ok(value) => value,
    }
}

pub(super) fn open_root(root: &std::path::Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(root)
}

/// Makes the child enter the opened directory without resolving its path again.
pub(crate) fn set_working_directory(
    command: &mut Command,
    scoped: &ScopedWorkingDirectory,
) -> Result<(), RemoteError> {
    let (_, directory) = open_working_directory(scoped)?;
    // SAFETY: the closure owns the descriptor through fork. fchdir is
    // async-signal-safe and performs no path lookup in the child.
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(directory.as_raw_fd()) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

/// Checks that every working directory component can be opened without following a symlink.
pub(super) fn open_working_directory(
    scoped: &ScopedWorkingDirectory,
) -> Result<(std::path::PathBuf, File), RemoteError> {
    let relative = if scoped.path == "." { "" } else { &scoped.path };
    let components = components(relative)?;
    let root = fs::host_path(&scoped.root)?;
    let directory = open_root(&root)
        .and_then(|root| directory(root, &components))
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                RemoteError::new(
                    ErrorCode::PermissionDenied,
                    "Scoped working directories cannot follow symbolic links",
                )
            } else {
                fs::io_error(error, &scoped.path)
            }
        })?;
    Ok((root.join(relative), directory))
}

pub(super) fn components(path: &str) -> Result<Vec<CString>, RemoteError> {
    let invalid = || RemoteError::new(ErrorCode::InvalidPath, "Expected a workspace-relative path");
    if path.starts_with('/') || path.contains('\\') {
        return Err(invalid());
    }
    path.split('/')
        .filter(|name| !name.is_empty())
        .map(|name| {
            if name == "." || name == ".." {
                return Err(invalid());
            }
            CString::new(name).map_err(|_| invalid())
        })
        .collect()
}

pub(super) fn open_at(parent: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<File> {
    // SAFETY: name is a live NUL-terminated string. The directory descriptor
    // remains open for the call. No creation flag or mode argument is used.
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a fresh, uniquely owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn directory(mut directory: File, path: &[CString]) -> io::Result<File> {
    for name in path {
        let parent = directory.as_raw_fd();
        directory = open_at(parent, name, DIRECTORY_FLAGS).map_err(|error| {
            // Linux can report ENOTDIR for O_DIRECTORY | O_NOFOLLOW on a link.
            // The follow-up stat only classifies the refusal; it cannot allow an open.
            if error.kind() == io::ErrorKind::NotADirectory
                && status(parent, Some(name)).is_ok_and(|entry| kind(&entry) == FileKind::Symlink)
            {
                io::Error::from_raw_os_error(libc::ELOOP)
            } else {
                error
            }
        })?;
    }
    Ok(directory)
}

pub(super) fn open_directory_at(parent: &File, name: &CStr) -> io::Result<File> {
    open_at(parent.as_raw_fd(), name, DIRECTORY_FLAGS)
}

/// Opens each component beneath an already opened watch root. Host event names
/// can be non-UTF-8, so preserve their original bytes during lookup.
#[cfg(target_os = "linux")]
pub(super) fn watch_directory(root: &File, relative: &std::path::Path) -> io::Result<File> {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    let mut directory = root.try_clone()?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let name = CString::new(name.as_bytes())?;
        directory = open_directory_at(&directory, &name)?;
    }
    Ok(directory)
}

pub(super) fn status(parent: RawFd, name: Option<&CStr>) -> io::Result<libc::stat> {
    let mut result = MaybeUninit::uninit();
    // SAFETY: parent is open for the call, each name is NUL-terminated and
    // result has writable storage for struct stat.
    let rc = unsafe {
        match name {
            Some(name) => libc::fstatat(
                parent,
                name.as_ptr(),
                result.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            ),
            None => libc::fstat(parent, result.as_mut_ptr()),
        }
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success, fstat or fstatat initialized the result.
    Ok(unsafe { result.assume_init() })
}

fn kind(status: &libc::stat) -> FileKind {
    match status.st_mode & libc::S_IFMT {
        libc::S_IFREG => FileKind::File,
        libc::S_IFDIR => FileKind::Directory,
        libc::S_IFLNK => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

fn read(root: File, path: &[CString], length: usize) -> io::Result<ScopedFileRead> {
    let Some((name, parents)) = path.split_last() else {
        return Err(io::ErrorKind::IsADirectory.into());
    };
    let directory = directory(root, parents)?;
    let (bytes, is_truncated) = read_regular_at(&directory, name, length)?;
    Ok(ScopedFileRead {
        data: encode_bytes(&bytes),
        is_truncated,
    })
}

pub(super) fn read_regular_at(
    directory: &File,
    name: &CStr,
    length: usize,
) -> io::Result<(Vec<u8>, bool)> {
    let expected = status(directory.as_raw_fd(), Some(name))?;
    require_regular(&expected)?;
    // Nonblocking also prevents a concurrently substituted FIFO from
    // hanging the worker between the first stat and the descriptor check.
    let file = open_at(
        directory.as_raw_fd(),
        name,
        libc::O_RDONLY | libc::O_NONBLOCK,
    )?;
    let actual = status(file.as_raw_fd(), None)?;
    require_regular(&actual)?;
    if expected.st_dev != actual.st_dev || expected.st_ino != actual.st_ino {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "The file changed while it was being opened",
        ));
    }
    let capacity = usize::try_from(expected.st_size)
        .unwrap_or_default()
        .min(length);
    let mut bytes = Vec::with_capacity(capacity);
    file.take((length + 1) as u64).read_to_end(&mut bytes)?;
    let is_truncated = bytes.len() > length;
    bytes.truncate(length);
    Ok((bytes, is_truncated))
}

fn require_regular(status: &libc::stat) -> io::Result<()> {
    match kind(status) {
        FileKind::File => Ok(()),
        FileKind::Directory => Err(io::ErrorKind::IsADirectory.into()),
        _ => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Scoped reads require a regular file",
        )),
    }
}

struct DirectoryStream(NonNull<libc::DIR>);

impl DirectoryStream {
    fn open(directory: File) -> io::Result<Self> {
        // SAFETY: the descriptor is open and exclusively owned by this file.
        let stream = unsafe { libc::fdopendir(directory.as_raw_fd()) };
        let stream = NonNull::new(stream).ok_or_else(io::Error::last_os_error)?;
        // fdopendir takes ownership only on success, and closedir closes it.
        let _transferred = directory.into_raw_fd();
        Ok(Self(stream))
    }

    fn next(&mut self) -> io::Result<Option<&libc::dirent>> {
        // SAFETY: the stream and this thread's errno pointer are valid.
        // readdir's storage remains valid until the next mutable call or drop.
        unsafe {
            *platform::errno_location() = 0;
            let entry = libc::readdir(self.0.as_ptr());
            if entry.is_null() && *platform::errno_location() != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(entry.as_ref())
        }
    }
}

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: this is the one close of our live stream.
        // log-exempt: cleanupBestEffort — a closed stream cannot be retried.
        unsafe { libc::closedir(self.0.as_ptr()) };
    }
}

pub(super) fn list(directory: File) -> io::Result<Vec<ScopedDirectoryEntry>> {
    list_with_budget(directory, |_| true)
}

/// Charges every enumerated name, including ones the listing cannot represent.
pub(super) fn list_with_budget(
    directory: File,
    mut charge: impl FnMut(usize) -> bool,
) -> io::Result<Vec<ScopedDirectoryEntry>> {
    let fd = directory.as_raw_fd();
    let mut stream = DirectoryStream::open(directory)?;
    let mut entries = Vec::new();
    let mut scanned = 0;
    let mut name_bytes = 0;
    while let Some(entry) = stream.next()? {
        // SAFETY: readdir supplies a NUL-terminated name valid for this borrow.
        let raw = unsafe { CStr::from_ptr(entry.d_name.as_ptr()) };
        if matches!(raw.to_bytes(), b"." | b"..") {
            continue;
        }
        scanned += 1;
        name_bytes += raw.to_bytes().len();
        if !charge(raw.to_bytes().len())
            || scanned > MAX_DIRECTORY_ENTRIES
            || name_bytes > MAX_DIRECTORY_NAME_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "The scoped directory listing exceeds its scan budget",
            ));
        }
        let Ok(name) = raw.to_str() else { continue };
        if name.contains('\\') {
            continue;
        }
        // A known d_type avoids a syscall. On filesystems without type hints,
        // fstatat can identify the entry without opening it or following links.
        let kind = match entry.d_type {
            libc::DT_REG => FileKind::File,
            libc::DT_DIR => FileKind::Directory,
            libc::DT_UNKNOWN => match status(fd, Some(raw)) {
                Ok(metadata) => kind(&metadata),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            },
            _ => continue,
        };
        if matches!(kind, FileKind::File | FileKind::Directory) {
            entries.push(ScopedDirectoryEntry {
                name: name.into(),
                kind,
            });
        }
    }
    entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}
