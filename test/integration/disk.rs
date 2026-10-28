//! The storage foundation's file I/O (RULES.md D30): the `FileSystem` and
//! `DiskFile` contracts, checked by the same tests for `OsFileSystem`, on
//! the real file system, and for `MemoryFileSystem`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use turborel::disk::io::{DiskFile, Durability, FileSystem, OpenMode, OsFileSystem};
use turborel::disk::measured::{IoCounts, MeasuredFileSystem};
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::Code;

use crate::common::Scratch;

fn os() -> Box<dyn FileSystem> {
    Box::new(OsFileSystem)
}

/// Each built-in file system, with `scratch`'s directory in it.
fn file_systems(scratch: &Scratch) -> [Box<dyn FileSystem>; 2] {
    let memory = MemoryFileSystem::new();
    memory.create_dir_all(&scratch.dir).unwrap();
    [os(), Box::new(memory)]
}

fn code<T>(r: turborel::error::Result<T>) -> Option<Code> {
    r.err().map(|e| e.code)
}

/// Verifies: [DESIGN] RULES.md D30, `DiskFile`: bytes written at any offset
/// read back at that offset; writing past the end extends the file, and the
/// bytes never written read as zero; `read_at` stops at the end of the file
/// and says how much it read; what was written survives closing and
/// reopening.
#[test]
fn files_are_read_and_written_at_offsets() {
    let scratch = Scratch::new("offsets");
    for fs in file_systems(&scratch) {
        let path = scratch.path("f");
        let file = fs.open(&path, OpenMode::CreateNew).unwrap();
        assert_eq!(file.size().unwrap(), 0);
        file.write_all_at(b"head", 0).unwrap();
        file.write_all_at(b"tail", 100).unwrap();
        assert_eq!(file.size().unwrap(), 104);
        let mut gap = [9_u8; 6];
        file.read_exact_at(&mut gap, 98).unwrap();
        assert_eq!(&gap, b"\0\0tail");
        let mut past = [0_u8; 10];
        assert_eq!(file.read_at(&mut past, 100).unwrap(), 4);
        assert_eq!(&past[..4], b"tail");
        assert_eq!(file.read_at(&mut past, 104).unwrap(), 0, "nothing at the end");
        assert_eq!(file.read_at(&mut [], 500).unwrap(), 0, "an empty read anywhere");
        drop(file);
        let again = fs.open(&path, OpenMode::ReadOnly).unwrap();
        let mut head = [0_u8; 4];
        again.read_exact_at(&mut head, 0).unwrap();
        assert_eq!(&head, b"head");
    }
}

/// Verifies: [DESIGN] RULES.md D30, `FileSystem::open`: `CreateNew` refuses
/// an existing file, `ReadWrite` and `ReadOnly` a missing one, and a file
/// opened read-only refuses writes, each with SQLCODE -915; `exists` and
/// `remove` agree, and removing a missing file fails.
#[test]
fn open_modes_are_honoured() {
    let scratch = Scratch::new("modes");
    for fs in file_systems(&scratch) {
        let (path, missing) = (scratch.path("f"), scratch.path("missing"));
        drop(fs.open(&path, OpenMode::CreateNew).unwrap());
        assert_eq!(code(fs.open(&path, OpenMode::CreateNew)), Some(Code::Io));
        assert_eq!(code(fs.open(&missing, OpenMode::ReadWrite)), Some(Code::Io));
        assert_eq!(code(fs.open(&missing, OpenMode::ReadOnly)), Some(Code::Io));
        let read_only = fs.open(&path, OpenMode::ReadOnly).unwrap();
        assert_eq!(code(read_only.write_all_at(b"x", 0)), Some(Code::Io));
        assert_eq!(code(read_only.set_size(10)), Some(Code::Io));
        assert_eq!((fs.exists(&path).unwrap(), fs.exists(&missing).unwrap()), (true, false));
        fs.remove(&path).unwrap();
        assert!(!fs.exists(&path).unwrap());
        assert_eq!(code(fs.remove(&path)), Some(Code::Io));
    }
}

/// Verifies: [DESIGN] RULES.md D30, `FileSystem::lock`: a lock makes its
/// file if it is not there, and no other may take it, in this process
/// either, until it is dropped; then one may. The same for the operating
/// system's file system and the one in memory.
#[test]
fn locks_are_held_until_dropped() {
    let scratch = Scratch::new("disk-locks");
    for fs in file_systems(&scratch) {
        let path = scratch.dir.join("lock");
        let held = fs.lock(&path).unwrap();
        assert!(fs.exists(&path).unwrap(), "made");
        assert_eq!(code(fs.lock(&path)), Some(Code::Io), "held");
        drop(held);
        let again = fs.lock(&path).unwrap();
        assert_eq!(code(fs.lock(&path)), Some(Code::Io));
        drop(again);
        assert!(format!("{:?}", fs.lock(&path).unwrap()).contains("FileLock"));
    }
}

/// Verifies: [DESIGN] RULES.md D30, `DiskFile`: `set_size` truncates and
/// extends with zeros; `read_exact_at` past the end fails with SQLCODE -915
/// and says where the file ends; an offset whose range passes 2^64 bytes
/// is refused before anything is read or written, never a panic.
#[test]
fn sizes_ends_and_offset_limits() {
    let scratch = Scratch::new("sizes");
    for fs in file_systems(&scratch) {
        let file = fs.open(&scratch.path("f"), OpenMode::CreateNew).unwrap();
        file.write_all_at(b"abcdef", 0).unwrap();
        file.set_size(3).unwrap();
        file.set_size(5).unwrap();
        let mut all = [9_u8; 5];
        file.read_exact_at(&mut all, 0).unwrap();
        assert_eq!(&all, b"abc\0\0");
        let short = file.read_exact_at(&mut [0_u8; 4], 3).unwrap_err();
        assert_eq!(short.code, Code::Io);
        assert!(short.msg.contains("ends 2 bytes after offset 3"), "{short}");
        assert_eq!(code(file.write_all_at(b"abcd", u64::MAX - 1)), Some(Code::Io));
        assert_eq!(code(file.read_at(&mut [0_u8; 4], u64::MAX - 1)), Some(Code::Io));
        assert_eq!(file.size().unwrap(), 5, "the refused write changed nothing");
    }
}

/// Verifies: [DESIGN] RULES.md D30: every `Durability` level syncs a file;
/// `create_dir_all` makes nested directories, `read_dir` lists entries
/// sorted, and `sync_dir` syncs a directory and fails for a missing one.
#[test]
fn files_and_directories_sync() {
    let scratch = Scratch::new("sync");
    for fs in file_systems(&scratch) {
        let nested = scratch.path("a").join("b");
        fs.create_dir_all(&nested).unwrap();
        for name in ["2", "1", "3"] {
            let file = fs.open(&nested.join(name), OpenMode::CreateNew).unwrap();
            file.write_all_at(name.as_bytes(), 0).unwrap();
            for durability in [Durability::None, Durability::Data, Durability::Full] {
                file.sync(durability).unwrap();
            }
        }
        fs.sync_dir(&nested).unwrap();
        let names: Vec<PathBuf> = fs.read_dir(&nested).unwrap();
        assert_eq!(names, ["1", "2", "3"].map(|n| nested.join(n)));
        assert_eq!(code(fs.sync_dir(&scratch.path("missing"))), Some(Code::Io));
        assert_eq!(code(fs.read_dir(&scratch.path("missing"))), Some(Code::Io));
        assert_eq!(code(fs.create_dir_all(&nested.join("1").join("c"))), Some(Code::Io), "a file is in the way");
    }
}

/// Threads writing at once to one file.
const WRITERS: usize = 8;
/// Bytes each writer owns, and rounds of writes it makes.
const REGION: usize = 4096;
const ROUNDS: u8 = 20;

/// Verifies: [DESIGN] RULES.md D30, `DiskFile`: reads and writes name their
/// offset, so threads writing disjoint ranges of one file at once never
/// disturb each other (no shared cursor).
#[test]
fn threads_write_disjoint_ranges_at_once() {
    let scratch = Scratch::new("threads");
    for fs in file_systems(&scratch) {
        let file: Arc<dyn DiskFile> = Arc::from(fs.open(&scratch.path("f"), OpenMode::CreateNew).unwrap());
        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let file = Arc::clone(&file);
                thread::spawn(move || {
                    let offset = u64::try_from(w * REGION).unwrap();
                    for round in 0..ROUNDS {
                        file.write_all_at(&[round; REGION], offset).unwrap();
                        let mut back = [0_u8; REGION];
                        file.read_exact_at(&mut back, offset).unwrap();
                        assert!(back.iter().all(|&b| b == round), "writer {w} round {round}");
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let mut all = vec![0_u8; WRITERS * REGION];
        file.read_exact_at(&mut all, 0).unwrap();
        assert!(all.iter().all(|&b| b == ROUNDS - 1));
    }
}

/// Verifies: [DESIGN] RULES.md D30: a measured file system counts, for the
/// heuristics above it, the reads and writes of its files and their bytes,
/// the syncs that reach the device (not `Durability::None`), and the
/// operations that failed; `since` gives the activity between two snapshots.
#[test]
fn a_measured_file_system_counts_its_io() {
    let scratch = Scratch::new("measured");
    let fs = MeasuredFileSystem::new(os());
    let stats = fs.stats();
    let file = fs.open(&scratch.path("f"), OpenMode::CreateNew).unwrap();
    file.write_all_at(b"hello", 0).unwrap();
    file.write_all_at(b"world", 5).unwrap();
    let before = stats.snapshot();
    let mut back = [0_u8; 20];
    assert_eq!(file.read_at(&mut back, 2).unwrap(), 8);
    file.read_exact_at(&mut back[..4], 0).unwrap();
    for durability in [Durability::None, Durability::Data, Durability::Full] {
        file.sync(durability).unwrap();
    }
    fs.sync_dir(&scratch.dir).unwrap();
    assert!(fs.open(&scratch.path("missing"), OpenMode::ReadOnly).is_err());
    assert!(file.read_exact_at(&mut back, 0).is_err(), "the file is shorter than 20 bytes");
    let now = stats.snapshot();
    assert_eq!((before.writes, before.bytes_written, before.reads), (2, 10, 0));
    let since = now.since(&before);
    assert_eq!(
        IoCounts { sync_time: since.sync_time, ..since },
        IoCounts { reads: 3, bytes_read: 22, writes: 0, bytes_written: 0, syncs: 3, sync_time: since.sync_time, failures: 1 }
    );
    assert_eq!(before.since(&now), IoCounts::default(), "an earlier snapshot never goes negative");
    file.set_size(0).unwrap();
    assert_eq!(file.size().unwrap(), 0);
    assert!(fs.exists(&scratch.path("f")).unwrap());
    fs.create_dir_all(&scratch.path("d")).unwrap();
    assert_eq!(fs.read_dir(&scratch.dir).unwrap().len(), 2);
    fs.remove(&scratch.path("f")).unwrap();
    assert_eq!(stats.snapshot().failures, now.failures, "only failures count as failures");
}

/// Fails to compile unless the file system can be shared between threads.
fn _shareable(fs: &dyn FileSystem, path: &Path) -> impl Send + Sync {
    (fs.exists(path).is_ok(), Arc::new(OsFileSystem))
}
