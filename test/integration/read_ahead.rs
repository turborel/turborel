//! The buffer pool's read-ahead (RULES.md D30): the pages after a miss
//! read into the pool in one read of the file, bounded by a share of its
//! frames and by the pages it is told to read at once, the database's
//! setting STORAGE.READ_AHEAD_PAGES; pages it cannot
//! take whole, frames it cannot have, pages in the
//! pool already, and a failed read left to the reads that want them; a
//! B-link tree's forward scan reading ahead of leaves laid out in order;
//! and a database's read-ahead job reading what its scans ask for.

use std::ops::Bound;
use std::path::Path;
use std::sync::Arc;

use turborel::disk::block::{BlockFile, BlockOptions};
use turborel::disk::buffer::{AheadReader, BufferPool, Clock, FileId, READ_AHEAD_PAGES};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, FileSystem, OsFileSystem};
use turborel::disk::pager::{PageId, Pager};
use turborel::engine::tree::{BLinkTree, Direction};
use turborel::error::Code;
use turborel::threads::{JobPolicy, JobState};
use turborel::value::{Decimal, Value};

use crate::common::{RunLength, Scratch, crc32c, rows};
use crate::faults::{FaultyFs, Op, Rule};

fn pager(fs: &dyn FileSystem, path: &Path) -> Arc<Pager> {
    Arc::new(Pager::create(fs, path, crc32c(), Pipeline::none(), 4096).unwrap())
}

fn pool(frames: usize) -> BufferPool {
    BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap()
}

/// `contents` written to `pager` through a pool of their own, flushed:
/// their pages, in order.
fn written(pager: &Arc<Pager>, contents: &[Vec<u8>]) -> Vec<PageId> {
    let pool = pool(contents.len().max(1));
    let file = pool.add_pager(Arc::clone(pager));
    let ids = contents
        .iter()
        .map(|content| {
            let (id, mut page) = pool.new_page(file).unwrap();
            page.extend_from_slice(content);
            id
        })
        .collect();
    pool.flush().unwrap();
    ids
}

fn get(pool: &BufferPool, file: FileId, id: PageId) -> Vec<u8> {
    pool.read(file, id.0).unwrap().to_vec()
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: a read-ahead from a page
/// not in the pool reads the pages after it in one read of the file, at
/// most an eighth of the pool's frames, each then read from the pool, a
/// hit; none from a page in the pool already; the pages it reads at once
/// are `READ_AHEAD_PAGES` until set, and none once set to 0.
#[test]
fn pages_are_read_ahead_in_one_read() {
    let scratch = Scratch::new("read-ahead");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &scratch.path("p"));
    let ids = written(&pager, &(0..20_u8).map(|n| vec![n; 100]).collect::<Vec<_>>());
    let pool = pool(64);
    let file = pool.add_pager(Arc::clone(&pager));
    assert_eq!(pool.read_ahead_pages(), READ_AHEAD_PAGES);
    // One read of the file, and no more.
    faults.inject(Rule::fail(Op::Read).after(1));
    assert_eq!(pool.read_ahead(file, ids[0].0), 8, "an eighth of 64 frames");
    faults.clear();
    assert_eq!((pool.counts().read_ahead, pool.counts().misses), (8, 0));
    for (n, id) in (0..8_u8).zip(&ids) {
        assert_eq!(get(&pool, file, *id), vec![n; 100]);
    }
    assert_eq!((pool.counts().hits, pool.counts().misses), (8, 0), "read from the pool");
    assert_eq!(pool.read_ahead(file, ids[0].0), 0, "its first page in the pool");
    assert_eq!(pool.read_ahead(file, ids[8].0), 8);
    pool.set_read_ahead(0);
    assert_eq!((pool.read_ahead_pages(), pool.read_ahead(file, ids[16].0)), (0, 0));
    pool.set_read_ahead(2);
    assert_eq!(pool.read_ahead(file, ids[16].0), 2);
    assert_eq!(get(&pool, file, ids[17]), vec![17_u8; 100]);
    assert_eq!(pool.counts().read_ahead, 18);
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: a read-ahead takes only
/// pages it can take whole from what it read: not a page whose content
/// goes on in overflow pages, nor an overflow page, a page freed, or one
/// past the file's end; a page in the pool, changed there, it leaves as it
/// is. Each is read as it is afterwards.
#[test]
fn a_read_ahead_leaves_what_it_cannot_take() {
    let scratch = Scratch::new("read-ahead-leaves");
    let pager = pager(&OsFileSystem, &scratch.path("p"));
    let long = vec![9_u8; 10_000];
    let ids = written(&pager, &[b"first".to_vec(), long.clone(), b"freed".to_vec(), b"held".to_vec()]);
    let pool = pool(64);
    let file = pool.add_pager(Arc::clone(&pager));
    pool.free(file, ids[2]).unwrap();
    pool.write(file, ids[3]).unwrap().extend_from_slice(b", changed");
    assert_eq!(pool.read_ahead(file, ids[0].0), 1, "the first page alone");
    assert_eq!(get(&pool, file, ids[0]), b"first");
    assert_eq!(pool.counts().misses, 1, "the page changed, read to change it");
    assert_eq!(get(&pool, file, ids[1]), long, "read when wanted, overflow and all");
    assert_eq!(get(&pool, file, ids[3]), b"held, changed");
    assert_eq!(pool.counts().misses, 2);
    let mut past = pager.read_ahead(&[PageId(pager.page_count() + 1)]).unwrap();
    assert_eq!(past.pop(), Some(None), "past the file's end");
}

/// Verifies: [DESIGN] RULES.md D30, buffer pool: a read-ahead whose read of
/// the file fails reads nothing and leaves the pool as it was: each page
/// is read when wanted, once the file reads again.
#[test]
fn a_failed_read_ahead_reads_nothing() {
    let scratch = Scratch::new("read-ahead-fails");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &scratch.path("p"));
    let ids = written(&pager, &[b"a".to_vec(), b"b".to_vec()]);
    let pool = pool(16);
    let file = pool.add_pager(Arc::clone(&pager));
    faults.inject(Rule::fail(Op::Read));
    assert_eq!(pool.read_ahead(file, ids[0].0), 0);
    faults.clear();
    assert_eq!((pool.counts().read_ahead, pool.counts().misses), (0, 0));
    assert_eq!((get(&pool, file, ids[0]), get(&pool, file, ids[1])), (b"a".to_vec(), b"b".to_vec()));
    assert_eq!(pool.counts().misses, 2);
}

/// Verifies: [DESIGN] RULES.md D30, D38, buffer pool: a read-ahead stops at
/// the first frame it cannot have: none when every frame is held, refused
/// (SQLCODE -918) for a read, never for it; none when the dirty page a
/// frame holds cannot be written back, which stays in the pool, dirty; none
/// of a block file. A page torn on the file is left to the read that wants
/// it, refused then (SQLCODE -916); a page of an encoded file is decoded.
#[test]
fn a_read_ahead_stops_where_it_cannot_go_on() {
    let scratch = Scratch::new("read-ahead-stops");
    let (fs, faults) = FaultyFs::with_faults();
    let pager = pager(&*fs, &scratch.path("p"));
    let ids = written(&pager, &(0..12_u8).map(|n| vec![n; 10]).collect::<Vec<_>>());
    let pool = pool(8);
    let file = pool.add_pager(Arc::clone(&pager));
    let held: Vec<_> = ids[..8].iter().map(|id| pool.write(file, *id).unwrap()).collect();
    assert_eq!(pool.read_ahead(file, ids[8].0), 0, "every frame held");
    assert_eq!(pool.read(file, ids[8].0).err().map(|e| e.code), Some(Code::Resources));
    drop(held);
    faults.inject(Rule::fail(Op::Write));
    assert_eq!(pool.read_ahead(file, ids[8].0), 0, "a dirty page not written back");
    faults.clear();
    assert_eq!(pool.counts().write_backs, 0);
    pool.flush().unwrap();
    assert_eq!(pool.counts().write_backs, 8, "the dirty pages kept");

    let blocks =
        Arc::new(BlockFile::create(&OsFileSystem, &scratch.path("b"), crc32c(), Pipeline::none(), BlockOptions::default()).unwrap());
    let block = blocks.append(b"block").unwrap();
    let of_blocks = pool.add_blocks(blocks);
    assert_eq!(pool.read_ahead(of_blocks, block.0), 0, "a block file");

    let torn = scratch.path("torn");
    let torn_pager = pager_on(&torn, Pipeline::none());
    let ids = written(&torn_pager, &[b"a".to_vec(), b"b".to_vec()]);
    torn_pager.sync(Durability::Data).unwrap();
    drop(torn_pager);
    // Page 1, at the third page-sized slot, after the header's.
    let mut bytes = std::fs::read(&torn).unwrap();
    bytes[2 * 4096 + 10] ^= 1;
    std::fs::write(&torn, bytes).unwrap();
    // Two pages read ahead at once, of 16 frames.
    let wide = self::pool(16);
    let reopened = wide.add_pager(Arc::new(Pager::open(&OsFileSystem, &torn, &[crc32c()], &[]).unwrap().0));
    assert_eq!(wide.read_ahead(reopened, ids[0].0), 1, "the page torn left");
    assert_eq!(wide.read(reopened, ids[1].0).err().map(|e| e.code), Some(Code::Corrupt));

    let encoded = pager_on(&scratch.path("encoded"), Pipeline::new(vec![Arc::new(RunLength)]).unwrap());
    let ids = written(&encoded, &[vec![5_u8; 3000]]);
    let of_encoded = wide.add_pager(encoded);
    assert_eq!(wide.read_ahead(of_encoded, ids[0].0), 1);
    assert_eq!(get(&wide, of_encoded, ids[0]), vec![5_u8; 3000]);
}

/// A new pager file at `path`, its pages encoded by `pipeline`.
fn pager_on(path: &Path, pipeline: Pipeline) -> Arc<Pager> {
    Arc::new(Pager::create(&OsFileSystem, path, crc32c(), pipeline, 4096).unwrap())
}

/// Verifies: [DESIGN] RULES.md D30, D31, B-link tree: a tree filled in key
/// order, larger than its pool, is scanned forward whole and in order,
/// its leaves read ahead; with read-ahead set to none, the same scan
/// reads none ahead; a backward scan reads none ahead.
#[test]
fn a_forward_scan_reads_ahead_of_its_leaves() {
    let scratch = Scratch::new("read-ahead-tree");
    let pool = Arc::new(pool(64));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("tree")));
    let tree = BLinkTree::create(Arc::clone(&pool), file).unwrap();
    let entries = 20_000_u64;
    for n in 0..entries {
        tree.insert(&n.to_be_bytes(), &[7_u8; 40]).unwrap();
    }
    let keys = |direction| -> Vec<u64> {
        let scan = tree.scan(Bound::Unbounded, Bound::Unbounded, direction);
        scan.map(|entry| u64::from_be_bytes(entry.unwrap().0.try_into().unwrap())).collect()
    };
    let before = pool.counts().read_ahead;
    assert_eq!(keys(Direction::Forward), (0..entries).collect::<Vec<_>>());
    assert!(pool.counts().read_ahead > before, "leaves read ahead");
    pool.set_read_ahead(0);
    let none = pool.counts().read_ahead;
    assert_eq!(keys(Direction::Forward).len(), 20_000_usize);
    pool.set_read_ahead(READ_AHEAD_PAGES);
    assert_eq!(keys(Direction::Backward), (0..entries).rev().collect::<Vec<_>>());
    assert_eq!(pool.counts().read_ahead, none, "none read ahead");
}

/// STORAGE.READ_AHEAD_PAGES as INFORMATION_SCHEMA.SETTINGS shows it: its
/// value, where it comes from, and when a change takes effect.
const READ_AHEAD: &str = "SELECT TRIM(SETTING_VALUE), TRIM(VALUE_SOURCE), TRIM(TAKES_EFFECT) \
                          FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'STORAGE.READ_AHEAD_PAGES'";

/// Verifies: [DESIGN] RULES.md D30, D36: the pages a scan reads ahead are
/// the database's setting STORAGE.READ_AHEAD_PAGES, `READ_AHEAD_PAGES`
/// by default, taken at once; set to none, or to more than
/// a pool's share, a table is read whole all the same; a value out of its
/// range is refused (SQLCODE -910); the value set holds when the
/// database is opened again.
#[test]
fn the_pages_read_ahead_are_a_setting() {
    let scratch = Scratch::new("read-ahead-setting");
    let dir = scratch.path("db");
    let open = || {
        let db = turborel::db::Database::new();
        db.open_files(&crate::common::os(), &dir, &turborel::db::OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, V CHARACTER(200))").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (0..2000).map(|k| format!("({k}, 'v{k}')")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, READ_AHEAD), Ok(vec![format!("{READ_AHEAD_PAGES},DEFAULT,STATEMENT")]));
    let set = |pages: i128| db.set_setting("STORAGE.READ_AHEAD_PAGES", &Value::Exact(Decimal::new(pages, 0))).map_err(|e| e.code);
    for pages in [0, 1_000_000] {
        set(pages).unwrap();
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec!["2000".to_owned()]), "{pages} pages");
    }
    assert_eq!(set(-1), Err(Code::Api));
    set(0).unwrap();
    s.commit().unwrap();
    db.close().unwrap();
    drop(db);
    let again = open();
    assert_eq!(rows(&again.session("S"), READ_AHEAD), Ok(vec!["0,DATABASE,STATEMENT".to_owned()]));
}

/// A tree of `entries` entries filled in key order through `pool`, of more
/// pages than its frames, so the first leaves are on the file alone.
fn tree_in(pool: &Arc<BufferPool>, file: FileId, entries: u64) -> BLinkTree {
    let tree = BLinkTree::create(Arc::clone(pool), file).unwrap();
    for n in 0..entries {
        tree.insert(&n.to_be_bytes(), &[7_u8; 40]).unwrap();
    }
    tree
}

/// The key of a tree's entry.
fn key_of(entry: turborel::error::Result<(Vec<u8>, Vec<u8>)>) -> u64 {
    u64::from_be_bytes(entry.unwrap().0.try_into().unwrap())
}

/// Verifies: [DESIGN] RULES.md D30, D55: in a pool a reader serves, a
/// forward scan asks for the leaves after the one it reads and reads on,
/// none read ahead in its thread; the reader's job (here run by the test,
/// as under JOBS.POLICY MANUAL) reads them into the pool, and the scan
/// finds them there: hits, no misses, and the whole tree in order.
#[test]
fn a_scan_finds_pages_the_job_read_ahead() {
    let scratch = Scratch::new("read-ahead-job");
    let pool = Arc::new(pool(64));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("tree")));
    let tree = tree_in(&pool, file, 20_000);
    let reader = AheadReader::new();
    reader.serve(&pool);
    let mut scan = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
    let first: Vec<u64> = scan.by_ref().take(1000).map(key_of).collect();
    assert_eq!(pool.counts().read_ahead, 0, "asked, none read in the scan's thread");
    let read = reader.read_queued();
    // At most an eighth of 64 frames queued.
    assert!((1..=8).contains(&read), "{read} read");
    let (misses, hits) = (pool.counts().misses, pool.counts().hits);
    let next: Vec<u64> = scan.by_ref().take(50).map(key_of).collect();
    assert_eq!((pool.counts().misses, pool.counts().read_ahead), (misses, u64::try_from(read).unwrap()), "found in the pool");
    assert!(pool.counts().hits > hits);
    let rest: Vec<u64> = scan.map(key_of).collect();
    assert_eq!([first, next, rest].concat(), (0..20_000).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] RULES.md D30, D55, D38: a scan ending before the
/// job reads what it asked has it passed over: the job reads none of it;
/// one ending after leaves the pages read unpinned. Either way the pool's
/// every frame can then be held at once.
#[test]
fn a_scan_ended_early_leaves_nothing_pinned() {
    let scratch = Scratch::new("read-ahead-ended");
    let pool = Arc::new(pool(64));
    let file = pool.add_pager(pager(&OsFileSystem, &scratch.path("tree")));
    let tree = tree_in(&pool, file, 20_000);
    let reader = AheadReader::new();
    reader.serve(&pool);
    let mut read = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
    assert_eq!(read.by_ref().take(1000).count(), 1000);
    let done = reader.read_queued();
    assert!(done > 0);
    drop(read);
    let mut ended = tree.scan(Bound::Excluded(&10_000_u64.to_be_bytes()[..]), Bound::Unbounded, Direction::Forward);
    assert_eq!(ended.by_ref().take(1000).count(), 1000);
    drop(ended);
    assert_eq!(reader.read_queued(), 0, "passed over");
    assert_eq!(pool.counts().read_ahead, u64::try_from(done).unwrap());
    let pages = pool.pages_in_use(file).unwrap();
    let held: Vec<_> = (1..pages).take(64).map(|id| pool.write(file, PageId(id)).unwrap()).collect();
    assert_eq!(held.len(), 64_usize, "every frame held");
}

/// Verifies: [DESIGN] RULES.md D30, D55: a read of the file the job makes
/// that fails reads nothing and fails no scan: the scan reads those pages
/// itself, once the file reads again, and gives the whole tree.
#[test]
fn a_failed_read_of_the_job_fails_no_scan() {
    let scratch = Scratch::new("read-ahead-job-fails");
    let (fs, faults) = FaultyFs::with_faults();
    let pool = Arc::new(pool(64));
    let file = pool.add_pager(pager(&*fs, &scratch.path("tree")));
    let tree = tree_in(&pool, file, 20_000);
    let reader = AheadReader::new();
    reader.serve(&pool);
    let mut scan = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
    let first: Vec<u64> = scan.by_ref().take(1000).map(key_of).collect();
    faults.inject(Rule::fail(Op::Read));
    assert_eq!(reader.read_queued(), 0);
    assert!(faults.injected() > 0, "the job's read failed");
    faults.clear();
    let misses = pool.counts().misses;
    let rest: Vec<u64> = scan.map(key_of).collect();
    assert_eq!([first, rest].concat(), (0..20_000).collect::<Vec<_>>());
    assert!(pool.counts().misses > misses, "read by the scan");
}

/// The job reading ahead of a database's scans.
const JOB: &str = "turborel-read-ahead";

/// Verifies: [DESIGN] RULES.md D30, D55, D31: a database kept in a
/// directory, its pool smaller than its tables, has its scans' pages read
/// ahead by its job `turborel-read-ahead`: under JOBS.POLICY MANUAL, a
/// scan of a row store table, and of a column store one's frozen groups,
/// asks for it, due until the program runs it; the scans answer whole.
/// Under AUTOMATIC the job has a thread of its own.
#[test]
fn a_database_reads_ahead_on_its_job() {
    let scratch = Scratch::new("read-ahead-database");
    let db = turborel::db::Database::new();
    db.set_setting("STORAGE.POOL_BYTES", &Value::Exact(Decimal::new(1 << 20, 0))).unwrap();
    db.open_files(&crate::common::os(), &scratch.path("db"), &turborel::db::OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE R (K INTEGER, V CHARACTER(200))
           CREATE TABLE C (K INTEGER, V CHARACTER(200)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let s = db.session("S");
    for table in ["R", "C"] {
        for first in (0..20_000).step_by(1000) {
            let values: Vec<String> = (first..first + 1000).map(|k| format!("({k}, 'v{k}')")).collect();
            s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    db.run_due().unwrap();
    let state = |of: &turborel::db::Database| of.jobs().into_iter().find(|j| j.label == JOB).unwrap();
    for table in ["R", "C"] {
        assert_eq!(state(&db).state, JobState::Idle, "{table}");
        let mut cursor = s.open_cursor(&format!("SELECT K, V FROM {table}")).unwrap();
        let mut read = 0_u32;
        while cursor.fetch().unwrap().is_some() {
            read += 1;
        }
        assert_eq!((read, state(&db).state), (20_000, JobState::Due), "{table}");
        let runs = state(&db).runs;
        db.run_due().unwrap();
        assert_eq!((state(&db).state, state(&db).runs), (JobState::Idle, runs + 1), "{table}");
    }
    assert_eq!(rows(&s, "SELECT COUNT(*), SUM(K) FROM C WHERE V <> 'x'"), Ok(vec!["20000,199990000".to_owned()]));
    s.commit().unwrap();
    db.set_job_policy(JobPolicy::Automatic).unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*), SUM(K) FROM R WHERE V <> 'x'"), Ok(vec!["20000,199990000".to_owned()]));
    assert!(state(&db).threaded, "a thread of its own");
}
