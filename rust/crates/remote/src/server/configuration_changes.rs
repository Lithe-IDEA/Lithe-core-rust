//! Publishes Core-planned configuration edits beneath an opened workspace descriptor.

use super::{fs, scoped_files, to_value};
use crate::protocol::methods::{
    is_sha256_hex, ApplyRunConfigChangesParams, ProjectPluginSettingsBaseline,
    ProjectPluginSettingsOperation, ProjectPluginSettingsParams, ProjectPluginSettingsSnapshot,
    RunConfigChange, MAX_SCOPED_CONFIGURATION_BYTES,
};
use crate::protocol::{ErrorCode, RemoteError};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const PLUGIN_SETTINGS_PATH: &str = ".lithe/plugin-settings.json";
const MAX_FILE_BYTES: usize = 4 << 20;
const MAX_BATCH_BYTES: usize = 16 << 20;
const MAX_CHANGES: usize = 520;
static PUBLICATION: Mutex<()> = Mutex::new(());
static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

struct Edit {
    change: RunConfigChange,
    components: Vec<CString>,
    previous: Option<Snapshot>,
}

struct Snapshot {
    bytes: Vec<u8>,
    mode: libc::mode_t,
}

/// Refuses stale batches before writing and restores already published files on failure.
pub(super) fn apply_run(params: ApplyRunConfigChangesParams) -> Result<(), RemoteError> {
    if params.changes.len() > MAX_CHANGES {
        return Err(too_large());
    }
    let mut paths = HashSet::new();
    let mut size = 0;
    for change in &params.changes {
        if !valid_path(&change.path)
            || !paths.insert(&change.path)
            || change
                .base_sha256
                .as_deref()
                .is_some_and(|hash| !is_sha256_hex(hash))
            || change.base_sha256.is_none() && change.content.is_none()
        {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "Invalid Run configuration change",
            ));
        }
        let length = change.content.as_ref().map_or(0, String::len);
        size += length;
        if length > MAX_FILE_BYTES || size > MAX_BATCH_BYTES {
            return Err(too_large());
        }
    }
    let root_path = fs::host_path(&params.root)?;
    let root =
        scoped_files::open_root(&root_path).map_err(|error| fs::io_error(error, &params.root))?;
    // Serializes edits issued by all sessions served by this process.
    let _publication = PUBLICATION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut edits = Vec::with_capacity(params.changes.len());
    for change in params.changes {
        let components = scoped_files::components(&change.path)?;
        let previous = capture(&root, &components, MAX_FILE_BYTES)
            .map_err(|error| file_error(error, &change.path))?;
        let hash = source_hash(previous.as_ref());
        if hash.as_deref()
            != change
                .base_sha256
                .as_ref()
                .map(|hash| hash.to_ascii_lowercase())
                .as_deref()
        {
            return Err(conflict(&change.path));
        }
        size += previous.as_ref().map_or(0, |file| file.bytes.len());
        if size > MAX_BATCH_BYTES {
            return Err(too_large());
        }
        edits.push(Edit {
            change,
            components,
            previous,
        });
    }
    // Publish replacements before removing the documents they migrate.
    edits.sort_by_key(|edit| edit.change.content.is_none());
    for (index, edit) in edits.iter().enumerate() {
        let previous = edit.previous.as_ref();
        let expected = previous.map(|file| file.bytes.as_slice());
        let mode = previous.map_or(0o600, |file| file.mode);
        let content = edit.change.content.as_deref().map(str::as_bytes);
        if let Err(error) = publish(
            &root,
            &edit.components,
            expected,
            content,
            mode,
            MAX_FILE_BYTES,
        )
        .map_err(|error| file_error(error, &edit.change.path))
        {
            let mut rollback_errors = Vec::new();
            for committed in edits[..index].iter().rev() {
                let previous = committed.previous.as_ref();
                let written = committed.change.content.as_deref().map(str::as_bytes);
                let restore = previous.map(|file| file.bytes.as_slice());
                let mode = previous.map_or(0o600, |file| file.mode);
                if let Err(error) = publish(
                    &root,
                    &committed.components,
                    written,
                    restore,
                    mode,
                    MAX_FILE_BYTES,
                )
                .map_err(|error| file_error(error, &committed.change.path))
                {
                    rollback_errors.push(
                        serde_json::json!({"path": committed.change.path, "reason": error.message}),
                    );
                }
            }
            return if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(error.with_details(serde_json::json!({"path": edit.change.path, "rollbackErrors": rollback_errors})))
            };
        }
    }
    Ok(())
}

/// Reads or publishes the one plugin preferences file, with no caller-supplied file path.
pub(super) fn plugin_settings(params: ProjectPluginSettingsParams) -> Result<Value, RemoteError> {
    let root_path = fs::host_path(&params.root)?;
    let root =
        scoped_files::open_root(&root_path).map_err(|error| fs::io_error(error, &params.root))?;
    let metadata = root
        .metadata()
        .map_err(|error| fs::io_error(error, &params.root))?;
    let workspace_identity = format!("{}:{}", metadata.dev(), metadata.ino());
    let components = scoped_files::components(PLUGIN_SETTINGS_PATH)?;
    let limit = MAX_SCOPED_CONFIGURATION_BYTES;
    let _publication = PUBLICATION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let previous = capture(&root, &components, limit)
        .map_err(|error| file_error(error, PLUGIN_SETTINGS_PATH))?;
    let base_sha256 = source_hash(previous.as_ref());
    match params.operation {
        ProjectPluginSettingsOperation::Read => {
            let text = previous
                .map(|source| String::from_utf8(source.bytes))
                .transpose()
                .map_err(|_| {
                    RemoteError::new(
                        ErrorCode::InvalidRequest,
                        "The plugin settings source is not UTF-8",
                    )
                })?;
            to_value(ProjectPluginSettingsSnapshot {
                path: PLUGIN_SETTINGS_PATH.into(),
                text,
                baseline: ProjectPluginSettingsBaseline {
                    workspace_identity,
                    base_sha256,
                },
            })
        }
        ProjectPluginSettingsOperation::Publish { baseline, content } => {
            if content.as_ref().is_some_and(|text| text.len() > limit) {
                return Err(too_large());
            }
            if workspace_identity != baseline.workspace_identity
                || base_sha256 != baseline.base_sha256
            {
                return Err(conflict(PLUGIN_SETTINGS_PATH));
            }
            let expected = previous.as_ref().map(|file| file.bytes.as_slice());
            let replacement = content.as_deref().map(str::as_bytes);
            if expected == replacement {
                return Ok(Value::Bool(false));
            }
            if let Some(file) = &previous {
                if file.mode & 0o222 == 0 {
                    return Err(file_error(
                        io::ErrorKind::PermissionDenied.into(),
                        PLUGIN_SETTINGS_PATH,
                    ));
                }
            }
            let mode = previous.as_ref().map_or(0o600, |file| file.mode);
            publish(&root, &components, expected, replacement, mode, limit)
                .map_err(|error| file_error(error, PLUGIN_SETTINGS_PATH))?;
            Ok(Value::Bool(true))
        }
    }
}

fn source_hash(previous: Option<&Snapshot>) -> Option<String> {
    previous.map(|file| format!("{:x}", Sha256::digest(&file.bytes)))
}

fn valid_path(path: &str) -> bool {
    matches!(
        path,
        ".lithe/run/configurations.yaml"
            | ".lithe/run/local.yaml"
            | ".lithe/run/configurations.json"
            | ".lithe/run/local.json"
            | ".lithe/toolchains/local.json"
    ) || path
        .strip_prefix(".lithe/run/plugin/")
        .is_some_and(|name| name.ends_with(".toml") && !name.contains(['/', '\\', '\0']))
}

fn capture(root: &File, components: &[CString], limit: usize) -> io::Result<Option<Snapshot>> {
    let (name, parents) = components.split_last().expect("validated file path");
    let capture = || {
        let parent = scoped_files::directory(root.try_clone()?, parents)?;
        capture_at(&parent, name, limit)
    };
    match capture() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        result => result,
    }
}

fn capture_at(parent: &File, name: &CStr, limit: usize) -> io::Result<Option<Snapshot>> {
    let status = match scoped_files::status(parent.as_raw_fd(), Some(name)) {
        Ok(status) => status,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let (bytes, truncated) = scoped_files::read_regular_at(parent, name, limit)?;
    if truncated {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    Ok(Some(Snapshot {
        bytes,
        mode: status.st_mode & 0o777,
    }))
}

fn publish(
    root: &File,
    components: &[CString],
    expected: Option<&[u8]>,
    content: Option<&[u8]>,
    mode: libc::mode_t,
    limit: usize,
) -> io::Result<()> {
    let (name, parents) = components.split_last().expect("validated file path");
    let parent = if content.is_some() && expected.is_none() {
        create_parents(root, parents)?
    } else {
        scoped_files::directory(root.try_clone()?, parents)?
    };
    let replacement = content
        .map(|bytes| Temporary::write(&parent, bytes, mode))
        .transpose()?;
    let current = capture_at(&parent, name, limit)?;
    if current.as_ref().map(|file| file.bytes.as_slice()) != expected {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    match replacement {
        Some(temporary) => temporary.install(name, expected.is_none()),
        None => unlink(&parent, name),
    }
}

fn create_parents(root: &File, names: &[CString]) -> io::Result<File> {
    let mut parent = root.try_clone()?;
    for name in names {
        match scoped_files::open_directory_at(&parent, name) {
            Ok(child) => parent = child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // SAFETY: descriptor and NUL-terminated name remain live for mkdirat.
                let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
                if created != 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::AlreadyExists {
                        return Err(error);
                    }
                }
                parent = scoped_files::open_directory_at(&parent, name)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(parent)
}

pub(super) struct Temporary<'a> {
    parent: &'a File,
    name: CString,
    installed: bool,
}

impl<'a> Temporary<'a> {
    fn write(parent: &'a File, content: &[u8], mode: libc::mode_t) -> io::Result<Self> {
        Self::write_with_metadata(parent, content, mode, None)
    }

    /// Creates a private replacement next to the target and preserves its metadata.
    pub(super) fn write_with_metadata(
        parent: &'a File,
        content: &[u8],
        mode: libc::mode_t,
        original: Option<&File>,
    ) -> io::Result<Self> {
        let name = CString::new(format!(
            ".lithe-config-{}-{}.tmp",
            std::process::id(),
            TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        // SAFETY: descriptor and name remain live, and O_EXCL refuses existing names.
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY | libc::O_CLOEXEC,
                libc::c_uint::from(mode & 0o600),
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let temporary = Self {
            parent,
            name,
            installed: false,
        };
        // SAFETY: successful openat returned an exclusively owned descriptor.
        let mut file = unsafe { File::from_raw_fd(descriptor) };
        file.write_all(content)?;
        if let Some(source) = original {
            let target = file.metadata()?;
            let origin = source.metadata()?;
            if (target.uid(), target.gid()) != (origin.uid(), origin.gid()) {
                // SAFETY: file owns the descriptor; uid and gid came from fstat.
                if unsafe { libc::fchown(file.as_raw_fd(), origin.uid(), origin.gid()) } != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        // SAFETY: file owns an open descriptor and mode contains permission bits.
        if unsafe { libc::fchmod(file.as_raw_fd(), mode) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if let Some(source) = original {
            super::platform::copy_configuration_attributes(source.as_raw_fd(), file.as_raw_fd())?;
        }
        file.sync_all()?;
        Ok(temporary)
    }

    pub(super) fn install(mut self, target: &CStr, create_only: bool) -> io::Result<()> {
        let parent = self.parent.as_raw_fd();
        // SAFETY: both names are NUL-terminated and both address the same open directory.
        let result = unsafe {
            if create_only {
                libc::linkat(parent, self.name.as_ptr(), parent, target.as_ptr(), 0)
            } else {
                libc::renameat(parent, self.name.as_ptr(), parent, target.as_ptr())
            }
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        self.installed = !create_only;
        Ok(())
    }
}

impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        if !self.installed {
            if let Err(error) = unlink(self.parent, &self.name) {
                eprintln!(
                    "remote configuration temporary cleanup failed: {:?}",
                    error.kind()
                );
            }
        }
    }
}

fn unlink(parent: &File, name: &CStr) -> io::Result<()> {
    // SAFETY: descriptor and name remain live; unlinkat never follows a final symlink.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn conflict(path: &str) -> RemoteError {
    RemoteError::new(
        ErrorCode::Conflict,
        "The configuration changed after it was read",
    )
    .with_details(serde_json::json!({"path": path}))
}

fn too_large() -> RemoteError {
    RemoteError::new(
        ErrorCode::TooLarge,
        "Configuration edits exceeded their size budget",
    )
}

fn file_error(error: io::Error, path: &str) -> RemoteError {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::AlreadyExists => conflict(path),
        io::ErrorKind::FileTooLarge => too_large(),
        _ if error.raw_os_error() == Some(libc::ELOOP) => RemoteError::new(
            ErrorCode::PermissionDenied,
            "Configurations cannot follow symbolic links",
        ),
        _ => fs::io_error(error, path),
    }
}
