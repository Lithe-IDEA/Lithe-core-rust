//! The accounts on the two ends of a Unix socket.
//!
//! A process that hands out secrets or a session over a Unix socket checks the
//! account on the other end before it reads or writes a byte, so the
//! permissions of the socket's directory are a second layer rather than the
//! only one. Linux reports the peer through `SO_PEERCRED`, macOS and the BSDs
//! through `getpeereid`; both report the effective user id the peer had when
//! it connected or started listening, so a peer that changes its ids later
//! cannot change the answer.

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

/// Effective user id of this process: the account that owns the files,
/// directories and sockets it creates, and the one peers are compared with.
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Effective user id of the process on the other end of `socket`.
///
/// For a stream accepted from a listener this is the account that connected;
/// for a client's stream it is the account that created the listening socket.
/// Callers that cannot read it must treat the peer as another account.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn peer_uid(socket: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the descriptor is borrowed from a live stream, and the
    // out-pointers reference live locals of the declared size.
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if status == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

/// Effective user id of the process on the other end of `socket`.
///
/// For a stream accepted from a listener this is the account that connected;
/// for a client's stream it is the account that created the listening socket.
/// Callers that cannot read it must treat the peer as another account.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn peer_uid(socket: &UnixStream) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: the descriptor is borrowed from a live stream, and the
    // out-pointers reference live locals.
    if unsafe { libc::getpeereid(socket.as_raw_fd(), &mut uid, &mut gid) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}
