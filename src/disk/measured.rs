//! \[DESIGN\] Statistics of file I/O, for the heuristics of the components
//! above (RULES.md D30): how much is read, written and synced, and how long
//! syncs take, so that a buffer pool, a log's group commit or a cost model
//! can decide from measurements rather than assumptions.
//!
//! `MeasuredFileSystem` wraps any `FileSystem`, so plug-ins are measured the
//! same way as the built-in one. Counting is lock-free: one atomic addition
//! per counter, with no ordering between counters, since a statistic need
//! only be close to the truth, never consistent with another.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode};
use crate::error::Result;

/// The counters of a measured file system, shared by every file it opened.
#[derive(Debug, Default)]
pub struct IoStats {
    reads: AtomicU64,
    bytes_read: AtomicU64,
    writes: AtomicU64,
    bytes_written: AtomicU64,
    syncs: AtomicU64,
    sync_nanos: AtomicU64,
    failures: AtomicU64,
}

/// The counters at one moment. Each one only grows; `since` gives the
/// activity between two moments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoCounts {
    /// Calls to `DiskFile::read_at` and `read_exact_at`.
    pub reads: u64,
    /// Bytes those calls returned.
    pub bytes_read: u64,
    /// Calls to `DiskFile::write_all_at`.
    pub writes: u64,
    /// Bytes those calls wrote.
    pub bytes_written: u64,
    /// Syncs that reached the device: `Durability::Data` or `Full`.
    pub syncs: u64,
    /// Time spent in those syncs.
    pub sync_time: Duration,
    /// Operations of the file system or its files that failed.
    pub failures: u64,
}

impl IoStats {
    /// The counters now.
    pub fn snapshot(&self) -> IoCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        IoCounts {
            reads: get(&self.reads),
            bytes_read: get(&self.bytes_read),
            writes: get(&self.writes),
            bytes_written: get(&self.bytes_written),
            syncs: get(&self.syncs),
            sync_time: Duration::from_nanos(get(&self.sync_nanos)),
            failures: get(&self.failures),
        }
    }

    /// Records the outcome of an operation: a failure, or what `count`
    /// adds for a success.
    fn record<T>(&self, outcome: Result<T>, count: impl FnOnce(&Self, &T)) -> Result<T> {
        match &outcome {
            Ok(value) => count(self, value),
            Err(_) => add(&self.failures, 1),
        }
        outcome
    }
}

impl IoCounts {
    /// The activity between `earlier` and these counts.
    #[must_use]
    pub fn since(&self, earlier: &IoCounts) -> IoCounts {
        IoCounts {
            reads: self.reads.saturating_sub(earlier.reads),
            bytes_read: self.bytes_read.saturating_sub(earlier.bytes_read),
            writes: self.writes.saturating_sub(earlier.writes),
            bytes_written: self.bytes_written.saturating_sub(earlier.bytes_written),
            syncs: self.syncs.saturating_sub(earlier.syncs),
            sync_time: self.sync_time.saturating_sub(earlier.sync_time),
            failures: self.failures.saturating_sub(earlier.failures),
        }
    }
}

/// Adds `n` to `counter`. It would wrap only after 2^64 bytes (16 EiB) or
/// operations, which no database reaches.
fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

/// `n` bytes as a count.
fn bytes(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// \[DESIGN\] A file system whose I/O is counted in its `IoStats`.
pub struct MeasuredFileSystem {
    inner: Box<dyn FileSystem>,
    stats: Arc<IoStats>,
}

impl MeasuredFileSystem {
    /// `inner`, measured.
    pub fn new(inner: Box<dyn FileSystem>) -> Self {
        MeasuredFileSystem { inner, stats: Arc::new(IoStats::default()) }
    }

    /// Its counters, which every file it opens adds to.
    pub fn stats(&self) -> Arc<IoStats> {
        Arc::clone(&self.stats)
    }
}

impl FileSystem for MeasuredFileSystem {
    fn lock(&self, path: &Path) -> Result<FileLock> {
        self.inner.lock(path)
    }
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let file = self.stats.record(self.inner.open(path, mode), |_, _| ())?;
        Ok(Box::new(MeasuredFile { inner: file, stats: Arc::clone(&self.stats) }))
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.stats.record(self.inner.remove(path), |_, ()| ())
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        self.stats.record(self.inner.exists(path), |_, _| ())
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.stats.record(self.inner.create_dir_all(path), |_, ()| ())
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.stats.record(self.inner.read_dir(path), |_, _| ())
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        let started = Instant::now();
        self.stats.record(self.inner.sync_dir(path), |s, ()| s.synced(started))
    }
}

impl IoStats {
    /// Counts a sync to the device that began at `started`.
    fn synced(&self, started: Instant) {
        add(&self.syncs, 1);
        add(&self.sync_nanos, u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    }
}

/// A file of a `MeasuredFileSystem`.
struct MeasuredFile {
    inner: Box<dyn DiskFile>,
    stats: Arc<IoStats>,
}

impl DiskFile for MeasuredFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.stats.record(self.inner.read_at(buf, offset), |s, &n| {
            add(&s.reads, 1);
            add(&s.bytes_read, bytes(n));
        })
    }
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        self.stats.record(self.inner.write_all_at(data, offset), |s, ()| {
            add(&s.writes, 1);
            add(&s.bytes_written, bytes(data.len()));
        })
    }
    fn size(&self) -> Result<u64> {
        self.stats.record(self.inner.size(), |_, _| ())
    }
    fn set_size(&self, size: u64) -> Result<()> {
        self.stats.record(self.inner.set_size(size), |_, ()| ())
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        let started = Instant::now();
        self.stats.record(self.inner.sync(durability), |s, ()| match durability {
            Durability::None => {}
            Durability::Data | Durability::Full => s.synced(started),
        })
    }
}
