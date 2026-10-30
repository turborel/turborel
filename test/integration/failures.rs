//! The storage foundation's failure paths (RULES.md D30), with a file
//! system that injects faults: what the pager, block files, the log and the
//! buffer pool do when an open, read, write, sync or removal fails, and
//! that each is usable once the fault has passed.

use std::path::Path;
use std::sync::Arc;

use turborel::disk::block::{BlockFile, BlockOptions};
use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, FileSystem};
use turborel::disk::pager::{Extent, PageId, Pager};
use turborel::error::Code;
use turborel::log::{FileLog, FileLogOptions, Log, Lsn};

use crate::common::{Kept, Scratch, crc32c, small_segments};
use crate::faults::{FaultyFs, Op, Rule};

/// 4 KiB pages; content past this overflows.
const PAGE: u32 = 4096;
const INLINE: usize = 4084 - 24;

fn pager(fs: &dyn FileSystem, path: &Path) -> Pager {
    Pager::create(fs, path, crc32c(), Pipeline::none(), PAGE).unwrap()
}

fn read(pager: &Pager, id: u64) -> turborel::error::Result<Vec<u8>> {
    let mut content = Vec::new();
    pager.read(PageId(id), &mut content)?;
    Ok(content)
}

/// Verifies: [DESIGN] RULES.md D30, pager: when writing a page fails after
/// its overflow pages were written, the write fails with SQLCODE -915, the
/// overflow pages are free again, and the page reads as before; a failed
/// read is reported, not taken for content.
#[test]
fn a_failed_page_write_changes_nothing() {
    let scratch = Scratch::new("failures-page-write");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &scratch.path("p"));
    pager.allocate(1);
    pager.write(PageId(0), b"before").unwrap();
    let free = pager.free_pages();
    // The overflow pages are one extent, one write; the page's write fails.
    faults.inject(Rule::fail(Op::Write).after(1).times(1));
    let failed = pager.write(PageId(0), &vec![7; INLINE * 3]);
    assert_eq!(failed.err().map(|e| e.code), Some(Code::Io));
    assert_eq!(read(&pager, 0).unwrap(), b"before");
    assert_eq!(pager.free_pages(), free + pager.page_count() - 1, "the overflow pages are free again");
    faults.inject(Rule::fail(Op::Read).times(1));
    assert_eq!(read(&pager, 0).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(read(&pager, 0).unwrap(), b"before");
    assert_eq!(faults.injected(), 2);
}

/// Verifies: [DESIGN] RULES.md D30, pager: when `sync` fails writing the
/// free list's trunk, or flushing, or writing the meta slot, it reports
/// SQLCODE -915, the previous record stays current (a reopened file has the
/// free list of the last sync that succeeded), and the next sync, the fault
/// passed, records the free list.
#[test]
fn a_failed_pager_sync_keeps_the_previous_record() {
    let scratch = Scratch::new("failures-page-sync");
    let path = scratch.path("p");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &path);
    pager.allocate(10);
    pager.write(PageId(9), b"last").unwrap();
    pager.free(&[Extent { start: PageId(2), count: 3 }]).unwrap();
    pager.sync(Durability::Data).unwrap();
    let rules = [Rule::fail(Op::Write).at_or_past(u64::from(PAGE)), Rule::fail(Op::Sync), Rule::fail(Op::Write).below(u64::from(PAGE))];
    for (page, rule) in (5..).zip(rules) {
        // Pages 5, 6 and 7: the test's own, never the pager's trunk.
        pager.free(&[Extent { start: PageId(page), count: 1 }]).unwrap();
        faults.inject(rule.times(1));
        assert_eq!(pager.sync(Durability::Data).err().map(|e| e.code), Some(Code::Io));
        let (reopened, recovery) = Pager::open(&*fs, &path, &[crc32c()], &[]).unwrap();
        drop(reopened);
        assert!(recovery.free < pager.free_pages(), "page 0's freeing is not recorded");
        pager.sync(Durability::Data).unwrap();
        let (reopened, recovery) = Pager::open(&*fs, &path, &[crc32c()], &[]).unwrap();
        drop(reopened);
        assert_eq!(recovery.free, pager.free_pages(), "recorded once the fault passed");
    }
    assert_eq!(faults.injected(), 3);
}

/// Verifies: [DESIGN] RULES.md D30, pager: freeing a page the pager uses for
/// its free list is refused with SQLCODE -910, freeing nothing.
///
/// Regression test (R-TEST-7): the test of failed syncs freed a page the
/// pager had taken for its free list's trunk, and the free list was lost.
#[test]
fn the_pagers_own_pages_cannot_be_freed() {
    let scratch = Scratch::new("failures-page-own");
    let (fs, _) = FaultyFs::with_faults();
    let pager = pager(&*fs, &scratch.path("p"));
    pager.allocate(10);
    pager.write(PageId(9), b"last").unwrap();
    pager.free(&[Extent { start: PageId(2), count: 3 }]).unwrap();
    pager.sync(Durability::Data).unwrap();
    let free = pager.free_pages();
    // The record took page 2, the first free page, for its trunk.
    assert_eq!(pager.free(&[Extent { start: PageId(2), count: 1 }]).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(pager.free_pages(), free);
    assert!(Pager::open(&*fs, &scratch.path("p"), &[crc32c()], &[]).is_ok());
}

/// Verifies: [DESIGN] RULES.md D30, pager: when zeroing freed pages fails,
/// `free` reports SQLCODE -915 and the pages stay allocated and readable.
#[test]
fn a_failed_free_keeps_the_pages() {
    let scratch = Scratch::new("failures-page-free");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &scratch.path("p"));
    pager.allocate(2);
    pager.write(PageId(1), b"kept").unwrap();
    faults.inject(Rule::fail(Op::Write).times(1));
    assert_eq!(pager.free(&[Extent { start: PageId(1), count: 1 }]).err().map(|e| e.code), Some(Code::Io));
    assert_eq!((pager.free_pages(), read(&pager, 1).unwrap()), (0, b"kept".to_vec()));
}

/// Verifies: [DESIGN] RULES.md D30, D56: a pager file, scratch or not, or a
/// block file, whose header cannot be written, or that cannot be synced,
/// is refused with SQLCODE -915 and removed, so it may be made again once
/// the fault passes.
///
/// Regression test (R-TEST-7): the half-made file was left, a spill
/// file leaked, and making it again was refused as it existed.
#[test]
fn a_file_half_made_is_removed() {
    let scratch = Scratch::new("failures-half-made-files");
    let (fs, faults) = FaultyFs::with_faults();
    let path = scratch.path("f");
    let make: [&dyn Fn() -> turborel::error::Result<()>; 3] = [
        &|| Pager::create(&*fs, &path, crc32c(), Pipeline::none(), PAGE).map(drop),
        &|| Pager::create_scratch(&*fs, &path, crc32c(), Pipeline::none(), PAGE).map(drop),
        &|| BlockFile::create(&*fs, &path, crc32c(), Pipeline::none(), BlockOptions::default()).map(drop),
    ];
    for (n, made) in make.iter().enumerate() {
        let op = if n == 1 { Op::Write } else { Op::Sync };
        faults.inject(Rule::fail(op).times(1));
        assert_eq!(made().err().map(|e| e.code), Some(Code::Io), "{n}");
        assert!(!path.exists(), "{n}: removed");
        made().unwrap();
        std::fs::remove_file(&path).unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D30, D56, file log: a log never synced
/// (`Durability::None`, the undo log's) syncs nothing, as it makes its
/// segments or removes them: every sync failing, it is made, rotates and
/// truncates; one that syncs is refused as it is made.
///
/// Regression test (R-TEST-7): the undo log synced each segment it made,
/// and its directory at each truncation.
#[test]
fn a_log_never_synced_syncs_nothing() {
    let scratch = Scratch::new("failures-unsynced-log");
    let (fs, faults) = FaultyFs::with_faults();
    faults.inject(Rule::fail(Op::Sync));
    faults.inject(Rule::fail(Op::SyncDir));
    let unsynced = FileLogOptions { durability: Durability::None, ..small_segments() };
    let log = FileLog::create(Arc::clone(&fs), &scratch.path("undo"), crc32c(), unsynced).unwrap();
    let last = (0..30_u8).map(|n| log.append(&[n; 400]).unwrap()).last().unwrap();
    assert!(log.counts().rotations > 1);
    log.truncate_before(last).unwrap();
    assert_eq!(faults.injected(), 0, "no sync asked");
    assert_eq!(FileLog::create(Arc::clone(&fs), &scratch.path("log"), crc32c(), small_segments()).err().map(|e| e.code), Some(Code::Io));
}

/// Verifies: [DESIGN] RULES.md D30, block manager: a failed sync or read is
/// reported with SQLCODE -915, and the block reads once the fault passes;
/// opening fails, with SQLCODE -915 or -916, when its open, size or read does.
#[test]
fn block_file_faults_are_reported() {
    let scratch = Scratch::new("failures-blocks");
    let path = scratch.path("b");
    let (fs, faults) = FaultyFs::with_faults();
    let blocks = BlockFile::create(&*fs, &path, crc32c(), Pipeline::none(), BlockOptions::default()).unwrap();
    let id = blocks.append(b"block").unwrap();
    faults.inject(Rule::fail(Op::Sync).times(1));
    assert_eq!(blocks.sync(Durability::Data).err().map(|e| e.code), Some(Code::Io));
    faults.inject(Rule::fail(Op::Read).times(1));
    assert_eq!(blocks.read(id).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(blocks.read(id).unwrap(), b"block");
    drop(blocks);
    for op in [Op::Open, Op::Size, Op::Read] {
        faults.inject(Rule::fail(op).times(1));
        let opened = BlockFile::open(&*fs, &path, &[crc32c()], &[], BlockOptions::default());
        assert!(matches!(opened.err().map(|e| e.code), Some(Code::Io | Code::Corrupt)), "{op:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D25, file log: when the next segment cannot
/// be created, appends still succeed, into the current segment, and the
/// next append after the fault passes begins the segment; a failed sync is
/// reported, and the next one, the fault passed, succeeds.
#[test]
fn a_log_survives_failed_rotations_and_syncs() {
    let scratch = Scratch::new("failures-log");
    let dir = scratch.path("log");
    let (fs, faults) = FaultyFs::with_faults();
    let log = FileLog::create(Arc::clone(&fs), &dir, crc32c(), small_segments()).unwrap();
    faults.inject(Rule::fail(Op::Open).on("segment-0000000001"));
    let positions: Vec<Lsn> = (0..20).map(|n| log.append(&[u8::try_from(n).unwrap(); 400]).unwrap()).collect();
    assert_eq!(log.counts().rotations, 0, "segment 1 could not be begun");
    faults.clear();
    log.append(b"after the fault").unwrap();
    assert_eq!(log.counts().rotations, 1, "the append began segment 1, then sealed segment 0");
    let next = log.append(b"in segment 1").unwrap();
    faults.inject(Rule::fail(Op::Sync).times(1));
    assert_eq!(log.sync(next).err().map(|e| e.code), Some(Code::Io));
    log.sync(next).unwrap();
    log.sync(positions[0]).unwrap();
    let read: Vec<_> = log.read_from(Lsn(0)).collect::<turborel::error::Result<_>>().unwrap();
    assert_eq!(read.len(), 22);
}

/// Verifies: [DESIGN] RULES.md D25, D56, file log: a segment whose header
/// cannot be written is left half made; the next rotation removes it and
/// makes it again, so the log rotates once the fault passes, the segment
/// before growing past its size meanwhile; the failure is told to the
/// log's sink once, however many rotations fail, and again after one
/// succeeds.
///
/// Regression test (R-TEST-7): a half-made segment refused every rotation
/// after (it existed), and the failure was never told.
#[test]
fn a_half_made_segment_is_made_again_and_told() {
    let scratch = Scratch::new("failures-half-made");
    let dir = scratch.path("log");
    let (fs, faults) = FaultyFs::with_faults();
    let kept = Kept::default();
    let log = FileLog::create(Arc::clone(&fs), &dir, crc32c(), small_segments()).unwrap().telling(Arc::new(kept.clone()));
    faults.inject(Rule::fail(Op::Write).on("segment-0000000001").times(3));
    for n in 0..20_u8 {
        log.append(&[n; 400]).unwrap();
    }
    assert_eq!((faults.injected(), log.counts().rotations), (3, 1), "made again past three faults");
    let told: Vec<String> = kept.taken().into_iter().map(|(_, e)| e).collect();
    assert_eq!(told.len(), 1, "told once: {told:?}");
    assert!(told[0].contains("could not begin a segment after segment 0"), "{told:?}");
    faults.inject(Rule::fail(Op::Open).on("segment-0000000002"));
    for n in 0..24_u8 {
        log.append(&[n; 400]).unwrap();
    }
    assert_eq!(kept.taken().len(), 1, "told again after a rotation succeeded");
    faults.clear();
    let read: Vec<_> = log.read_from(Lsn(0)).collect::<turborel::error::Result<_>>().unwrap();
    assert_eq!(read.len(), 44);
}

/// Verifies: [DESIGN] RULES.md D25, file log: a segment whose file cannot be
/// removed stays in the log, readable, and truncation reports SQLCODE -915;
/// opening a log whose directory cannot be read fails the same way.
///
/// Regression test (R-TEST-7): truncation dropped a segment from the log
/// before removing its file, so a failed removal lost its records to
/// readers.
#[test]
fn a_failed_truncation_keeps_the_segment() {
    let scratch = Scratch::new("failures-log-truncate");
    let dir = scratch.path("log");
    let (fs, faults) = FaultyFs::with_faults();
    let log = FileLog::create(Arc::clone(&fs), &dir, crc32c(), small_segments()).unwrap();
    let positions: Vec<Lsn> = (0..30).map(|n| log.append(&[u8::try_from(n).unwrap(); 400]).unwrap()).collect();
    log.sync(positions[29]).unwrap();
    faults.inject(Rule::fail(Op::Remove));
    assert_eq!(log.truncate_before(positions[29]).err().map(|e| e.code), Some(Code::Io));
    let read: Vec<_> = log.read_from(Lsn(0)).collect::<turborel::error::Result<_>>().unwrap();
    assert_eq!(read.len(), 30, "nothing lost");
    faults.clear();
    drop(log);
    faults.inject(Rule::fail(Op::ReadDir));
    assert_eq!(FileLog::open(Arc::clone(&fs), &dir, &[crc32c()], &[], small_segments()).err().map(|e| e.code), Some(Code::Io));
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: a page that cannot be read
/// is reported and leaves the pool usable; when a flush fails, the page
/// stays dirty and the next flush writes it.
#[test]
fn buffer_pool_faults_leave_it_usable() {
    let scratch = Scratch::new("failures-buffer");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = Arc::new(pager(&*fs, &scratch.path("p")));
    let pool = BufferPool::new(2, Box::new(Clock::new(2))).unwrap();
    let file = pool.add_pager(Arc::clone(&pager));
    pager.allocate(1);
    pager.write(PageId(0), b"on disk").unwrap();
    faults.inject(Rule::fail(Op::Read).times(1));
    assert_eq!(pool.read(file, 0).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(&*pool.read(file, 0).unwrap(), b"on disk");
    pool.write(file, PageId(0)).unwrap().extend_from_slice(b", changed");
    faults.inject(Rule::fail(Op::Write).times(1));
    assert_eq!(pool.flush().err().map(|e| e.code), Some(Code::Io));
    pool.flush().unwrap();
    assert_eq!(read(&pager, 0).unwrap(), b"on disk, changed");
}
