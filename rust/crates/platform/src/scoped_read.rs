//! Handle-relative reads that never traverse a symbolic link or Windows reparse point.
//!
//! The caller validates relative components. Unix resolves them with openat and
//! O_NOFOLLOW; Windows holds each ancestor without delete sharing while opening
//! the next component. Handles belong to a single request and close on drop.

use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// The entry itself, without following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link or other Windows reparse point.
    Symlink,
    /// A device, socket, pipe or another non-regular entry.
    Other,
}

/// Portable metadata obtained without following the leaf.
#[derive(Clone, Copy, Debug)]
pub struct EntryMetadata {
    /// The entry's own type.
    pub kind: Kind,
    /// The native byte length.
    pub size: u64,
    /// Milliseconds since the Unix epoch, absent when unavailable or earlier.
    pub modified_milliseconds: Option<u64>,
}

fn metadata(value: Metadata) -> EntryMetadata {
    let kind = if is_link(&value) {
        Kind::Symlink
    } else if value.is_dir() {
        Kind::Directory
    } else if value.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    EntryMetadata {
        kind,
        size: value.len(),
        modified_milliseconds: value
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|time| u64::try_from(time.as_millis()).ok()),
    }
}

fn is_link(value: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        value.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        value.file_type().is_symlink()
    }
}

fn link_refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Reading through a symbolic link or reparse point is forbidden",
    )
}

/// An opened directory anchoring all descendant lookups.
pub struct Directory {
    handle: File,
    #[cfg(windows)]
    path: std::path::PathBuf,
    #[cfg(windows)]
    ancestors: Vec<File>,
}

/// An opened regular file, with Windows ancestor replacement prevented until drop.
pub struct ScopedFile {
    handle: File,
    #[cfg(windows)]
    _ancestors: Vec<File>,
}

impl Read for ScopedFile {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.handle.read(bytes)
    }
}

impl Directory {
    /// Opens an authorized absolute root. The root alias may be canonicalized;
    /// no component supplied by the plugin is canonicalized or followed.
    pub fn open_root(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let handle = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(root)?;
            Ok(Self { handle })
        }
        #[cfg(windows)]
        {
            let mut path = std::path::PathBuf::new();
            let mut ancestors = Vec::new();
            for component in root.components() {
                path.push(component);
                if matches!(component, std::path::Component::Prefix(_)) {
                    continue;
                }
                let handle = windows_open(&path, false)?;
                let info = metadata(handle.metadata()?);
                if info.kind == Kind::Symlink {
                    return Err(link_refused());
                }
                if info.kind != Kind::Directory {
                    return Err(io::ErrorKind::NotADirectory.into());
                }
                ancestors.push(handle);
            }
            let handle = ancestors.pop().ok_or(io::ErrorKind::InvalidInput)?;
            Ok(Self {
                handle,
                path,
                ancestors,
            })
        }
    }

    /// Opens a single directory component without following it.
    pub fn directory(&self, name: &OsStr) -> io::Result<Self> {
        validate_component(name)?;
        #[cfg(unix)]
        {
            Ok(Self {
                handle: unix_open(&self.handle, name, libc::O_DIRECTORY)?,
            })
        }
        #[cfg(windows)]
        {
            let path = self.path.join(name);
            let handle = windows_open(&path, false)?;
            let info = metadata(handle.metadata()?);
            if info.kind == Kind::Symlink {
                return Err(link_refused());
            }
            if info.kind != Kind::Directory {
                return Err(io::ErrorKind::NotADirectory.into());
            }
            Ok(Self {
                handle,
                path,
                ancestors: self.retained_handles()?,
            })
        }
    }

    /// Opens a single regular file without following it or waiting on a FIFO.
    pub fn file(&self, name: &OsStr) -> io::Result<ScopedFile> {
        validate_component(name)?;
        #[cfg(unix)]
        let handle = unix_open(&self.handle, name, libc::O_NONBLOCK)?;
        #[cfg(windows)]
        let handle = windows_open(&self.path.join(name), true)?;
        let info = metadata(handle.metadata()?);
        if info.kind == Kind::Symlink {
            return Err(link_refused());
        }
        if info.kind != Kind::File {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The entry is not a regular file",
            ));
        }
        Ok(ScopedFile {
            handle,
            #[cfg(windows)]
            _ancestors: self.retained_handles()?,
        })
    }

    /// Metadata for this directory's held handle.
    pub fn metadata(&self) -> io::Result<EntryMetadata> {
        self.handle.metadata().map(metadata)
    }

    /// Metadata for one child, including the link itself when it is a link.
    pub fn child_metadata(&self, name: &OsStr) -> io::Result<EntryMetadata> {
        validate_component(name)?;
        #[cfg(unix)]
        {
            unix_metadata(&self.handle, name)
        }
        #[cfg(windows)]
        {
            windows_open(&self.path.join(name), false)?
                .metadata()
                .map(metadata)
        }
    }

    /// Visits children using constant enumeration memory. The callback can stop
    /// with an error, including the request's cancellation or deadline.
    pub fn visit(
        &self,
        mut visitor: impl FnMut(&OsStr, EntryMetadata) -> io::Result<()>,
    ) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::ffi::CStr;
            use std::os::fd::IntoRawFd;
            use std::os::unix::ffi::OsStrExt;
            let handle = unix_open(&self.handle, OsStr::new("."), libc::O_DIRECTORY)?;
            let fd = handle.into_raw_fd();
            // SAFETY: fd is newly owned; fdopendir takes ownership only on success.
            let stream = unsafe { libc::fdopendir(fd) };
            if stream.is_null() {
                let error = io::Error::last_os_error();
                // SAFETY: fdopendir failed, so fd is still ours.
                unsafe {
                    libc::close(fd);
                }
                return Err(error);
            }
            struct Stream(*mut libc::DIR);
            impl Drop for Stream {
                fn drop(&mut self) {
                    // SAFETY: the stream is owned by this guard and closed once.
                    unsafe {
                        libc::closedir(self.0);
                    }
                }
            }
            let stream = Stream(stream);
            loop {
                // SAFETY: errno is thread-local and stream remains live throughout this loop.
                unsafe {
                    *errno() = 0;
                }
                // SAFETY: readdir's pointer stays valid until the next call on this stream.
                let entry = unsafe { libc::readdir(stream.0) };
                if entry.is_null() {
                    let error = io::Error::last_os_error();
                    return if error.raw_os_error() == Some(0) {
                        Ok(())
                    } else {
                        Err(error)
                    };
                }
                // SAFETY: a successful readdir supplies a NUL-terminated d_name.
                let name = OsStr::from_bytes(
                    unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes(),
                );
                if name == "." || name == ".." {
                    continue;
                }
                match unix_metadata(&self.handle, name) {
                    Ok(info) => visitor(name, info)?,
                    // A concurrently removed entry contributes no result.
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        #[cfg(windows)]
        {
            for entry in std::fs::read_dir(&self.path)? {
                let entry = entry?;
                visitor(&entry.file_name(), metadata(entry.metadata()?))?;
            }
            Ok(())
        }
    }

    #[cfg(windows)]
    fn retained_handles(&self) -> io::Result<Vec<File>> {
        self.ancestors
            .iter()
            .chain(std::iter::once(&self.handle))
            .map(File::try_clone)
            .collect()
    }
}

fn validate_component(name: &OsStr) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(())
}

#[cfg(unix)]
fn unix_open(parent: &File, name: &OsStr, flags: i32) -> io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: parent is live, name is NUL-terminated, and no creation flag needs a mode.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | flags,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        // Some hosts report ENOTDIR for O_DIRECTORY|O_NOFOLLOW on a link.
        if error.raw_os_error() == Some(libc::ELOOP)
            || (error.kind() == io::ErrorKind::NotADirectory
                && unix_metadata(parent, OsStr::from_bytes(name.as_bytes()))?.kind == Kind::Symlink)
        {
            return Err(link_refused());
        }
        return Err(error);
    }
    // SAFETY: openat returned a fresh descriptor transferred exactly once.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn unix_metadata(parent: &File, name: &OsStr) -> io::Result<EntryMetadata> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstatat writes info on success; the parent and C string remain live.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            info.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat succeeded and initialized every field.
    let info = unsafe { info.assume_init() };
    let kind = match info.st_mode & libc::S_IFMT {
        libc::S_IFREG => Kind::File,
        libc::S_IFDIR => Kind::Directory,
        libc::S_IFLNK => Kind::Symlink,
        _ => Kind::Other,
    };
    Ok(EntryMetadata {
        kind,
        size: u64::try_from(info.st_size).unwrap_or(0),
        modified_milliseconds: u64::try_from(info.st_mtime)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
            .and_then(|ms| ms.checked_add((info.st_mtime_nsec / 1_000_000) as u64)),
    })
}

#[cfg(target_os = "macos")]
unsafe fn errno() -> *mut i32 {
    // SAFETY: the OS returns the current thread's errno storage.
    unsafe { libc::__error() }
}

#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn errno() -> *mut i32 {
    // SAFETY: the OS returns the current thread's errno storage.
    unsafe { libc::__errno_location() }
}

#[cfg(windows)]
fn windows_open(path: &Path, read: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut options = std::fs::OpenOptions::new();
    if read {
        options.read(true);
    } else {
        options.access_mode(FILE_READ_ATTRIBUTES);
    }
    options
        .share_mode(if read {
            FILE_SHARE_READ | FILE_SHARE_WRITE
        } else {
            FILE_SHARE_READ
        })
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}
