//! Unit tests of the log of an instance's older versions: records in pages
//! of its file, read by position and from one, freed as truncated
//! (RULES.md D49, R-TEST-1).

use std::path::Path;

use super::*;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::hash::crc::{CRC32_ISCSI, Crc};

/// A log in a new pager file in memory, of pages of 4 KiB, in a pool of
/// `frames`.
fn log(frames: usize) -> PagedLog {
    let hash = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Pager::create(&MemoryFileSystem::new(), Path::new("undo"), hash, Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    PagedLog::new(pool, file)
}

/// Verifies: [DESIGN] RULES.md D49: records appended to the log are read
/// back by position, and from a position on, a record longer than a page
/// holds too; a position never given holds none; the log is not durable,
/// syncs through what it gave, and is truncated a page at a time, the
/// pages freed in its file.
#[test]
fn records_go_to_pages_and_are_freed_as_truncated() {
    let undo = log(8);
    assert!(!undo.capabilities().durable);
    let long = vec![9_u8; 10_000];
    let at: Vec<Lsn> = [b"one".to_vec(), long.clone(), b"three".to_vec()].iter().map(|r| undo.append(r).unwrap()).collect();
    assert_eq!(at, [Lsn(1), Lsn(2), Lsn(3)]);
    assert_eq!(undo.end(), Lsn(4));
    assert_eq!(undo.read_at(Lsn(2)).unwrap(), Some(long));
    assert_eq!(undo.read_at(Lsn(9)).unwrap(), None);
    let from: Vec<(Lsn, Vec<u8>)> = undo.read_from(Lsn(3)).map(Result::unwrap).collect();
    assert_eq!(from, [(Lsn(3), b"three".to_vec())]);
    undo.sync(Lsn(3)).unwrap();
    assert_eq!(undo.sync(Lsn(4)).err().map(|e| e.code), Some(Code::Api));
    let in_use = undo.pool.pages_in_use(undo.file).unwrap();
    undo.truncate_before(Lsn(3)).unwrap();
    assert!(undo.pool.pages_in_use(undo.file).unwrap() < in_use, "their pages freed");
    assert_eq!((undo.read_at(Lsn(1)).unwrap(), undo.read_at(Lsn(3)).unwrap()), (None, Some(b"three".to_vec())));
}

/// Verifies: [DESIGN] RULES.md D49: a record's page held by a reader is
/// not freed by a truncation, which fails and keeps it and those after
/// for a later one; an append for which the pool has no frame fails and
/// gives no position.
#[test]
fn what_cannot_be_freed_or_written_is_refused() {
    let undo = log(1);
    let first = undo.append(b"held").unwrap();
    let page = lock_builtin(&undo.state).pages[&first];
    let held = undo.pool.read(undo.file, page.0).unwrap();
    assert!(undo.truncate_before(Lsn(2)).is_err(), "held, so not freed");
    assert_eq!(undo.append(b"no frame").err().map(|e| e.code), Some(Code::Resources));
    drop(held);
    assert_eq!((undo.read_at(first).unwrap(), undo.end()), (Some(b"held".to_vec()), Lsn(2)));
    undo.truncate_before(Lsn(2)).unwrap();
    assert_eq!(undo.read_at(first).unwrap(), None);
}
