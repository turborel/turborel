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
//! A file closed with writes not yet synced would lose the error of their
//! writeback: on Linux, a sync through a descriptor opened after the error
//! need not report it, so a later sync could succeed though data was lost.
//! So a file with writes not yet synced is closed only after the files
//! without, and synced (`Durability::Data`) before it is closed. If that
//! sync fails, the file stays open, the failure is told, and it is kept
//! for the file's next sync to return, so no later sync stands for the
//! writes lost.
//!
//! The registry is the database's, shared by its snapshots, so
//! INFORMATION_SCHEMA.FILES shows each file (`system/files.rs`), of its
//! kind, class of durability and owner (`classes.rs`). As the
//! database closes, every descriptor is closed (`Registry::close_all`),
//! a file with writes not yet synced synced first, before its directory's
//! lock is given up, and none is opened after.

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
    /// The files noted, not the database's (`noted.rs`).
    noted: Mutex<HashMap<u64, Weak<noted::Entry>>>,
    /// What it tells, and where (`told.rs`).
    teller: told::Teller,
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
    /// Whether it is not the database's, but noted as another's: a
    /// server's (`Registry::note`).
    pub noted: bool,
    /// Its kind, by its name and its directory's (`classes.rs`).
    pub kind: FileKind,
    /// Its class of durability; none for a file not the database's.
    pub class: Option<FileClass>,
    /// Whose it is.
    pub owner: FileOwner,
}

impl FileInfo {
    /// The file at `path`, of `kind`, open if `open`, of `counts` (reads,
    /// writes and syncs): noted as another's if it is a server's; its
    /// class and owner known by its kind and path.
    fn at(path: &Path, kind: FileKind, open: bool, counts: [u64; 3]) -> FileInfo {
        let [reads, writes, syncs] = counts;
        let noted = kind == FileKind::Server;
        FileInfo { path: path.to_path_buf(), open, reads, writes, syncs, noted, kind, class: kind.class(), owner: kind.owner(path) }
    }
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
    /// Whether it has writes not yet synced: set by a write or a change of
    /// size, cleared by a sync of some durability that succeeds.
    dirty: AtomicBool,
    /// The failure of a sync as it was to be closed, which no caller has
    /// seen: its next sync returns it.
    lost: Mutex<Option<SqlError>>,
}

/// What came of closing a handle's descriptor.
struct Closing {
    /// Whether it was closed.
    closed: bool,
    /// The failure of its sync before, if that failure is new to it: kept
    /// for its next sync, and to be told.
    failed: Option<SqlError>,
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
        let noted: Vec<Arc<noted::Entry>> = self.notes().values().filter_map(Weak::upgrade).collect();
        infos.extend(noted.iter().map(|n| n.info()));
        infos.sort_by(|a, b| a.path.cmp(&b.path));
        infos
    }

    /// Closes every file's descriptor, as the database closes, and lets
    /// none be opened after: the paths of those still being read or
    /// written, left open. A file with writes not yet synced, which a
    /// clean checkpoint leaves only of the classes never synced, is synced
    /// first; if that fails, the failure is told, and it is closed all the
    /// same, as nothing would sync it later.
    pub fn close_all(&self) -> Vec<PathBuf> {
        self.shut.store(true, Ordering::Release);
        let handles: Vec<Arc<Handle>> = self.locked().values().filter_map(Weak::upgrade).collect();
        let mut left = Vec::new();
        for handle in &handles {
            let closing = handle.close_if_idle(false);
            if let Some(e) = &closing.failed {
                self.tell_failed(e);
            }
            if !closing.closed && handle.info().open {
                left.push(handle.path.clone());
            }
        }
        left
    }

    /// The error of a file opened after `close_all`, if it was called.
    fn refused(&self, path: &Path) -> Result<()> {
        if self.shut.load(Ordering::Acquire) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} is not opened: its database is closed", path.display())));
        }
        Ok(())
    }

    fn notes(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Weak<noted::Entry>>> {
        // Entries are added and removed whole, so a poisoned map is whole.
        self.noted.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Weak<Handle>>> {
        // Entries are added and removed whole, so a poisoned map is whole.
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A descriptor's place, for `asking`, which holds none: once fewer than
    /// the limit are open, closing a file not in use if none is free (see
    /// `evict`), else waiting for one to be done. The failures of syncs of
    /// files to be closed, for the caller to tell once it holds no lock.
    fn admit(&self, asking: u64) -> Vec<SqlError> {
        let mut failed = Vec::new();
        loop {
            let limit = self.limit.load(Ordering::Acquire);
            let open = self.open.load(Ordering::Acquire);
            if limit == 0 || open < limit {
                if self.open.compare_exchange(open, open.saturating_add(1), Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    return failed;
                }
                continue;
            }
            if !self.evict(asking, &mut failed) {
                let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
                drop(self.done.wait_timeout(waiting, LOOK_AGAIN).unwrap_or_else(PoisonError::into_inner));
            }
        }
    }

    /// Closes an open file, but `asking`, that no one reads or writes now:
    /// of those without writes not yet synced, the one used least
    /// recently; else, of those with, the one used least recently whose
    /// sync succeeds. Whether it closed one; a sync's failure new to its
    /// file added to `failed`.
    fn evict(&self, asking: u64, failed: &mut Vec<SqlError>) -> bool {
        let mut handles: Vec<(u64, Arc<Handle>)> =
            self.locked().iter().filter(|(id, _)| **id != asking).filter_map(|(id, h)| Some((*id, h.upgrade()?))).collect();
        handles.sort_by_key(|(_, h)| (h.dirty.load(Ordering::Acquire), h.used.load(Ordering::Relaxed)));
        for (_, handle) in handles {
            let closing = handle.close_if_idle(true);
            failed.extend(closing.failed);
            if closing.closed {
                return true;
            }
        }
        false
    }

    /// Tells each of `failed`.
    fn tell_each(&self, failed: &[SqlError]) {
        for e in failed {
            self.tell_failed(e);
        }
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
            let (opened, failed) = self.open_again(id);
            // Told once its own lock is given back (RULES.md R-ERR-4).
            self.registry.tell_each(&failed);
            opened?;
        }
    }

    /// Opens its descriptor again, if it is closed, its lock held whole
    /// meanwhile: the failures of syncs of files closed for it, to tell.
    fn open_again(&self, id: u64) -> (Result<()>, Vec<SqlError>) {
        let mut write = self.file.write().unwrap_or_else(PoisonError::into_inner);
        if write.is_some() {
            return (Ok(()), Vec::new());
        }
        if let Err(e) = self.registry.refused(&self.path) {
            return (Err(e), Vec::new());
        }
        let failed = self.registry.admit(id);
        match self.fs.open(&self.path, self.reopen) {
            Ok(file) => {
                *write = Some(file);
                self.registry.reopened(std::time::Instant::now());
                (Ok(()), failed)
            }
            Err(e) => {
                self.registry.open.fetch_sub(1, Ordering::AcqRel);
                (Err(e), failed)
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

    /// Does `op`, a write or a change of size, with its descriptor: its
    /// writes not yet synced from then, though it fails, as part of it may
    /// have been written.
    fn write<T>(&self, id: u64, op: impl FnOnce(&dyn DiskFile) -> Result<T>) -> Result<T> {
        self.with(id, Use::Write, |f| {
            let done = op(f);
            self.dirty.store(true, Ordering::Release);
            done
        })
    }

    /// Syncs it as `durability` says. Of a durability but `None`, a failure
    /// kept as it was to be closed is returned first, once, so no sync
    /// stands for the writes it lost; its writes are synced from then if
    /// the sync succeeds.
    fn sync(&self, id: u64, durability: Durability) -> Result<()> {
        self.with(id, Use::Sync, |f| {
            if durability == Durability::None {
                return f.sync(durability);
            }
            if let Some(e) = self.lost().take() {
                return Err(e);
            }
            // Cleared before, so a write done meanwhile is not taken as
            // synced: it marks the file again.
            self.dirty.store(false, Ordering::Release);
            f.sync(durability).inspect_err(|_| self.dirty.store(true, Ordering::Release))
        })
    }

    fn lost(&self) -> std::sync::MutexGuard<'_, Option<SqlError>> {
        // A failure is set or taken whole, so a poisoned one is whole.
        self.lost.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Closes its descriptor, if it is open and no one reads or writes it,
    /// synced first if it has writes not yet synced. If that sync fails,
    /// the failure is kept for its next sync, and it stays open if
    /// `keep_if_failed`.
    fn close_if_idle(&self, keep_if_failed: bool) -> Closing {
        let Ok(mut file) = self.file.try_write() else { return Closing { closed: false, failed: None } };
        let Some(open) = file.as_deref() else { return Closing { closed: false, failed: None } };
        let mut failed = None;
        if self.dirty.load(Ordering::Acquire) {
            if let Some(count) = self.counts.get(Use::Sync.at()) {
                count.fetch_add(1, Ordering::Relaxed);
            }
            match open.sync(Durability::Data) {
                Ok(()) => self.dirty.store(false, Ordering::Release),
                Err(e) => {
                    failed = self.kept(e);
                    if keep_if_failed {
                        return Closing { closed: false, failed };
                    }
                }
            }
        }
        drop(file.take());
        self.registry.open.fetch_sub(1, Ordering::AcqRel);
        Closing { closed: true, failed }
    }

    /// Keeps `e` for its next sync, unless one is kept already: `e`, if it
    /// was kept, to be told.
    fn kept(&self, e: SqlError) -> Option<SqlError> {
        let mut lost = self.lost();
        if lost.is_some() {
            return None;
        }
        *lost = Some(e.clone());
        Some(e)
    }

    fn info(&self) -> FileInfo {
        let count = |u: Use| self.counts.get(u.at()).map_or(0, |c| c.load(Ordering::Relaxed));
        let open = self.file.try_read().map_or(true, |f| f.is_some());
        FileInfo::at(&self.path, FileKind::of(&self.path), open, [count(Use::Read), count(Use::Write), count(Use::Sync)])
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
        self.handle.write(self.id, |f| f.write_all_at(data, offset))
    }
    fn size(&self) -> Result<u64> {
        self.handle.with(self.id, Use::Read, |f| f.size())
    }
    fn set_size(&self, size: u64) -> Result<()> {
        self.handle.write(self.id, |f| f.set_size(size))
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        self.handle.sync(self.id, durability)
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
        let failed = self.registry.admit(id);
        self.registry.tell_each(&failed);
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
            dirty: AtomicBool::new(false),
            lost: Mutex::new(None),
        });
        self.registry.locked().insert(id, Arc::downgrade(&handle));
        Ok(Box::new(Managed { id, handle }))
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.inner.remove(path)
    }
    /// A failure told to the database's sink, as no caller tells it.
    fn discard(&self, path: &Path) {
        if let Err(e) = self.inner.remove(path) {
            self.registry.tell_failed(&e);
        }
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

mod classes;
mod noted;
pub mod plugins;
mod told;

pub use classes::{FileClass, FileKind, FileOwner};
pub use noted::Noted;

// Unit tests: the limit kept by closing the file used least recently,
// files opened again as used, and the counts (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
