//! Incremental reading of session files: deciding whether a file only grew
//! or was rewritten, and cutting the new bytes into bounded chunks of whole
//! lines.
//!
//! Agents append to their session files, so a sync normally reads only what
//! was added. A file that shrank, or whose already read bytes changed, is
//! read again from the start; the fingerprint below detects the change
//! without reading the whole file.

use std::fs::{File, Metadata};
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::time::UNIX_EPOCH;

use lithe_protocol::CoreError;
use sha2::{Digest, Sha256};

/// Most bytes of whole lines handed to a parser at once. A single longer line
/// (up to [`MAX_LINE_BYTES`]) is handed over alone.
pub const MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Longest line, newline included, handed to a parser. Agents can write a
/// whole attachment on one line; longer lines are skipped, counted in the
/// sync report, and never held in memory whole.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Bytes at each end of the read part of a file that its fingerprint covers.
const FINGERPRINT_EDGE_BYTES: u64 = 4096;

/// Marks a modification time the platform did not report. It never equals a
/// real time, so such a file is always checked against its fingerprint.
pub(crate) const UNKNOWN_MODIFIED_MS: i64 = i64::MIN;

/// Bounds of one pass over a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkLimits {
    pub(crate) max_chunk_bytes: usize,
    pub(crate) max_line_bytes: usize,
}

impl Default for ChunkLimits {
    fn default() -> Self {
        Self {
            max_chunk_bytes: MAX_CHUNK_BYTES,
            max_line_bytes: MAX_LINE_BYTES,
        }
    }
}

/// Size and modification time of a file when a sync looked at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub(crate) size: u64,
    pub(crate) modified_ms: i64,
}

impl FileStamp {
    pub(crate) fn of(metadata: &Metadata) -> Self {
        // Platforms without modification times still sync correctly: the
        // fingerprint check below decides instead of the stamp.
        let modified_ms = match metadata.modified() {
            Ok(time) => match time.duration_since(UNIX_EPOCH) {
                Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
                Err(before_epoch) => {
                    -i64::try_from(before_epoch.duration().as_millis()).unwrap_or(i64::MAX)
                }
            },
            Err(_) => UNKNOWN_MODIFIED_MS,
        };
        Self {
            size: metadata.len(),
            modified_ms,
        }
    }
}

/// What the store remembers about a file between syncs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    /// End of the last whole line read; everything before it is in the cache.
    pub(crate) offset: u64,
    /// The stamp of the file when it was last read to the end. `None` while
    /// a pass is unfinished, so the next sync never takes the file as
    /// unchanged.
    pub(crate) stamp: Option<FileStamp>,
    /// [`fingerprint`] of the bytes before `offset`.
    pub(crate) fingerprint: Vec<u8>,
}

/// How a sync reads a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    /// The file has the stamp it had when it was last read to the end.
    Unchanged,
    /// The read bytes are as they were; read on from `offset`.
    Append { offset: u64 },
    /// The file is new, shrank or changed; drop what came from it and read it
    /// from the start.
    FromStart,
}

/// Decides how to read `file`, which has `stamp` now and `cursor` from the
/// previous sync.
pub(crate) fn plan(file: &mut File, stamp: FileStamp, cursor: Option<&Cursor>) -> io::Result<Plan> {
    let Some(cursor) = cursor else {
        return Ok(Plan::FromStart);
    };
    if cursor.stamp == Some(stamp) && stamp.modified_ms != UNKNOWN_MODIFIED_MS {
        return Ok(Plan::Unchanged);
    }
    if stamp.size < cursor.offset {
        return Ok(Plan::FromStart);
    }
    if fingerprint(file, cursor.offset)? != cursor.fingerprint {
        return Ok(Plan::FromStart);
    }
    Ok(Plan::Append {
        offset: cursor.offset,
    })
}

/// SHA-256 of the first and the last [`FINGERPRINT_EDGE_BYTES`] before
/// `offset`, without overlap. Appending never changes it; rewriting the
/// start of the file, or the lines read last, does. A rewrite that keeps
/// both ends and the length is not detected, which agents that append to
/// their files do not do.
pub(crate) fn fingerprint(file: &mut File, offset: u64) -> io::Result<Vec<u8>> {
    let head_end = offset.min(FINGERPRINT_EDGE_BYTES);
    let tail_start = offset.saturating_sub(FINGERPRINT_EDGE_BYTES).max(head_end);
    let mut hasher = Sha256::new();
    hash_range(file, 0, head_end, &mut hasher)?;
    hash_range(file, tail_start, offset, &mut hasher)?;
    Ok(hasher.finalize().to_vec())
}

fn hash_range(file: &mut File, start: u64, end: u64, hasher: &mut Sha256) -> io::Result<()> {
    if start >= end {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    let length = usize::try_from(end - start).unwrap_or(usize::MAX);
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes)?;
    hasher.update(&bytes);
    Ok(())
}

/// What one pass over a file read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LineProgress {
    /// Offset just past the last whole line, handed over or skipped.
    pub(crate) end: u64,
    /// Bytes consumed from the file, a trailing partial line included.
    pub(crate) bytes_read: u64,
    /// Lines longer than the limit, which were skipped.
    pub(crate) skipped_lines: u64,
}

/// Reads whole lines from `reader`, which is positioned at file offset
/// `start`, consuming at most `limit` bytes. `on_chunk` receives each chunk
/// with the file offset of its first byte. The bytes of a chunk are
/// contiguous in the file: a skipped line ends the chunk before it. A last
/// line without a newline is left for a later pass, when the agent has
/// finished writing it. A read failure becomes `E` through [`read_failed`];
/// an error of `on_chunk` stops the pass and is returned as is.
pub(crate) fn read_lines<E: From<CoreError>>(
    reader: &mut impl BufRead,
    start: u64,
    limit: u64,
    limits: ChunkLimits,
    mut on_chunk: impl FnMut(u64, &[u8]) -> Result<(), E>,
) -> Result<LineProgress, E> {
    let read_end = start.saturating_add(limit);
    let mut position = start;
    let mut progress = LineProgress {
        end: start,
        ..LineProgress::default()
    };
    let mut chunk: Vec<u8> = Vec::new();
    let mut chunk_start = start;
    let mut line: Vec<u8> = Vec::new();
    let mut skipping = false;

    while position < read_end {
        let available = reader
            .fill_buf()
            .map_err(|error| E::from(read_failed(error)))?;
        if available.is_empty() {
            break;
        }
        let allowed = usize::try_from(read_end - position).unwrap_or(usize::MAX);
        let available = &available[..available.len().min(allowed)];
        let (taken, line_ended) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        if !skipping {
            if line.len() + taken > limits.max_line_bytes {
                skipping = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(&available[..taken]);
            }
        }
        reader.consume(taken);
        position += taken as u64;
        if !line_ended {
            continue;
        }

        if skipping {
            progress.skipped_lines += 1;
            skipping = false;
            if !chunk.is_empty() {
                on_chunk(chunk_start, &chunk)?;
                chunk.clear();
            }
        } else {
            if !chunk.is_empty() && chunk.len() + line.len() > limits.max_chunk_bytes {
                on_chunk(chunk_start, &chunk)?;
                chunk.clear();
            }
            if chunk.is_empty() {
                chunk_start = position - line.len() as u64;
            }
            chunk.extend_from_slice(&line);
            line.clear();
        }
        progress.end = position;
    }
    if !chunk.is_empty() {
        on_chunk(chunk_start, &chunk)?;
    }
    progress.bytes_read = position - start;
    Ok(progress)
}

pub(crate) fn read_failed(error: io::Error) -> CoreError {
    crate::store::io_error("Could not read an agent session file", error)
}
