//! Persistent sessions that survive a dropped connection.
//!
//! The transport runs `lithe-remote-server --connect <key>`, a small proxy.
//! It connects to the session daemon for `<key>` over a Unix socket, starting
//! the daemon first if none runs, and then copies bytes between its standard
//! streams and the socket. When SSH drops, only the proxy dies: the daemon
//! keeps terminals and builds running, buffers their events, and hands them
//! to the next connection. A daemon whose session has no client and no
//! running process or terminal ends it after the idle timeout the client
//! requested; running programs end only through `server.shutdown` or the
//! user stopping them.
//!
//! The socket lives in a directory only the user can access, and the daemon
//! also verifies the peer's user id, so other users on a shared host cannot
//! attach to a session. Within the account, a session that had a client
//! admits only connections presenting its token, and a refused connection
//! does not disconnect the attached client.
//!
//! The daemon's standard error is its log, `log/<name>.log` in the state
//! directory: mode 0600, never opened through a symbolic link, and holding
//! lifecycle diagnostics only, never file contents, program output or
//! tokens. Once it reaches [`LOG_ROTATE_BYTES`] it moves to `<name>.log.1`,
//! replacing the previous one, both when a proxy starts a daemon and while
//! the daemon runs.

use crate::server::connection::{self, Ended};
use crate::server::platform;
use crate::server::session::Session;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Bound for the proxy to see a freshly started daemon's socket. It covers a
/// predecessor that is still ending its session (terminating process groups
/// and draining requests takes up to about 13 seconds) before the new daemon
/// can take the lock.
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound for a new daemon to wait for a predecessor's lock.
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(25);

/// Pause after a failed `accept`, such as `EMFILE`, which otherwise repeats
/// immediately and spins.
const ACCEPT_FAILURE_BACKOFF: Duration = Duration::from_millis(200);
const DAEMON_START_POLL: Duration = Duration::from_millis(20);

/// Longest wait for a connection before the daemon re-checks its idle
/// deadline and shutdown flag. A connection ends the wait immediately.
const IDLE_CHECK_INTERVAL: Duration = Duration::from_millis(200);

/// Size at which a daemon log moves aside. A connection adds a few lines, so
/// this holds months of diagnostics while the log and its one predecessor
/// stay within about 2 MiB of the user's home directory.
const LOG_ROTATE_BYTES: u64 = 1024 * 1024;

/// Socket paths are limited to about 104 bytes on macOS and 108 on Linux.
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Short, always-present directory for sockets whose regular path is too
/// long. Its per-user subdirectory is private, like the regular location.
const SHORT_SOCKET_ROOT: &str = "/tmp";

/// Session key and where its state lives.
#[derive(Clone, Debug)]
pub struct PersistentOptions {
    /// Client-chosen session name: 1–64 characters from `[A-Za-z0-9._-]`.
    pub key: String,
    /// Defaults to `$HOME/.lithe-server`.
    pub state_directory: Option<PathBuf>,
}

struct Paths {
    socket: PathBuf,
    lock: PathBuf,
    log: PathBuf,
}

impl PersistentOptions {
    fn paths(&self) -> io::Result<Paths> {
        if self.key.is_empty()
            || self.key.len() > 64
            || !self
                .key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session key",
            ));
        }
        let state = match &self.state_directory {
            Some(directory) => directory.clone(),
            None => PathBuf::from(
                platform::current_account()
                    .home_directory
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?,
            )
            .join(".lithe-server"),
        };
        // Preview builds can change protocol features without changing version.
        // Hash both identities to keep maximal session keys within socket limits.
        let identity = crate::server::identity()?;
        let name = daemon_name(&self.key, &identity.sha256);
        let run = state.join("run");
        private_directory(&run)?;
        let mut socket = run.join(format!("{name}.sock"));
        if socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
            // `/tmp` rather than the temporary directory: on macOS the latter
            // is itself too long for a socket path.
            let fallback = PathBuf::from(SHORT_SOCKET_ROOT)
                .join(format!("lithe-server-{}", platform::current_uid()));
            private_directory(&fallback)?;
            socket = fallback.join(format!("{name}.sock"));
            if socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the session key is too long for a socket path",
                ));
            }
        }
        let log_directory = state.join("log");
        private_directory(&log_directory)?;
        Ok(Paths {
            socket,
            lock: run.join(format!("{name}.lock")),
            log: log_directory.join(format!("{name}.log")),
        })
    }
}

fn daemon_name(key: &str, executable_sha256: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(key.as_bytes());
    digest.update([0]);
    digest.update(executable_sha256.as_bytes());
    crate::protocol::methods::encode_hex(&digest.finalize()[..16])
}

/// Creates `path` with mode 0700, or verifies an existing one is a real
/// directory owned by the user that nobody else can write to.
fn private_directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
    {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != platform::current_uid()
        || metadata.mode() & 0o022 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} must be a private directory owned by the user",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Implements `--connect`: relays standard streams to the session daemon,
/// starting it when needed.
pub fn run_proxy(options: &PersistentOptions) -> io::Result<()> {
    let paths = options.paths()?;
    let stream = match UnixStream::connect(&paths.socket) {
        Ok(stream) => stream,
        Err(_) => {
            // Serialize launchers, not just daemon ownership. Otherwise a losing
            // daemon can miss the winning socket's whole lifetime and restart
            // the session after its explicit shutdown.
            let deadline = Instant::now() + DAEMON_START_TIMEOUT;
            let _startup = startup_lock(&paths.lock.with_extension("start"), deadline)?;
            match UnixStream::connect(&paths.socket) {
                Ok(stream) => stream,
                Err(_) => {
                    start_daemon(options, &paths)?;
                    wait_for_socket(&paths.socket, deadline)?
                }
            }
        }
    };
    relay(stream)
}

// Keep the inode after unlock: unlinking would let later launchers lock a
// different file while an existing waiter still owns the old one.
fn startup_lock(path: &Path, deadline: Instant) -> io::Result<File> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)?;
    loop {
        // SAFETY: flock operates on a descriptor owned by this process.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(lock);
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) {
            return Err(error);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "another proxy did not finish starting the session",
            ));
        }
        thread::sleep(DAEMON_START_POLL);
    }
}

fn start_daemon(options: &PersistentOptions, paths: &Paths) -> io::Result<()> {
    rotate_log(&paths.log, LOG_ROTATE_BYTES)?;
    let log = open_log(&paths.log)?;
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("--daemon").arg(&options.key);
    if let Some(directory) = &options.state_directory {
        command.arg("--state-dir").arg(directory);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    // A new session detaches the daemon from the SSH session, so the hangup
    // that ends the SSH session does not reach it.
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // The daemon is not waited for: it outlives this proxy and is adopted by
    // init when the proxy exits.
    command.spawn().map(|_| ())
}

/// Opens a daemon log for appending. A file an earlier run or the user left
/// with wider permissions becomes private again, and a symbolic link in its
/// place is refused instead of followed.
fn open_log(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

fn rotated_log(path: &Path) -> PathBuf {
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    PathBuf::from(rotated)
}

/// Moves the log at `path` to `<path>.1`, replacing an older one, once it
/// holds `limit` bytes. Returns whether it moved the file.
fn rotate_log(path: &Path, limit: u64) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.len() >= limit => {
            fs::rename(path, rotated_log(path))?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Keeps a running daemon's log bounded. Once the file `target` writes to is
/// the log at `path` and holds `limit` bytes, that file moves aside and
/// `target` keeps its descriptor number but refers to a fresh log. A
/// `target` that is another file, such as the terminal of a `--daemon`
/// started by hand or a log a newer proxy already rotated, is left alone.
fn bound_log(path: &Path, target: BorrowedFd<'_>, limit: u64) -> io::Result<()> {
    // SAFETY: the borrowed descriptor stays open for this call, and
    // ManuallyDrop keeps the temporary File from closing it.
    let current = ManuallyDrop::new(unsafe { File::from_raw_fd(target.as_raw_fd()) });
    let current = current.metadata()?;
    let on_disk = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if (current.dev(), current.ino()) != (on_disk.dev(), on_disk.ino()) || on_disk.len() < limit {
        return Ok(());
    }
    fs::rename(path, rotated_log(path))?;
    let fresh = open_log(path)?;
    // SAFETY: both descriptors are open; dup2 atomically repoints `target`,
    // so a concurrent write lands in either the old or the fresh file.
    if unsafe { libc::dup2(fresh.as_raw_fd(), target.as_raw_fd()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn wait_for_socket(socket: &Path, deadline: Instant) -> io::Result<UnixStream> {
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(DAEMON_START_POLL),
        }
    }
}

/// Copies bytes in both directions until the daemon closes the connection.
fn relay(stream: UnixStream) -> io::Result<()> {
    let mut to_daemon = stream.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut to_daemon);
        // End-of-file from the transport is the client's disconnect.
        let _ = to_daemon.shutdown(std::net::Shutdown::Write);
    });
    let mut from_daemon = stream;
    let mut stdout = io::stdout().lock();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = match from_daemon.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        // Flushing every chunk keeps interactive output latency low.
        if stdout
            .write_all(&buffer[..read])
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return Ok(());
        }
    }
}

/// Implements `--daemon`: serves the persistent session for `options.key`.
pub fn run_daemon(options: &PersistentOptions) -> io::Result<()> {
    let paths = options.paths()?;
    // The lock decides which daemon owns the key.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&paths.lock)?;
    if !acquire_lock(&lock, &paths.socket) {
        return Ok(());
    }
    write_pid(&lock)?;
    // A socket left by a crashed daemon refuses connections; the lock proves
    // nobody serves it.
    match fs::remove_file(&paths.socket) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(&paths.socket)?;
    fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;

    let session = Session::new(true)?;
    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        // A client that reconnects right after `server.shutdown` must not
        // reach this ending session: the socket disappears before the
        // response, so its proxy starts a new daemon, which waits for the lock.
        let socket = paths.socket.clone();
        let shutdown = Arc::clone(&shutdown);
        session.set_shutdown_hook(move || {
            shutdown.store(true, Ordering::Release);
            let _ = fs::remove_file(socket);
        });
    }
    let attached = Arc::new(AtomicUsize::new(0));
    // The attached connection's id and a copy of its socket, kept so that a
    // replacing connection can disconnect it.
    let current: Arc<Mutex<Option<(u64, UnixStream)>>> = Arc::new(Mutex::new(None));
    let mut next_connection: u64 = 0;
    let mut idle_since = Instant::now();
    let mut log_rotation_failed = false;

    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if platform::peer_uid(stream.as_raw_fd()).ok() != Some(platform::current_uid()) {
                    eprintln!("lithe-remote-server: rejected a connection from another user");
                    continue;
                }
                stream.set_nonblocking(false)?;
                next_connection += 1;
                let connection_id = next_connection;
                let replacement = stream.try_clone()?;
                let current = Arc::clone(&current);
                attached.fetch_add(1, Ordering::AcqRel);
                let session = Arc::clone(&session);
                let attached = Arc::clone(&attached);
                let shutdown = Arc::clone(&shutdown);
                thread::Builder::new()
                    .name("remote-connection".into())
                    .spawn(move || {
                        let input = match stream.try_clone() {
                            Ok(input) => input,
                            Err(error) => {
                                eprintln!("lithe-remote-server: {error}");
                                attached.fetch_sub(1, Ordering::AcqRel);
                                return;
                            }
                        };
                        // An admitted client replaces the attached one, which
                        // may be half-dead after a network change and not yet
                        // noticed. A connection the session refuses leaves it
                        // alone, and its copy closes with the unused closure.
                        let attach = {
                            let current = Arc::clone(&current);
                            move || {
                                if let Some((_, previous)) = crate::server::lock(&current)
                                    .replace((connection_id, replacement))
                                {
                                    // log-exempt: Expected absence: the replaced client may already have disconnected, which is why it is being replaced.
                                    let _ = previous.shutdown(std::net::Shutdown::Both);
                                }
                            }
                        };
                        let ended = connection::serve(&session, input, stream, attach);
                        // The copy kept for replacement is the last open
                        // descriptor of this socket. Closing it delivers
                        // end-of-file to the proxy, which would otherwise
                        // linger on the host after every disconnect.
                        // (`shutdown` is not enough: BSD rejects it with
                        // ENOTCONN once the peer has half-closed.)
                        let mut current = crate::server::lock(&current);
                        if matches!(&*current, Some((id, _)) if *id == connection_id) {
                            current.take();
                        }
                        drop(current);
                        if matches!(ended, Ok(Ended::Shutdown)) {
                            shutdown.store(true, Ordering::Release);
                        }
                        attached.fetch_sub(1, Ordering::AcqRel);
                    })?;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if attached.load(Ordering::Acquire) > 0 || session.keeps_programs() {
                    idle_since = Instant::now();
                } else if idle_since.elapsed() >= session.idle_timeout() {
                    break;
                }
                // Reported once per failure streak: the check repeats every
                // interval, and a failure that persists would flood the log.
                match bound_log(&paths.log, io::stderr().as_fd(), LOG_ROTATE_BYTES) {
                    Ok(()) => log_rotation_failed = false,
                    Err(error) if !log_rotation_failed => {
                        eprintln!("lithe-remote-server: cannot rotate the log: {error}");
                        log_rotation_failed = true;
                    }
                    Err(_) => {}
                }
                wait_for_connection(&listener, IDLE_CHECK_INTERVAL);
            }
            Err(error) => {
                // Persistent failures such as EMFILE return immediately; the
                // pause keeps the loop from spinning and flooding the log.
                eprintln!("lithe-remote-server: accept failed: {error}");
                thread::sleep(ACCEPT_FAILURE_BACKOFF);
            }
        }
    }

    let _ = fs::remove_file(&paths.socket);
    session.terminate_all();
    if let Some((_, stream)) = crate::server::lock(&current).take() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    drop(lock);
    Ok(())
}

/// Takes the exclusive lock for the key. Another daemon holding it is either
/// serving the key, then this daemon is redundant and exits, or ending its
/// session after removing the socket, then this daemon waits for it so the
/// proxy that started it finds a fresh session instead of nothing.
fn acquire_lock(lock: &File, socket: &Path) -> bool {
    let deadline = Instant::now() + LOCK_WAIT_TIMEOUT;
    loop {
        // SAFETY: flock on a descriptor this process owns.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return true;
        }
        // The socket's presence is the signal, not a connection to it: the
        // serving daemon would accept a probe connection as a new client and
        // disconnect the real one it replaces.
        if fs::symlink_metadata(socket).is_ok() {
            eprintln!("lithe-remote-server: another daemon serves this session; exiting");
            return false;
        }
        if Instant::now() >= deadline {
            eprintln!("lithe-remote-server: the previous daemon did not release the session lock");
            return false;
        }
        thread::sleep(DAEMON_START_POLL);
    }
}

fn wait_for_connection(listener: &UnixListener, timeout: Duration) {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one live pollfd is passed with its count.
    unsafe { libc::poll(&mut descriptor, 1, timeout.as_millis() as libc::c_int) };
}

fn write_pid(mut lock: &File) -> io::Result<()> {
    lock.set_len(0)?;
    writeln!(lock, "{}", std::process::id())?;
    lock.flush()
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn startup_lock_is_exclusive_bounded_and_released_on_drop() {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!("lithe-startup-lock-{}", std::process::id()));
        let _cleanup = Cleanup(path.clone());
        let owner = startup_lock(&path, Instant::now()).unwrap();
        assert_eq!(
            startup_lock(&path, Instant::now()).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        drop(owner);
        // A test elsewhere in this binary can fork while `owner` is open; the
        // child holds a copy of the descriptor, and so the lock, until it
        // execs. Allow a bounded wait for that instead of failing at once.
        let _next = startup_lock(&path, Instant::now() + Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn same_version_binary_rebuilds_cannot_reuse_a_daemon() {
        let key = "a".repeat(64);
        let old = daemon_name(&key, &"1".repeat(64));
        assert_eq!(old.len(), 32);
        assert_eq!(old, daemon_name(&key, &"1".repeat(64)));
        assert_ne!(old, daemon_name(&key, &"2".repeat(64)));
        assert_ne!(old, daemon_name("other-session", &"1".repeat(64)));
        let fallback = PathBuf::from(SHORT_SOCKET_ROOT)
            .join(format!("lithe-server-{}", u32::MAX))
            .join(format!("{old}.sock"));
        assert!(fallback.as_os_str().len() <= MAX_SOCKET_PATH_BYTES);
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;

    /// Directory removed on drop, also after a failed assertion.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("lithe-daemon-log-{label}-{}", std::process::id()));
            // log-exempt: Expected absence: a previous run of this test usually left nothing behind.
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // log-exempt: Expected absence: the directory may already be gone when a test removed it.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o777
    }

    #[test]
    fn logs_move_aside_by_size_and_stay_private() {
        let scratch = Scratch::new("rotate");
        let path = scratch.0.join("daemon.log");
        fs::write(&path, b"old entry\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(!rotate_log(&path, 1024).unwrap(), "rotated below the limit");
        drop(open_log(&path).unwrap());
        assert_eq!(mode(&path), 0o600, "a wider log stayed readable to others");

        assert!(rotate_log(&path, 4).unwrap());
        assert_eq!(fs::read(rotated_log(&path)).unwrap(), b"old entry\n");
        assert!(!path.exists());
        assert!(!rotate_log(&path, 4).unwrap(), "rotated a missing log");

        fs::write(&path, b"second\n").unwrap();
        assert!(rotate_log(&path, 4).unwrap());
        assert_eq!(
            fs::read(rotated_log(&path)).unwrap(),
            b"second\n",
            "the older rotated log was not replaced"
        );
        drop(open_log(&path).unwrap());
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read(&path).unwrap(), b"");
    }

    #[test]
    fn a_running_daemon_repoints_its_full_log_at_a_fresh_file() {
        let scratch = Scratch::new("bound");
        let path = scratch.0.join("daemon.log");
        let mut target = open_log(&path).unwrap();
        target.write_all(b"before\n").unwrap();

        bound_log(&path, target.as_fd(), 1024).unwrap();
        assert!(!rotated_log(&path).exists(), "rotated below the limit");

        bound_log(&path, target.as_fd(), 4).unwrap();
        target.write_all(b"after\n").unwrap();
        assert_eq!(fs::read(rotated_log(&path)).unwrap(), b"before\n");
        assert_eq!(fs::read(&path).unwrap(), b"after\n");
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn descriptors_that_are_not_the_log_are_left_alone() {
        let scratch = Scratch::new("foreign");
        let path = scratch.0.join("daemon.log");
        fs::write(&path, b"daemon entries\n").unwrap();
        let mut other = open_log(&scratch.0.join("other.log")).unwrap();
        other.write_all(b"other entries\n").unwrap();

        bound_log(&path, other.as_fd(), 1).unwrap();
        assert!(!rotated_log(&path).exists());
        assert_eq!(fs::read(&path).unwrap(), b"daemon entries\n");
    }

    #[test]
    fn the_log_is_never_opened_through_a_symbolic_link() {
        let scratch = Scratch::new("link");
        let elsewhere = scratch.0.join("elsewhere");
        let path = scratch.0.join("daemon.log");
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();

        assert!(open_log(&path).is_err());
        assert!(!elsewhere.exists(), "the link's target was created");
    }
}
