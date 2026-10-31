//! The buffer pool (RULES.md D30): pages cached, written back on flush and
//! on eviction, frames pinned while held, block files cached for reading, a
//! failed write-back or load leaving the pool consistent, and threads using
//! the pool at once.

use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use turborel::disk::block::{BlockFile, BlockOptions};
use turborel::disk::buffer::{BufferPool, Clock, FileId};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, FileSystem, OsFileSystem};
use turborel::disk::pager::{PageId, Pager};
use turborel::error::{Code, Result};

use crate::common::{Scratch, crc32c};
use crate::faults::{FaultyFs, Op, Rule};

pub(crate) fn pager(fs: &dyn FileSystem, path: &Path) -> Arc<Pager> {
    Arc::new(Pager::create(fs, path, crc32c(), Pipeline::none(), 4096).unwrap())
}

pub(crate) fn pool(frames: usize) -> BufferPool {
    BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap()
}

/// A new page of `file` holding `content`; its number.
pub(crate) fn put(pool: &BufferPool, file: FileId, content: &[u8]) -> PageId {
    let (id, mut page) = pool.new_page(file).unwrap();
    page.extend_from_slice(content);
    id
}

pub(crate) fn get(pool: &BufferPool, file: FileId, id: PageId) -> Result<Vec<u8>> {
    Ok(pool.read(file, id.0)?.to_vec())
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: pages made and written
/// through the pool read back from it without reading the file (hits);
/// `flush` writes the dirty ones to the pager, which then reads them; a
/// page written again is dirty again.
#[test]
fn pages_are_cached_and_written_back_on_flush() {
    let scratch = Scratch::new("buffer-cache");
    let pager = pager(&OsFileSystem, &scratch.path("p"));
    let pool = pool(4);
    let file = pool.add_pager(Arc::clone(&pager));
    let ids: Vec<PageId> = (0..3).map(|n| put(&pool, file, format!("page {n}").as_bytes())).collect();
    assert_eq!(get(&pool, file, ids[1]).unwrap(), b"page 1");
    let counts = pool.counts();
    assert_eq!((counts.hits, counts.misses, counts.write_backs), (1, 0, 0));
    assert_eq!(pager.counts().writes, 0, "nothing written yet");
    pool.flush().unwrap();
    assert_eq!(pool.counts().write_backs, 3);
    let mut content = Vec::new();
    pager.read(ids[2], &mut content).unwrap();
    assert_eq!(content, b"page 2");
    pool.flush().unwrap();
    assert_eq!(pool.counts().write_backs, 3, "nothing dirty");
    let mut page = pool.write(file, ids[0]).unwrap();
    assert_eq!(page.len(), 6, "a page held for writing reads as it is");
    page.extend_from_slice(b", again");
    drop(page);
    pool.sync(file, Durability::Data).unwrap();
    pager.read(ids[0], &mut content).unwrap();
    assert_eq!((content.as_slice(), pool.frames()), (&b"page 0, again"[..], 4));
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: with more pages than
/// frames, frames are evicted, dirty pages written back first, and pages
/// read again on a miss come back as written.
#[test]
fn eviction_writes_back_dirty_pages() {
    let scratch = Scratch::new("buffer-evict");
    let pool = pool(2);
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let ids: Vec<PageId> = (0..6).map(|n| put(&pool, file, &vec![n; 5000])).collect();
    for (n, id) in (0..6).zip(&ids) {
        assert_eq!(get(&pool, file, *id).unwrap(), vec![n; 5000], "page {n}");
    }
    let counts = pool.counts();
    assert!(counts.evictions >= 4 && counts.write_backs >= 4 && counts.misses >= 4, "{counts:?}");
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: a held page pins its frame:
/// with every frame held, another page cannot be read (SQLCODE -918, D38:
/// the pool too small for the work, refused at once) until one is
/// released; a held page cannot be freed; a freed page leaves the pool and
/// its file.
#[test]
fn held_pages_pin_their_frames() {
    let scratch = Scratch::new("buffer-pins");
    let pool = pool(1);
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let a = put(&pool, file, b"a");
    let b = put(&pool, file, b"b");
    let held = pool.read(file, a.0).unwrap();
    assert_eq!(&*held, b"a");
    let full = pool.read(file, b.0).err();
    assert_eq!(full.as_ref().map(|e| e.code), Some(Code::Resources), "the only frame is held");
    assert!(full.is_some_and(|e| e.msg.contains("STORAGE.POOL_BYTES")), "names what to raise");
    assert_eq!(pool.free(file, a).err().map(|e| e.code), Some(Code::Api), "held");
    drop(held);
    assert_eq!(get(&pool, file, b).unwrap(), b"b");
    pool.free(file, b).unwrap();
    assert_eq!(pool.read(file, b.0).err().map(|e| e.code), Some(Code::Api), "freed in the pager");
    assert_eq!(get(&pool, file, a).unwrap(), b"a");
}

/// How long a read that must be refused at once is given before the test
/// takes it to be spinning: far more than any refusal takes.
pub(crate) const REFUSAL_LIMIT: Duration = Duration::from_secs(10);

/// Verifies: [DESIGN] RULES.md D30, D38, buffer pool: a page read by its
/// hint holds its frame's latch and no pin; with that the only frame,
/// another page is refused at once (SQLCODE -918), as with a pin.
///
/// Regression test (R-TEST-7): the frame looked evictable, being unpinned,
/// but could not be latched, and the read wanting it spun for ever.
#[test]
fn a_page_read_by_its_hint_holds_its_frame() {
    let scratch = Scratch::new("buffer-hinted-held");
    let pool = Arc::new(pool(1));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let (a, b) = (put(&pool, file, b"a"), put(&pool, file, b"b"));
    let (sender, receiver) = mpsc::channel();
    let reader = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let held = pool.read(file, b.0).unwrap();
            assert_eq!((&*held, pool.counts().misses), (&b"b"[..], 0_u64), "a hit: by its hint");
            sender.send(pool.read(file, a.0).err().map(|e| e.code)).unwrap();
        })
    };
    let refused = receiver.recv_timeout(REFUSAL_LIMIT).expect("refused at once, not spinning");
    assert_eq!(refused, Some(Code::Resources), "the only frame is held");
    reader.join().unwrap();
    assert_eq!(get(&pool, file, a).unwrap(), b"a", "released, the frame is taken");
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: `free_when_released` of a
/// page the calling thread holds read by its hint, latched and unpinned,
/// waits a bounded while, then is refused (SQLCODE -910), not waiting for
/// itself for ever; released, the page is freed.
///
/// Regression test (R-TEST-7): it yielded and looked again for ever.
#[test]
fn freeing_a_page_its_caller_holds_by_hint_is_refused() {
    let scratch = Scratch::new("buffer-free-hinted");
    let pool = Arc::new(pool(2));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let a = put(&pool, file, b"a");
    let (sender, receiver) = mpsc::channel();
    let freer = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let held = pool.read(file, a.0).unwrap();
            assert_eq!((&*held, pool.counts().misses), (&b"a"[..], 0_u64), "a hit: by its hint");
            sender.send(pool.free_when_released(file, a).err().map(|e| e.code)).unwrap();
        })
    };
    let refused = receiver.recv_timeout(REFUSAL_LIMIT).expect("refused in the end, not waiting for ever");
    assert_eq!(refused, Some(Code::Api), "held by the caller");
    freer.join().unwrap();
    pool.free_when_released(file, a).unwrap();
    assert_eq!(pool.read(file, a.0).err().map(|e| e.code), Some(Code::Api), "freed");
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: `free_when_released` of a
/// page the calling thread holds pinned, read on a miss, waits a bounded
/// while, then is refused (SQLCODE -910) naming the page, not waiting for
/// itself for ever; a page another thread pins for a moment is waited
/// for, and freed once released.
///
/// Regression test (R-TEST-7): a page pinned was waited for without
/// limit, so a caller holding a pin on the page it freed never returned.
#[test]
fn freeing_a_page_its_caller_pins_is_refused() {
    let scratch = Scratch::new("buffer-free-pinned");
    let pool = Arc::new(pool(2));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let a = put(&pool, file, b"a");
    let (b, c) = (put(&pool, file, b"b"), put(&pool, file, b"c"));
    let (sender, receiver) = mpsc::channel();
    let freer = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let misses = pool.counts().misses;
            let held = pool.read(file, a.0).unwrap();
            assert_eq!((&*held, pool.counts().misses), (&b"a"[..], misses + 1), "a miss: pinned");
            sender.send(pool.free_when_released(file, a).err().map(|e| (e.code, e.msg))).unwrap();
        })
    };
    let refused = receiver.recv_timeout(REFUSAL_LIMIT).expect("refused in the end, not waiting for ever");
    let (code, msg) = refused.expect("refused");
    assert_eq!(code, Code::Api, "held by the caller");
    assert!(msg.contains(&format!("page {}", a.0)), "{msg}");
    freer.join().unwrap();
    let (sender, receiver) = mpsc::channel();
    let holder = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let misses = pool.counts().misses;
            let held = pool.read(file, b.0).unwrap();
            assert_eq!((&*held, pool.counts().misses), (&b"b"[..], misses + 1), "a miss: pinned");
            sender.send(()).unwrap();
            thread::sleep(LATCH_MOMENT);
        })
    };
    receiver.recv_timeout(REFUSAL_LIMIT).unwrap();
    pool.free_when_released(file, b).unwrap();
    holder.join().unwrap();
    assert_eq!(pool.read(file, b.0).err().map(|e| e.code), Some(Code::Api), "freed");
    assert_eq!(get(&pool, file, c).unwrap(), b"c");
}

/// How long a thread holds a page read by its hint, in a test where
/// another waits for its latch: a moment, well within a claim's wait.
const LATCH_MOMENT: Duration = Duration::from_millis(1);

/// Verifies: [DESIGN] RULES.md D30, D38, buffer pool: with the only frame
/// latched by another thread's read by its hint, unpinned, a read of
/// another page waits for the latch to be let go, a moment later, and then
/// takes the frame, rather than being refused (SQLCODE -918).
#[test]
fn a_claim_waits_for_a_frame_latched_for_a_moment() {
    let scratch = Scratch::new("buffer-hinted-moment");
    let pool = Arc::new(pool(1));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let (a, b) = (put(&pool, file, b"a"), put(&pool, file, b"b"));
    let (sender, receiver) = mpsc::channel();
    let holder = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            let held = pool.read(file, b.0).unwrap();
            assert_eq!((&*held, pool.counts().misses), (&b"b"[..], 0_u64), "a hit: by its hint");
            sender.send(()).unwrap();
            thread::sleep(LATCH_MOMENT);
        })
    };
    receiver.recv_timeout(REFUSAL_LIMIT).unwrap();
    assert_eq!(get(&pool, file, a).unwrap(), b"a", "waited for, the frame is taken");
    holder.join().unwrap();
}

/// Threads each reading its own page over and over, by its hint, one
/// fewer than the pool's frames; the pages one more thread reads in turn,
/// none of them in the pool; and the rounds of each.
const HOT_READERS: u8 = 4;
const COLD_PAGES: u8 = 8;
const HOT_ROUNDS: usize = 2000;
const COLD_ROUNDS: usize = 300;
/// How long a hot reader holds its page, and then lets it go, each round:
/// half the time latched.
const HOT_HOLD: Duration = Duration::from_micros(50);
/// How long the threads are given before the test takes them to be stuck.
const STUCK_LIMIT: Duration = Duration::from_secs(120);

/// One page read by its hint, held for `HOT_HOLD`, then let go as long.
fn read_hot(pool: &BufferPool, file: FileId, id: PageId, content: u8) {
    let page = pool.read(file, id.0).unwrap();
    assert_eq!(&*page, &[content]);
    thread::sleep(HOT_HOLD);
    drop(page);
    thread::sleep(HOT_HOLD);
}

/// Verifies: [DESIGN] RULES.md D30, D38, buffer pool: threads holding
/// pages read by their hints, latched for a moment and unpinned, while
/// another reads pages not in the pool, claiming frames, and each, its
/// page evicted, claims one again; with as many frames as threads, no read
/// is refused (no SQLCODE -918) and none hangs; every page reads as
/// written.
#[test]
fn claims_wait_for_frames_latched_by_hinted_reads() {
    let scratch = Scratch::new("buffer-hinted-busy");
    let pool = Arc::new(pool(usize::from(HOT_READERS) + 1));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let cold: Vec<PageId> = (0..COLD_PAGES).map(|n| put(&pool, file, &[HOT_READERS + n])).collect();
    let hot: Vec<PageId> = (0..HOT_READERS).map(|n| put(&pool, file, &[n])).collect();
    let (sender, receiver) = mpsc::channel();
    let mut threads: Vec<_> = (0..HOT_READERS)
        .zip(hot)
        .map(|(n, id)| {
            let (pool, sender) = (Arc::clone(&pool), sender.clone());
            thread::spawn(move || {
                (0..HOT_ROUNDS).for_each(|_| read_hot(&pool, file, id, n));
                sender.send(()).unwrap();
            })
        })
        .collect();
    threads.push({
        let pool = Arc::clone(&pool);
        thread::spawn(move || {
            for round in 0..COLD_ROUNDS {
                let n = round % cold.len();
                assert_eq!(get(&pool, file, cold[n]).unwrap(), [HOT_READERS + u8::try_from(n).unwrap()]);
            }
            sender.send(()).unwrap();
        })
    });
    for _ in 0..threads.len() {
        receiver.recv_timeout(STUCK_LIMIT).expect("every thread done, none refused or stuck");
    }
    for t in threads {
        t.join().unwrap();
    }
    assert!(pool.counts().evictions > 0, "the cold reader claimed frames");
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: a block file's blocks are
/// cached for reading, a second read being a hit; they cannot be written
/// through the pool, nor pages made in a block file (SQLCODE -910); an
/// unregistered file and a pool without frames are refused.
#[test]
fn block_files_are_cached_for_reading() {
    let scratch = Scratch::new("buffer-blocks");
    let blocks =
        Arc::new(BlockFile::create(&OsFileSystem, &scratch.path("b"), crc32c(), Pipeline::none(), BlockOptions::default()).unwrap());
    let first = blocks.append(b"first block").unwrap();
    let second = blocks.append(b"second block").unwrap();
    let pool = pool(4);
    let file = pool.add_blocks(Arc::clone(&blocks));
    assert_eq!(&*pool.read(file, second.0).unwrap(), b"second block");
    assert_eq!(&*pool.read(file, first.0).unwrap(), b"first block");
    assert_eq!(&*pool.read(file, second.0).unwrap(), b"second block");
    assert_eq!((pool.counts().misses, pool.counts().hits), (2, 1));
    assert_eq!(blocks.counts().reads, 2, "the hit did not read the file");
    assert_eq!(pool.write(file, PageId(first.0)).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(pool.new_page(file).err().map(|e| e.code), Some(Code::Api));
    pool.sync(file, Durability::Data).unwrap();
    assert_eq!(pool.read(FileId(9), 0).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(BufferPool::new(0, Box::new(Clock::new(0))).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: when writing back an
/// evicted dirty page fails, the read that needed its frame fails with
/// that error, and the page stays in the pool, dirty, with its content, to
/// be written back later; when a page cannot be read (corrupt), the read
/// fails and the pool stays usable.
#[test]
fn failed_write_backs_and_loads_leave_the_pool_consistent() {
    let scratch = Scratch::new("buffer-failures");
    let path = scratch.path("p");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &path);
    let pool = pool(1);
    let file = pool.add_pager(Arc::clone(&pager));
    let b = put(&pool, file, b"b");
    pool.flush().unwrap();
    let a = put(&pool, file, b"a, not yet written");
    faults.inject(Rule::fail(Op::Write).at_or_past(4096));
    assert!(pool.read(file, b.0).err().unwrap().msg.contains("injected"));
    assert_eq!(get(&pool, file, a).unwrap(), b"a, not yet written", "still in the pool");
    faults.clear();
    assert_eq!(get(&pool, file, b).unwrap(), b"b", "a written back, b read");
    let mut content = Vec::new();
    pager.read(a, &mut content).unwrap();
    assert_eq!(content, b"a, not yet written");
    // Corrupt page b on disk, and evict it.
    let mut bytes = std::fs::read(&path).unwrap();
    let at = usize::try_from((b.0 + 1) * 4096 + 30).unwrap();
    bytes[at] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(get(&pool, file, a).unwrap(), b"a, not yet written");
    assert_eq!(get(&pool, file, b).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(get(&pool, file, a).unwrap(), b"a, not yet written", "the pool still works");
}

/// Threads using the pool at once, and pages each owns.
const THREADS: u8 = 4;
const PAGES_EACH: u8 = 8;
const ROUNDS: u8 = 20;

/// Verifies: [DESIGN] RULES.md D30, buffer pool: threads reading and
/// writing their own pages through a pool smaller than all their pages, at
/// once, see their own writes, and every page ends as last written.
#[test]
fn threads_share_a_small_pool() {
    let scratch = Scratch::new("buffer-threads");
    let pool = Arc::new(pool(6));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("p")));
    let workers: Vec<_> = (0..THREADS)
        .map(|t| {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                let ids: Vec<PageId> = (0..PAGES_EACH).map(|_| put(&pool, file, &[t, 0])).collect();
                for round in 1..=ROUNDS {
                    for &id in &ids {
                        let mut page = pool.write(file, id).unwrap();
                        assert_eq!(page.as_slice(), &[t, round - 1]);
                        page[1] = round;
                    }
                }
                ids
            })
        })
        .collect();
    let all: Vec<(u8, Vec<PageId>)> = workers.into_iter().enumerate().map(|(t, w)| (u8::try_from(t).unwrap(), w.join().unwrap())).collect();
    for (t, ids) in all {
        for id in ids {
            assert_eq!(get(&pool, file, id).unwrap(), vec![t, ROUNDS]);
        }
    }
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: a dirty page whose frame
/// another thread takes is found again only once it is written back, so a
/// thread wanting it while the write-back runs waits for it and reads what
/// was last written, not the file's older copy.
#[test]
fn a_page_being_written_back_is_waited_for() {
    let scratch = Scratch::new("buffer-write-back-race");
    let (fs, faults) = FaultyFs::with_faults();
    let pool = Arc::new(pool(3));
    let file = pool.add_pager(pager(&*fs, &scratch.path("p")));
    let (p, q, s) = (put(&pool, file, b"old"), put(&pool, file, b"q"), put(&pool, file, b"s"));
    pool.flush().unwrap();
    pool.write(file, p).unwrap().copy_from_slice(b"new");
    // Q and S held, so P's frame is the one a new page takes; its write-back held up.
    let (held_q, held_s) = (pool.read(file, q.0).unwrap(), pool.read(file, s.0).unwrap());
    let offset = (p.0 + 1) * 4096;
    faults.inject(Rule::delay(Duration::from_millis(300)).at_or_past(offset).below(offset + 4096).times(1));
    let taker = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || pool.new_page(file).map(|(id, _)| id).unwrap())
    };
    while faults.injected() == 0 {
        thread::yield_now();
    }
    drop(held_s);
    assert_eq!(get(&pool, file, p).unwrap(), b"new", "not the file's older copy");
    drop(held_q);
    taker.join().unwrap();
    assert_eq!(get(&pool, file, p).unwrap(), b"new");
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: `flush` returns only once
/// a dirty page another thread is writing back, to take its frame, is in
/// the file; `free` of such a page waits for it too.
#[test]
fn flush_waits_for_write_backs_under_way() {
    let scratch = Scratch::new("buffer-flush-race");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &scratch.path("p"));
    let pool = Arc::new(pool(1));
    let file = pool.add_pager(Arc::clone(&pager));
    let p = put(&pool, file, b"old");
    pool.flush().unwrap();
    pool.write(file, p).unwrap().copy_from_slice(b"new");
    let offset = (p.0 + 1) * 4096;
    faults.inject(Rule::delay(Duration::from_millis(300)).at_or_past(offset).below(offset + 4096).times(1));
    let taker = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || drop(pool.new_page(file).unwrap()))
    };
    while faults.injected() == 0 {
        thread::yield_now();
    }
    pool.flush().unwrap();
    let mut content = Vec::new();
    pager.read(p, &mut content).unwrap();
    assert_eq!(content, b"new");
    taker.join().unwrap();
    // A page freed while it is written back is freed once it is.
    pool.write(file, p).unwrap().copy_from_slice(b"end");
    faults.inject(Rule::delay(Duration::from_millis(300)).at_or_past(offset).below(offset + 4096).times(1));
    let taker = {
        let pool = Arc::clone(&pool);
        thread::spawn(move || drop(pool.new_page(file).unwrap()))
    };
    while faults.injected() == 1 {
        thread::yield_now();
    }
    pool.free(file, p).unwrap();
    taker.join().unwrap();
    assert_eq!(get(&pool, file, p).err().map(|e| e.code), Some(Code::Api), "freed");
}
