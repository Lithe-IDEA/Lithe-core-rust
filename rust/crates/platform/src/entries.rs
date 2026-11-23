//! File-system changes whose system calls differ between hosts.
//!
//! `std::fs::rename` replaces an existing target on every host, and std
//! creates a symbolic link through a different function on Unix and Windows.
//! Callers that must never overwrite a file, or that create a link a user
//! asked for, call these instead of repeating a `cfg` branch.

use std::io;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

/// Renames `from` to `to`, failing with [`io::ErrorKind::AlreadyExists`]
/// instead of replacing an entry already at `to`.
///
/// macOS and Linux make the check and the rename one system call. On volumes
/// that refuse that call, such as SMB or FAT mounts and Linux kernels before
/// 3.15, the check and the rename are separate calls, so an entry created at
/// `to` between them can still be replaced. Windows never replaces without a
/// flag asking for it.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    host_rename_no_replace(from, to)
}

#[cfg(target_os = "macos")]
fn host_rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let from_c = c_path(from)?;
    let to_c = c_path(to)?;
    // SAFETY: both pointers reference NUL-terminated strings that outlive the
    // call.
    let status = unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        // Volumes such as SMB or FAT mounts may not support RENAME_EXCL.
        Some(libc::ENOTSUP) | Some(libc::EINVAL) => rename_if_absent(from, to),
        _ => Err(error),
    }
}

/// `renameat2` is called through `syscall` because musl and older glibc
/// builds may lack the wrapper.
#[cfg(target_os = "linux")]
fn host_rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let from_c = c_path(from)?;
    let to_c = c_path(to)?;
    // SAFETY: both pointers reference NUL-terminated strings that outlive the
    // call; the remaining arguments are plain integers.
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        // Kernels before 3.15 lack the call; some filesystems reject the flag.
        Some(libc::ENOSYS) | Some(libc::EINVAL) => rename_if_absent(from, to),
        _ => Err(error),
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn host_rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    rename_if_absent(from, to)
}

/// `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` fails when `to`
/// exists, and without `MOVEFILE_COPY_ALLOWED` never copies across volumes.
#[cfg(windows)]
fn host_rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let from_w = wide_path(from);
    let to_w = wide_path(to);
    // SAFETY: both pointers reference NUL-terminated UTF-16 strings that
    // outlive the call.
    if unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The check and the rename are separate calls, so an entry created at `to`
/// between them can still be replaced.
#[cfg(unix)]
fn rename_if_absent(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(to) {
        Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::fs::rename(from, to),
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn c_path(path: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Creates a symbolic link at `link` that stores `target` as given: absolute,
/// or relative to the directory that holds `link`. The target need not exist.
/// An entry already at `link` fails with [`io::ErrorKind::AlreadyExists`].
///
/// Windows stores a file link and a directory link differently and follows
/// a relative target only through `\`. There the link is a directory link
/// when `target` names an existing directory at creation, otherwise a file
/// link, and each `/` in `target` is stored as `\`. Windows refuses to create
/// a link unless Developer Mode is on or the process runs elevated; that
/// refusal is returned as the system reports it.
pub fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        let target = windows_link_target(target);
        let resolved = match link.parent() {
            Some(parent) if target.is_relative() => parent.join(&target),
            _ => target.clone(),
        };
        if std::fs::metadata(resolved).is_ok_and(|metadata| metadata.is_dir()) {
            std::os::windows::fs::symlink_dir(&target, link)
        } else {
            std::os::windows::fs::symlink_file(&target, link)
        }
    }
}

#[cfg(windows)]
fn windows_link_target(target: &Path) -> PathBuf {
    match target.to_str() {
        Some(text) => PathBuf::from(text.replace('/', "\\")),
        None => target.to_path_buf(),
    }
}
