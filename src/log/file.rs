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
//! refused as corruption.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};

use super::{Log, LogCapabilities, Lsn, Records};
use crate::disk::block::{BlockFile, BlockId, BlockOptions, HEADER_LEN, ReadWith};
use crate::disk::encoding::{Encoding, Pipeline};
use crate::disk::io::{Durability, FileSystem, OpenMode};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::sync::lock_builtin;

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
        fs.create_dir_all(dir)?;
        if fs.read_dir(dir)?.iter().any(|p| segment_number(p).is_some()) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} already holds a log", dir.display())));
        }
        let first = BlockFile::create(&*fs, &dir.join(segment_name(0)), Arc::clone(&hash), options.pipeline.clone(), options.blocks)?;
        FileLog::over(fs, dir, hash, options, vec![Segment::new(0, first)])
    }

    /// Opens the log in directory `dir`, whose hash function must be among
    /// `hashes` and encodings among `encodings`, and recovers it.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a segment is missing between others, or is not a
    /// block file; SQLCODE -910 if `dir` holds no log, or a hash function or
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

    /// Opens the log as `open` does, its records known durable through
    /// `through`, as a checkpoint's image records it: a segment before
    /// through's was synced and sealed whole before it, so it is taken to
    /// its end unchecked; through's is checked from there, and the later
    /// ones whole. A segment cut short ends the log: the ones after it are
    /// removed, as no record of theirs was synced (a sync seals the
    /// segments before its own first).
    ///
    /// # Errors
    ///
    /// As `open`'s.
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
        let last = numbered.len().saturating_sub(1);
        let mut segments = Vec::with_capacity(numbered.len());
        let mut cut = 0_u64;
        let (known, offset) = split(through);
        for (n, (number, path)) in numbered.iter().enumerate() {
            let expected = first.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
            if *number != expected {
                return Err(SqlError::new(Code::Corrupt, format!("[DESIGN] the log in {} is missing segment {expected}", dir.display())));
            }
            let from = known_through(&*fs, (*number, path), (known, offset))?;
            match BlockFile::open_from(&*fs, path, formats, options.blocks, from) {
                Ok((blocks, recovery)) => {
                    cut = cut.saturating_add(recovery.cut);
                    segments.push(Segment::new(*number, blocks));
                    if recovery.cut > 0 && n != last {
                        cut = cut.saturating_add(remove_after(&*fs, dir, numbered.get(n.saturating_add(1)..).unwrap_or_default())?);
                        break;
                    }
                }
                // Only the last may be one a rotation a crash interrupted
                // left half created, holding no record: it is removed.
                Err(e) => {
                    if n != last || !half_created(&*fs, path, &e)? {
                        return Err(e);
                    }
                    fs.remove(path)?;
                    fs.sync_dir(dir)?;
                }
            }
        }
        // New segments are checksummed and encoded as the last one is.
        let newest = segments.last().ok_or_else(|| {
            SqlError::new(Code::Api, format!("[DESIGN] {} holds no log: its only segment was never completely created", dir.display()))
        })?;
        let hash = Arc::clone(newest.blocks.hash());
        let mut options = options;
        options.pipeline = newest.blocks.pipeline().clone();
        let count = u64::try_from(segments.len()).unwrap_or(u64::MAX);
        for older in segments.iter().take(segments.len().saturating_sub(1)) {
            older.durable_end.store(older.blocks.end().0, Ordering::Release);
            older.sealed.store(true, Ordering::Release);
        }
        let log = FileLog::over(fs, dir, hash, options, segments)?;
        let end = log.end();
        Ok((log, LogRecovery { segments: count, end, cut }))
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
        })
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
            // its size; the record is appended either way.
            let _ = self.rotate(&segment);
        }
        Ok(lsn(segment.number, id))
    }

    fn sync(&self, through: Lsn) -> Result<()> {
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
        self.fs.sync_dir(&self.dir)
    }

    fn end(&self) -> Lsn {
        let active = self.active.read().unwrap_or_else(PoisonError::into_inner);
        lsn(active.number, active.blocks.end())
    }
}

impl FileLog {
    /// Begins the segment after `full`, if `full` is still the current one
    /// and no other thread is beginning one; then seals `full`.
    fn rotate(&self, full: &Arc<Segment>) -> Result<()> {
        if self.rotating.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let current = Arc::ptr_eq(&self.active.read().unwrap_or_else(PoisonError::into_inner), full);
        let begun = if current { self.begin_after(full) } else { Ok(()) };
        self.rotating.store(false, Ordering::Release);
        begun?;
        if current {
            self.seal(full)?;
        }
        Ok(())
    }

    /// Creates the segment after `full` and makes it current.
    fn begin_after(&self, full: &Segment) -> Result<()> {
        let number = full.number.saturating_add(1);
        let blocks = BlockFile::create(
            &*self.fs,
            &self.dir.join(segment_name(number)),
            Arc::clone(&self.hash),
            self.options.pipeline.clone(),
            self.options.blocks,
        )?;
        let next = Segment::new(number, blocks);
        lock_builtin(&self.segments).insert(number, Arc::clone(&next));
        *self.active.write().unwrap_or_else(PoisonError::into_inner) = next;
        add(&self.counters.rotations, 1);
        Ok(())
    }

    /// Seals `segment`, no longer current: waits for its appends, syncs it.
    fn seal(&self, segment: &Segment) -> Result<()> {
        if segment.sealed.load(Ordering::Acquire) {
            return Ok(());
        }
        segment.drain();
        segment.blocks.sync(self.options.durability)?;
        segment.durable_end.fetch_max(segment.blocks.end().0, Ordering::AcqRel);
        segment.sealed.store(true, Ordering::Release);
        Ok(())
    }

    /// Makes block `id` of `segment` durable, grouping with other syncs: if
    /// one is running, wait for it, then check again; otherwise sync for
    /// everyone, through the blocks published when the sync began.
    fn sync_through(&self, segment: &Segment, id: BlockId) -> Result<()> {
        if id.0 < segment.durable_end.load(Ordering::Acquire) {
            return Ok(());
        }
        add(&self.counters.sync_requests, 1);
        let mut syncing = lock_builtin(&self.syncing);
        loop {
            if id.0 < segment.durable_end.load(Ordering::Acquire) {
                return Ok(());
            }
            if !*syncing {
                break;
            }
            syncing = self.synced.wait(syncing).unwrap_or_else(PoisonError::into_inner);
        }
        *syncing = true;
        drop(syncing);
        let end = segment.blocks.end();
        let synced = segment.blocks.sync(self.options.durability);
        if synced.is_ok() {
            segment.durable_end.fetch_max(end.0, Ordering::AcqRel);
            add(&self.counters.syncs, 1);
        }
        *lock_builtin(&self.syncing) = false;
        self.synced.notify_all();
        synced
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
