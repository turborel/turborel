//! Unit tests of the file manager (RULES.md R-TEST-1): the limit of open
//! files kept by closing the one used least recently, a file opened again
//! as it is next used, a file waiting while every open one is in use, a
//! file with writes not yet synced synced before it is closed and a failed
//! sync kept for its next, and the counts shown. Split from files.rs
//! (RULES.md R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::diagnostics::{Diagnostics, Event};
use crate::disk::memory::MemoryFileSystem;

/// A manager of a file system in memory, of at most `limit` open files.
fn manager(limit: usize) -> (FileManager, Arc<Registry>) {
    let registry = Arc::new(Registry::default());
    registry.set_limit(limit);
    (FileManager::over(Arc::new(MemoryFileSystem::new()), Arc::clone(&registry)), registry)
}

/// Verifies: [DESIGN] RULES.md D56: past the limit, the file used least
/// recently is closed to open another; each is opened again as it is
/// next used, what was written in it kept; no more than the limit are
/// ever open; each is shown, open or not, with its counts.
#[test]
fn files_past_the_limit_close_the_one_used_least_recently() {
    let (fs, registry) = manager(2);
    let files: Vec<Box<dyn DiskFile>> = ["/a", "/b", "/c"].iter().map(|p| fs.open(Path::new(p), OpenMode::CreateNew).unwrap()).collect();
    assert_eq!(registry.open.load(Ordering::Acquire), 2, "the limit kept");
    let open = |p: &str| registry.infos().iter().any(|i| i.path == Path::new(p) && i.open);
    assert!(!open("/a") && open("/b") && open("/c"), "/a, used least recently, closed");
    files[0].write_all_at(b"kept", 0).unwrap();
    assert!(open("/a") && !open("/b"), "/a opened again, /b closed for it");
    for file in &files {
        file.sync(Durability::Full).unwrap();
    }
    let mut back = [0_u8; 4];
    files[0].read_exact_at(&mut back, 0).unwrap();
    assert_eq!(&back, b"kept");
    assert!(registry.open.load(Ordering::Acquire) <= 2);
    let a = registry.infos().into_iter().find(|i| i.path == Path::new("/a")).unwrap();
    assert_eq!((a.reads, a.writes, a.syncs), (1, 1, 1));
    drop(files);
    assert_eq!((registry.open.load(Ordering::Acquire), registry.infos().len()), (0, 0), "all closed and gone");
}

/// Verifies: [DESIGN] RULES.md D56: of no limit, 0, every file stays open.
#[test]
fn no_limit_keeps_every_file_open() {
    let (fs, registry) = manager(0);
    let files: Vec<Box<dyn DiskFile>> = (0..5_u32).map(|n| fs.open(Path::new(&format!("/{n}")), OpenMode::CreateNew).unwrap()).collect();
    assert_eq!(registry.open.load(Ordering::Acquire), 5);
    assert!(registry.infos().iter().all(|i| i.open));
    drop(files);
}

/// Verifies: [DESIGN] RULES.md D56: while every open file is in use, a file
/// that needs a descriptor waits until one is done, then takes its place.
#[test]
fn a_file_waits_while_every_open_one_is_in_use() {
    let (fs, registry) = manager(1);
    let first = fs.open(Path::new("/one"), OpenMode::CreateNew).unwrap();
    // As a read of it in progress holds it.
    let handle = registry.locked().values().next().and_then(Weak::upgrade).unwrap();
    let in_use = handle.file.read().unwrap();
    let opening = std::thread::spawn(move || fs.open(Path::new("/two"), OpenMode::CreateNew).map(|_| ()));
    std::thread::sleep(Duration::from_millis(30));
    assert!(!opening.is_finished(), "it waits");
    drop(in_use);
    opening.join().unwrap().unwrap();
    assert!(registry.open.load(Ordering::Acquire) <= 1);
    drop((handle, first));
}

/// Verifies: [DESIGN] RULES.md D56: closing the database closes every
/// file's descriptor but one being read or written, which it tells; after,
/// no file is opened, nor opened again (SQLCODE -910).
#[test]
fn closing_closes_every_file_and_opens_none_after() {
    let (fs, registry) = manager(0);
    let files: Vec<Box<dyn DiskFile>> = ["/a", "/b"].iter().map(|p| fs.open(Path::new(p), OpenMode::CreateNew).unwrap()).collect();
    let busy = registry.locked().values().filter_map(Weak::upgrade).find(|h| h.path == Path::new("/b")).unwrap();
    let reading = busy.file.read().unwrap();
    assert_eq!(registry.close_all(), vec![Path::new("/b").to_path_buf()], "the one in use told");
    drop(reading);
    assert_eq!(registry.open.load(Ordering::Acquire), 1);
    let code = |r: Result<()>| r.err().map(|e| e.code);
    assert_eq!(code(files[0].write_all_at(b"x", 0)), Some(Code::Api), "not opened again");
    assert_eq!(code(fs.open(Path::new("/c"), OpenMode::CreateNew).map(drop)), Some(Code::Api));
    assert_eq!(registry.close_all(), Vec::<PathBuf>::new());
    assert_eq!(registry.open.load(Ordering::Acquire), 0);
}

/// Verifies: [DESIGN] RULES.md D56: a file noted, not the database's, is
/// shown as noted while its note lives, open as told, with the reads,
/// writes and syncs it is told; not under the limit; gone as it is dropped.
#[test]
fn a_file_noted_is_shown_while_its_note_lives() {
    let (_, registry) = manager(1);
    let noted = registry.note(Path::new("/server.log"), true);
    noted.read();
    noted.wrote();
    noted.synced();
    noted.set_open(false);
    let shown = registry.infos();
    assert_eq!(shown.iter().map(|f| (f.noted, f.open, f.reads, f.writes, f.syncs)).collect::<Vec<_>>(), [(true, false, 1, 1, 1)]);
    assert_eq!(registry.open.load(Ordering::Acquire), 0, "not under the limit");
    drop(noted);
    assert_eq!(registry.infos(), Vec::<FileInfo>::new());
}

/// A sink keeping the events it is told.
#[derive(Default)]
struct Told(Mutex<Vec<Event>>);

impl Diagnostics for Told {
    fn event(&self, event: &Event) {
        self.0.lock().unwrap().push(event.clone());
    }
}

/// Verifies: [DESIGN] RULES.md D56: more files opened again in a minute
/// than FILES.MAX_OPEN is told once, as a warning naming the count and the
/// limit; told again only a minute after; and a file nothing needs that
/// cannot be removed is told as a failure.
#[test]
fn reopens_past_the_limit_and_files_not_discarded_are_told() {
    let (fs, registry) = manager(1);
    let told = Arc::new(Told::default());
    let sink: Arc<dyn Diagnostics> = told.clone();
    registry.telling(sink);
    let files: Vec<Box<dyn DiskFile>> = ["/a", "/b"].iter().map(|p| fs.open(Path::new(p), OpenMode::CreateNew).unwrap()).collect();
    for _ in 0..3_u32 {
        for file in &files {
            file.write_all_at(b"x", 0).unwrap();
        }
    }
    let warned = |t: &Told| t.0.lock().unwrap().iter().filter(|e| matches!(e, Event::FilesReopened { .. })).count();
    assert_eq!(warned(&told), 1, "once in the minute");
    assert!(matches!(told.0.lock().unwrap().first(), Some(Event::FilesReopened { reopened: 2, limit: 1 })));
    let later = std::time::Instant::now().checked_add(Duration::from_secs(61)).unwrap();
    registry.reopened(later);
    registry.reopened(later);
    assert_eq!(warned(&told), 2, "again a minute after");
    fs.discard(Path::new("/missing"));
    let last = told.0.lock().unwrap().last().cloned();
    let Some(Event::Failed(failed)) = last else { panic!("{last:?}") };
    assert_eq!(failed.code, Code::Io);
}

/// What `Syncing` files share: each sync done, by path and durability, and
/// the path whose syncs fail.
#[derive(Default)]
struct Syncs {
    done: Mutex<Vec<(PathBuf, Durability)>>,
    failing: Mutex<Option<PathBuf>>,
}

/// A file system in memory noting each sync of its files, those of one
/// path failing while it is told so.
struct Syncing {
    inner: MemoryFileSystem,
    syncs: Arc<Syncs>,
}

impl FileSystem for Syncing {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let inner = self.inner.open(path, mode)?;
        Ok(Box::new(SyncingFile { inner, path: path.to_path_buf(), syncs: Arc::clone(&self.syncs) }))
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

/// A file of a `Syncing` file system.
struct SyncingFile {
    inner: Box<dyn DiskFile>,
    path: PathBuf,
    syncs: Arc<Syncs>,
}

impl DiskFile for SyncingFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.inner.read_at(buf, offset)
    }
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        self.inner.write_all_at(data, offset)
    }
    fn size(&self) -> Result<u64> {
        self.inner.size()
    }
    fn set_size(&self, size: u64) -> Result<()> {
        self.inner.set_size(size)
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        let failing = self.syncs.failing.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if failing.as_ref() == Some(&self.path) {
            return Err(SqlError::new(Code::Io, format!("lost: {}", self.path.display())));
        }
        self.syncs.done.lock().unwrap_or_else(PoisonError::into_inner).push((self.path.clone(), durability));
        self.inner.sync(durability)
    }
}

/// A manager of a `Syncing` file system, of at most `limit` open files,
/// telling a `Told`.
fn syncing(limit: usize) -> (FileManager, Arc<Registry>, Arc<Syncs>, Arc<Told>) {
    let registry = Arc::new(Registry::default());
    registry.set_limit(limit);
    let told = Arc::new(Told::default());
    let sink: Arc<dyn Diagnostics> = told.clone();
    registry.telling(sink);
    let syncs = Arc::new(Syncs::default());
    let fs = Syncing { inner: MemoryFileSystem::new(), syncs: Arc::clone(&syncs) };
    (FileManager::over(Arc::new(fs), Arc::clone(&registry)), registry, syncs, told)
}

/// Whether the file at `path` is open now.
fn is_open(registry: &Registry, path: &str) -> bool {
    registry.infos().iter().any(|i| i.path == Path::new(path) && i.open)
}

/// Verifies: [DESIGN] RULES.md D56: a file with writes not yet synced is
/// synced (`Durability::Data`) before it is closed for another, so no
/// writeback error is lost with its descriptor; one synced since is closed
/// with no sync, and a sync of no durability leaves it to be synced.
/// Regression test (R-TEST-7): a file was closed with its writes unsynced.
#[test]
fn a_file_with_writes_not_synced_is_synced_before_it_is_closed() {
    let (fs, registry, syncs, _) = syncing(1);
    let a = fs.open(Path::new("/a"), OpenMode::CreateNew).unwrap();
    a.write_all_at(b"data", 0).unwrap();
    a.sync(Durability::None).unwrap();
    let b = fs.open(Path::new("/b"), OpenMode::CreateNew).unwrap();
    assert!(!is_open(&registry, "/a"));
    assert_eq!(*syncs.done.lock().unwrap(), [(PathBuf::from("/a"), Durability::None), (PathBuf::from("/a"), Durability::Data)]);
    b.set_size(8_u64).unwrap();
    b.sync(Durability::Full).unwrap();
    syncs.done.lock().unwrap().clear();
    a.write_all_at(b"more", 0).unwrap();
    assert!(!is_open(&registry, "/b"), "/b closed for /a");
    assert_eq!(*syncs.done.lock().unwrap(), [], "/b synced already");
    let shown = registry.infos().into_iter().find(|i| i.path == Path::new("/a")).unwrap();
    assert_eq!(shown.syncs, 2, "the sync before closing counted");
}

/// Verifies: [DESIGN] RULES.md D56: past the limit, a file with no writes
/// not yet synced is closed before one with, though that one was used
/// less recently. Regression test (R-TEST-7): the one used least recently
/// was closed, its writes unsynced.
#[test]
fn files_synced_are_closed_before_files_not() {
    let (fs, registry, syncs, _) = syncing(2);
    let a = fs.open(Path::new("/a"), OpenMode::CreateNew).unwrap();
    a.write_all_at(b"data", 0).unwrap();
    let b = fs.open(Path::new("/b"), OpenMode::CreateNew).unwrap();
    b.write_all_at(b"data", 0).unwrap();
    b.sync(Durability::Data).unwrap();
    let c = fs.open(Path::new("/c"), OpenMode::CreateNew).unwrap();
    assert!(is_open(&registry, "/a") && !is_open(&registry, "/b"), "/b closed, not /a");
    assert_eq!(*syncs.done.lock().unwrap(), [(PathBuf::from("/b"), Durability::Data)], "/a not synced");
    drop((a, b, c));
}

/// Verifies: [DESIGN] RULES.md D56: a file whose sync fails as it is to be
/// closed stays open, another closed in its place; the failure is told,
/// once, and returned by the file's next sync of some durability, though a
/// sync then would succeed, so no sync stands for the writes lost; the
/// one after succeeds. Regression test (R-TEST-7): a file was closed with
/// its writes unsynced, and a sync of it opened again succeeded.
#[test]
fn a_failed_sync_before_closing_keeps_the_file_open_and_fails_its_next_sync() {
    let (fs, registry, syncs, told) = syncing(2);
    let a = fs.open(Path::new("/a"), OpenMode::CreateNew).unwrap();
    a.write_all_at(b"data", 0).unwrap();
    let b = fs.open(Path::new("/b"), OpenMode::CreateNew).unwrap();
    b.write_all_at(b"data", 0).unwrap();
    *syncs.failing.lock().unwrap() = Some(PathBuf::from("/a"));
    let c = fs.open(Path::new("/c"), OpenMode::CreateNew).unwrap();
    assert!(is_open(&registry, "/a") && !is_open(&registry, "/b"), "/a kept open, /b synced and closed");
    let failures = |t: &Told| t.0.lock().unwrap().iter().filter(|e| matches!(e, Event::Failed(_))).count();
    assert_eq!(failures(&told), 1);
    // /b opened again: /a's sync tried again and failing again, not told again.
    b.write_all_at(b"data", 0).unwrap();
    assert!(is_open(&registry, "/a") && !is_open(&registry, "/c"));
    assert_eq!(failures(&told), 1);
    *syncs.failing.lock().unwrap() = None;
    a.sync(Durability::None).unwrap();
    let lost = a.sync(Durability::Full).unwrap_err();
    assert_eq!((lost.code, lost.msg.as_str()), (Code::Io, "lost: /a"));
    a.sync(Durability::Full).unwrap();
    drop((a, b, c));
}

/// Verifies: [DESIGN] RULES.md D56: what the manager does not open it
/// passes to the file system it wraps: directories made, listed and
/// synced, files looked for, locked and removed; and a file opened again
/// after it was closed for another reads back what was written in it.
#[test]
fn the_manager_passes_the_rest_to_its_file_system() {
    let (fs, registry, _, _) = syncing(1);
    fs.create_dir_all(Path::new("/d")).unwrap();
    let a = fs.open(Path::new("/d/a"), OpenMode::CreateNew).unwrap();
    a.write_all_at(b"data", 0).unwrap();
    let lock = fs.lock(Path::new("/d/lock")).unwrap();
    assert!(fs.exists(Path::new("/d/a")).unwrap());
    assert_eq!(fs.read_dir(Path::new("/d")).unwrap().len(), 2);
    fs.sync_dir(Path::new("/d")).unwrap();
    let b = fs.open(Path::new("/d/b"), OpenMode::CreateNew).unwrap();
    assert!(!is_open(&registry, "/d/a"));
    let mut back = [0_u8; 4];
    a.read_exact_at(&mut back, 0).unwrap();
    assert_eq!((&back, a.size().unwrap()), (b"data", 4_u64));
    drop((a, b, lock));
    fs.remove(Path::new("/d/a")).unwrap();
    assert!(!fs.exists(Path::new("/d/a")).unwrap());
}

/// Verifies: [DESIGN] RULES.md D56: closing the database syncs a file with
/// writes not yet synced before it closes it; if that sync fails, the
/// failure is told and the file closed all the same.
#[test]
fn closing_syncs_files_with_writes_not_synced() {
    let (fs, registry, syncs, told) = syncing(0);
    let a = fs.open(Path::new("/a"), OpenMode::CreateNew).unwrap();
    a.write_all_at(b"data", 0).unwrap();
    let b = fs.open(Path::new("/b"), OpenMode::CreateNew).unwrap();
    b.write_all_at(b"data", 0).unwrap();
    let c = fs.open(Path::new("/c"), OpenMode::CreateNew).unwrap();
    *syncs.failing.lock().unwrap() = Some(PathBuf::from("/b"));
    assert_eq!(registry.close_all(), Vec::<PathBuf>::new());
    assert_eq!(registry.open.load(Ordering::Acquire), 0, "all closed");
    assert_eq!(*syncs.done.lock().unwrap(), [(PathBuf::from("/a"), Durability::Data)], "/c had nothing to sync");
    let last = told.0.lock().unwrap().last().cloned();
    let Some(Event::Failed(failed)) = last else { panic!("{last:?}") };
    assert_eq!(failed.msg, "lost: /b");
    drop((a, b, c));
}
