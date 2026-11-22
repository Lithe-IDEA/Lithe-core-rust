//! Bounds ZIP metadata before the ZIP reader allocates entry records.

use super::managed_runtime::{check_cancelled, invalid, MAX_ENTRIES};
use crate::protocol::RemoteError;
use crate::server::fs::io_error;
use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::rc::Rc;
use zip::read::{ArchiveOffset, Config};
use zip::result::ZipError;
use zip::ZipArchive;

const EOCD_LEN: usize = 22;
const EOCD: [u8; 4] = *b"PK\x05\x06";
const ZIP64_LOCATOR: [u8; 4] = *b"PK\x06\x07";
const ZIP64_END: [u8; 4] = *b"PK\x06\x06";
const CENTRAL_HEADER: [u8; 4] = *b"PK\x01\x02";
const LOCAL_HEADER_LEN: usize = 30;
const LOCAL_HEADER: [u8; 4] = *b"PK\x03\x04";
const MAX_CENTRAL: u64 = 32 << 20;

pub(super) fn error(_: ZipError) -> RemoteError {
    invalid("invalid runtime ZIP archive")
}

pub(super) fn open<'a>(
    mut file: File,
    length: u64,
    cancelled: &'a dyn Fn() -> bool,
) -> Result<ZipArchive<ZipView<'a>>, RemoteError> {
    let (offset, size, count) = directory(&mut file, length)?;
    let mut central = vec![0; size as usize];
    file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
    super::managed_runtime::CheckedRead::new(&mut file, cancelled)
        .read_exact(&mut central)
        .map_err(read_error)?;
    let mut rest = central.as_slice();
    for _ in 0..count {
        check_cancelled(cancelled)?;
        if rest.len() < 46 || !rest.starts_with(&CENTRAL_HEADER) {
            return Err(invalid("invalid runtime ZIP central entry"));
        }
        let size = 46
            + usize::from(word(rest, 28))
            + usize::from(word(rest, 30))
            + usize::from(word(rest, 32));
        rest = rest
            .get(size..)
            .ok_or_else(|| invalid("truncated runtime ZIP central entry"))?;
    }
    if !rest.is_empty() || central.windows(EOCD.len()).any(|bytes| bytes == EOCD) {
        return Err(invalid("ambiguous runtime ZIP central directory"));
    }

    // Present just the checked directory and a minimal footer to the library.
    // The ZIP reader also inspects fixed local headers while loading metadata.
    // Expose just their signature and length fields during those reads so a
    // rejected directory cannot cause it to find another footer in the payload.
    // A small original ZIP64 directory fits the same bounded ZIP32 view.
    let mut end = [0; EOCD_LEN];
    end[..4].copy_from_slice(&EOCD);
    end[8..10].copy_from_slice(&(count as u16).to_le_bytes());
    end[10..12].copy_from_slice(&(count as u16).to_le_bytes());
    end[12..16].copy_from_slice(&(size as u32).to_le_bytes());
    end[16..20].copy_from_slice(&(offset as u32).to_le_bytes());
    central.extend_from_slice(&end);
    let metadata_only = Rc::new(Cell::new(true));
    let view = ZipView {
        file,
        offset,
        central,
        position: 0,
        metadata_only: Rc::clone(&metadata_only),
        cancelled,
    };
    let config = Config {
        archive_offset: ArchiveOffset::Known(0),
    };
    let archive = ZipArchive::with_config(config, view).map_err(error)?;
    if archive.len() != count as usize || archive.central_directory_start() != offset {
        return Err(invalid("duplicate or conflicting runtime ZIP entry"));
    }
    metadata_only.set(false);
    Ok(archive)
}

fn directory(file: &mut File, length: u64) -> Result<(u64, u64, u64), RemoteError> {
    let tail_size = length.min((EOCD_LEN + u16::MAX as usize) as u64);
    let mut tail = vec![0; tail_size as usize];
    file.seek(SeekFrom::Start(length - tail_size))
        .map_err(read_error)?;
    file.read_exact(&mut tail).map_err(read_error)?;
    let (index, end) = tail
        .windows(EOCD_LEN)
        .enumerate()
        .rev()
        .find(|(index, bytes)| {
            bytes.starts_with(&EOCD)
                && index + EOCD_LEN + usize::from(word(bytes, 20)) == tail.len()
        })
        .ok_or_else(|| invalid("missing runtime ZIP footer"))?;
    let footer = length - tail_size + index as u64;
    if word(end, 4) != 0 || word(end, 6) != 0 || word(end, 8) != word(end, 10) {
        return Err(invalid("split runtime ZIP archives are unsupported"));
    }
    let count = u64::from(word(end, 10));
    let offset = u64::from(dword(end, 16));
    let size = u64::from(dword(end, 12));
    let (offset, size, count, footer) = if count == u64::from(u16::MAX)
        || size == u64::from(u32::MAX)
        || offset == u64::from(u32::MAX)
        || offset.checked_add(size) != Some(footer)
    {
        zip64_directory(file, footer)?
    } else {
        (offset, size, count, footer)
    };
    if count == 0
        || count > MAX_ENTRIES as u64
        || size > MAX_CENTRAL
        || size < 46 * count
        || offset.checked_add(size) != Some(footer)
    {
        return Err(invalid("invalid runtime ZIP central directory budget"));
    }
    Ok((offset, size, count))
}

fn zip64_directory(file: &mut File, footer: u64) -> Result<(u64, u64, u64, u64), RemoteError> {
    let locator_offset = footer
        .checked_sub(20)
        .ok_or_else(|| invalid("missing runtime ZIP64 locator"))?;
    let mut locator = [0; 20];
    file.seek(SeekFrom::Start(locator_offset))
        .map_err(read_error)?;
    file.read_exact(&mut locator).map_err(read_error)?;
    if !locator.starts_with(&ZIP64_LOCATOR) || dword(&locator, 4) != 0 || dword(&locator, 16) != 1 {
        return Err(invalid("invalid runtime ZIP64 locator"));
    }
    let position = qword(&locator, 8);
    if position
        .checked_add(56)
        .is_none_or(|end| end > locator_offset)
    {
        return Err(invalid("invalid runtime ZIP64 footer position"));
    }
    let mut end = [0; 56];
    file.seek(SeekFrom::Start(position)).map_err(read_error)?;
    file.read_exact(&mut end).map_err(read_error)?;
    if !end.starts_with(&ZIP64_END)
        || dword(&end, 16) != 0
        || dword(&end, 20) != 0
        || qword(&end, 24) != qword(&end, 32)
        || position
            .checked_add(12)
            .and_then(|start| start.checked_add(qword(&end, 4)))
            != Some(locator_offset)
    {
        return Err(invalid("invalid runtime ZIP64 footer"));
    }
    Ok((qword(&end, 48), qword(&end, 40), qword(&end, 32), position))
}

fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("bounded ZIP header"))
}

fn dword(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("bounded ZIP header"))
}

fn qword(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("bounded ZIP header"))
}

fn read_error(error: io::Error) -> RemoteError {
    io_error(error, "runtime ZIP archive")
}

/// Keeps local header offsets intact while exposing only checked metadata to
/// the initial ZIP parse. Once parsed, content is streamed from the archive.
pub(super) struct ZipView<'a> {
    file: File,
    offset: u64,
    central: Vec<u8>,
    position: u64,
    metadata_only: Rc<Cell<bool>>,
    cancelled: &'a dyn Fn() -> bool,
}

impl Read for ZipView<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            return Err(io::Error::other("runtime staging cancelled"));
        }
        let count = if self.position < self.offset {
            let bound = (self.offset - self.position).min(bytes.len() as u64) as usize;
            if self.metadata_only.get() {
                if self.offset - self.position > 65536 {
                    return Err(io::Error::other(
                        "ZIP footer search left the validated directory",
                    ));
                }
                let is_local_header = bytes.len() == LOCAL_HEADER_LEN && bound == LOCAL_HEADER_LEN;
                if is_local_header {
                    self.file.seek(SeekFrom::Start(self.position))?;
                    self.file.read_exact(bytes)?;
                }
                if is_local_header && bytes.starts_with(&LOCAL_HEADER) {
                    if bytes[26..30] == EOCD {
                        return Err(io::Error::other("ambiguous runtime ZIP local header"));
                    }
                    bytes[4..26].fill(0);
                } else {
                    bytes[..bound].fill(0);
                }
                bound
            } else {
                self.file.seek(SeekFrom::Start(self.position))?;
                self.file.read(&mut bytes[..bound])?
            }
        } else if self.metadata_only.get() {
            let index = (self.position - self.offset).min(self.central.len() as u64);
            (&self.central[index as usize..]).read(bytes)?
        } else {
            0
        };
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for ZipView<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.position = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => {
                (self.offset + self.central.len() as u64).checked_add_signed(delta)
            }
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid ZIP offset"))?;
        Ok(self.position)
    }
}
