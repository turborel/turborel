//! Unit tests of the built-in catalog store: records read back as written,
//! the last whole one stands, the file is compacted into the other and a
//! crash during the switch leaves one standing (RULES.md R-TEST-1,
//! R-TEST-2). Split from catalog_file.rs (RULES.md R-SIZE-4).

use std::path::Path;

use super::*;
use crate::disk::memory::MemoryFileSystem;
use crate::hash::crc::{CRC32_ISCSI, Crc};

fn hash() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

/// Files checksummed with `hash`.
fn formats() -> FileFormats {
    FileFormats::of(hash())
}

fn disk() -> Arc<dyn FileSystem> {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    fs.create_dir_all(Path::new("/db")).unwrap();
    fs
}

const DIR: &str = "/db";

/// The store of `CatalogFiles` in `dir` of `fs`, if one is there.
fn opened(fs: &Arc<dyn FileSystem>, dir: &str) -> Option<Box<dyn CatalogStore>> {
    CatalogFiles.open(&StorePlace::new(fs, Path::new(dir), &formats())).unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, the built-in catalog store: none is
/// where none was made, and one cannot be made where one is; a new one
/// has no record; each record stands until the next; opened again, it
/// gives its last.
#[test]
fn records_read_back_as_written() {
    let fs = disk();
    assert!(opened(&fs, DIR).is_none(), "none made yet");
    let store = CatalogFiles.create(&StorePlace::new(&fs, Path::new(DIR), &formats())).unwrap();
    assert!(CatalogFiles.create(&StorePlace::new(&fs, Path::new(DIR), &formats())).is_err(), "one is there");
    assert!(store.last().is_none());
    store.record(b"first", COMPACT_AFTER).unwrap();
    assert_eq!(store.last().as_deref(), Some(&b"first"[..]));
    store.record(b"second", COMPACT_AFTER).unwrap();
    drop(store);
    assert_eq!(opened(&fs, DIR).unwrap().last().as_deref(), Some(&b"second"[..]));
    assert_eq!(CatalogFiles.name(), "files");
}

/// Verifies: [DESIGN] RULES.md D25, compaction: after `COMPACT_AFTER`
/// records the next is written alone to the other file and the first
/// removed, so the catalog never grows past that; opened again it gives the
/// last record. A crash during the switch: with the new file made but
/// holding no record, the old stands; with both holding records, the later
/// stands; the other is removed.
#[test]
fn the_catalog_is_compacted_into_the_other_file() {
    let fs = disk();
    let dir = Path::new(DIR);
    let store = CatalogFile::create(Arc::clone(&fs), dir.to_path_buf(), (hash(), Pipeline::none())).unwrap();
    for n in 0..=COMPACT_AFTER {
        store.record(&n.to_le_bytes(), COMPACT_AFTER).unwrap();
    }
    assert!(fs.exists(&dir.join(name(1))).unwrap() && !fs.exists(&dir.join(name(0))).unwrap());
    let last = COMPACT_AFTER.to_le_bytes().to_vec();
    assert_eq!(store.last(), Some(last.clone()));
    drop(store);
    assert_eq!(opened(&fs, DIR).unwrap().last(), Some(last.clone()));
    // The switch began: the other file made, no record in it yet.
    drop(BlockFile::create(&*fs, &dir.join(name(0)), hash(), Pipeline::none(), BlockOptions::default()).unwrap());
    assert_eq!(opened(&fs, DIR).unwrap().last(), Some(last));
    assert!(!fs.exists(&dir.join(name(0))).unwrap(), "the empty file is removed");
    // The switch wrote the new file, the old not yet removed.
    let newer = BlockFile::create(&*fs, &dir.join(name(0)), hash(), Pipeline::none(), BlockOptions::default()).unwrap();
    newer.append(&framed(u64::try_from(COMPACT_AFTER).unwrap() + 1, b"newer")).unwrap();
    drop(newer);
    assert_eq!(opened(&fs, DIR).unwrap().last().as_deref(), Some(&b"newer"[..]));
    assert!(!fs.exists(&dir.join(name(1))).unwrap(), "the older file is removed");
}

/// Verifies: [DESIGN] RULES.md D25, R-ERR-1: a record too short to hold its
/// sequence number is SQLCODE -916.
#[test]
fn a_record_without_its_sequence_is_corrupt() {
    assert_eq!(unframed(&[1, 2]).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(unframed(&framed(7, b"x")).unwrap(), (7, b"x".to_vec()));
}

/// What a call back got: the last record, and the outcome of the record
/// it asked for.
type CalledBack = (Option<Vec<u8>>, Option<Code>);

/// A file system in memory that, as it syncs a directory, asks the
/// catalog store it watches for its last record and to write another: as
/// a plug-in that called back would.
#[derive(Default)]
struct CallingBack {
    files: MemoryFileSystem,
    store: std::sync::OnceLock<std::sync::Weak<CatalogFile>>,
    /// What each call back got: the last record, and the outcome of the
    /// record asked for.
    got: Mutex<Vec<CalledBack>>,
}

impl FileSystem for CallingBack {
    fn open(&self, path: &Path, mode: crate::disk::io::OpenMode) -> Result<Box<dyn crate::disk::io::DiskFile>> {
        self.files.open(path, mode)
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.files.remove(path)
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        self.files.exists(path)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.files.create_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.files.read_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        if let Some(store) = self.store.get().and_then(std::sync::Weak::upgrade) {
            let again = store.record(b"again", 0).err().map(|e| e.code);
            lock_builtin(&self.got).push((store.last(), again));
        }
        self.files.sync_dir(path)
    }
    fn lock(&self, path: &Path) -> Result<crate::disk::io::FileLock> {
        self.files.lock(path)
    }
}

/// Verifies: [DESIGN] RULES.md R-ERR-4, D25: a record is written with no
/// lock held over the file system, a plug-in: called back as it syncs the
/// directory, the store gives its last record, the one before, and
/// refuses another record (SQLCODE -910) rather than wait for itself.
/// Regression test (R-TEST-7): the store's lock was held across the file
/// system's calls, and the call back waited for it for ever.
#[test]
fn a_record_is_written_with_no_lock_held() {
    let fs = Arc::new(CallingBack::default());
    fs.create_dir_all(Path::new(DIR)).unwrap();
    let files: Arc<dyn FileSystem> = fs.clone();
    let store = Arc::new(CatalogFile::create(files, Path::new(DIR).to_path_buf(), (hash(), Pipeline::none())).unwrap());
    store.record(b"first", COMPACT_AFTER).unwrap();
    fs.store.set(Arc::downgrade(&store)).unwrap();
    // Compacted at once: the directory synced as the file is made and
    // after, and as the other is removed.
    store.record(b"second", 0).unwrap();
    let got = lock_builtin(&fs.got).clone();
    assert!(!got.is_empty(), "called back");
    assert!(got.iter().all(|g| *g == (Some(b"first".to_vec()), Some(Code::Api))), "{got:?}");
    assert_eq!(store.last().as_deref(), Some(&b"second"[..]));
}
