//! Unit tests of read-ahead (`pool/ahead.rs`): an undo reads nothing
//! ahead; a scan's requests bounded by the window, and passed over once it
//! ends.

use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};

/// A pager file in memory of `pages` pages written, flushed; and a pool of
/// `frames` reading it, none of them in it.
fn cold(count: u8, frames: usize) -> (BufferPool, FileId, Vec<u64>) {
    let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Arc::new(Pager::create(&MemoryFileSystem::new(), Path::new("/p"), hash, Pipeline::none(), 4096).unwrap());
    let writer = BufferPool::new(1, Box::new(Clock::new(1))).unwrap();
    let written = writer.add_pager(Arc::clone(&pager));
    let ids = (0..count)
        .map(|n| {
            let (id, mut page) = writer.new_page(written).unwrap();
            page.push(n);
            id.0
        })
        .collect();
    writer.flush().unwrap();
    let pool = BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap();
    let file = pool.add_pager(pager);
    (pool, file, ids)
}

/// Verifies: [DESIGN] RULES.md D30, D38: while its thread undoes work, a
/// pool reads nothing ahead, nor asks for anything to be; then it does.
#[test]
fn an_undo_reads_nothing_ahead() {
    let (pool, file, ids) = cold(1, 8);
    let id = ids[0];
    let undoing = Undoing::begin();
    assert_eq!(pool.read_ahead(file, id), 0);
    assert_eq!(pool.ahead_pages(&mut Ahead::default(), file, &[id]), 1, "taken, none read");
    drop(undoing);
    assert_eq!(pool.counts().read_ahead, 0);
    assert_eq!(pool.read_ahead(file, id), 1);
}

/// Verifies: [DESIGN] RULES.md D30, D55: in a pool a reader serves, pages
/// asked for are queued, not read, at most its window (here 4 pages, of 32
/// frames) queued at once, pages in the pool passed over; the reader's job
/// reads them, each run of pages one after another in one read, and they
/// are hits then; a scan that asked, ended, has what it asked passed over
/// and leaves room for others.
#[test]
fn pages_asked_for_are_queued_for_the_reader() {
    let (pool, file, ids) = cold(12, 32);
    let pool = Arc::new(pool);
    let reader = AheadReader::new();
    reader.serve(&pool);
    pool.set_read_ahead(4);
    drop(pool.read(file, ids[1]).unwrap());
    let mut ahead = Ahead::default();
    assert_eq!(pool.ahead_pages(&mut ahead, file, &[ids[0], ids[1], ids[2], ids[5], ids[6], ids[7]]), 5, "4 pages, 1 in the pool");
    assert_eq!(pool.counts().read_ahead, 0, "queued, not read");
    assert_eq!(pool.ahead_pages(&mut ahead, file, &[ids[7]]), 0, "no room left");
    assert_eq!(reader.read_queued(), 4);
    assert_eq!(pool.counts().read_ahead, 4);
    let misses = pool.counts().misses;
    for n in [0, 2, 5, 6] {
        assert_eq!(pool.read(file, ids[n]).unwrap().to_vec(), vec![u8::try_from(n).unwrap()]);
    }
    assert_eq!(pool.counts().misses, misses, "hits");
    let mut ended = Ahead::default();
    assert_eq!(pool.ahead_pages(&mut ended, file, &ids[8..12]), 4);
    drop(ended);
    assert_eq!(pool.ahead_pages(&mut ahead, file, &ids[8..12]), 4, "room again");
    drop(ahead);
    assert_eq!(reader.read_queued(), 0, "passed over");
    assert_eq!(pool.counts().read_ahead, 4);
}

/// Verifies: [DESIGN] RULES.md D30: a scan reading pages in order asks for
/// the window after the page it reads, then again only once fewer than
/// half a window are left ahead of it; a pool no reader serves reads them
/// at once, in the scan's thread; and none once read-ahead is set to 0.
#[test]
fn a_scan_in_order_asks_a_window_ahead() {
    let (pool, file, ids) = cold(16, 64);
    let mut ahead = Ahead::default();
    pool.ahead_of(&mut ahead, file, ids[0]);
    assert_eq!((pool.counts().read_ahead, ahead.until), (8, ids[9]), "an eighth of 64 frames, after the first");
    pool.ahead_of(&mut ahead, file, ids[1]);
    assert_eq!(pool.counts().read_ahead, 8, "more than half a window left");
    pool.ahead_of(&mut ahead, file, ids[5]);
    assert_eq!((pool.counts().read_ahead, ahead.until), (13, ids[14]), "the rest of the window");
    pool.set_read_ahead(0);
    pool.ahead_of(&mut ahead, file, ids[13]);
    assert_eq!(pool.counts().read_ahead, 13);
}
