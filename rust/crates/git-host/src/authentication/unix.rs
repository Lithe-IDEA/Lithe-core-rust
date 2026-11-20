//! Unix AskPass channel: a socket inside a fresh owner-only directory.
//!
//! Both ends check the account on the other side of the socket, so the
//! directory permissions are a second layer rather than the only one. A
//! process of another account that swaps or guesses the path can at most
//! make the prompt fail; it never receives the session token or an answer.

use super::{random_hex, Accept, PROMPT_TIMEOUT, WRITE_TIMEOUT};
use crate::failure_log;
use lithe_platform::fs::{
    restrict_directory_to_owner, OWNER_ONLY_DIRECTORY_MODE, OWNER_ONLY_FILE_MODE,
};
use lithe_platform::socket::{effective_uid, peer_uid};
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// The socket's file name inside the private directory.
const SOCKET_NAME: &str = "askpass.sock";
/// `sun_path` holds 104 bytes on macOS and 108 on Linux, terminator included;
/// longer candidate paths are skipped rather than truncated.
const MAX_SOCKET_PATH: usize = 100;
/// Name collisions are retried with a fresh random suffix this many times.
const CREATE_ATTEMPTS: usize = 4;

/// The server end the session reads requests from and writes answers to.
pub(super) type Stream = UnixStream;

/// Listening socket for one session. Dropping it closes the socket, then
/// removes the socket file and the private directory.
pub(super) struct Listener {
    listener: UnixListener,
    socket: Entry,
    _directory: Entry,
}

impl Listener {
    /// Binds a socket in the first usable temporary directory.
    pub(super) fn bind() -> io::Result<Self> {
        let mut last = io::Error::new(
            io::ErrorKind::NotFound,
            "No temporary directory can hold the AskPass socket",
        );
        for parent in candidates() {
            match bind_in(&parent) {
                Ok(listener) => return Ok(listener),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    /// The socket path the helper connects to.
    pub(super) fn address(&self) -> &OsStr {
        self.socket.path.as_os_str()
    }

    /// Takes one waiting connection without blocking.
    pub(super) fn accept(&mut self) -> io::Result<Accept<Stream>> {
        let stream = match self.listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(Accept::Empty),
            Err(error) => return Err(error),
        };
        // A peer whose account cannot be read is treated like another account.
        if peer_uid(&stream).ok() != Some(effective_uid()) {
            return Ok(Accept::Rejected("peer-identity"));
        }
        // Linux does not carry O_NONBLOCK from the listener to accepted sockets.
        stream.set_nonblocking(true)?;
        Ok(Accept::Peer(stream))
    }
}

/// Opens the helper's end of the channel and checks that the session's own
/// account owns the socket before any byte is sent.
pub(super) fn connect(address: &OsStr) -> io::Result<UnixStream> {
    let path = Path::new(address);
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let stream = UnixStream::connect(path)?;
    if peer_uid(&stream)? != effective_uid() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    // The session's own prompt and write deadlines end first and close the
    // socket; this timeout only covers a session that stopped polling.
    stream.set_read_timeout(Some(PROMPT_TIMEOUT + WRITE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(stream)
}

/// A path the listener created and removes when dropped.
struct Entry {
    path: PathBuf,
    stage: &'static str,
    remove: fn(&Path) -> io::Result<()>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        failure_log::cleanup(
            "vcs.host.cleanup_failed",
            self.stage,
            (self.remove)(&self.path),
        );
    }
}

/// Per-user runtime directory first, then the temporary directory. Relative
/// values are ignored because they would depend on the working directory.
fn candidates() -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .into_iter()
        .chain([std::env::temp_dir(), PathBuf::from("/tmp")])
        .filter(|path| path.is_absolute())
        .collect();
    list.dedup();
    list
}

fn bind_in(parent: &Path) -> io::Result<Listener> {
    let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
    for _ in 0..CREATE_ATTEMPTS {
        let directory = parent.join(format!("lithe-askpass-{}", random_hex::<8>()?));
        let socket = directory.join(SOCKET_NAME);
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The AskPass socket path is too long",
            ));
        }
        match DirBuilder::new()
            .mode(OWNER_ONLY_DIRECTORY_MODE)
            .create(&directory)
        {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last = error;
                continue;
            }
            Err(error) => return Err(error),
        }
        // Created by this call, so removal on every later failure is safe.
        let directory = Entry {
            path: directory,
            stage: "askpass_directory",
            remove: |path| fs::remove_dir(path),
        };
        verify_private(&directory.path)?;
        let listener = UnixListener::bind(&socket)?;
        let socket = Entry {
            path: socket,
            stage: "askpass_socket",
            remove: |path| fs::remove_file(path),
        };
        fs::set_permissions(&socket.path, Permissions::from_mode(OWNER_ONLY_FILE_MODE))?;
        listener.set_nonblocking(true)?;
        return Ok(Listener {
            listener,
            socket,
            _directory: directory,
        });
    }
    Err(last)
}

/// The umask can clear owner bits and a racing process can replace the entry,
/// so the directory is restricted explicitly and checked without following
/// links.
fn verify_private(directory: &Path) -> io::Result<()> {
    restrict_directory_to_owner(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    if metadata.is_dir() && metadata.uid() == effective_uid() && metadata.mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}
