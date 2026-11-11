//! \[DESIGN\] The block manager (RULES.md D30): one file of checksummed
//! blocks, written sequentially, addressed by byte offset, and read at
//! random or by a cursor that moves forwards and backwards. It knows
//! nothing of what the blocks hold or of any component above it.
//!
//! An append is a write: it reserves the bytes after the last block with
//! one atomic step, writes the block there with one positional write, and
//! returns once the operating system has it; `sync` makes it durable.
//! Appenders on several threads write at once, each to its own range. A
//! block is published (readable, and counted in `end`) once every block
//! before it is written, so readers never meet a gap; an appender waits
//! only for earlier appenders to finish, and holds no lock while it hashes
//! or writes (RULES.md R-ERR-4). Batching blocks is for the components
//! above, which know what belongs together.
//!
//! A file given `BlockOptions::ahead` keeps zeros written past its blocks,
//! so a sync after an append carries no growth of the file (`fill.rs`).
//!
//! A write that fails leaves a gap the blocks after it cannot be reached
//! across, so the file then refuses appends until it is reopened, which
//! recovers it.
//!
//! On open, the blocks are scanned from the first, or from a point the
//! caller knows they are valid through (`open_from`); the first one that is
//! incomplete or fails its checksum ends the valid part, and what follows
//! is cut off or refused, as `TailPolicy` says; but valid blocks after it,
//! with no sign of a write a crash interrupted, are damage, and the file is
//! refused, unchanged (`tail.rs`).
//!
//! A read takes the first `FIRST_READ` bytes of a block at once, so a small
//! block is read in one call; a larger one takes a second for the rest.

mod counts;
mod cursor;
mod fill;
mod format;
mod tail;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

pub use counts::BlockCounts;
use counts::{Counters, add};
pub use cursor::Cursor;
pub use format::{FORMAT_VERSION, HEADER_LEN};
pub(crate) use tail::damaged;

use super::encoding::{Encoding, Pipeline};
use super::io::{DiskFile, Durability, FileSystem, begin_file};
use crate::error::{Code, Result, SqlError};
use crate::hash::{HashFunction, HashId};
use crate::sync::lock_builtin;

/// \[DESIGN\] A block's id: the byte offset where it starts. Ids grow in the
/// order blocks are appended, so they can serve as positions in a log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u64);

/// What `BlockFile::open` does with bytes after the last valid block.
/// Under either, valid blocks after them that show no interrupted write
/// are damage, and the file is refused (`Recovery::damage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailPolicy {
    /// Cut them off, durably, and report how many there were: the bytes of
    /// a write a crash interrupted.
    Truncate,
    /// Refuse to open the file (SQLCODE -916), leaving it unchanged.
    Refuse,
}

/// \[DESIGN\] Default limit on a block's payload, in bytes. Fixed, not a
/// setting (RULES.md D36): it guards the reading of a format, refusing a
/// length no block was written with; `BlockOptions` gives another.
pub const DEFAULT_MAX_BLOCK: u32 = 64 * 1024 * 1024;

/// How a block file behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockOptions {
    /// The longest payload a block may have; longer ones are refused, and
    /// on open a block claiming more is treated as corrupt.
    pub max_block: u32,
    /// What opening does with bytes after the last valid block.
    pub tail: TailPolicy,
    /// Bytes kept written with zeros past the appended blocks, 0 for
    /// none, so a sync after an append carries no growth of the file
    /// (`fill.rs`).
    pub ahead: u64,
}

impl Default for BlockOptions {
    fn default() -> Self {
        BlockOptions { max_block: DEFAULT_MAX_BLOCK, tail: TailPolicy::Truncate, ahead: 0 }
    }
}

/// What opening a block file found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recovery {
    /// The end of the last valid block, where the next block goes.
    pub end: BlockId,
    /// Bytes after it that were cut off.
    pub cut: u64,
    /// The first valid block after those bytes, if one follows them.
    pub follows: Option<BlockId>,
    /// Whether those bytes show a write a crash interrupted: a sector of
    /// zeros, never written; a zero length ending them; or a block running
    /// past the end of the file (`tail.rs`).
    pub interrupted: bool,
}

/// The hash functions and encodings a file may be read with.
pub type ReadWith<'a> = (&'a [Arc<dyn HashFunction>], &'a [Arc<dyn Encoding>]);

/// \[DESIGN\] The bytes a read of a block takes first: a block that fits,
/// as most of a log's do, is read in one call. A shadow file's block of a
/// granule, 4096 bytes of content framed by at most 81 more (its length
/// before and after, a digest of at most 64 bytes, and the granule's kind
/// and number), fits too, as a page read from a side file reads one for
/// each of its granules.
const FIRST_READ: usize = 4096 + 128;

/// \[DESIGN\] Times an appender checks whether the blocks before its own are
/// published before it parks: most appends finish within a few checks, and
/// parking costs a lock and a wake-up.
const SPINS_BEFORE_PARKING: u32 = 64;

/// \[DESIGN\] An open block file. It may be shared between threads:
/// appends write at once, and reads run alongside them.
pub struct BlockFile {
    file: Box<dyn DiskFile>,
    path: PathBuf,
    hash: Arc<dyn HashFunction>,
    /// The encodings every payload passes through.
    pipeline: Pipeline,
    options: BlockOptions,
    /// The end of the bytes reserved by appends, written or not.
    reserved: AtomicU64,
    /// The end of the published blocks: each one before it is written.
    end: AtomicU64,
    /// Whether a write failed, leaving a gap at `end`.
    failed: AtomicBool,
    /// Appenders parked until `end` reaches their block; only they, and
    /// the appenders that wake them, take `parking`.
    parked: AtomicUsize,
    parking: Mutex<()>,
    /// Signalled when a block is published or a write fails.
    advanced: Condvar,
    /// The end of the bytes written, by blocks or by zeros filled ahead
    /// of them; only the thread holding `filling` raises it.
    filled: AtomicU64,
    filling: Mutex<()>,
    counters: Counters,
}

/// What reading a block at an offset found.
enum Found {
    /// A valid block, and the offset after it.
    Block(u64),
    /// No valid block, and why.
    Invalid(String),
}

impl BlockFile {
    /// Creates a block file at `path`, checksummed with `hash`, its
    /// payloads encoded by `pipeline`, and makes its creation durable.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the file exists or cannot be written and synced.
    pub fn create(
        fs: &dyn FileSystem,
        path: &Path,
        hash: Arc<dyn HashFunction>,
        pipeline: Pipeline,
        options: BlockOptions,
    ) -> Result<BlockFile> {
        BlockFile::made(fs, path, hash, (pipeline, options), true)
    }

    /// As `create`, but nothing synced: for a file a crash may lose, as a
    /// log never synced (`FileLog` of `Durability::None`).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the file exists or cannot be written.
    pub fn create_unsynced(
        fs: &dyn FileSystem,
        path: &Path,
        hash: Arc<dyn HashFunction>,
        pipeline: Pipeline,
        options: BlockOptions,
    ) -> Result<BlockFile> {
        BlockFile::made(fs, path, hash, (pipeline, options), false)
    }

    /// `create`'s work, made durable if `durable`.
    fn made(
        fs: &dyn FileSystem,
        path: &Path,
        hash: Arc<dyn HashFunction>,
        (pipeline, options): (Pipeline, BlockOptions),
        durable: bool,
    ) -> Result<BlockFile> {
        let file = begin_file(fs, path, &format::header(&*hash, &pipeline), durable)?;
        let mut blocks = BlockFile::new(file, path, hash, options, HEADER_LEN);
        blocks.pipeline = pipeline;
        Ok(blocks)
    }

    /// Opens the block file at `path`, whose hash function must be among
    /// `hashes` and whose encodings among `encodings`, and recovers it: the
    /// blocks are checked from the first, and bytes after the last valid one
    /// are handled as `options.tail` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the file cannot be read, or cut; SQLCODE -916 if it is
    /// not a block file, has invalid bytes at its end under
    /// `TailPolicy::Refuse`, or is damaged (`Recovery::damage`), the file
    /// then unchanged; SQLCODE -910 if its hash function or one of its
    /// encodings is not given.
    pub fn open(
        fs: &dyn FileSystem,
        path: &Path,
        hashes: &[Arc<dyn HashFunction>],
        encodings: &[Arc<dyn Encoding>],
        options: BlockOptions,
    ) -> Result<(BlockFile, Recovery)> {
        BlockFile::open_from(fs, path, (hashes, encodings), options, BlockId(HEADER_LEN))
    }

    /// Opens the block file at `path` as `open` does, its blocks known
    /// valid through `from`: only those after it are checked. `from` is
    /// taken if a valid block ends there, and is the file's size or less;
    /// otherwise every block is checked.
    ///
    /// # Errors
    ///
    /// As `open`'s.
    pub fn open_from(
        fs: &dyn FileSystem,
        path: &Path,
        formats: ReadWith<'_>,
        options: BlockOptions,
        from: BlockId,
    ) -> Result<(BlockFile, Recovery)> {
        let (blocks, recovery) = BlockFile::open_uncut(fs, path, formats, options, from)?;
        blocks.refuse_tail(&recovery, options.tail)?;
        blocks.cut_tail(&recovery)?;
        Ok((blocks, recovery))
    }

    fn new(file: Box<dyn DiskFile>, path: &Path, hash: Arc<dyn HashFunction>, options: BlockOptions, end: u64) -> BlockFile {
        BlockFile {
            file,
            path: path.to_path_buf(),
            hash,
            pipeline: Pipeline::none(),
            options,
            reserved: AtomicU64::new(end),
            end: AtomicU64::new(end),
            failed: AtomicBool::new(false),
            parked: AtomicUsize::new(0),
            parking: Mutex::new(()),
            advanced: Condvar::new(),
            filled: AtomicU64::new(end),
            filling: Mutex::new(()),
            counters: Counters::default(),
        }
    }

    /// Sets the end after a cut, before the file is shared.
    fn reset_end(&self, end: u64) {
        self.reserved.store(end, Ordering::Release);
        self.end.store(end, Ordering::Release);
    }

    /// Where the check of a file of `size` bytes begins, known valid
    /// through `from`: there, if a valid block ends there; else at the
    /// first block.
    fn known(&self, from: BlockId, size: u64) -> Result<u64> {
        if from.0 <= HEADER_LEN || from.0 > size {
            return Ok(HEADER_LEN);
        }
        let Ok(Some(last)) = self.before(from) else { return Ok(HEADER_LEN) };
        let ends_there = match self.block_at(last.0, from.0, &mut Vec::new())? {
            Found::Block(next) => next == from.0,
            Found::Invalid(_) => false,
        };
        Ok(if ends_there { from.0 } else { HEADER_LEN })
    }

    /// The end of the valid blocks of a file of `size` bytes, from `at`, a
    /// block's start.
    fn valid_end(&self, mut at: u64, size: u64) -> Result<u64> {
        let mut scratch = Vec::new();
        while let Found::Block(next) = self.block_at(at, size, &mut scratch)? {
            at = next;
        }
        Ok(at)
    }

    /// The id of the first block, whether or not there is one.
    pub fn first(&self) -> BlockId {
        BlockId(HEADER_LEN)
    }

    /// The end of the published blocks: every block before it is readable.
    pub fn end(&self) -> BlockId {
        BlockId(self.end.load(Ordering::Acquire))
    }

    /// The id of the hash function that checksums the file.
    pub fn hash_id(&self) -> HashId {
        self.hash.id()
    }

    /// The hash function that checksums the file.
    pub fn hash(&self) -> &Arc<dyn HashFunction> {
        &self.hash
    }

    /// The encodings its payloads pass through.
    pub fn pipeline(&self) -> &Pipeline {
        &self.pipeline
    }

    /// A cursor positioned at `at`, a block's id or the end.
    pub fn cursor(&self, at: BlockId) -> Cursor<'_> {
        Cursor::new(self, at)
    }

    /// The error for an offset beyond 2^64 bytes.
    fn beyond(&self) -> SqlError {
        SqlError::new(Code::Io, format!("[DESIGN] {} would grow beyond 2^64 bytes", self.path.display()))
    }

    /// The error for a file that refuses appends after a failed write.
    fn broken(&self) -> SqlError {
        SqlError::new(Code::Io, format!("[DESIGN] a write to {} failed, so it takes no appends until it is reopened", self.path.display()))
    }

    /// Reserves `len` bytes after the last reservation, and returns where
    /// they start.
    fn reserve(&self, len: usize) -> Result<u64> {
        let len = u64::try_from(len).map_err(|_| self.beyond())?;
        let mut start = self.reserved.load(Ordering::Acquire);
        loop {
            let end = start.checked_add(len).ok_or_else(|| self.beyond())?;
            match self.reserved.compare_exchange_weak(start, end, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(start),
                Err(now) => start = now,
            }
        }
    }

    /// Publishes the block written at `start..end` once every block before
    /// it is, or reports the failed write that left a gap before it.
    fn publish(&self, start: u64, end: u64, written: Result<()>) -> Result<()> {
        if let Err(e) = written {
            self.failed.store(true, Ordering::SeqCst);
            self.wake();
            return Err(e);
        }
        let mut spins = 0;
        loop {
            if self.failed.load(Ordering::SeqCst) {
                return Err(self.broken());
            }
            if self.end.compare_exchange(start, end, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                self.wake();
                return Ok(());
            }
            if spins < SPINS_BEFORE_PARKING {
                spins = spins.saturating_add(1);
                std::hint::spin_loop();
            } else {
                self.park(start);
            }
        }
    }

    /// Waits until the published end reaches `start` or a write fails. The
    /// count of parked appenders is raised before the check, so an appender
    /// that publishes after the check sees it and wakes this one.
    fn park(&self, start: u64) {
        let mut gate = lock_builtin(&self.parking);
        self.parked.fetch_add(1, Ordering::SeqCst);
        while self.end.load(Ordering::SeqCst) != start && !self.failed.load(Ordering::SeqCst) {
            gate = self.advanced.wait(gate).unwrap_or_else(PoisonError::into_inner);
        }
        self.parked.fetch_sub(1, Ordering::SeqCst);
    }

    /// Wakes the parked appenders, if there are any, so they check again.
    fn wake(&self) {
        if self.parked.load(Ordering::SeqCst) > 0 {
            drop(lock_builtin(&self.parking));
            self.advanced.notify_all();
        }
    }
}

impl BlockFile {
    /// Appends a block holding `payload` and returns its id, once the
    /// block is written and published; it is durable after `sync`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the payload is longer than `max_block`; SQLCODE -915
    /// if the write fails, or an earlier one did.
    pub fn append(&self, payload: &[u8]) -> Result<BlockId> {
        // Block ids are known only once written, so the context is 0
        // (`Encoding`).
        let stored = self.pipeline.encode(0, payload)?;
        let frame = format::frame(&*self.hash, &stored, self.options.max_block)?;
        if self.failed.load(Ordering::SeqCst) {
            return Err(self.broken());
        }
        let start = self.reserve(frame.len())?;
        let end = start.saturating_add(u64::try_from(frame.len()).unwrap_or(u64::MAX));
        let written = self.fill_ahead(end).and_then(|()| self.file.write_all_at(&frame, start));
        self.publish(start, end, written)?;
        add(&self.counters.appends, 1);
        add(&self.counters.appended_bytes, payload.len());
        add(&self.counters.stored_bytes, stored.len());
        Ok(BlockId(start))
    }

    /// Writes `payload`, as long as the block's, over block `id`. A crash
    /// can tear it among valid blocks, which an append cannot, so this is
    /// for a file thrown away unless synced whole (`disk/shadow.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the block cannot be read or written, which leaves
    /// the file refusing appends; -916 if its length is not the payload's.
    pub fn overwrite(&self, id: BlockId, payload: &[u8]) -> Result<()> {
        let stored = self.pipeline.encode(0, payload)?;
        let frame = format::frame(&*self.hash, &stored, self.options.max_block)?;
        if self.failed.load(Ordering::SeqCst) {
            return Err(self.broken());
        }
        let mut length = [0; format::LEN_BYTES];
        self.file.read_exact_at(&mut length, id.0)?;
        if frame.get(..format::LEN_BYTES) != Some(&length[..]) {
            let msg = format!("[DESIGN] the block at offset {} of {} is of another length", id.0, self.path.display());
            return Err(SqlError::new(Code::Corrupt, msg));
        }
        let written = self.file.write_all_at(&frame, id.0);
        if written.is_err() {
            self.failed.store(true, Ordering::SeqCst);
            self.wake();
        }
        written
    }

    /// Makes the appended blocks durable as `durability` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the sync fails.
    pub fn sync(&self, durability: Durability) -> Result<()> {
        add(&self.counters.syncs, 1);
        self.file.sync(durability)
    }

    /// The payload of block `id`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if no valid block starts at `id`; SQLCODE -915 if the
    /// file cannot be read.
    pub fn read(&self, id: BlockId) -> Result<Vec<u8>> {
        let mut payload = Vec::new();
        self.read_into(id, &mut payload)?;
        Ok(payload)
    }

    /// Reads the payload of block `id` into `payload`, replacing what it
    /// held, decoded by the file's pipeline, and returns the id after it.
    ///
    /// # Errors
    ///
    /// As `read`.
    pub fn read_into(&self, id: BlockId, payload: &mut Vec<u8>) -> Result<BlockId> {
        match self.block_at(id.0, self.end.load(Ordering::Acquire), payload)? {
            Found::Block(next) => {
                if let Cow::Owned(decoded) = self.pipeline.decode(0, payload)? {
                    *payload = decoded;
                }
                add(&self.counters.reads, 1);
                add(&self.counters.read_bytes, payload.len());
                Ok(BlockId(next))
            }
            Found::Invalid(why) => {
                add(&self.counters.corrupt_reads, 1);
                Err(SqlError::new(Code::Corrupt, format!("[DESIGN] no valid block at offset {} of {}: {why}", id.0, self.path.display())))
            }
        }
    }

    /// The id of the block that ends at `at`, if `at` is after the first.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the length before `at` does not lead back to a block
    /// start; SQLCODE -915 if the file cannot be read.
    pub(super) fn before(&self, at: BlockId) -> Result<Option<BlockId>> {
        if at.0 <= HEADER_LEN {
            return Ok(None);
        }
        let mut trailer = [0; format::LEN_BYTES];
        let trailer_at = at.0.checked_sub(u64::try_from(format::LEN_BYTES).unwrap_or(u64::MAX)).ok_or_else(|| self.beyond())?;
        self.file.read_exact_at(&mut trailer, trailer_at)?;
        let len = usize::try_from(u32::from_le_bytes(trailer)).unwrap_or(usize::MAX);
        let frame = u64::try_from(format::frame_len(len, self.hash.digest_len())).unwrap_or(u64::MAX);
        match at.0.checked_sub(frame).filter(|&start| start >= HEADER_LEN) {
            Some(start) => Ok(Some(BlockId(start))),
            None => Err(SqlError::new(
                Code::Corrupt,
                format!(
                    "[DESIGN] the block ending at offset {} of {} claims a length that starts before the first block",
                    at.0,
                    self.path.display()
                ),
            )),
        }
    }

    /// Reads the block at `at`, if a valid one lies wholly before `limit`,
    /// leaving its payload in `out`: its first `FIRST_READ` bytes in one
    /// read, the rest, if any, in a second.
    fn block_at(&self, at: u64, limit: u64, out: &mut Vec<u8>) -> Result<Found> {
        let digest_len = self.hash.digest_len();
        let prefix_len = format::LEN_BYTES.saturating_add(digest_len);
        let before_limit = usize::try_from(limit.saturating_sub(at)).unwrap_or(usize::MAX);
        if before_limit < prefix_len {
            return Ok(Found::Invalid("it would extend past the end of the blocks".into()));
        }
        let first = before_limit.min(FIRST_READ.max(prefix_len));
        out.resize(first, 0);
        self.file.read_exact_at(out, at)?;
        let len = format::le_u32(out, 0).unwrap_or(u32::MAX);
        let payload_len = usize::try_from(len).unwrap_or(usize::MAX);
        let frame = format::frame_len(payload_len, digest_len);
        if frame > before_limit || len > self.options.max_block {
            return Ok(Found::Invalid(format!("its length, {len}, takes it past the end of the blocks or the limit")));
        }
        if frame > first {
            out.resize(frame, 0);
            let rest = out.get_mut(first..).unwrap_or_default();
            self.file.read_exact_at(rest, at.saturating_add(u64::try_from(first).unwrap_or(u64::MAX)))?;
        }
        out.truncate(frame);
        let digest = out.get(format::LEN_BYTES..prefix_len).unwrap_or_default();
        if !format::verify(&*self.hash, len, digest, out.get(prefix_len..).unwrap_or_default()) {
            return Ok(Found::Invalid("it fails its checksum".into()));
        }
        out.drain(..prefix_len);
        out.truncate(payload_len);
        Ok(Found::Block(at.saturating_add(u64::try_from(frame).unwrap_or(u64::MAX))))
    }
}
