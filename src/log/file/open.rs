//! \[DESIGN\] A file log opened and recovered (RULES.md D25, D30): see the
//! parent module. Split from file.rs (RULES.md R-SIZE-2).
//!
//! The segments are opened in order, nothing cut, until one has bytes to
//! cut: a torn end. What is changed is decided before anything is: a
//! segment damaged in its middle (`Recovery::damage`) refuses the log; so
//! does one whose invalid end shows no interrupted write while a segment
//! after it holds a valid block, as that is damage too, not a crash before
//! it was sealed. Otherwise the torn end is cut and the segments after it
//! removed, as no record of theirs was synced: a sync seals the segments
//! before its own first.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{FileLog, FileLogOptions, LogRecovery, Segment, check_segment_size, segment_number, split};
use crate::disk::block::{BlockFile, BlockId, HEADER_LEN, ReadWith, Recovery};
use crate::disk::io::{Durability, FileSystem, OpenMode};
use crate::error::{Code, Result, SqlError};
use crate::log::{Log, Lsn};

/// A segment opened, nothing cut: its number, path, file and what follows
/// its valid blocks.
type Opened = (u64, PathBuf, BlockFile, Recovery);

impl FileLog {
    /// Opens the log as `open` does, its records known durable through
    /// `through`, as a checkpoint's image records it: a segment before
    /// through's was synced and sealed whole before it, so it is taken to
    /// its end unchecked; through's is checked from there, and the later
    /// ones whole. A segment cut short ends the log: the ones after it are
    /// removed, as no record of theirs was synced (a sync seals the
    /// segments before its own first). A segment damaged is refused, the
    /// log unchanged.
    ///
    /// # Errors
    ///
    /// As `open`'s; SQLCODE -916 if a segment is damaged: invalid bytes
    /// followed by a valid block, in it or in a later segment, that show no
    /// write a crash interrupted.
    pub fn open_through(
        fs: Arc<dyn FileSystem>,
        dir: &Path,
        formats: ReadWith<'_>,
        options: FileLogOptions,
        through: Lsn,
    ) -> Result<(FileLog, LogRecovery)> {
        check_segment_size(options.segment_size)?;
        let mut numbered: Vec<(u64, PathBuf)> = fs.read_dir(dir)?.into_iter().filter_map(|p| Some((segment_number(&p)?, p))).collect();
        numbered.sort();
        let Some(&(first, _)) = numbered.first() else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} holds no log", dir.display())));
        };
        for (n, (number, _)) in numbered.iter().enumerate() {
            let expected = first.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
            if *number != expected {
                return Err(SqlError::new(Code::Corrupt, format!("[DESIGN] the log in {} is missing segment {expected}", dir.display())));
            }
        }
        let (opened, half) = open_segments(&*fs, &numbered, (formats, &options), split(through))?;
        let cut = settle(&*fs, dir, (&opened, &numbered, half.as_deref()), (formats, &options))?;
        let mut segments = Vec::with_capacity(opened.len());
        for (number, _, blocks, _) in opened {
            segments.push(Segment::new(number, blocks));
        }
        // New segments are checksummed and encoded as the last one is.
        let newest = segments.last().ok_or_else(|| {
            SqlError::new(Code::Api, format!("[DESIGN] {} holds no log: its only segment was never completely created", dir.display()))
        })?;
        let hash = Arc::clone(newest.blocks.hash());
        let mut options = options;
        options.pipeline = newest.blocks.pipeline().clone();
        let count = u64::try_from(segments.len()).unwrap_or(u64::MAX);
        // Opened syncing nothing, the older segments are not taken as
        // durable: a run before may have left them unsynced, so the first
        // sync after the log's durability rises syncs them.
        let durable = options.durability != Durability::None;
        for older in segments.iter().take(segments.len().saturating_sub(1)) {
            if durable {
                older.durable_end.store(older.blocks.end().0, Ordering::Release);
            }
            older.sealed.store(true, Ordering::Release);
        }
        let log = FileLog::over(fs, dir, hash, options, segments)?;
        let end = log.end();
        Ok((log, LogRecovery { segments: count, end, cut }))
    }
}

/// Opens the segments `numbered`, nothing cut, the log known durable
/// through block `offset` of segment `known`, until one has bytes to cut,
/// which ends them; and the last, if a crash left it half created, holding
/// no record, to be removed.
fn open_segments(
    fs: &dyn FileSystem,
    numbered: &[(u64, PathBuf)],
    (formats, options): (ReadWith<'_>, &FileLogOptions),
    (known, offset): (u64, BlockId),
) -> Result<(Vec<Opened>, Option<PathBuf>)> {
    let last = numbered.len().saturating_sub(1);
    let mut opened = Vec::with_capacity(numbered.len());
    for (n, (number, path)) in numbered.iter().enumerate() {
        let from = known_through(fs, (*number, path), (known, offset))?;
        match BlockFile::open_uncut(fs, path, formats, options.blocks, from) {
            Ok((blocks, recovery)) => {
                let torn = recovery.cut > 0;
                opened.push((*number, path.clone(), blocks, recovery));
                if torn {
                    break;
                }
            }
            // Only the last may be one a rotation a crash interrupted
            // left half created, holding no record.
            Err(e) => {
                if n != last || !half_created(fs, path, &e)? {
                    return Err(e);
                }
                return Ok((opened, Some(path.clone())));
            }
        }
    }
    Ok((opened, None))
}

/// Refuses the log if the last of `opened` is damaged; else cuts its torn
/// end, if it has one, and removes the segments of `numbered` after it, or
/// the one `half` made, durably. The bytes cut and removed.
fn settle(
    fs: &dyn FileSystem,
    dir: &Path,
    (opened, numbered, half): (&[Opened], &[(u64, PathBuf)], Option<&Path>),
    (formats, options): (ReadWith<'_>, &FileLogOptions),
) -> Result<u64> {
    let Some((number, path, blocks, recovery)) = opened.last().filter(|o| o.3.cut > 0) else {
        if let Some(half) = half {
            fs.remove(half)?;
            fs.sync_dir(dir)?;
        }
        return Ok(0);
    };
    let after = numbered.iter().position(|(n, _)| n == number).map_or(numbered.len(), |n| n.saturating_add(1));
    let later = numbered.get(after..).unwrap_or_default();
    blocks.refuse_tail(recovery, options.blocks.tail)?;
    if !recovery.interrupted {
        for (other, at) in later {
            if holds_a_block(fs, at, formats, options)? {
                return Err(SqlError::new(
                    Code::Corrupt,
                    format!(
                        "[DESIGN] {} is damaged at offset {}: the bytes from there to its end are not valid blocks, but segment {other} after it holds a valid block, so the log is not opened, and nothing of it is cut",
                        path.display(),
                        recovery.end.0
                    ),
                ));
            }
        }
    }
    blocks.cut_tail(recovery)?;
    if later.is_empty() {
        return Ok(recovery.cut);
    }
    Ok(recovery.cut.saturating_add(remove_after(fs, dir, later)?))
}

/// Whether the segment at `path` holds a valid block; not if it is not a
/// block file, as one half created is not.
fn holds_a_block(fs: &dyn FileSystem, path: &Path, formats: ReadWith<'_>, options: &FileLogOptions) -> Result<bool> {
    let blocks = options.blocks;
    match BlockFile::open_uncut(fs, path, formats, blocks, BlockId(HEADER_LEN)) {
        Ok((_, recovery)) => Ok(recovery.end.0 > HEADER_LEN || recovery.follows.is_some()),
        Err(e) => {
            if e.code == Code::Corrupt {
                return Ok(false);
            }
            Err(e)
        }
    }
}

/// Whether the segment at `path`, which failed to open with `error`, is one
/// a crash left half created: shorter than a block file's header.
fn half_created(fs: &dyn FileSystem, path: &Path, error: &SqlError) -> Result<bool> {
    if error.code != Code::Corrupt {
        return Ok(false);
    }
    Ok(fs.open(path, OpenMode::ReadOnly)?.size()? < HEADER_LEN)
}

/// Where segment `number` at `path` is known valid through, the log
/// known durable through block `offset` of segment `known`: a segment
/// before `known` to its end, `known` to `offset`, a later one nowhere.
fn known_through(fs: &dyn FileSystem, (number, path): (u64, &Path), (known, offset): (u64, BlockId)) -> Result<BlockId> {
    Ok(match number.cmp(&known) {
        std::cmp::Ordering::Less => BlockId(fs.open(path, OpenMode::ReadOnly)?.size()?),
        std::cmp::Ordering::Equal => offset,
        std::cmp::Ordering::Greater => BlockId(HEADER_LEN),
    })
}

/// Removes the segments `after`, past a segment cut short, durably: the
/// bytes they held.
fn remove_after(fs: &dyn FileSystem, dir: &Path, after: &[(u64, PathBuf)]) -> Result<u64> {
    let mut bytes = 0_u64;
    for (_, path) in after {
        bytes = bytes.saturating_add(fs.open(path, OpenMode::ReadOnly)?.size()?);
        fs.remove(path)?;
    }
    fs.sync_dir(dir)?;
    Ok(bytes)
}
