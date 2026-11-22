//! Extracts a pinned runtime in connection-owned scratch, before cache publication.
//! Archive paths and links are validated independently of the workspace directory.

use crate::protocol::methods::{encode_hex, ManagedRuntimeLayout};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::fs::io_error;
use crate::server::plugin_deployment::hash_valid;
use crate::server::run_directory::private_directory;
use crate::server::runtime_zip;
use flate2::read::MultiGzDecoder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::{symlink, DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tar::{Archive, PaxExtensions};

pub(super) const MAX_ENTRIES: usize = 50_000;
const MAX_ARCHIVE: u64 = 512 << 20;
const MAX_EXPANDED: u64 = 4 << 30;
const MAX_TAR_STREAM: u64 = MAX_EXPANDED + (MAX_ENTRIES as u64 * 1024) + (32 << 20);
const MAX_PATH: usize = 512;
const MAX_TAR_METADATA: u64 = 64 << 10;
const MAX_RECEIPT: u64 = 4096;

pub(super) fn invalid(message: &'static str) -> RemoteError {
    RemoteError::new(ErrorCode::InvalidRequest, message)
}

pub(super) fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), RemoteError> {
    if cancelled() {
        return Err(RemoteError::new(
            ErrorCode::NotFound,
            "runtime staging cancelled",
        ));
    }
    Ok(())
}

pub(super) fn validate_layout(layout: &ManagedRuntimeLayout) -> Result<(), RemoteError> {
    if !hash_valid(&layout.archive_sha256) || !matches!(layout.archive.as_str(), "zip" | "tar.gz") {
        return Err(invalid("invalid runtime archive identity or format"));
    }
    for name in [&layout.strip_prefix, &layout.home, &layout.executable] {
        if !name.is_empty() {
            relative_path(name)?;
        }
    }
    Ok(())
}

/// Opens the same file for hashing and extraction and never follows an upload symlink.
fn open_private_file(path: &Path, limit: u64) -> Result<(File, u64), RemoteError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| io_error(error, "runtime input"))?;
    let info = file
        .metadata()
        .map_err(|error| io_error(error, "runtime input"))?;
    if !info.is_file()
        || info.uid() != crate::server::platform::current_uid()
        || info.mode() & 0o277 != 0
        || info.len() > limit
    {
        return Err(invalid("invalid private runtime file"));
    }
    Ok((file, info.len()))
}

/// Verifies the original compressed bytes and the declared layout before publication.
pub(super) fn prepare(
    directory: &Path,
    layout: &ManagedRuntimeLayout,
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, RemoteError> {
    let (mut archive, length) = open_private_file(&directory.join("archive"), MAX_ARCHIVE)?;
    let mut hash = Sha256::new();
    let count = io::copy(
        &mut CheckedRead::new(&mut archive, cancelled).take(length + 1),
        &mut hash,
    )
    .map_err(|error| io_error(error, "runtime archive checksum"))?;
    if count != length || encode_hex(&hash.finalize()) != layout.archive_sha256 {
        return Err(invalid("runtime archive checksum mismatch"));
    }
    archive
        .rewind()
        .map_err(|error| io_error(error, "runtime archive"))?;

    let staged = directory.join("runtime");
    private_directory(&staged)?;
    let root = staged.join("contents");
    private_directory(&root)?;
    let mut extraction = Extraction {
        root: &root,
        strip_prefix: &layout.strip_prefix,
        cancelled,
        entries: BTreeMap::new(),
        links: Vec::new(),
        expanded: 0,
    };
    match layout.archive.as_str() {
        "tar.gz" => extraction.tar_gzip(archive)?,
        "zip" => extraction.zip(archive, length)?,
        _ => return Err(invalid("invalid runtime archive format")),
    }
    extraction.finish_links()?;
    paths(&staged, layout)?;
    let receipt = serde_json::to_vec(layout).expect("runtime layout always serializes");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(staged.join("receipt.json"))
        .map_err(|error| io_error(error, "runtime receipt"))?;
    file.write_all(&receipt)
        .map_err(|error| io_error(error, "runtime receipt"))?;
    check_cancelled(cancelled)?;
    Ok(staged)
}

/// Checks the cache receipt and obtains native paths after acquiring its lease.
pub(super) fn published_paths(
    directory: &Path,
    layout: &ManagedRuntimeLayout,
) -> Result<Value, RemoteError> {
    let (file, _) = open_private_file(&directory.join("receipt.json"), MAX_RECEIPT)?;
    let receipt: ManagedRuntimeLayout = serde_json::from_reader(file.take(MAX_RECEIPT))
        .map_err(|_| invalid("invalid managed runtime receipt"))?;
    if receipt != *layout {
        return Err(invalid(
            "managed runtime layout does not match cache receipt",
        ));
    }
    paths(directory, layout)
}

fn paths(directory: &Path, layout: &ManagedRuntimeLayout) -> Result<Value, RemoteError> {
    let contents = directory.join("contents");
    let metadata =
        fs::symlink_metadata(&contents).map_err(|error| io_error(error, "runtime root"))?;
    crate::server::run_directory::require_private_directory(&metadata)?;
    let root = fs::canonicalize(contents).map_err(|error| io_error(error, "runtime root"))?;
    let home = fs::canonicalize(root.join(&layout.home))
        .map_err(|error| io_error(error, "runtime home"))?;
    if !home.starts_with(&root) || !home.is_dir() {
        return Err(invalid("invalid runtime home"));
    }
    let executable = if layout.executable.is_empty() {
        None
    } else {
        let file = fs::canonicalize(home.join(&layout.executable))
            .map_err(|error| io_error(error, "runtime executable"))?;
        let info = file
            .metadata()
            .map_err(|error| io_error(error, "runtime executable"))?;
        if !file.starts_with(&root) || !info.is_file() || info.mode() & 0o777 != 0o500 {
            return Err(invalid("invalid runtime executable"));
        }
        Some(file)
    };
    Ok(json!({"home": home, "executable": executable}))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    Link,
}

struct Link {
    name: String,
    target: PathBuf,
    hard: bool,
}

struct Extraction<'a> {
    root: &'a Path,
    strip_prefix: &'a str,
    cancelled: &'a dyn Fn() -> bool,
    // Case aliases must fail on a case-sensitive host too. Implicit parents
    // may be declared later, but actual archive members are unique.
    entries: BTreeMap<String, (String, Kind, bool)>,
    links: Vec<Link>,
    expanded: u64,
}

impl Extraction<'_> {
    fn tar_gzip(&mut self, file: File) -> Result<(), RemoteError> {
        let gzip = MultiGzDecoder::new(CheckedRead::new(file, self.cancelled));
        let mut reader = CheckedRead::new(gzip, self.cancelled).take(MAX_TAR_STREAM + 1);
        let mut archive = Archive::new(&mut reader);
        let entries = archive.entries().map_err(tar_error)?.raw(true);
        let mut metadata = TarMetadata::default();
        for (index, entry) in entries.enumerate() {
            check_cancelled(self.cancelled)?;
            if index >= MAX_ENTRIES {
                return Err(invalid("too many runtime archive entries"));
            }
            let mut entry = entry.map_err(tar_error)?;
            let kind = entry.header().entry_type();
            if kind.is_gnu_longname()
                || kind.is_gnu_longlink()
                || kind.is_pax_local_extensions()
                || kind.is_pax_global_extensions()
            {
                metadata.read_extension(&mut entry)?;
                continue;
            }
            let (layout, hard) = match kind {
                tar::EntryType::Regular => (Kind::File, false),
                tar::EntryType::Directory => (Kind::Directory, false),
                tar::EntryType::Symlink => (Kind::Link, false),
                tar::EntryType::Link => (Kind::Link, true),
                _ => return Err(invalid("unsupported runtime tar entry")),
            };
            metadata.validate_entry(entry.size(), layout)?;
            let name = match metadata.name.take() {
                Some(name) => name,
                None => utf8(&entry.path_bytes())?.to_owned(),
            };
            let target = match metadata.link.take() {
                Some(target) => Some(target),
                None => entry
                    .link_name_bytes()
                    .map(|name| utf8(&name).map(str::to_owned))
                    .transpose()?,
            };
            metadata = TarMetadata::default();
            let mode = entry.header().mode().map_err(tar_error)?;
            self.entry(&name, layout, mode, entry.size(), &mut entry, target, hard)?;
        }
        if metadata.present() {
            return Err(invalid("runtime tar metadata has no following entry"));
        }
        // Consume the gzip trailer (and only zero tar padding) to check CRC
        // and to count decompressed bytes hidden after the last tar header.
        let mut bytes = [0; 8192];
        loop {
            let size = reader.read(&mut bytes).map_err(tar_error)?;
            if size == 0 {
                break;
            }
            if reader.limit() == 0 || bytes[..size].iter().any(|byte| *byte != 0) {
                return Err(invalid("invalid runtime tar padding or stream budget"));
            }
        }
        Ok(())
    }

    fn zip(&mut self, file: File, length: u64) -> Result<(), RemoteError> {
        let mut archive = runtime_zip::open(file, length, self.cancelled)?;
        for index in 0..archive.len() {
            check_cancelled(self.cancelled)?;
            let mut entry = archive.by_index(index).map_err(runtime_zip::error)?;
            let name = utf8(entry.name_raw())?.to_owned();
            let mode = entry.unix_mode().unwrap_or(0);
            let kind = match mode & 0o170000 {
                0 if entry.is_dir() => Kind::Directory,
                0 | 0o100000 if !entry.is_dir() => Kind::File,
                0o040000 if entry.is_dir() => Kind::Directory,
                0o120000 if !entry.is_dir() => Kind::Link,
                _ => return Err(invalid("unsupported runtime ZIP entry")),
            };
            self.entry(&name, kind, mode, entry.size(), &mut entry, None, false)?;
        }
        Ok(())
    }

    /// Reserves paths before creating anything and defers every link until files exist.
    fn entry(
        &mut self,
        raw: &str,
        kind: Kind,
        mode: u32,
        size: u64,
        reader: &mut dyn Read,
        target: Option<String>,
        hard: bool,
    ) -> Result<(), RemoteError> {
        self.expanded = self
            .expanded
            .checked_add(size)
            .filter(|size| *size <= MAX_EXPANDED)
            .ok_or_else(|| invalid("runtime archive exceeds its expanded byte budget"))?;
        let Some(name) = self.stripped(raw, kind == Kind::Directory)? else {
            if size != 0 {
                return Err(invalid("runtime archive directory contains data"));
            }
            return Ok(());
        };
        self.reserve(name, kind, true)?;
        let path = self.root.join(name);
        let parent = path.parent().expect("runtime entry has a parent");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|error| io_error(error, "runtime archive directory"))?;
        match kind {
            Kind::Directory if size == 0 => DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|error| io_error(error, "runtime archive directory")),
            Kind::Directory => Err(invalid("runtime archive directory contains data")),
            Kind::Link => {
                let target = match target {
                    Some(target) if size == 0 => target,
                    Some(_) => return Err(invalid("runtime tar link contains data")),
                    None => {
                        let mut target = Vec::new();
                        CheckedRead::new(reader, self.cancelled)
                            .take(MAX_PATH as u64 + 1)
                            .read_to_end(&mut target)
                            .map_err(|error| io_error(error, "runtime link target"))?;
                        if target.len() as u64 != size {
                            return Err(invalid("invalid runtime link size"));
                        }
                        utf8(&target)?.to_owned()
                    }
                };
                let target = if hard {
                    PathBuf::from(
                        self.stripped(&target, false)?
                            .ok_or_else(|| invalid("invalid runtime hard link target"))?,
                    )
                } else {
                    relative_link(name, &target)?
                };
                self.links.push(Link {
                    name: name.to_owned(),
                    target,
                    hard,
                });
                Ok(())
            }
            Kind::File => {
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
                    .map_err(|error| io_error(error, "runtime archive file"))?;
                let actual = io::copy(
                    &mut CheckedRead::new(reader, self.cancelled).take(size + 1),
                    &mut output,
                )
                .map_err(|error| io_error(error, "runtime archive file"))?;
                if actual != size {
                    return Err(invalid("runtime archive file size mismatch"));
                }
                output
                    .set_permissions(Permissions::from_mode(0o400 | (mode & 0o100)))
                    .map_err(|error| io_error(error, "runtime archive file permissions"))
            }
        }
    }

    fn stripped<'a>(&self, raw: &'a str, directory: bool) -> Result<Option<&'a str>, RemoteError> {
        let raw = raw.trim_start_matches("./");
        let raw = if directory {
            raw.strip_suffix('/').unwrap_or(raw)
        } else {
            raw
        };
        if directory && (raw == "." || raw.is_empty()) {
            return Ok(None);
        }
        relative_path(raw)?;
        if self.strip_prefix.is_empty() {
            return Ok(Some(raw));
        }
        if directory && raw == self.strip_prefix {
            return Ok(None);
        }
        let name = raw
            .strip_prefix(self.strip_prefix)
            .and_then(|name| name.strip_prefix('/'))
            .ok_or_else(|| invalid("runtime archive entry is outside strip prefix"))?;
        Ok(Some(name))
    }

    fn reserve(&mut self, name: &str, kind: Kind, explicit: bool) -> Result<(), RemoteError> {
        let key = name.to_lowercase();
        if let Some((previous, existing, already_explicit)) = self.entries.get_mut(&key) {
            if previous != name
                || *existing != Kind::Directory
                || kind != Kind::Directory
                || (explicit && *already_explicit)
            {
                return Err(invalid("duplicate or conflicting runtime archive path"));
            }
            *already_explicit |= explicit;
            return Ok(());
        }
        if self.entries.len() >= MAX_ENTRIES {
            return Err(invalid("too many runtime paths"));
        }
        self.entries.insert(key, (name.to_owned(), kind, explicit));
        if let Some((parent, _)) = name.rsplit_once('/') {
            self.reserve(parent, Kind::Directory, false)?;
        }
        Ok(())
    }

    fn finish_links(&self) -> Result<(), RemoteError> {
        for link in &self.links {
            check_cancelled(self.cancelled)?;
            let name = self.root.join(&link.name);
            if link.hard {
                let source = self.root.join(&link.target);
                let target = link
                    .target
                    .to_str()
                    .expect("archive link was validated as UTF-8");
                let regular = self
                    .entries
                    .get(&target.to_lowercase())
                    .is_some_and(|(name, kind, _)| name == target && *kind == Kind::File);
                if !regular {
                    return Err(invalid(
                        "runtime hard link target is not a regular archive file",
                    ));
                }
                fs::hard_link(source, name)
                    .map_err(|error| io_error(error, "runtime hard link"))?;
            } else {
                symlink(&link.target, name).map_err(|error| io_error(error, "runtime symlink"))?;
            }
        }
        let root = fs::canonicalize(self.root).map_err(|error| io_error(error, "runtime root"))?;
        for link in &self.links {
            check_cancelled(self.cancelled)?;
            let destination = fs::canonicalize(self.root.join(&link.name))
                .map_err(|error| io_error(error, "runtime link target"))?;
            if !destination.starts_with(&root) {
                return Err(invalid("runtime link escapes extracted directory"));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct TarMetadata {
    name: Option<String>,
    link: Option<String>,
    size: Option<u64>,
    pax: bool,
}

impl TarMetadata {
    fn present(&self) -> bool {
        self.name.is_some() || self.link.is_some() || self.pax
    }

    fn read_extension<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
    ) -> Result<(), RemoteError> {
        let kind = entry.header().entry_type();
        if entry.size() > MAX_TAR_METADATA {
            return Err(invalid("runtime tar metadata is too large"));
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(tar_error)?;
        if data.len() as u64 != entry.size() {
            return Err(invalid("truncated runtime tar metadata"));
        }
        if kind.is_gnu_longname() || kind.is_gnu_longlink() {
            let value = utf8(data.strip_suffix(&[0]).unwrap_or(&data))?;
            let slot = if kind.is_gnu_longname() {
                &mut self.name
            } else {
                &mut self.link
            };
            return set_path(slot, value);
        }

        let local = kind.is_pax_local_extensions();
        if local && std::mem::replace(&mut self.pax, true) {
            return Err(invalid("duplicate runtime tar PAX header"));
        }
        for property in PaxExtensions::new(&data) {
            let property = property.map_err(tar_error)?;
            match property.key_bytes() {
                b"path" if local => set_path(&mut self.name, utf8(property.value_bytes())?)?,
                b"linkpath" if local => set_path(&mut self.link, utf8(property.value_bytes())?)?,
                b"size" if local && self.size.is_none() => {
                    self.size = Some(
                        utf8(property.value_bytes())?
                            .parse()
                            .map_err(|_| invalid("invalid runtime tar PAX size"))?,
                    );
                }
                b"path" | b"linkpath" | b"size" => {
                    return Err(invalid("ambiguous runtime tar PAX layout"))
                }
                key if key.starts_with(b"GNU.sparse.") || key == b"SCHILY.realsize" => {
                    return Err(invalid("sparse runtime tar entries are unsupported"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_entry(&self, size: u64, kind: Kind) -> Result<(), RemoteError> {
        if self.size.is_some_and(|expected| expected != size)
            || (self.link.is_some() && kind != Kind::Link)
        {
            return Err(invalid("runtime tar PAX layout disagrees with its entry"));
        }
        Ok(())
    }
}

fn set_path(slot: &mut Option<String>, path: &str) -> Result<(), RemoteError> {
    if slot.is_some() || path.len() > MAX_PATH {
        return Err(invalid("invalid runtime tar path metadata"));
    }
    *slot = Some(path.to_owned());
    Ok(())
}

fn tar_error(error: io::Error) -> RemoteError {
    io_error(error, "runtime tar archive")
}

fn utf8(value: &[u8]) -> Result<&str, RemoteError> {
    std::str::from_utf8(value).map_err(|_| invalid("runtime archive path is not UTF-8"))
}

fn relative_path(name: &str) -> Result<(), RemoteError> {
    if name.is_empty()
        || name.len() > MAX_PATH
        || name.trim() != name
        || name.contains(['\\', ':'])
        || name.chars().any(char::is_control)
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.len() > 255)
    {
        return Err(invalid("invalid relative runtime archive path"));
    }
    Ok(())
}

/// Normalize relative traversal before creating the link, so intervening
/// symlinks cannot change the meaning of a later parent component.
fn relative_link(name: &str, target: &str) -> Result<PathBuf, RemoteError> {
    if target.is_empty()
        || target.len() > MAX_PATH
        || target.starts_with('/')
        || target.contains(['\\', ':'])
        || target.chars().any(char::is_control)
    {
        return Err(invalid("invalid runtime symlink target"));
    }
    let parent: Vec<_> = name.split('/').take(name.matches('/').count()).collect();
    let mut destination = parent.clone();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                destination
                    .pop()
                    .ok_or_else(|| invalid("runtime link escapes extracted directory"))?;
            }
            value => {
                relative_path(value)?;
                destination.push(value);
            }
        }
    }
    let common = parent
        .iter()
        .zip(&destination)
        .take_while(|(lhs, rhs)| lhs == rhs)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..parent.len() {
        relative.push("..");
    }
    relative.extend(&destination[common..]);
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    Ok(relative)
}

/// Checks cancellation on each bounded I/O operation, including archive hashing.
pub(super) struct CheckedRead<'a, R> {
    source: R,
    cancelled: &'a dyn Fn() -> bool,
}

impl<'a, R> CheckedRead<'a, R> {
    pub(super) fn new(source: R, cancelled: &'a dyn Fn() -> bool) -> Self {
        Self { source, cancelled }
    }
}

impl<R: Read> Read for CheckedRead<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            return Err(io::Error::other("runtime staging cancelled"));
        }
        self.source.read(bytes)
    }
}
