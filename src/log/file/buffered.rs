//! \[DESIGN\] A segment's file written through a buffer (RULES.md D25,
//! D921): the records appended go into memory, and reach the file in one
//! write for many, when the buffer fills, when a sync, a cut or a close
//! needs them, when the log is asked to write what it holds
//! (`Log::write_buffered`), or with the first append made `WRITE_AFTER`
//! past the oldest record held. Until then a record is in this process
//! alone, so a commit that does not wait for its sync is lost by a failure
//! of the process as it was by a failure of the system before the sync.
//!
//! The buffer holds runs of bytes, each at its offset in the file, the
//! records of appenders one after another making one run; a write that
//! arrives before the one reserved behind it makes a run of its own,
//! joined to the first once the bytes between them come. Nothing is held
//! for the bytes between two runs, which the file may hold already. A
//! write over bytes held has those written first, and a write as long as
//! the buffer goes to the file: of records, after what is held, so the
//! file never holds a record past bytes held (RULES.md D1141); of zeros
//! written ahead, leaving what is held. Reads see the
//! runs over the file, so a record is read back from the moment it is
//! appended.
//!
//! A write to the file that fails is kept: every later write, sync and
//! cut fails with it, as what the buffer held is then not in the file,
//! and the block file above takes no append after a failed write.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::disk::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode};
use crate::error::{Result, SqlError};
use crate::sync::lock_builtin;

/// \[DESIGN\] How long a record may stay in the buffer while records are
/// appended after it: a second, as LOG.SYNC_AFTER_MS bounds by default
/// what a failure of the system loses of the commits not waiting for the
/// log (`db/log_sync.rs`), so a failure of the process loses no more.
pub(super) const WRITE_AFTER: Duration = Duration::from_secs(1);

/// \[DESIGN\] The bytes of records the database's log holds in memory
/// before it writes them (`FileLogOptions::buffer`, RULES.md D921).
pub const LOG_BUFFER: usize = 64 * 1024;

/// A file system whose files opened for writing are buffered, `bytes`
/// each: a file log's, as it makes or opens its segments.
pub(super) struct Buffering<'f> {
    pub(super) fs: &'f dyn FileSystem,
    pub(super) bytes: usize,
}

impl FileSystem for Buffering<'_> {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let file = self.fs.open(path, mode)?;
        if self.bytes == 0 {
            return Ok(file);
        }
        Ok(match mode {
            OpenMode::CreateNew | OpenMode::ReadWrite => Box::new(Buffered::over(file, self.bytes)),
            OpenMode::ReadOnly => file,
        })
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.fs.remove(path)
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        self.fs.exists(path)
    }
    fn discard(&self, path: &Path) {
        self.fs.discard(path);
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.fs.create_dir_all(path)
    }
    fn create_dir_durably(&self, path: &Path) -> Result<()> {
        self.fs.create_dir_durably(path)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.fs.rename(from, to)
    }
    fn remove_tree(&self, path: &Path) -> Result<()> {
        self.fs.remove_tree(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.fs.read_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.fs.sync_dir(path)
    }
    fn lock(&self, path: &Path) -> Result<FileLock> {
        self.fs.lock(path)
    }
}

/// A run of bytes held for the file, from offset `at`.
struct Run {
    at: u64,
    bytes: Vec<u8>,
}

impl Run {
    /// The offset after its last byte.
    fn end(&self) -> u64 {
        self.at.saturating_add(offset_of(self.bytes.len()))
    }

    /// Whether it shares a byte with `len` bytes at `offset`.
    fn overlaps(&self, offset: u64, len: usize) -> bool {
        offset < self.end() && offset.saturating_add(offset_of(len)) > self.at
    }
}

/// The bytes held for the file: runs in the order of their offsets, no
/// two sharing a byte or touching, `bytes` in all, the oldest held since
/// `since`.
#[derive(Default)]
struct Tail {
    runs: Vec<Run>,
    bytes: usize,
    since: Option<Instant>,
    /// The room of a run written, kept for the next.
    spare: Vec<u8>,
    /// The failure of a write of them to the file, once one failed.
    failed: Option<SqlError>,
}

/// A file written through a buffer of `limit` bytes.
pub(super) struct Buffered {
    file: Box<dyn DiskFile>,
    tail: Mutex<Tail>,
    /// The bytes `tail` holds, stored under its lock: a read while it
    /// holds none takes no lock.
    held: AtomicUsize,
    limit: usize,
}

/// `n` as an offset.
fn offset_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Whether bytes held since `since` have been held `WRITE_AFTER` at `now`.
fn late(since: Option<Instant>, now: Instant) -> bool {
    since.is_some_and(|since| now.saturating_duration_since(since) >= WRITE_AFTER)
}

impl Tail {
    /// Whether `len` bytes at `offset` share a byte with those held.
    fn overlaps(&self, offset: u64, len: usize) -> bool {
        self.runs.iter().any(|run| run.overlaps(offset, len))
    }

    /// The offset after the last byte held, 0 with none.
    fn end(&self) -> u64 {
        self.runs.last().map_or(0, Run::end)
    }

    /// Holds `data`, which shares no byte with those held, at `offset`:
    /// added to the run it follows, which then takes in the run it
    /// reaches, or a run of its own in its place.
    fn put(&mut self, data: &[u8], offset: u64) {
        let next = self.runs.partition_point(|run| run.at < offset);
        let end = offset.saturating_add(offset_of(data.len()));
        let follows = next.checked_sub(1).filter(|&before| self.runs.get(before).is_some_and(|run| run.end() == offset));
        let touches = self.runs.get(next).is_some_and(|run| run.at == end);
        let after = if touches { self.runs.remove(next).bytes } else { Vec::new() };
        if let Some(run) = follows.and_then(|before| self.runs.get_mut(before)) {
            run.bytes.extend_from_slice(data);
            run.bytes.extend_from_slice(&after);
        } else {
            let mut bytes = std::mem::take(&mut self.spare);
            bytes.extend_from_slice(data);
            bytes.extend_from_slice(&after);
            self.runs.insert(next, Run { at: offset, bytes });
        }
        self.bytes = self.bytes.saturating_add(data.len());
        self.since.get_or_insert_with(Instant::now);
    }
}

impl Buffered {
    fn over(file: Box<dyn DiskFile>, limit: usize) -> Buffered {
        Buffered { file, tail: Mutex::default(), held: AtomicUsize::new(0), limit }
    }

    /// Writes the bytes held to the file, a write a run, and holds none.
    /// A failure is kept, the bytes with it, still read.
    fn write_held(&self, tail: &mut Tail) -> Result<()> {
        if let Some(e) = &tail.failed {
            return Err(e.clone());
        }
        for run in &tail.runs {
            if let Err(e) = self.file.write_all_at(&run.bytes, run.at) {
                tail.failed = Some(e.clone());
                return Err(e);
            }
        }
        // The first run's room is kept for the records to come.
        if let Some(run) = tail.runs.drain(..).next() {
            tail.spare = run.bytes;
            tail.spare.clear();
        }
        (tail.bytes, tail.since) = (0, None);
        self.held.store(0, Ordering::Release);
        Ok(())
    }

    /// Takes `data` at `offset` into the buffer, or writes it to the
    /// file: see the module's documentation.
    fn take(&self, tail: &mut Tail, data: &[u8], offset: u64) -> Result<()> {
        if let Some(e) = &tail.failed {
            return Err(e.clone());
        }
        // Over bytes held, those go to the file first, which then takes
        // both in the order they were written.
        if tail.overlaps(offset, data.len()) {
            self.write_held(tail)?;
        }
        if data.len() >= self.limit {
            // [DESIGN] RULES.md D1141: what is held goes first, but for
            // zeros written ahead, so the file never holds a record past
            // bytes still held, which a failure of the process would leave
            // as zeros before it: a segment's header lost under its records.
            if data.iter().any(|&b| b != 0) {
                self.write_held(tail)?;
            }
            return self.file.write_all_at(data, offset);
        }
        tail.put(data, offset);
        self.held.store(tail.bytes, Ordering::Release);
        if tail.bytes >= self.limit || late(tail.since, Instant::now()) { self.write_held(tail) } else { Ok(()) }
    }

    /// Reads into `buf` from `offset` with the bytes `tail` holds over
    /// the file's: as many as the file has, or as those held reach.
    fn read_over(&self, tail: &Tail, buf: &mut [u8], offset: u64) -> Result<usize> {
        let read = self.file.read_at(buf, offset)?;
        // Past the file's end, bytes not held read as zeros, as they will
        // once those held after them are written.
        buf.get_mut(read..).unwrap_or_default().fill(0);
        let len = buf.len();
        for run in tail.runs.iter().filter(|run| run.overlaps(offset, len)) {
            // The run from where the read begins, and the read from where
            // the run begins.
            let skipped = usize::try_from(offset.saturating_sub(run.at)).unwrap_or(usize::MAX);
            let from = usize::try_from(run.at.saturating_sub(offset)).unwrap_or(usize::MAX);
            let held = run.bytes.get(skipped..).unwrap_or_default();
            let into = buf.get_mut(from..).unwrap_or_default();
            let n = held.len().min(into.len());
            if let (Some(into), Some(held)) = (into.get_mut(..n), held.get(..n)) {
                into.copy_from_slice(held);
            }
        }
        // The file ends where the last byte held does, if that is later.
        let within = usize::try_from(tail.end().saturating_sub(offset)).unwrap_or(usize::MAX).min(len);
        Ok(read.max(within))
    }
}

impl DiskFile for Buffered {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        if self.held.load(Ordering::Acquire) != 0 {
            let tail = lock_builtin(&self.tail);
            // Before the end of those held, which is the file's then.
            if offset < tail.end() {
                return self.read_over(&tail, buf, offset);
            }
        }
        self.file.read_at(buf, offset)
    }

    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        let mut tail = lock_builtin(&self.tail);
        self.take(&mut tail, data, offset)
    }

    fn size(&self) -> Result<u64> {
        let end = lock_builtin(&self.tail).end();
        Ok(self.file.size()?.max(end))
    }

    /// What is held is written first, so the cut or the growth is of the
    /// file as it was written.
    fn set_size(&self, size: u64) -> Result<()> {
        let mut tail = lock_builtin(&self.tail);
        self.write_held(&mut tail)?;
        self.file.set_size(size)
    }

    /// What is held is written first, whatever `durability`: a sync that
    /// syncs nothing, which the file is not asked for, still leaves it
    /// holding every record.
    fn sync(&self, durability: Durability) -> Result<()> {
        {
            let mut tail = lock_builtin(&self.tail);
            self.write_held(&mut tail)?;
        }
        match durability {
            Durability::None => Ok(()),
            Durability::Data | Durability::Full => self.file.sync(durability),
        }
    }

    fn reserve(&self, size: u64) -> Result<()> {
        self.file.reserve(size)
    }

    fn write_captured_at(&self, data: &[u8], offset: u64) -> Result<()> {
        self.file.write_captured_at(data, offset)
    }
}

impl Drop for Buffered {
    /// A log dropped without a sync, as a database dropped without being
    /// closed, leaves its file holding every record all the same. A
    /// failure cannot be told from here; the records it loses were not
    /// synced, so nothing was promised of them.
    fn drop(&mut self) {
        let mut tail = lock_builtin(&self.tail);
        let _ = self.write_held(&mut tail);
    }
}

// Unit tests: writes in and out of order, reads over the file, the
// buffer's bounds and a failed write (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
