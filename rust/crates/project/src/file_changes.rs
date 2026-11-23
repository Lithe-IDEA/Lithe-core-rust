//! Creating, renaming and copying entries below a workspace root.
//!
//! A host asks for these changes on a plugin's behalf (plan API-11), so
//! every one is bounded the same way on every host:
//!
//! - **One spelling of one root.** A path is `/`-separated and names an entry
//!   below the root without empty, `.` or `..` components
//!   ([`invalid_entry_path`]). The directory that holds the entry must exist
//!   and resolve inside the root; an entry is never created through a link
//!   that leaves it.
//! - **The last component is not followed.** A symbolic link is renamed or
//!   copied as a link, never as the entry it points to, and a copied
//!   directory keeps the links inside it as links.
//! - **Nothing is replaced.** A target that exists fails with
//!   [`ErrorCode::EntryExists`]. A rename that changes only the case of a name
//!   on a file system that ignores case is not a collision.
//!
//! The host keeps what only it owns: resolving a plugin's context to a root,
//! saving and moving the open documents under an entry, refreshing its views,
//! and moving entries to the system trash. Each command answers with the
//! entry's path below the root, its `fullPath` spelled from the root the host
//! sent, which is how the host's open documents name it, and its kind.
//!
//! Only local file systems are served; hosts refuse remote contexts before
//! asking.

use lithe_platform::entries::{create_symlink, rename_no_replace};
use lithe_platform::paths::invalid_entry_path;
use lithe_platform::Platform;
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const COPY_FAILED: &str = "Could not copy the entry";

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum EntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Create one file, directory or symbolic link (`workspace.createEntry`).
struct CreateEntryRequest {
    root: String,
    path: String,
    kind: EntryKind,
    /// The new file's UTF-8 text; files only, empty when absent.
    #[serde(default)]
    text: Option<String>,
    /// What the new link stores; symbolic links only.
    #[serde(default)]
    link_target: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Rename or copy one entry to a new path of the same root.
struct MoveEntryRequest {
    root: String,
    path: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Name one existing entry (`workspace.resolveEntry`).
struct ResolveEntryRequest {
    root: String,
    path: String,
}

/// The root the host sent and the directory it resolves to.
struct Root {
    given: PathBuf,
    canonical: PathBuf,
}

/// An entry below a root whose parent directory exists inside the root.
struct Entry {
    /// The request's `/`-separated path below the root.
    relative: String,
    /// The path spelled from the root the host sent.
    full: PathBuf,
    /// The entry in its resolved parent; the last component is not followed.
    resolved: PathBuf,
    /// The parent directory, resolved.
    parent: PathBuf,
}

impl Entry {
    fn result(&self, kind: &str) -> Value {
        json!({
            "path": self.relative,
            "fullPath": self.full.to_string_lossy(),
            "kind": kind,
        })
    }

    fn name(&self) -> &std::ffi::OsStr {
        self.resolved
            .file_name()
            .expect("an entry path has a last component")
    }
}

fn decode<T: for<'de> Deserialize<'de>>(payload: Value) -> Result<T, CoreError> {
    serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid file change request")
            .with_details(error.to_string())
    })
}

fn root(value: &str) -> Result<Root, CoreError> {
    let given = PathBuf::from(value);
    if !given.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Workspace root must be an absolute path",
        ));
    }
    let canonical = fs::canonicalize(&given).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => {
            CoreError::new(ErrorCode::WorkspaceNotFound, "Workspace does not exist")
        }
        _ => CoreError::new(ErrorCode::Unknown, "Could not open the workspace")
            .with_details(error.to_string()),
    })?;
    if !canonical.is_dir() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Workspace root must be a directory",
        ));
    }
    Ok(Root { given, canonical })
}

fn entry(root: &Root, path: &str) -> Result<Entry, CoreError> {
    if invalid_entry_path(path, Platform::HOST) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Path must name an entry below the workspace root",
        ));
    }
    let components: Vec<&str> = path.split('/').collect();
    let (name, parents) = components
        .split_last()
        .expect("a valid entry path has a component");
    let unresolved = parents
        .iter()
        .fold(root.canonical.clone(), |path, part| path.join(part));
    let parent = fs::canonicalize(&unresolved).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => missing_parent(),
        _ => failure("Could not open the entry's directory", &error),
    })?;
    if !parent.starts_with(&root.canonical) {
        return Err(CoreError::new(
            ErrorCode::PermissionDenied,
            "Path is outside the workspace",
        ));
    }
    if !parent.is_dir() {
        return Err(missing_parent());
    }
    Ok(Entry {
        relative: path.to_string(),
        full: components
            .iter()
            .fold(root.given.clone(), |path, part| path.join(part)),
        resolved: parent.join(name),
        parent,
    })
}

fn missing_parent() -> CoreError {
    CoreError::new(
        ErrorCode::EntryNotFound,
        "The directory that would hold the entry does not exist",
    )
}

fn failure(message: &str, error: &io::Error) -> CoreError {
    CoreError::new(ErrorCode::Unknown, message).with_details(error.to_string())
}

/// Maps a failure of the call that acts on the entry itself: an existing
/// target and a missing entry have codes of their own. A refusal by the
/// operating system is a failure, not [`ErrorCode::PermissionDenied`],
/// which says the path left the workspace.
fn entry_failure(message: &'static str) -> impl Fn(io::Error) -> CoreError {
    move |error| match error.kind() {
        io::ErrorKind::AlreadyExists => CoreError::new(
            ErrorCode::EntryExists,
            "An entry already exists at the target",
        ),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
            CoreError::new(ErrorCode::EntryNotFound, "The entry does not exist")
        }
        _ => failure(message, &error),
    }
}

/// `file`, `directory`, `symlink`, or `other` for a socket, pipe or device.
fn kind_name(metadata: &Metadata) -> &'static str {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_dir() {
        "directory"
    } else if file_type.is_file() {
        "file"
    } else {
        "other"
    }
}

/// The entry at `entry` without following it.
fn existing(entry: &Entry) -> Result<Metadata, CoreError> {
    fs::symlink_metadata(&entry.resolved).map_err(entry_failure("Could not read the entry"))
}

/// Creates one file, directory or symbolic link (`workspace.createEntry`).
///
/// `{ root, path, kind, text?, linkTarget? }`: a file takes optional UTF-8
/// `text`; a symbolic link takes the `linkTarget` it stores verbatim, which
/// may be absolute, relative to the link's directory, outside the root, or
/// missing (see [`create_symlink`] for Windows). Returns
/// `{ path, fullPath, kind }`.
pub fn create_entry_command(payload: Value) -> Result<Value, CoreError> {
    let request: CreateEntryRequest = decode(payload)?;
    let fields_match = match request.kind {
        EntryKind::File => request.link_target.is_none(),
        EntryKind::Directory => request.text.is_none() && request.link_target.is_none(),
        EntryKind::Symlink => {
            request.text.is_none()
                && request
                    .link_target
                    .as_deref()
                    .is_some_and(|target| !target.is_empty() && !target.contains('\0'))
        }
    };
    if !fields_match {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Only a file takes text, and only a symbolic link takes a non-empty linkTarget",
        ));
    }
    let root = root(&request.root)?;
    let entry = entry(&root, &request.path)?;
    let created = match request.kind {
        EntryKind::File => create_file(
            &entry.resolved,
            request.text.as_deref().unwrap_or_default().as_bytes(),
        ),
        EntryKind::Directory => fs::create_dir(&entry.resolved),
        EntryKind::Symlink => create_symlink(
            Path::new(request.link_target.as_deref().unwrap_or_default()),
            &entry.resolved,
        ),
    };
    created.map_err(entry_failure("Could not create the entry"))?;
    Ok(entry.result(match request.kind {
        EntryKind::File => "file",
        EntryKind::Directory => "directory",
        EntryKind::Symlink => "symlink",
    }))
}

/// Writes a new file, removing it again when the write fails, so a failed
/// create leaves nothing behind.
fn create_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if written.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    written
}

/// The source and target of a rename or copy, after the checks both share.
fn move_entries(payload: Value) -> Result<(Entry, Entry, Metadata), CoreError> {
    let request: MoveEntryRequest = decode(payload)?;
    let root = root(&request.root)?;
    let source = entry(&root, &request.path)?;
    let target = entry(&root, &request.target)?;
    let metadata = existing(&source)?;
    // A link is moved as a link, so only a real directory can contain the
    // target; its resolved path is then its canonical path.
    if metadata.is_dir() && target.parent.starts_with(&source.resolved) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The target cannot be inside the entry",
        ));
    }
    Ok((source, target, metadata))
}

/// Renames one entry to a new path of the same root
/// (`workspace.renameEntry`).
///
/// `{ root, path, target }`. Fails with [`ErrorCode::EntryExists`] instead
/// of replacing an entry at `target`, except when `target` differs from
/// `path` only in case and the file system ignores case. Returns
/// `{ path, fullPath, kind }` of the target. A target that names the source
/// itself is an existing entry: macOS reports success for that rename and
/// Linux refuses it, so the kernel refuses it before asking either.
pub fn rename_entry_command(payload: Value) -> Result<Value, CoreError> {
    let (source, target, metadata) = move_entries(payload)?;
    if source.resolved == target.resolved {
        return Err(CoreError::new(
            ErrorCode::EntryExists,
            "An entry already exists at the target",
        ));
    }
    let renamed = if changes_only_case(&source, &target) {
        fs::rename(&source.resolved, &target.resolved)
    } else {
        rename_no_replace(&source.resolved, &target.resolved)
    };
    renamed.map_err(entry_failure("Could not rename the entry"))?;
    Ok(target.result(kind_name(&metadata)))
}

/// Whether the rename only changes the case of the name on a file system
/// that ignores case: the target is found, yet no entry of the directory has
/// its exact name, so the name found is the source's own. Two names that
/// differ only in case cannot both exist there.
fn changes_only_case(source: &Entry, target: &Entry) -> bool {
    if source.parent != target.parent || source.name() == target.name() {
        return false;
    }
    let (Some(source_name), Some(target_name)) = (source.name().to_str(), target.name().to_str())
    else {
        return false;
    };
    if source_name.to_lowercase() != target_name.to_lowercase()
        || fs::symlink_metadata(&target.resolved).is_err()
    {
        return false;
    }
    fs::read_dir(&target.parent).is_ok_and(|entries| {
        !entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == target.name())
    })
}

/// Copies one entry to a new path of the same root (`workspace.copyEntry`).
///
/// `{ root, path, target }`. A directory is copied with everything in it;
/// symbolic links stay links, and sockets, pipes and devices fail the copy.
/// Nothing at `target` is replaced, and a copy that fails part way removes
/// what it created. Returns `{ path, fullPath, kind }` of the copy.
pub fn copy_entry_command(payload: Value) -> Result<Value, CoreError> {
    let (source, target, metadata) = move_entries(payload)?;
    copy_entry(
        &source.resolved,
        &target.resolved,
        &metadata,
        &entry_failure(COPY_FAILED),
    )?;
    Ok(target.result(kind_name(&metadata)))
}

fn copy_failure(error: io::Error) -> CoreError {
    failure(COPY_FAILED, &error)
}

/// Creates `to` as a copy of `from`, which `metadata` describes without
/// following it. `create_failure` maps a failure to create `to` itself;
/// every later failure removes what this call created.
fn copy_entry(
    from: &Path,
    to: &Path,
    metadata: &Metadata,
    create_failure: &dyn Fn(io::Error) -> CoreError,
) -> Result<(), CoreError> {
    lithe_protocol::cancellation::check()?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let link_target = fs::read_link(from).map_err(copy_failure)?;
        return create_symlink(&link_target, to).map_err(create_failure);
    }
    if file_type.is_dir() {
        fs::create_dir(to).map_err(create_failure)?;
        let filled = copy_children(from, to)
            .and_then(|()| fs::set_permissions(to, metadata.permissions()).map_err(copy_failure));
        if filled.is_err() {
            let _ = fs::remove_dir_all(to);
        }
        return filled;
    }
    if file_type.is_file() {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .map_err(create_failure)?;
        let filled = fs::File::open(from)
            .and_then(|mut input| io::copy(&mut input, &mut output))
            .and_then(|_| output.set_permissions(metadata.permissions()))
            .map_err(copy_failure);
        if filled.is_err() {
            drop(output);
            let _ = fs::remove_file(to);
        }
        return filled;
    }
    Err(CoreError::new(
        ErrorCode::Unknown,
        "Only files, directories and symbolic links can be copied",
    )
    .with_details(from.display().to_string()))
}

fn copy_children(from: &Path, to: &Path) -> Result<(), CoreError> {
    for child in fs::read_dir(from).map_err(copy_failure)? {
        let child = child.map_err(copy_failure)?;
        // `DirEntry::metadata` does not follow a link on any host.
        let metadata = child.metadata().map_err(copy_failure)?;
        copy_entry(
            &child.path(),
            &to.join(child.file_name()),
            &metadata,
            &copy_failure,
        )?;
    }
    Ok(())
}

/// Names one existing entry without following it (`workspace.resolveEntry`).
///
/// `{ root, path }`. Returns `{ path, fullPath, kind }`, so a host can save
/// the open documents under an entry or move it to the system trash by the
/// path the checks above admit.
pub fn resolve_entry_command(payload: Value) -> Result<Value, CoreError> {
    let request: ResolveEntryRequest = decode(payload)?;
    let root = root(&request.root)?;
    let entry = entry(&root, &request.path)?;
    let metadata = existing(&entry)?;
    Ok(entry.result(kind_name(&metadata)))
}
