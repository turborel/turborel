//! Unit tests of the file manager (RULES.md R-TEST-1): the limit of open
//! files kept by closing the one used least recently, a file opened again
//! as it is next used, a file waiting while every open one is in use, and
//! the counts shown. Split from files.rs (RULES.md R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use super::*;
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
