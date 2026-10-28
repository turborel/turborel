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
