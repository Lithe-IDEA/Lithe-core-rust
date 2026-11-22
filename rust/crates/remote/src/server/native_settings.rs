//! Host-side native settings transactions. The editor receives paths and digests.
//! Only the approved helper and this transaction touch original or staged bytes.

use super::{configuration_changes::Temporary, connection::Context, fs, scoped_files, to_value};
use crate::protocol::{
    methods::{
        is_sha256_hex, NativeSettingsBaseline, NativeSettingsOperation, NativeSettingsParams,
        NativeSettingsReplacement, NativeSettingsScope, NativeSettingsTarget,
    },
    ErrorCode, RemoteError,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 4 * MAX_SOURCE_BYTES;
const MAX_SOURCES: usize = 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024;
static PUBLICATION: Mutex<()> = Mutex::new(());

pub(super) fn handle(
    context: &Context,
    params: NativeSettingsParams,
) -> Result<Value, RemoteError> {
    let scratch = &context.session.run_directories;
    let roots = Roots::new(&params.root, scratch.account_home())?;
    match params.operation {
        NativeSettingsOperation::Inspect { targets } => to_value(inspect(&roots, targets)?),
        NativeSettingsOperation::Stage {
            directory_id,
            baselines,
        } => scratch.prepare_staging(context, directory_id, |directory, cancelled| {
            stage(&roots, directory, baselines, cancelled)
        }),
        NativeSettingsOperation::Publish {
            directory_id,
            replacements,
        } => scratch.consume_staging(context, directory_id, false, |directory, cancelled| {
            publish(&roots, directory, replacements, cancelled)
        }),
    }
}

struct Roots {
    workspace: PathBuf,
    home: Option<PathBuf>,
}

impl Roots {
    /// Resolves root aliases once; every target component below a root is opened without links.
    fn new(workspace: &str, home: Option<&str>) -> Result<Self, RemoteError> {
        let workspace = fs::host_path(workspace)?
            .canonicalize()
            .map_err(|error| fs::io_error(error, workspace))?;
        let home = home.map(fs::host_path).transpose()?;
        Ok(Self { workspace, home })
    }

    fn inspect(&self, target: NativeSettingsTarget) -> Result<Source, RemoteError> {
        if target.path.is_empty()
            || target.path.len() > 4096
            || target.path.split('/').any(str::is_empty)
        {
            return Err(RemoteError::new(
                ErrorCode::InvalidPath,
                "Invalid native settings path",
            ));
        }
        let components = scoped_files::components(&target.path)?;
        let (name, parents) = components.split_last().expect("validated nonempty path");
        let root = match target.scope {
            NativeSettingsScope::Workspace => self.workspace.clone(),
            NativeSettingsScope::Home => self
                .home
                .as_ref()
                .ok_or_else(|| {
                    RemoteError::new(ErrorCode::Unsupported, "Remote account home is unavailable")
                })?
                .canonicalize()
                .map_err(|error| source_error(error, &target.path))?,
        };
        let host_path = root.join(&target.path);
        // The server's installed binary, plugin packages, and runtime cache are immutable.
        // Reject a workspace opened inside that tree as well as a home-relative spelling.
        if host_path.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(".lithe-server"))
        }) {
            return Err(RemoteError::new(
                ErrorCode::PermissionDenied,
                "Native settings target is protected",
            ));
        }
        let workspace =
            open_root(&self.workspace).map_err(|error| source_error(error, &target.path))?;
        let workspace_metadata = workspace
            .metadata()
            .map_err(|error| source_error(error, &target.path))?;
        let mut identity = Sha256::new();
        append_identity(&mut identity, &workspace_metadata);
        identity.update(host_path.as_os_str().as_encoded_bytes());
        let host_path = host_path.into_os_string().into_string().map_err(|_| {
            RemoteError::new(
                ErrorCode::InvalidPath,
                "Native settings path cannot be displayed",
            )
        })?;
        let mut parent = match target.scope {
            NativeSettingsScope::Workspace => workspace,
            NativeSettingsScope::Home => {
                open_root(&root).map_err(|error| source_error(error, &target.path))?
            }
        };
        append_identity(
            &mut identity,
            &parent
                .metadata()
                .map_err(|error| source_error(error, &target.path))?,
        );
        let parent_result = parents.iter().try_for_each(|component| {
            parent = scoped_files::directory(parent.try_clone()?, std::slice::from_ref(component))?;
            append_identity(&mut identity, &parent.metadata()?);
            Ok::<(), io::Error>(())
        });
        let (parent, file) = match parent_result {
            Ok(()) => {
                let file = match read_file(&parent, name, MAX_SOURCE_BYTES) {
                    Ok(file) => Some(file),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(source_error(error, &target.path)),
                };
                (Some(parent), file)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (None, None),
            Err(error) => return Err(source_error(error, &target.path)),
        };
        let (revision, length, file) = match file {
            Some((file, metadata, bytes)) => {
                append_file_stamp(&mut identity, &metadata);
                (digest(&bytes), bytes.len(), Some((file, metadata)))
            }
            None => ("missing".into(), 0, None),
        };
        let writable = match (&parent, &file) {
            (Some(parent), Some((_, metadata))) => {
                can_replace(parent) && metadata.mode() & 0o222 != 0 && writable_file(parent, name)
            }
            (Some(parent), None) => can_replace(parent),
            (None, _) => false,
        };
        Ok(Source {
            baseline: NativeSettingsBaseline {
                target,
                host_path,
                revision,
                identity: format!("{:x}", identity.finalize()),
                writable,
            },
            parent,
            name: name.clone(),
            file,
            length,
        })
    }
}

struct Source {
    baseline: NativeSettingsBaseline,
    parent: Option<File>,
    name: CString,
    file: Option<(File, Metadata)>,
    length: usize,
}

fn open_root(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

fn append_identity(hash: &mut Sha256, metadata: &Metadata) {
    hash.update(metadata.dev().to_le_bytes());
    hash.update(metadata.ino().to_le_bytes());
}

fn append_file_stamp(hash: &mut Sha256, metadata: &Metadata) {
    append_identity(hash, metadata);
    hash.update(metadata.ctime().to_le_bytes());
    hash.update(metadata.ctime_nsec().to_le_bytes());
    hash.update(metadata.mode().to_le_bytes());
    hash.update(metadata.uid().to_le_bytes());
    hash.update(metadata.gid().to_le_bytes());
}

fn file_stamp(metadata: &Metadata) -> impl PartialEq {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.size(),
        metadata.nlink(),
    )
}

/// Refuses aliases and special files before reading bounded source or staging bytes.
fn read_file(parent: &File, name: &CStr, limit: usize) -> io::Result<(File, Metadata, Vec<u8>)> {
    let entry = scoped_files::status(parent.as_raw_fd(), Some(name))?;
    if entry.st_mode & libc::S_IFMT != libc::S_IFREG || entry.st_nlink != 1 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let file = scoped_files::open_at(parent.as_raw_fd(), name, libc::O_RDONLY | libc::O_NONBLOCK)?;
    let opened = scoped_files::status(file.as_raw_fd(), None)?;
    let before = file.metadata()?;
    if !before.is_file()
        || before.nlink() != 1
        || opened.st_dev != entry.st_dev
        || opened.st_ino != entry.st_ino
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    if before.len() > limit as u64 {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&file).take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    let after = file.metadata()?;
    if file_stamp(&before) != file_stamp(&after) {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    Ok((file, after, bytes))
}

fn writable_file(parent: &File, name: &CStr) -> bool {
    // SAFETY: parent owns the open directory descriptor and name is NUL-terminated.
    let access = unsafe {
        libc::faccessat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::W_OK,
            libc::AT_EACCESS | libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    access == 0
}

fn can_replace(parent: &File) -> bool {
    // SAFETY: parent owns the open directory descriptor and the name is static.
    let access = unsafe {
        libc::faccessat(
            parent.as_raw_fd(),
            c".".as_ptr(),
            libc::W_OK | libc::X_OK,
            libc::AT_EACCESS,
        )
    };
    access == 0
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn inspect(
    roots: &Roots,
    targets: Vec<NativeSettingsTarget>,
) -> Result<Vec<NativeSettingsBaseline>, RemoteError> {
    if targets.len() > MAX_SOURCES {
        return Err(too_large());
    }
    let mut total = 0;
    let mut paths = HashSet::new();
    let mut files = HashSet::new();
    targets
        .into_iter()
        .map(|target| {
            let source = roots.inspect(target)?;
            total += source.length;
            if total > MAX_TOTAL_BYTES {
                return Err(too_large());
            }
            if !paths.insert(source.baseline.host_path.clone())
                || source
                    .file
                    .as_ref()
                    .is_some_and(|(_, file)| !files.insert((file.dev(), file.ino())))
            {
                return Err(conflict());
            }
            Ok(source.baseline)
        })
        .collect()
}

fn verify(
    current: &NativeSettingsBaseline,
    baseline: &NativeSettingsBaseline,
) -> Result<(), RemoteError> {
    if current != baseline {
        return Err(conflict());
    }
    if !baseline.writable {
        return Err(RemoteError::new(
            ErrorCode::PermissionDenied,
            "Native settings source is read-only",
        ));
    }
    Ok(())
}

fn stage(
    roots: &Roots,
    directory: &Path,
    baselines: Vec<NativeSettingsBaseline>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, RemoteError> {
    let sources = inspect(
        roots,
        baselines
            .iter()
            .map(|baseline| baseline.target.clone())
            .collect(),
    )?;
    for (source, baseline) in sources.iter().zip(&baselines) {
        verify(source, baseline)?;
    }
    let mut output = Vec::with_capacity(baselines.len());
    for (index, baseline) in baselines.into_iter().enumerate() {
        check_cancelled(cancelled)?;
        let record = serde_json::to_vec(&baseline).map_err(|_| invalid_stage())?;
        if record.len() > MAX_RECORD_BYTES {
            return Err(too_large());
        }
        let result = (|| {
            create_private(&directory.join(format!("{index}.baseline")))?.write_all(&record)?;
            let file = directory.join(format!("{index}.output"));
            create_private(&file)?;
            Ok(file)
        })()
        .map_err(|error| source_error(error, "native settings staging"))?;
        output.push(result);
    }
    to_value(output)
}

fn create_private(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

struct Prepared {
    replacement: NativeSettingsReplacement,
    content: Vec<u8>,
}

fn publish(
    roots: &Roots,
    directory: &Path,
    replacements: Vec<NativeSettingsReplacement>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Value, RemoteError> {
    // Serializes native settings publishers inside the same persistent daemon.
    let _publication = super::lock(&PUBLICATION);
    let scratch =
        open_root(directory).map_err(|error| source_error(error, "native settings staging"))?;
    let sources = inspect(
        roots,
        replacements
            .iter()
            .map(|entry| entry.baseline.target.clone())
            .collect(),
    )?;
    let mut total = 0;
    let mut slots = HashSet::new();
    let mut prepared = Vec::with_capacity(replacements.len());
    for (source, replacement) in sources.into_iter().zip(replacements) {
        check_cancelled(cancelled)?;
        verify(&source, &replacement.baseline)?;
        if !slots.insert(replacement.slot)
            || !replacement
                .revision
                .strip_prefix("sha256:")
                .is_some_and(is_sha256_hex)
        {
            return Err(invalid_stage());
        }
        let slot = replacement.slot;
        let (_, _, record) = read_file(&scratch, &slot_name(slot, "baseline"), MAX_RECORD_BYTES)
            .map_err(|error| source_error(error, "native settings staging"))?;
        let baseline: NativeSettingsBaseline =
            serde_json::from_slice(&record).map_err(|_| invalid_stage())?;
        if baseline != replacement.baseline {
            return Err(conflict());
        }
        let (_, _, content) = read_file(&scratch, &slot_name(slot, "output"), MAX_SOURCE_BYTES)
            .map_err(|error| source_error(error, "native settings staging"))?;
        total += content.len();
        if total > MAX_TOTAL_BYTES {
            return Err(too_large());
        }
        if digest(&content) != replacement.revision || baseline.revision == replacement.revision {
            return Err(invalid_stage());
        }
        prepared.push(Prepared {
            replacement,
            content,
        });
    }
    // All helper outputs have been verified. Keep only one target open at a
    // time so batches do not exhaust the server's descriptor budget.
    let mut committed = Vec::new();
    for entry in prepared {
        let baseline = &entry.replacement.baseline;
        let result = (|| {
            check_cancelled(cancelled)?;
            let source = roots.inspect(baseline.target.clone())?;
            verify(&source.baseline, baseline)?;
            let parent = source.parent.as_ref().ok_or_else(conflict)?;
            let metadata = source.file.as_ref();
            let mode = metadata.map_or(0o600, |(_, file)| file.mode() & 0o7777);
            let temporary = Temporary::write_with_metadata(
                parent,
                &entry.content,
                mode as libc::mode_t,
                metadata.map(|(file, _)| file),
            )
            .map_err(|error| source_error(error, &baseline.target.path))?;
            check_cancelled(cancelled)?;
            verify(&roots.inspect(baseline.target.clone())?.baseline, baseline)?;
            temporary
                .install(&source.name, source.file.is_none())
                .map_err(|error| source_error(error, &baseline.target.path))
        })();
        if let Err(error) = result {
            return Err(error.with_details(json!({"committedSlots": committed})));
        }
        committed.push(entry.replacement.slot);
    }
    Ok(json!({"committedSlots": committed}))
}

fn slot_name(slot: u16, suffix: &str) -> CString {
    CString::new(format!("{slot}.{suffix}")).expect("staging name has no NUL")
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), RemoteError> {
    if cancelled() {
        return Err(RemoteError::new(
            ErrorCode::NotFound,
            "Native settings staging was cancelled",
        ));
    }
    Ok(())
}

fn invalid_stage() -> RemoteError {
    RemoteError::new(
        ErrorCode::InvalidRequest,
        "Invalid native settings staging receipt",
    )
}

fn conflict() -> RemoteError {
    RemoteError::new(
        ErrorCode::Conflict,
        "Native settings source changed after it was read",
    )
}

fn too_large() -> RemoteError {
    RemoteError::new(
        ErrorCode::TooLarge,
        "Native settings exceeded their size budget",
    )
}

fn source_error(error: io::Error, path: &str) -> RemoteError {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::AlreadyExists => conflict(),
        io::ErrorKind::FileTooLarge => too_large(),
        _ if error.raw_os_error() == Some(libc::ELOOP) => RemoteError::new(
            ErrorCode::PermissionDenied,
            "Native settings cannot follow symbolic links",
        ),
        _ => fs::io_error(error, path),
    }
}
