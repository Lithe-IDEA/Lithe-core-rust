//! Session-owned private directories for deployed runtime inputs.
//! Persistent disconnects retain them; explicit release or session termination
//! removes them, even if the desktop vanished before receiving allocation.

use crate::protocol::methods::{self, RunDirectoryParams};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::connection::Context;
use crate::server::fs::io_error;
use crate::server::{lock, parse, platform};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_DIRECTORIES: usize = 256;
const STAGING_DIRECTORY: &str = "deployment-staging";

pub(crate) struct RunDirectories {
    home: Option<String>,
    entries: Mutex<BTreeMap<String, Arc<Directory>>>,
}

struct Directory {
    path: PathBuf,
    allocated: AtomicBool,
    connection: Option<u64>,
    cancelled: AtomicBool,
    removing: Mutex<()>,
    _staging_lease: OnceLock<File>,
}

impl Drop for Directory {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!("remote run directory cleanup failed: {:?}", error.kind());
        }
    }
}

impl Directory {
    /// Creates a new directory and leases scratch before a sweep can see it.
    fn allocate(&self, parent: &Path) -> Result<(), RemoteError> {
        let _parent_lease = if self.connection.is_some() {
            Some(
                lock_private_directory(parent, libc::LOCK_SH)?.ok_or_else(|| {
                    RemoteError::new(ErrorCode::NotFound, "deployment staging parent changed")
                })?,
            )
        } else {
            None
        };
        // Never adopt pre-existing paths. The worker's Arc cleans a late
        // allocation if cancellation has already drained the registry.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&self.path)
            .map_err(|error| io_error(error, "run directory"))?;
        self.allocated.store(true, Ordering::Release);
        if self.connection.is_some() {
            let lease = lock_private_directory(&self.path, libc::LOCK_SH | libc::LOCK_NB)?
                .ok_or_else(|| {
                    RemoteError::new(ErrorCode::Busy, "cannot lease deployment staging")
                })?;
            self._staging_lease
                .set(lease)
                .expect("the allocating worker sets the staging lease once");
        }
        Ok(())
    }

    fn remove(&self) -> std::io::Result<()> {
        if self.allocated.load(Ordering::Acquire) {
            match fs::remove_dir_all(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            self.allocated.store(false, Ordering::Release);
        }
        Ok(())
    }
}

pub(crate) fn validate(params: &RunDirectoryParams) -> Result<(), RemoteError> {
    if params.directory_id.is_empty()
        || params.directory_id.len() > 64
        || !params
            .directory_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(RemoteError::new(
            ErrorCode::InvalidRequest,
            "invalid run directory identity",
        ));
    }
    Ok(())
}

impl RunDirectories {
    pub(crate) fn new(home: Option<String>) -> Self {
        Self {
            home,
            entries: Mutex::default(),
        }
    }

    /// The account home resolved once for this session.
    pub(super) fn account_home(&self) -> Option<&str> {
        self.home.as_deref()
    }

    pub(crate) fn handle(
        &self,
        context: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value, RemoteError> {
        let params: RunDirectoryParams = parse(params)?;
        validate(&params)?;
        if method == methods::FS_RELEASE_RUN_DIRECTORY {
            // Keep failed cleanup registered for retry. Recursive deletion must
            // not hold the registry lock or report success on a failed removal.
            let directory = lock(&self.entries).get(&params.directory_id).cloned();
            let Some(directory) = directory else {
                return Ok(Value::Null);
            };
            if directory.connection.is_some() {
                // The allocating or extracting worker may still hold this directory.
                // Its last Arc removes the staging tree after it observes cancellation.
                self.forget(&params.directory_id, &directory);
                return Ok(Value::Null);
            }
            let _removing = directory.removing.try_lock().map_err(|_| {
                RemoteError::new(ErrorCode::Busy, "run directory removal in progress")
            })?;
            directory
                .remove()
                .map_err(|error| io_error(error, "run directory"))?;
            self.forget(&params.directory_id, &directory);
            return Ok(Value::Null);
        }
        let connection =
            (method == methods::FS_CREATE_RUNTIME_STAGING).then_some(context.connection);
        if context.session.is_ending()
            || ((connection.is_some() || !context.session.persistent)
                && context.closed.load(Ordering::Acquire))
        {
            return Err(RemoteError::new(ErrorCode::NotFound, "session ended"));
        }
        let home = self
            .home
            .as_deref()
            .ok_or_else(|| RemoteError::new(ErrorCode::Unsupported, "host home unavailable"))?;
        let home = crate::server::fs::host_path(home)?;
        let base = home.join(".lithe-server");
        private_directory(&base)?;
        let parent = base.join(if connection.is_some() {
            STAGING_DIRECTORY
        } else {
            "plugin-runs"
        });
        private_directory(&parent)?;
        let parent = fs::canonicalize(parent).map_err(|error| io_error(error, "run parent"))?;
        let directory = Arc::new(Directory {
            path: parent.join(&params.directory_id),
            allocated: AtomicBool::new(false),
            connection,
            cancelled: AtomicBool::new(false),
            removing: Mutex::new(()),
            _staging_lease: OnceLock::new(),
        });
        {
            let mut entries = lock(&self.entries);
            if context.session.is_ending()
                || (connection.is_some() && context.closed.load(Ordering::Acquire))
            {
                return Err(RemoteError::new(ErrorCode::NotFound, "session ended"));
            }
            if entries.contains_key(&params.directory_id) {
                return Err(RemoteError::new(
                    ErrorCode::ResourceIdInUse,
                    "run directory identity is active",
                ));
            }
            if entries.len() >= MAX_DIRECTORIES {
                return Err(RemoteError::new(
                    ErrorCode::Busy,
                    "too many run directories",
                ));
            }
            entries.insert(params.directory_id.clone(), Arc::clone(&directory));
        }
        if let Err(error) = directory.allocate(&parent) {
            self.forget(&params.directory_id, &directory);
            return Err(error);
        }
        Ok(json!({"path": directory.path}))
    }

    /// Consumes scratch after a deployment attempt, including failures.
    /// Older plugin publishers can still commit from a session run directory.
    /// The callback can check cancellation between bounded reads and before rename.
    pub(crate) fn consume_staging<T>(
        &self,
        context: &Context,
        directory_id: String,
        accept_run_directory: bool,
        action: impl FnOnce(&Path, &dyn Fn() -> bool) -> Result<T, RemoteError>,
    ) -> Result<T, RemoteError> {
        self.with_staging(context, directory_id, accept_run_directory, true, action)
    }

    /// Protects an editor's scratch from concurrent publication and disconnect.
    pub(crate) fn prepare_staging<T>(
        &self,
        context: &Context,
        directory_id: String,
        action: impl FnOnce(&Path, &dyn Fn() -> bool) -> Result<T, RemoteError>,
    ) -> Result<T, RemoteError> {
        self.with_staging(context, directory_id, false, false, action)
    }

    fn with_staging<T>(
        &self,
        context: &Context,
        directory_id: String,
        accept_run_directory: bool,
        consume: bool,
        action: impl FnOnce(&Path, &dyn Fn() -> bool) -> Result<T, RemoteError>,
    ) -> Result<T, RemoteError> {
        let params = RunDirectoryParams { directory_id };
        validate(&params)?;
        let directory = lock(&self.entries)
            .get(&params.directory_id)
            .cloned()
            .filter(|directory| match directory.connection {
                Some(connection) => connection == context.connection,
                None => accept_run_directory,
            })
            .ok_or_else(|| RemoteError::new(ErrorCode::NotFound, "unknown deployment staging"))?;
        let _removing = directory
            .removing
            .try_lock()
            .map_err(|_| RemoteError::new(ErrorCode::Busy, "deployment staging is in use"))?;
        let cancelled = || {
            context.closed.load(Ordering::Acquire)
                || context.session.is_ending()
                || directory.cancelled.load(Ordering::Acquire)
        };
        if !directory.allocated.load(Ordering::Acquire) || cancelled() {
            return Err(RemoteError::new(
                ErrorCode::NotFound,
                "deployment staging was cancelled",
            ));
        }
        let result = action(&directory.path, &cancelled);
        if directory.connection.is_some() && (consume || result.is_err()) {
            self.forget(&params.directory_id, &directory);
        }
        result
    }

    /// Reclaims only upload and extraction scratch with no living owner.
    pub(crate) fn sweep_staging(&self) -> Result<(), RemoteError> {
        let Some(home) = &self.home else {
            return Ok(());
        };
        let parent = crate::server::fs::host_path(home)?
            .join(".lithe-server")
            .join(STAGING_DIRECTORY);
        let entries = match fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(error, "deployment staging")),
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error(error, "deployment staging"))?;
            let Ok(directory_id) = entry.file_name().into_string() else {
                continue;
            };
            if validate(&RunDirectoryParams { directory_id }).is_err()
                || !entry
                    .file_type()
                    .map_err(|error| io_error(error, "deployment staging"))?
                    .is_dir()
            {
                continue;
            }
            // No allocator can be between mkdir and taking the child's lease.
            // Release the parent before recursively deleting any scratch.
            let Some(parent_lease) =
                lock_private_directory(&parent, libc::LOCK_EX | libc::LOCK_NB)?
            else {
                continue;
            };
            let path = entry.path();
            let lease = lock_private_directory(&path, libc::LOCK_EX | libc::LOCK_NB)?;
            drop(parent_lease);
            if let Some(_lease) = lease {
                fs::remove_dir_all(&path).map_err(|error| io_error(error, "deployment staging"))?;
            }
        }
        Ok(())
    }

    /// Abandons in-flight uploads and extraction when a client disconnects.
    pub(crate) fn release_connection(&self, connection: u64) {
        let removed = {
            let mut entries = lock(&self.entries);
            let ids: Vec<_> = entries
                .iter()
                .filter(|(_, directory)| directory.connection == Some(connection))
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| entries.remove(&id))
                .collect::<Vec<_>>()
        };
        drop(removed);
    }

    pub(crate) fn clear(&self) {
        let removed = std::mem::take(&mut *lock(&self.entries));
        drop(removed);
    }

    fn forget(&self, id: &str, directory: &Arc<Directory>) {
        let removed = {
            let mut entries = lock(&self.entries);
            if entries
                .get(id)
                .is_some_and(|entry| Arc::ptr_eq(entry, directory))
            {
                directory.cancelled.store(true, Ordering::Release);
                entries.remove(id)
            } else {
                None
            }
        };
        drop(removed);
    }
}

pub(crate) fn private_directory(path: &Path) -> Result<(), RemoteError> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(error, "run parent")),
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(error, "run parent"))?;
    require_private_directory(&metadata)
}

/// Validates a directory without following a link or creating a missing path.
pub(crate) fn require_private_directory(metadata: &fs::Metadata) -> Result<(), RemoteError> {
    if !metadata.is_dir()
        || metadata.uid() != platform::current_uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err(RemoteError::new(
            ErrorCode::PermissionDenied,
            "run parent must be a private directory owned by the host user",
        ));
    }
    Ok(())
}

/// Locks the directory inode and checks that its name still identifies it.
pub(super) fn lock_private_directory(
    path: &Path,
    mode: libc::c_int,
) -> Result<Option<File>, RemoteError> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error, "private directory")),
    };
    let metadata = file
        .metadata()
        .map_err(|error| io_error(error, "private directory"))?;
    require_private_directory(&metadata)?;
    // SAFETY: the descriptor belongs to this live File for the duration of flock.
    if unsafe { libc::flock(file.as_raw_fd(), mode) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(io_error(error, "private directory lease"));
    }
    let present = match fs::symlink_metadata(path) {
        Ok(current) => current.dev() == metadata.dev() && current.ino() == metadata.ino(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(io_error(error, "private directory")),
    };
    Ok(present.then_some(file))
}
