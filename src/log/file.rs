//! \[DESIGN\] `FileLog` (RULES.md D25, D30): a log in a directory of
//! segments, each a block file, the next begun when the current one reaches
//! the segment size. Segments are the log's concern, not the block
//! manager's: a position is the segment's number in its high bits and the
//! record's block id within the segment in its low `OFFSET_BITS`, so
//! positions sort in log order.
//!
//! Appends go to the current segment through the block manager, several
//! threads at once. Beginning a segment waits for the appends still writing
//! to the old one, then syncs and seals it; no lock is held during the I/O
//! (RULES.md R-ERR-4). Syncs are grouped: one thread syncs while the others
//! wait, and finds their records durable too (group commit).
//!
//! On open, the segments are opened in order, those before the position
//! known durable taken unchecked, and a torn end cut off: a segment cut
//! short ends the log, and the segments after it are removed. A last
//! segment a crash left half created (shorter than a block file's header)
//! holds no record and is removed; a missing segment between two others is
//! refused as corruption, and so is a segment damaged, valid blocks
//! following invalid ones (`open.rs`), the log then unchanged.
//!
//! A segment that cannot be begun leaves the current one growing past the
//! segment size, its appends going on; the failure is told to the sink the
//! log is given (`telling`), once until a segment is begun again (RULES.md
//! D56). One a failed rotation left half made holds no record, and is made
//! again by the next.
//!
//! A failed sync, of a segment sealed or of a group, fails every later
//! `sync` with it, and is told to the sink: what the failed sync was to
//! make durable may be lost, and a sync of the same file tried again may
//! succeed without it, as some systems forget a failed write once they
//! report it; so no record appended before is ever claimed durable after
//! it (RULES.md D25). The database stops taking changes at the next sync.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};

use super::{Log, LogCapabilities, Lsn, Records};
use crate::diagnostics::{Diagnostics, NoDiagnostics};
use crate::disk::block::{BlockFile, BlockId, BlockOptions, HEADER_LEN};
use crate::disk::encoding::{Encoding, Pipeline};
use crate::disk::io::{Durability, FileSystem};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::sync::lock_builtin;

mod open;
mod segments;

/// Bits of a position that hold the block id within its segment: segments
/// of up to 1 TiB, and 2^24 of them.
const OFFSET_BITS: u32 = 40;
/// The block id bits of a position.
const OFFSET_MASK: u64 = (1 << OFFSET_BITS) - 1;
/// What segment files are named: the prefix, ten digits of the segment's
/// number, and the suffix, so a sorted listing is in log order.
const SEGMENT_PREFIX: &str = "segment-";
const SEGMENT_SUFFIX: &str = ".log";
/// \[DESIGN\] The default size at which a segment is ended.
pub const DEFAULT_SEGMENT_SIZE: u64 = 64 * 1024 * 1024;
/// \[DESIGN\] The smallest segment size allowed: a few blocks at least.
pub const MIN_SEGMENT_SIZE: u64 = 4096;
/// \[DESIGN\] The largest segment size: a position's block id bits.
pub const MAX_SEGMENT_SIZE: u64 = OFFSET_MASK;

/// How a file log behaves.
#[derive(Clone)]
pub struct FileLogOptions {
    /// Bytes after which a segment is ended and the next begun.
    pub segment_size: u64,
    /// How durable `sync` makes records.
    pub durability: Durability,
    /// The encodings records pass through (RULES.md D30).
    pub pipeline: Pipeline,
    /// The block files' options.
    pub blocks: BlockOptions,
}

impl Default for FileLogOptions {
    fn default() -> Self {
        FileLogOptions {
            segment_size: DEFAULT_SEGMENT_SIZE,
            durability: Durability::Data,
            pipeline: Pipeline::none(),
            blocks: BlockOptions::default(),
        }
    }
}

/// What opening a file log found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRecovery {
    /// Segments in the log.
    pub segments: u64,
    /// The position after the last record.
    pub end: Lsn,
    /// Bytes cut from the ends of segments: writes a crash interrupted.
    pub cut: u64,
}

/// One segment: its block file, the appends writing to it, and how far it
/// is durable.
struct Segment {
    number: u64,
    blocks: BlockFile,
    /// Appends that took this segment and have not finished.
    in_flight: AtomicUsize,
    idle: Mutex<()>,
    drained: Condvar,
    /// Every block before this offset is durable.
    durable_end: AtomicU64,
    /// Whether it was ended, all its appends finished, and synced.
    sealed: AtomicBool,
}

/// The counters of a file log, for the heuristics above it (RULES.md D30).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogCounts {
    /// Records appended.
    pub appends: u64,
    /// Record bytes appended.
    pub appended_bytes: u64,
    /// Calls to `sync` that needed a sync.
    pub sync_requests: u64,
    /// Syncs of a segment done; fewer than the requests when grouped.
    pub syncs: u64,
    /// Segments begun after the first.
    pub rotations: u64,
}

#[derive(Debug, Default)]
struct Counters {
    appends: AtomicU64,
    appended_bytes: AtomicU64,
    sync_requests: AtomicU64,
    syncs: AtomicU64,
    rotations: AtomicU64,
}

/// Adds `n` to `counter`; it would wrap only after 2^64, which no log reaches.
fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

/// \[DESIGN\] A log in a directory of block-file segments.
pub struct FileLog {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    hash: Arc<dyn HashFunction>,
    options: FileLogOptions,
    /// The segment appends go to.
    active: RwLock<Arc<Segment>>,
    /// Every segment not truncated, by number.
    segments: Mutex<BTreeMap<u64, Arc<Segment>>>,
    /// Whether a thread is beginning a segment.
    rotating: AtomicBool,
    /// Whether a thread is syncing for a group; the others wait on `synced`.
    syncing: Mutex<bool>,
    synced: Condvar,
    counters: Counters,
    /// Where a failure it goes on past is told.
    events: Arc<dyn Diagnostics>,
    /// Whether a rotation's failure was told since one last succeeded.
    told: AtomicBool,
    /// The failure of a sync, once one failed: every later sync fails.
    failure: Mutex<Option<SqlError>>,
}

/// The position of block `id` of segment `number`.
fn lsn(number: u64, id: BlockId) -> Lsn {
    Lsn((number << OFFSET_BITS) | (id.0 & OFFSET_MASK))
}

/// The segment number and block id of position `lsn`.
fn split(lsn: Lsn) -> (u64, BlockId) {
    (lsn.0 >> OFFSET_BITS, BlockId(lsn.0 & OFFSET_MASK))
}

/// The file name of segment `number`.
fn segment_name(number: u64) -> String {
    format!("{SEGMENT_PREFIX}{number:010}{SEGMENT_SUFFIX}")
}

/// The segment number a file name gives, if it is a segment's.
fn segment_number(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix(SEGMENT_PREFIX)?.strip_suffix(SEGMENT_SUFFIX)?.parse().ok()
}

impl Segment {
    fn new(number: u64, blocks: BlockFile) -> Arc<Segment> {
        Arc::new(Segment {
            number,
            blocks,
            in_flight: AtomicUsize::new(0),
            idle: Mutex::new(()),
            drained: Condvar::new(),
            durable_end: AtomicU64::new(HEADER_LEN),
            sealed: AtomicBool::new(false),
        })
    }

    /// Records that an append taking it finished, waking a thread waiting
    /// for its appends to drain.
    fn finished(&self) {
        if self.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _idle = lock_builtin(&self.idle);
            self.drained.notify_all();
        }
    }

    /// Waits until no append is writing to it.
    fn drain(&self) {
        let mut idle = lock_builtin(&self.idle);
        while self.in_flight.load(Ordering::Acquire) > 0 {
            idle = self.drained.wait(idle).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl FileLog {
    /// Whether directory `dir` of `fs` holds a log: a segment of one.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the directory cannot be read.
    pub fn exists(fs: &dyn FileSystem, dir: &Path) -> Result<bool> {
        Ok(fs.exists(dir)? && fs.read_dir(dir)?.iter().any(|p| segment_number(p).is_some()))
    }

    /// Creates a log in directory `dir`, made if missing, which must hold no
    /// segment, with its first segment.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the segment size is out of range or `dir` holds a
    /// segment already; SQLCODE -915 if the segment cannot be created.
    pub fn create(fs: Arc<dyn FileSystem>, dir: &Path, hash: Arc<dyn HashFunction>, options: FileLogOptions) -> Result<FileLog> {
        check_segment_size(options.segment_size)?;
        match options.durability {
            Durability::None => fs.create_dir_all(dir)?,
            Durability::Data | Durability::Full => fs.create_dir_durably(dir)?,
        }
        if fs.read_dir(dir)?.iter().any(|p| segment_number(p).is_some()) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} already holds a log", dir.display())));
        }
        let first = segment_file(&*fs, &dir.join(segment_name(0)), Arc::clone(&hash), &options)?;
        FileLog::over(fs, dir, hash, options, vec![Segment::new(0, first)])
    }

    /// Opens the log in directory `dir`, whose hash function must be among
    /// `hashes` and encodings among `encodings`, and recovers it.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a segment is missing between others, is not a block
    /// file, or is damaged (`open_through`); SQLCODE -910 if `dir` holds no log, or a hash function or
    /// encoding is not given; SQLCODE -915 if a segment cannot be read.
    pub fn open(
        fs: Arc<dyn FileSystem>,
        dir: &Path,
        hashes: &[Arc<dyn HashFunction>],
        encodings: &[Arc<dyn Encoding>],
        options: FileLogOptions,
    ) -> Result<(FileLog, LogRecovery)> {
        FileLog::open_through(fs, dir, (hashes, encodings), options, Lsn(0))
    }

    /// The log over `segments`, the last one current.
    fn over(
        fs: Arc<dyn FileSystem>,
        dir: &Path,
        hash: Arc<dyn HashFunction>,
        options: FileLogOptions,
        segments: Vec<Arc<Segment>>,
    ) -> Result<FileLog> {
        let active =
            segments.last().map(Arc::clone).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {} holds no log", dir.display())))?;
        Ok(FileLog {
            fs,
            dir: dir.to_path_buf(),
            hash,
            options,
            active: RwLock::new(active),
            segments: Mutex::new(segments.into_iter().map(|s| (s.number, s)).collect()),
            rotating: AtomicBool::new(false),
            syncing: Mutex::new(false),
            synced: Condvar::new(),
            counters: Counters::default(),
            events: Arc::new(NoDiagnostics),
            told: AtomicBool::new(false),
            failure: Mutex::new(None),
        })
    }

    /// The log, telling `events` of the failures it goes on past: a
    /// segment that cannot be begun, or synced as it is sealed.
    #[must_use]
    pub fn telling(self, events: Arc<dyn Diagnostics>) -> FileLog {
        FileLog { events, ..self }
    }

    /// The counters now.
    pub fn counts(&self) -> LogCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let c = &self.counters;
        LogCounts {
            appends: get(&c.appends),
            appended_bytes: get(&c.appended_bytes),
            sync_requests: get(&c.sync_requests),
            syncs: get(&c.syncs),
            rotations: get(&c.rotations),
        }
    }

    /// The segment numbered `number`, if it is kept.
    fn segment(&self, number: u64) -> Option<Arc<Segment>> {
        lock_builtin(&self.segments).get(&number).map(Arc::clone)
    }

    /// The current segment, counted as taken by an append before the lock
    /// on it is released, so a rotation that replaces it waits for the
    /// append.
    fn take_active(&self) -> Arc<Segment> {
        let active = self.active.read().unwrap_or_else(PoisonError::into_inner);
        active.in_flight.fetch_add(1, Ordering::AcqRel);
        Arc::clone(&active)
    }
}

/// A new segment's block file at `path`, its creation synced unless the
/// log is never synced (`Durability::None`).
fn segment_file(fs: &dyn FileSystem, path: &Path, hash: Arc<dyn HashFunction>, options: &FileLogOptions) -> Result<BlockFile> {
    match options.durability {
        Durability::None => BlockFile::create_unsynced(fs, path, hash, options.pipeline.clone(), options.blocks),
        Durability::Data | Durability::Full => BlockFile::create(fs, path, hash, options.pipeline.clone(), options.blocks),
    }
}

/// Checks a segment size.
fn check_segment_size(size: u64) -> Result<()> {
    if (MIN_SEGMENT_SIZE..=MAX_SEGMENT_SIZE).contains(&size) {
        return Ok(());
    }
    Err(SqlError::new(Code::Api, format!("[DESIGN] a log segment must be from {MIN_SEGMENT_SIZE} to {MAX_SEGMENT_SIZE} bytes, not {size}")))
}

impl Log for FileLog {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: self.options.durability != Durability::None }
    }

    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let segment = self.take_active();
        let appended = segment.blocks.append(record);
        segment.finished();
        let id = appended?;
        add(&self.counters.appends, 1);
        add(&self.counters.appended_bytes, u64::try_from(record.len()).unwrap_or(u64::MAX));
        if segment.blocks.end().0 >= self.options.segment_size {
            // A segment that cannot be begun leaves this one growing past
            // its size; the record is appended either way, a failure told,
            // and a failed seal kept to fail the next sync.
            self.rotate(&segment);
        }
        Ok(lsn(segment.number, id))
    }

    fn sync(&self, through: Lsn) -> Result<()> {
        self.healthy()?;
        let (number, id) = split(through);
        let segment = self.segment(number).filter(|s| id < s.blocks.end()).ok_or_else(|| {
            SqlError::new(Code::Api, format!("[DESIGN] the log in {} has no record at position {}", self.dir.display(), through.0))
        })?;
        // Earlier segments are synced when sealed; seal any not yet sealed.
        let earlier: Vec<Arc<Segment>> = lock_builtin(&self.segments).range(..number).map(|(_, s)| Arc::clone(s)).collect();
        for older in earlier {
            self.seal(&older)?;
        }
        self.sync_through(&segment, id)
    }

    fn read_from(&self, from: Lsn) -> Records<'_> {
        Box::new(Reader { log: self, at: from, payload: Vec::new() })
    }

    fn truncate_before(&self, before: Lsn) -> Result<()> {
        let (number, _) = split(before);
        let removable: Vec<u64> =
            lock_builtin(&self.segments).range(..number).filter(|(_, s)| s.sealed.load(Ordering::Acquire)).map(|(&n, _)| n).collect();
        for n in removable {
            // The file first: if it cannot be removed, the segment stays,
            // readable, rather than its records being lost to readers.
            self.fs.remove(&self.dir.join(segment_name(n)))?;
            lock_builtin(&self.segments).remove(&n);
        }
        // A log never synced does not sync its removals either.
        match self.options.durability {
            Durability::None => Ok(()),
            Durability::Data | Durability::Full => self.fs.sync_dir(&self.dir),
        }
    }

    fn end(&self) -> Lsn {
        let active = self.active.read().unwrap_or_else(PoisonError::into_inner);
        lsn(active.number, active.blocks.end())
    }
}

/// Reads a file log's records in order, segment after segment.
struct Reader<'a> {
    log: &'a FileLog,
    at: Lsn,
    payload: Vec<u8>,
}

impl Iterator for Reader<'_> {
    type Item = Result<(Lsn, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (number, id) = split(self.at);
            // The first segment kept at or after the position's.
            let segment = lock_builtin(&self.log.segments).range(number..).next().map(|(_, s)| Arc::clone(s))?;
            let id = if segment.number == number { id.max(segment.blocks.first()) } else { segment.blocks.first() };
            if id >= segment.blocks.end() {
                if self.log.segment(segment.number.saturating_add(1)).is_none() {
                    self.at = lsn(segment.number, id);
                    return None;
                }
                self.at = lsn(segment.number.saturating_add(1), BlockId(0));
                continue;
            }
            return Some(match segment.blocks.read_into(id, &mut self.payload) {
                Ok(next) => {
                    self.at = lsn(segment.number, next);
                    Ok((lsn(segment.number, id), self.payload.clone()))
                }
                Err(e) => {
                    // Stop after an error rather than repeat it.
                    self.at = Lsn(u64::MAX);
                    Err(e)
                }
            });
        }
    }
}
