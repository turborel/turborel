//! \[DESIGN\] The file manager (RULES.md D56): every file a database opens,
//! through the `FileSystem` it was given, registered and limited.
//!
//! `FileManager` is a `FileSystem` over the one the database was given:
//! each file it opens is a handle that keeps where it is and how it is
//! opened again, and counts its reads, writes and syncs. At most
//! FILES.MAX_OPEN of a database's files are open at once (0 for no
//! limit): to open one past it, the one used least recently and not being
//! read or written is closed, and opened again as it is next used. While
//! every open file is being read or written, the next waits for one to be
//! done. Reads and writes of one file take its handle shared, so they go
//! on at once as they did; only closing it takes it whole.
//!
//! The registry is the database's, shared by its snapshots, so
//! INFORMATION_SCHEMA.FILES shows each file (`system/files.rs`). As the
//! database closes, every descriptor is closed (`Registry::close_all`)
//! before its directory's lock is given up, and none is opened after.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock, RwLockReadGuard, Weak};
use std::time::Duration;

use crate::disk::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode};
use crate::error::{Code, Result, SqlError};

/// \[DESIGN\] The default of FILES.MAX_OPEN: well under the 1024 most
/// systems allow a process by default, for the connections and the rest
/// a server holds besides.
pub const DEFAULT_MAX_OPEN: usize = 1024;

/// \[DESIGN\] How long a file waiting for a descriptor sleeps before it
/// looks again, if no file done with one wakes it first.
const LOOK_AGAIN: Duration = Duration::from_millis(10);

/// \[DESIGN\] A database's files: those it holds now, the most it may
/// hold open, and a clock of their uses.
#[derive(Default)]
pub struct Registry {
    files: Mutex<HashMap<u64, Weak<Handle>>>,
    next: AtomicU64,
    /// FILES.MAX_OPEN; 0 for no limit.
    limit: AtomicUsize,
    /// The files open now.
    open: AtomicUsize,
    /// The uses so far, each file's last kept as its place in the order.
    clock: AtomicU64,
    /// Woken as a file is done being read or written, for one waiting.
    done: Condvar,
    waiting: Mutex<()>,
    /// Set as the database closes: no file is opened after.
    shut: AtomicBool,
}

/// \[DESIGN\] A file, as INFORMATION_SCHEMA.FILES shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Where it is.
    pub path: PathBuf,
    /// Whether its descriptor is open now.
    pub open: bool,
    /// Its reads, writes and syncs.
    pub reads: u64,
    /// See `reads`.
    pub writes: u64,
    /// See `reads`.
    pub syncs: u64,
}

/// A file of the manager's: where it is, how it is opened again, and its
/// descriptor while open.
struct Handle {
    fs: Arc<dyn FileSystem>,
    path: PathBuf,
    reopen: OpenMode,
    file: RwLock<Option<Box<dyn DiskFile>>>,
    used: AtomicU64,
    counts: [AtomicU64; 3],
    registry: Arc<Registry>,
}

/// Which of a handle's counts.
#[derive(Clone, Copy)]
enum Use {
    Read,
    Write,
    Sync,
}

impl Use {
    /// Its place among a handle's counts.
    fn at(self) -> usize {
        match self {
            Use::Read => 0,
            Use::Write => 1,
            Use::Sync => 2,
        }
    }
}

impl Registry {
    /// Takes `limit` as FILES.MAX_OPEN; 0 for none.
    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Release);
        self.done.notify_all();
    }

    /// Each file the database holds, as `FileInfo` shows it, by path.
    pub fn infos(&self) -> Vec<FileInfo> {
        let handles: Vec<Arc<Handle>> = self.locked().values().filter_map(Weak::upgrade).collect();
        let mut infos: Vec<FileInfo> = handles.iter().map(|h| h.info()).collect();
        infos.sort_by(|a, b| a.path.cmp(&b.path));
        infos
    }

    /// Closes every file's descriptor, as the database closes, and lets
    /// none be opened after: the paths of those still being read or
    /// written, left open.
    pub fn close_all(&self) -> Vec<PathBuf> {
        self.shut.store(true, Ordering::Release);
        let handles: Vec<Arc<Handle>> = self.locked().values().filter_map(Weak::upgrade).collect();
        handles.iter().filter(|h| !h.close_if_idle() && h.info().open).map(|h| h.path.clone()).collect()
    }

    /// The error of a file opened after `close_all`, if it was called.
    fn refused(&self, path: &Path) -> Result<()> {
        if self.shut.load(Ordering::Acquire) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} is not opened: its database is closed", path.display())));
        }
        Ok(())
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Weak<Handle>>> {
        // Entries are added and removed whole, so a poisoned map is whole.
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A descriptor's place, for `asking`, which holds none: once fewer than
    /// the limit are open, closing the file used least recently that is
    /// not in use if none is free, else waiting for one to be done.
    fn admit(&self, asking: u64) {
        loop {
            let limit = self.limit.load(Ordering::Acquire);
            let open = self.open.load(Ordering::Acquire);
            if limit == 0 || open < limit {
                if self.open.compare_exchange(open, open.saturating_add(1), Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    return;
                }
                continue;
            }
            if !self.evict(asking) {
                let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
                drop(self.done.wait_timeout(waiting, LOOK_AGAIN).unwrap_or_else(PoisonError::into_inner));
            }
        }
    }

    /// Closes the open file, but `asking`, used least recently that no one
    /// reads or writes now: whether it closed one.
    fn evict(&self, asking: u64) -> bool {
        let mut handles: Vec<(u64, Arc<Handle>)> =
            self.locked().iter().filter(|(id, _)| **id != asking).filter_map(|(id, h)| Some((*id, h.upgrade()?))).collect();
        handles.sort_by_key(|(_, h)| h.used.load(Ordering::Relaxed));
        handles.into_iter().any(|(_, h)| h.close_if_idle())
    }
}

impl Handle {
    /// Its descriptor, shared, opened again if it was closed.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be opened again.
    fn held(&self, id: u64) -> Result<RwLockReadGuard<'_, Option<Box<dyn DiskFile>>>> {
        loop {
            let read = self.file.read().unwrap_or_else(PoisonError::into_inner);
            if read.is_some() {
                return Ok(read);
            }
            drop(read);
            let mut write = self.file.write().unwrap_or_else(PoisonError::into_inner);
            if write.is_none() {
                self.registry.refused(&self.path)?;
                self.registry.admit(id);
                match self.fs.open(&self.path, self.reopen) {
                    Ok(file) => *write = Some(file),
                    Err(e) => {
                        self.registry.open.fetch_sub(1, Ordering::AcqRel);
                        return Err(e);
                    }
                }
            }
        }
    }

    /// Does `op` with its descriptor, counted as `what`, its use marked.
    fn with<T>(&self, id: u64, what: Use, op: impl FnOnce(&dyn DiskFile) -> Result<T>) -> Result<T> {
        let held = self.held(id)?;
        self.used.store(self.registry.clock.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
        if let Some(count) = self.counts.get(what.at()) {
            count.fetch_add(1, Ordering::Relaxed);
        }
        let done = match held.as_deref() {
            Some(file) => op(file),
            None => Err(closed()),
        };
        drop(held);
        self.registry.done.notify_one();
        done
    }

    /// Closes its descriptor, if it is open and no one reads or writes it:
    /// whether it did.
    fn close_if_idle(&self) -> bool {
        let Ok(mut file) = self.file.try_write() else { return false };
        if file.take().is_none() {
            return false;
        }
        self.registry.open.fetch_sub(1, Ordering::AcqRel);
        true
    }

    fn info(&self) -> FileInfo {
        let count = |u: Use| self.counts.get(u.at()).map_or(0, |c| c.load(Ordering::Relaxed));
        let open = self.file.try_read().map_or(true, |f| f.is_some());
        FileInfo { path: self.path.clone(), open, reads: count(Use::Read), writes: count(Use::Write), syncs: count(Use::Sync) }
    }
}

/// The error of a descriptor closed under a read or write, which the
/// handle's lock never lets be.
fn closed() -> SqlError {
    SqlError::new(Code::Io, "[DESIGN] a file's descriptor was closed under it".to_owned())
}

/// \[DESIGN\] A file the manager opened: its handle, while it lives.
struct Managed {
    id: u64,
    handle: Arc<Handle>,
}

impl DiskFile for Managed {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.handle.with(self.id, Use::Read, |f| f.read_at(buf, offset))
    }
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        self.handle.with(self.id, Use::Write, |f| f.write_all_at(data, offset))
    }
    fn size(&self) -> Result<u64> {
        self.handle.with(self.id, Use::Read, |f| f.size())
    }
    fn set_size(&self, size: u64) -> Result<()> {
        self.handle.with(self.id, Use::Write, |f| f.set_size(size))
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        self.handle.with(self.id, Use::Sync, |f| f.sync(durability))
    }
    fn reserve(&self, size: u64) -> Result<()> {
        self.handle.with(self.id, Use::Write, |f| f.reserve(size))
    }
}

impl Drop for Managed {
    fn drop(&mut self) {
        self.handle.registry.locked().remove(&self.id);
        if self.handle.file.write().unwrap_or_else(PoisonError::into_inner).take().is_some() {
            self.handle.registry.open.fetch_sub(1, Ordering::AcqRel);
            self.handle.registry.done.notify_one();
        }
    }
}

/// \[DESIGN\] A `FileSystem` over `inner`, its files registered in
/// `registry` and limited as it says.
pub struct FileManager {
    inner: Arc<dyn FileSystem>,
    registry: Arc<Registry>,
}

impl FileManager {
    /// The manager of the files of `inner`, in `registry`.
    pub fn over(inner: Arc<dyn FileSystem>, registry: Arc<Registry>) -> FileManager {
        FileManager { inner, registry }
    }
}

impl FileSystem for FileManager {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        self.registry.refused(path)?;
        let id = self.registry.next.fetch_add(1, Ordering::Relaxed);
        self.registry.admit(id);
        let file = self.inner.open(path, mode).inspect_err(|_| {
            self.registry.open.fetch_sub(1, Ordering::AcqRel);
        })?;
        let reopen = match mode {
            OpenMode::CreateNew | OpenMode::ReadWrite => OpenMode::ReadWrite,
            OpenMode::ReadOnly => OpenMode::ReadOnly,
        };
        let handle = Arc::new(Handle {
            fs: Arc::clone(&self.inner),
            path: path.to_path_buf(),
            reopen,
            file: RwLock::new(Some(file)),
            used: AtomicU64::new(self.registry.clock.fetch_add(1, Ordering::Relaxed)),
            counts: Default::default(),
            registry: Arc::clone(&self.registry),
        });
        self.registry.locked().insert(id, Arc::downgrade(&handle));
        Ok(Box::new(Managed { id, handle }))
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.inner.remove(path)
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        self.inner.exists(path)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.inner.create_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.inner.sync_dir(path)
    }
    fn lock(&self, path: &Path) -> Result<FileLock> {
        self.inner.lock(path)
    }
}

pub mod plugins;

// Unit tests: the limit kept by closing the file used least recently,
// files opened again as used, and the counts (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
