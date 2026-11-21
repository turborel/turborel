//! Unit tests of a checkpoint's capture, copy on write (RULES.md D590):
//! the sealed batch holds each page as it was when the capture began,
//! whoever took it, and the pool reads each as written since.

use std::path::Path;
use std::sync::Arc;

use crate::disk::buffer::{BufferPool, Clock, FileId};
use crate::disk::encoding::Pipeline;
use crate::disk::io::{Durability, FileSystem};
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::{PageId, Pager};
use crate::disk::shadow::ShadowFileSystem;
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};

const PATH: &str = "/d/data";
const PAGE: u32 = 4096;
/// Pages of filler, read to evict the others.
const FILLER: u64 = 24;

fn hash() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

/// The data file on `shadows` in a pool of `frames`.
fn pool_over(shadows: &ShadowFileSystem, frames: usize, made: bool) -> (BufferPool, FileId) {
    let path = Path::new(PATH);
    let pager = if made {
        Pager::create(shadows, path, hash(), Pipeline::none(), PAGE).unwrap()
    } else {
        Pager::open(shadows, path, &[hash()], &[]).unwrap().0
    };
    let pool = BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap();
    let file = pool.add_pager(Arc::new(pager));
    (pool, file)
}

/// Page `id` of `file` holds `content` from now on.
fn put(pool: &BufferPool, file: FileId, id: u64, content: &[u8]) {
    let mut page = pool.write(file, PageId(id)).unwrap();
    page.clear();
    page.extend_from_slice(content);
}

/// What page `id` of `file` holds.
fn got(pool: &BufferPool, file: FileId, id: u64) -> Vec<u8> {
    pool.read(file, id).unwrap().to_vec()
}

/// Version `v` of page `id`: small, but for page 4's second, past its
/// page, so written with writers held.
fn version(id: u64, v: u8) -> Vec<u8> {
    let len = if id == 4 && v == 2 { 6000 } else { 100 };
    vec![u8::try_from(id).unwrap().wrapping_mul(16).wrapping_add(v); len]
}

/// A disk holding pages 0 to 4 at version 1 and `FILLER` pages after,
/// committed; and a pool of `frames` over it, pages 0 to 4 at version 2
/// and page 5, new, too, all dirty.
fn dirty(frames: usize) -> (Arc<dyn FileSystem>, Arc<ShadowFileSystem>, BufferPool, FileId) {
    let disk: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    disk.create_dir_all(Path::new("/d")).unwrap();
    let shadows = Arc::new(ShadowFileSystem::new(Arc::clone(&disk), hash()));
    let (pool, file) = pool_over(&shadows, frames, true);
    for id in 0..5 + FILLER {
        let (page, mut held) = pool.new_page(file).unwrap();
        assert_eq!(page, PageId(id));
        held.extend_from_slice(&version(id, 1));
    }
    pool.sync(file, Durability::Full).unwrap();
    shadows.commit().unwrap();
    for id in 0..5 {
        put(&pool, file, id, &version(id, 2));
    }
    let (page, mut held) = pool.new_page(file).unwrap();
    assert_eq!(page, PageId(5 + FILLER));
    held.extend_from_slice(&version(5, 2));
    drop(held);
    (disk, shadows, pool, file)
}

/// The capture a checkpoint begins with its writers held: held, owed,
/// the rest written, sealed.
fn begin(shadows: &ShadowFileSystem, pool: &BufferPool, file: FileId) -> super::Capture {
    shadows.hold().unwrap();
    let capture = pool.capture(file).unwrap();
    pool.sync(file, Durability::Full).unwrap();
    shadows.seal_capturing().unwrap();
    capture
}

/// After a crash, the pages as the capture began: each at version 2.
fn crashed_holds_the_capture(disk: &Arc<dyn FileSystem>) {
    let shadows = ShadowFileSystem::new(Arc::clone(disk), hash());
    let (pool, file) = pool_over(&shadows, 8, false);
    for id in 0..5 {
        assert_eq!(got(&pool, file, id), version(id, 2), "page {id}");
    }
    assert_eq!(got(&pool, file, 5 + FILLER), version(5, 2), "the page made before");
}

/// Verifies: [DESIGN] RULES.md D590, a capture, writers going on: a page
/// written after it began is copied first, one freed too, one past its
/// page written with writers held, its overflow on a page freed before so
/// the file is not grown by it, the new page past the file's end made room
/// for, the rest by the walk; reads see what
/// was written since, before the batch is committed and after; and after
/// a crash the file holds every page as the capture began, the page freed
/// since too, the pages written since in no batch committed.
#[test]
fn a_capture_holds_pages_as_they_were() {
    let (disk, shadows, pool, file) = dirty(64);
    pool.free(file, PageId(4 + FILLER)).unwrap();
    let capture = begin(&shadows, &pool, file);
    assert_eq!(capture.pages.len(), 5, "all but the page past its page owed");
    put(&pool, file, 0, &version(0, 3));
    put(&pool, file, 5 + FILLER, &version(5, 3));
    pool.free(file, PageId(1)).unwrap();
    pool.capture_rest(capture).unwrap();
    assert!(pool.owed.copies.lock().unwrap().is_empty(), "every copy written");
    shadows.commit_sealed().unwrap();
    assert_eq!(got(&pool, file, 0), version(0, 3));
    assert_eq!(got(&pool, file, 2), version(2, 2));
    assert_eq!(got(&pool, file, 5 + FILLER), version(5, 3));
    drop((pool, shadows));
    crashed_holds_the_capture(&disk);
}

/// Verifies: [DESIGN] RULES.md D590, pages owed evicted before the walk
/// reaches them are captured as they go, kept mapped meanwhile: read
/// back, from the sealed batch, as they were; written after, read as
/// written; after a crash, as the capture began.
#[test]
fn evicted_pages_are_captured_as_they_go() {
    let (disk, shadows, pool, file) = dirty(8);
    let capture = begin(&shadows, &pool, file);
    assert_ne!(capture.pages.len(), 0);
    for id in 5..5 + FILLER {
        assert_eq!(got(&pool, file, id), version(id, 1));
    }
    assert_eq!(got(&pool, file, 2), version(2, 2), "read back from the sealed batch");
    put(&pool, file, 3, &version(3, 3));
    pool.capture_rest(capture).unwrap();
    assert_eq!(pool.owed.in_flight.load(std::sync::atomic::Ordering::Acquire), 0);
    shadows.commit_sealed().unwrap();
    assert_eq!(got(&pool, file, 3), version(3, 3));
    assert_eq!(got(&pool, file, 0), version(0, 2));
    drop((pool, shadows));
    crashed_holds_the_capture(&disk);
}

/// Verifies: [DESIGN] RULES.md D590, a capture given up before its batch
/// is sealed leaves every page owed dirty again, so written back as any
/// other, and nothing owed.
#[test]
fn a_capture_given_up_leaves_pages_dirty() {
    let (_disk, shadows, pool, file) = dirty(64);
    shadows.hold().unwrap();
    let capture = pool.capture(file).unwrap();
    let owed: Vec<usize> = capture.pages.iter().map(|(index, _)| *index).collect();
    pool.abandon(capture);
    shadows.release().unwrap();
    for index in owed {
        let frame = &pool.frames[index];
        assert!(frame.dirty.load(std::sync::atomic::Ordering::Acquire), "frame {index} dirty again");
        assert!(!frame.owed.load(std::sync::atomic::Ordering::Acquire), "frame {index} owed no more");
    }
    shadows.commit().unwrap();
    pool.sync(file, Durability::Full).unwrap();
    shadows.commit().unwrap();
    assert_eq!(got(&pool, file, 2), version(2, 2));
}
