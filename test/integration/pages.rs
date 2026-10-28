//! The pager (RULES.md D30): content of any length written and read in
//! place, overflow kept by the pager in extents and directories, page
//! allocation and the free list, its persistence by `sync` in alternating
//! meta slots, the file shrinking, torn, corrupted and misdirected pages,
//! and pages written from several threads at once.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, OsFileSystem};
use turborel::disk::measured::MeasuredFileSystem;
use turborel::disk::pager::{DEFAULT_PAGE_SIZE, Extent, MAX_PAGE_SIZE, MIN_PAGE_SIZE, PageId, Pager, PagerRecovery};
use turborel::error::{Code, Result};
use turborel::hash::HashFunction;
use turborel::hash::crc::{CRC64_NVME, Crc};

use crate::common::{Scratch, crc32c};
use crate::faults::{FaultyFs, Rule};

/// The test page size. With CRC-32C a page's body is the page less its
/// 12-byte trailer: 4084 bytes, of which a data page's header takes 24 and
/// an overflow page's 8.
const PAGE: u32 = 4096;
const INLINE: usize = 4084 - 24;
const OVERFLOW: usize = 4084 - 8;

fn create(path: &Path) -> Pager {
    Pager::create(&OsFileSystem, path, crc32c(), Pipeline::none(), PAGE).unwrap()
}

fn open(path: &Path) -> Result<(Pager, PagerRecovery)> {
    Pager::open(&OsFileSystem, path, &[crc32c()], &[])
}

fn read(pager: &Pager, id: u64) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    pager.read(PageId(id), &mut content)?;
    Ok(content)
}

/// `len` bytes that differ from page to page and position to position.
fn content(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap() ^ seed).collect()
}

fn one(start: u64) -> Extent {
    Extent { start: PageId(start), count: 1 }
}

/// Overwrites the bytes at `at` of the file at `path`.
fn overwrite(path: &Path, at: u64, bytes: &[u8]) {
    let mut f = OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(bytes).unwrap();
}

/// The byte offset of page `id`.
fn offset(id: u64) -> u64 {
    (id + 1) * u64::from(PAGE)
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

/// Verifies: [DESIGN] RULES.md D30, pager: allocated pages are written out
/// of order and rewritten in place, and read back as last written; a page
/// allocated and never written has no content; pages are aligned to their
/// size after the header page; contents survive `sync`, closing and
/// reopening; the counters count it.
#[test]
fn pages_are_written_and_read_in_place() {
    let scratch = Scratch::new("pages-rw");
    let path = scratch.path("p");
    let pager = create(&path);
    assert_eq!((pager.page_size(), pager.inline_capacity(), pager.page_count()), (4096, INLINE, 0));
    assert_eq!(pager.allocate(4), vec![Extent { start: PageId(0), count: 4 }]);
    pager.write(PageId(3), b"three").unwrap();
    pager.write(PageId(0), b"first").unwrap();
    pager.write(PageId(0), b"zero").unwrap();
    assert_eq!(read(&pager, 0).unwrap(), b"zero", "rewritten in place");
    assert_eq!(read(&pager, 1).unwrap(), b"", "allocated, never written");
    pager.sync(Durability::Full).unwrap();
    let counts = pager.counts();
    assert_eq!((counts.writes, counts.reads, counts.new_reads, counts.syncs, counts.corrupt_reads), (3, 2, 1, 1, 0));
    drop(pager);
    let (pager, recovery) = open(&path).unwrap();
    assert_eq!(recovery, PagerRecovery { pages: 4, free: 0, cut: 0 });
    assert_eq!((read(&pager, 0).unwrap(), read(&pager, 3).unwrap()), (b"zero".to_vec(), b"three".to_vec()));
    assert_eq!(file_len(&path), offset(4));
}

/// Verifies: [DESIGN] RULES.md D30, overflow: content of any length, at
/// the page's limit, one byte over it, many pages, and a mebibyte, reads
/// back whole after the pager put what did not fit in overflow pages,
/// without the caller saying so; rewriting content shorter frees its
/// overflow pages, which the next `sync` cuts from the end of the file.
#[test]
fn content_of_any_length_overflows_on_its_own() {
    let scratch = Scratch::new("pages-overflow");
    let path = scratch.path("p");
    let pager = create(&path);
    let sizes = [0, 1, INLINE, INLINE + 1, INLINE + 3 * OVERFLOW, 100 * 1024, 1024 * 1024];
    let ids: Vec<u64> = (0..sizes.len()).map(|_| pager.allocate(1)[0].start.0).collect();
    for (&id, &len) in ids.iter().zip(&sizes) {
        pager.write(PageId(id), &content(len, u8::try_from(id).unwrap())).unwrap();
    }
    for (&id, &len) in ids.iter().zip(&sizes) {
        assert_eq!(read(&pager, id).unwrap(), content(len, u8::try_from(id).unwrap()), "{len} bytes");
    }
    assert_eq!(pager.counts().overflow_reads, 4, "only content past the page's limit overflowed");
    assert_eq!(pager.page_count(), 7 + 1 + 4 + 25 + 257, "the pages, and just the overflow pages each needed");
    pager.sync(Durability::Data).unwrap();
    drop(pager);
    let (pager, _) = open(&path).unwrap();
    assert_eq!(read(&pager, ids[6]).unwrap(), content(1024 * 1024, 6));
    pager.write(PageId(ids[6]), b"short now").unwrap();
    assert_eq!((read(&pager, ids[6]).unwrap(), pager.free_pages()), (b"short now".to_vec(), 257));
    pager.sync(Durability::Data).unwrap();
    assert_eq!((pager.page_count(), pager.free_pages()), (7 + 1 + 4 + 25, 0), "the free pages at the end are cut");
    assert_eq!(file_len(&path), offset(7 + 1 + 4 + 25));
}

/// Verifies: [DESIGN] RULES.md D30, overflow: content whose overflow pages
/// come from many separate free runs lists the first extents in its page
/// and the rest in directory pages, and reads back whole; the file does not
/// grow while free pages remain; freeing the page frees its overflow and
/// directory pages too.
#[test]
fn scattered_overflow_uses_directories_and_free_pages_first() {
    let scratch = Scratch::new("pages-directories");
    let pager = create(&scratch.path("p"));
    let first = pager.allocate(400)[0].start.0;
    // Free every other page: 200 runs of one page.
    let scattered: Vec<Extent> = (0..200).map(|n| one(first + 2 * n + 1)).collect();
    pager.free(&scattered).unwrap();
    assert_eq!(pager.free_pages(), 200);
    let big = content(150 * OVERFLOW, 9);
    pager.write(PageId(first), &big).unwrap();
    assert_eq!(read(&pager, first).unwrap(), big);
    assert_eq!(pager.page_count(), 400, "free pages were used before the file grew");
    assert_eq!(pager.free_pages(), 200 - 150 - 1, "150 overflow pages and one directory page");
    pager.free(&[one(first)]).unwrap();
    assert_eq!(pager.free_pages(), 200 + 1, "the page, its overflow and its directory");
}

/// Verifies: [DESIGN] RULES.md D30, allocation: reading or writing a page
/// not allocated, freeing a page twice or past the end, and reading or
/// writing a page the pager uses for overflow are refused with SQLCODE
/// -910; freed pages are zeroed, so a reused page reads as new.
#[test]
fn allocation_is_checked_and_freed_pages_are_zeroed() {
    let scratch = Scratch::new("pages-alloc");
    let path = scratch.path("p");
    let pager = create(&path);
    assert_eq!(read(&pager, 0).err().map(|e| e.code), Some(Code::Api), "nothing allocated");
    let page = pager.allocate(1)[0].start.0;
    pager.write(PageId(page), &content(INLINE + 10, 1)).unwrap();
    let overflow_page = page + 1;
    assert_eq!(read(&pager, overflow_page).err().map(|e| e.code), Some(Code::Api), "the pager's own page");
    assert_eq!(pager.write(PageId(overflow_page), b"x").err().map(|e| e.code), Some(Code::Api));
    pager.free(&[one(page)]).unwrap();
    assert_eq!(pager.free(&[one(page)]).err().map(|e| e.code), Some(Code::Api), "freed twice");
    assert_eq!(pager.free(&[one(9)]).err().map(|e| e.code), Some(Code::Api), "past the end");
    assert_eq!(pager.write(PageId(page), b"x").err().map(|e| e.code), Some(Code::Api), "free");
    let all = std::fs::read(&path).unwrap();
    assert!(all[usize::try_from(offset(page)).unwrap()..].iter().all(|&b| b == 0), "both pages zeroed");
    let reused = pager.allocate(2);
    assert_eq!(reused, vec![Extent { start: PageId(page), count: 2 }]);
    assert_eq!((read(&pager, page).unwrap(), read(&pager, overflow_page).unwrap()), (vec![], vec![]), "reused, read as new");
}

/// Verifies: [DESIGN] RULES.md D30, persistence: the free list is recorded
/// by `sync` and found on open; what was allocated or freed after the last
/// `sync` is, after closing without one, as it was at that `sync`, and pages
/// written past the pages it knew are free.
#[test]
fn the_free_list_is_that_of_the_last_sync() {
    let scratch = Scratch::new("pages-freelist");
    let path = scratch.path("p");
    let pager = create(&path);
    pager.allocate(10);
    for id in 0..10 {
        pager.write(PageId(id), &[u8::try_from(id).unwrap()]).unwrap();
    }
    pager.free(&[one(2), one(3), one(7)]).unwrap();
    // The record takes page 7, the smallest free run, for its trunk, and
    // records 2 and 3 free.
    pager.sync(Durability::Full).unwrap();
    pager.free(&[one(5)]).unwrap();
    // The three free pages, then two new: 10 and 11.
    assert_eq!(pager.allocate(5).last(), Some(&Extent { start: PageId(10), count: 2 }));
    pager.write(PageId(10), b"after the sync").unwrap();
    drop(pager);
    let (pager, recovery) = open(&path).unwrap();
    assert_eq!(recovery, PagerRecovery { pages: 11, free: 3, cut: 0 }, "2, 3, and 10, written past the 10 pages known");
    assert_eq!(read(&pager, 5).unwrap(), b"", "freed (zeroed) after the sync: in use again, empty");
    assert_eq!(pager.allocate(2), vec![Extent { start: PageId(2), count: 2 }]);
}

/// Verifies: [DESIGN] RULES.md D30, persistence: `sync` records the free
/// list only when it changed since its last record; otherwise, and after a
/// record whose old trunk pages it already listed as free, it only syncs
/// the pages written: no trunk or meta write.
#[test]
fn an_unchanged_free_list_is_not_recorded_again() {
    let scratch = Scratch::new("pages-unchanged");
    let fs = MeasuredFileSystem::new(Box::new(OsFileSystem));
    let stats = fs.stats();
    let pager = Pager::create(&fs, &scratch.path("p"), crc32c(), Pipeline::none(), PAGE).unwrap();
    pager.allocate(20);
    pager.free(&[Extent { start: PageId(2), count: 3 }]).unwrap();
    pager.sync(Durability::Data).unwrap();
    pager.free(&[one(10)]).unwrap();
    pager.sync(Durability::Data).unwrap();
    let before = stats.snapshot();
    pager.write(PageId(0), b"only this").unwrap();
    pager.sync(Durability::Data).unwrap();
    pager.sync(Durability::Data).unwrap();
    let since = stats.snapshot().since(&before);
    assert_eq!((since.writes, since.syncs), (1, 2), "the page alone, then two syncs with nothing to record");
    assert_eq!(pager.free_pages(), 4 - 1, "2-4 and 10, less one page for the trunk");
}

/// Verifies: [DESIGN] RULES.md D30, meta slots: `sync` records the free
/// list in the meta slot that is not current, so a crash while writing it
/// (a torn slot) leaves the previous record, and the trunk pages it names,
/// whole: opening uses it. With both slots damaged, opening is refused
/// with SQLCODE -916.
#[test]
fn a_torn_meta_slot_falls_back_to_the_other() {
    let scratch = Scratch::new("pages-meta");
    let path = scratch.path("p");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = Pager::create(&*fs, &path, crc32c(), Pipeline::none(), PAGE).unwrap();
    pager.allocate(10);
    pager.write(PageId(9), b"last").unwrap();
    // Pages 1 to 4 free: the record takes page 1 for its trunk, so it
    // records 2 to 4 free.
    pager.free(&[Extent { start: PageId(1), count: 4 }]).unwrap();
    pager.sync(Durability::Full).unwrap();
    pager.free(&[one(0)]).unwrap();
    // Writes to the header slot, where the meta slots are, are torn.
    faults.inject(Rule::tear().below(u64::from(PAGE)));
    assert!(pager.sync(Durability::Full).unwrap_err().msg.contains("injected"));
    drop(pager);
    let (pager, recovery) = Pager::open(&OsFileSystem, &path, &[crc32c()], &[]).unwrap();
    assert_eq!(recovery, PagerRecovery { pages: 10, free: 3, cut: 0 }, "the first sync's record");
    assert_eq!(pager.allocate(3), vec![Extent { start: PageId(2), count: 3 }]);
    drop(pager);
    overwrite(&path, 1024, &[0xFF]);
    overwrite(&path, 2048, &[0xFF]);
    assert_eq!(open(&path).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D30, pager: a page that is torn or corrupted
/// fails its checksum, a correct page written in the wrong place is caught
/// by its page number, and a damaged overflow page fails the read of its
/// content, each with SQLCODE -916 and counted.
///
/// MC/DC of the page check: page number right with the digest wrong (torn,
/// corrupted); page number wrong with the digest right (misdirected); both
/// right (page 0).
#[test]
fn torn_corrupt_and_misdirected_pages_are_caught() {
    let scratch = Scratch::new("pages-damage");
    let path = scratch.path("p");
    let pager = create(&path);
    pager.allocate(5);
    for id in 0..4 {
        pager.write(PageId(id), &content(100, u8::try_from(id).unwrap())).unwrap();
    }
    pager.write(PageId(4), &content(INLINE + 10, 4)).unwrap();
    overwrite(&path, offset(1), &[0xEE; 2048]);
    overwrite(&path, offset(2) + 100, &[0]);
    let all = std::fs::read(&path).unwrap();
    let first_page = &all[usize::try_from(offset(0)).unwrap()..usize::try_from(offset(1)).unwrap()];
    overwrite(&path, offset(3), first_page);
    overwrite(&path, offset(5) + 50, &[0xDE, 0xAD, 0xBE, 0xEF]);
    for (id, why) in [(1, "checksum"), (2, "checksum"), (3, "holds page 0"), (4, "checksum")] {
        let error = read(&pager, id).unwrap_err();
        assert_eq!(error.code, Code::Corrupt, "page {id}");
        assert!(error.msg.contains(why), "page {id}: {error}");
    }
    assert_eq!(read(&pager, 0).unwrap(), content(100, 0));
    assert_eq!(pager.counts().corrupt_reads, 4);
}

/// Verifies: [DESIGN] RULES.md D30: page sizes are powers of two from
/// `MIN_PAGE_SIZE` to `MAX_PAGE_SIZE`, recorded in the file; others are
/// refused with SQLCODE -910; another hash function leaves another inline
/// capacity.
#[test]
fn page_sizes_are_checked() {
    let scratch = Scratch::new("pages-sizes");
    for (n, size) in [MIN_PAGE_SIZE, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE].into_iter().enumerate() {
        let path = scratch.path(&format!("ok{n}"));
        let pager = Pager::create(&OsFileSystem, &path, crc32c(), Pipeline::none(), size).unwrap();
        pager.allocate(1);
        pager.write(PageId(0), &content(100_000, 3)).unwrap();
        pager.sync(Durability::None).unwrap();
        drop(pager);
        let (pager, _) = open(&path).unwrap();
        assert_eq!((pager.page_size(), read(&pager, 0).unwrap()), (usize::try_from(size).unwrap(), content(100_000, 3)));
    }
    for (n, size) in [0, 512, 3000, 128 * 1024].into_iter().enumerate() {
        let made = Pager::create(&OsFileSystem, &scratch.path(&format!("bad{n}")), crc32c(), Pipeline::none(), size);
        assert_eq!(made.err().map(|e| e.code), Some(Code::Api), "page size {size}");
    }
    let nvme: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC64_NVME).unwrap());
    assert_eq!(Pager::create(&OsFileSystem, &scratch.path("nvme"), nvme, Pipeline::none(), PAGE).unwrap().inline_capacity(), INLINE - 4);
}

/// Verifies: [DESIGN] RULES.md D30: a file that is not a pager file (too
/// short, wrong magic, unknown version, header failing its checksum, an
/// invalid page size) is refused with SQLCODE -916; one checksummed with a
/// hash function not given, with SQLCODE -910; a partial page at the end
/// is cut off and reported.
#[test]
fn files_that_are_not_pager_files_are_refused() {
    let scratch = Scratch::new("pages-header");
    let path = scratch.path("p");
    drop(create(&path));
    for (at, bytes, damage) in [
        (0, &b"X"[..], "magic"),
        (8, &[9][..], "version"),
        (44, &[0xA5, 0x5A][..], "digest"),
        (24, &[7][..], "encodings"),
        (17, &[0x11][..], "page size"),
    ] {
        let p = scratch.path(damage);
        std::fs::copy(&path, &p).unwrap();
        overwrite(&p, at, bytes);
        assert_eq!(open(&p).err().map(|e| e.code), Some(Code::Corrupt), "{damage}");
    }
    let short = scratch.path("short");
    std::fs::write(&short, b"TBPAGES").unwrap();
    assert_eq!(open(&short).err().map(|e| e.code), Some(Code::Corrupt));
    let nvme: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC64_NVME).unwrap());
    assert_eq!(Pager::open(&OsFileSystem, &path, &[nvme], &[]).err().map(|e| e.code), Some(Code::Api));
    OpenOptions::new().write(true).open(&path).unwrap().set_len(offset(0) + 1000).unwrap();
    assert_eq!(open(&path).unwrap().1, PagerRecovery { pages: 0, free: 0, cut: 1000 });
}

/// Threads writing at once, and pages each writes.
const WRITERS: u8 = 8;
const PAGES_EACH: u8 = 12;

/// Verifies: [DESIGN] RULES.md D30: threads allocating pages and writing
/// content, some overflowing, at once never disturb each other.
#[test]
fn threads_allocate_and_write_at_once() {
    let scratch = Scratch::new("pages-threads");
    let pager = Arc::new(create(&scratch.path("p")));
    let writers: Vec<_> = (0..WRITERS)
        .map(|w| {
            let pager = Arc::clone(&pager);
            thread::spawn(move || {
                (0..PAGES_EACH)
                    .map(|n| {
                        let id = pager.allocate(1)[0].start;
                        let body = content(usize::from(n) * 1500, w * 16 + n);
                        pager.write(id, &body).unwrap();
                        (id, body)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let written: Vec<(PageId, Vec<u8>)> = writers.into_iter().flat_map(|w| w.join().unwrap()).collect();
    for (id, body) in written {
        assert_eq!(read(&pager, id.0).unwrap(), body);
    }
}

/// Verifies: [DESIGN] RULES.md D30, the pager's knowledge of overflow: a
/// page it read or wrote is written again, and freed, without reading its
/// old version, its old overflow freed all the same; a page it knows
/// nothing of, opened again, is read first; content read back is what was
/// written last.
#[test]
fn a_known_page_is_written_without_reading_it() {
    let scratch = Scratch::new("pages-known");
    let path = scratch.path("f");
    let reads_of = |fs: &MeasuredFileSystem, work: &dyn Fn()| {
        let before = fs.stats().snapshot();
        work();
        fs.stats().snapshot().since(&before).reads
    };
    let fs = MeasuredFileSystem::new(Box::new(OsFileSystem));
    let pager = Pager::create(&fs, &path, crc32c(), Pipeline::none(), PAGE).unwrap();
    let ids = pager.allocate(2);
    let (small, large) = (ids[0].start.0, ids[0].start.0 + 1);
    pager.write(PageId(small), b"small").unwrap();
    pager.write(PageId(large), &vec![3; 3 * OVERFLOW]).unwrap();
    let grown = pager.page_count();
    assert_eq!(reads_of(&fs, &|| pager.write(PageId(small), b"again").unwrap()), 0, "known: written before");
    assert_eq!(reads_of(&fs, &|| pager.write(PageId(large), b"shrunk").unwrap()), 0, "its overflow known");
    pager.sync(Durability::Full).unwrap();
    assert!(pager.page_count() < grown || pager.free_pages() > 0, "the old overflow freed");
    assert_eq!(read(&pager, large).unwrap(), b"shrunk");
    drop(pager);

    let fs = MeasuredFileSystem::new(Box::new(OsFileSystem));
    let (pager, _) = Pager::open(&fs, &path, &[crc32c()], &[]).unwrap();
    assert_eq!(reads_of(&fs, &|| pager.write(PageId(small), b"third").unwrap()), 1, "unknown: read first");
    assert_eq!(read(&pager, large).unwrap(), b"shrunk");
    assert_eq!(reads_of(&fs, &|| pager.write(PageId(large), b"fourth").unwrap()), 0, "known once read");
    assert_eq!(reads_of(&fs, &|| pager.free(&[Extent { start: PageId(small), count: 2 }]).unwrap()), 0, "freed as known");
}
