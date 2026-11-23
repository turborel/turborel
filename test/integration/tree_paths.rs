//! The B-link tree's paths of RULES.md D840: writers that find their leaf
//! from its parent and change it through the frame they own, readers that
//! search nodes over their slots and descend through inner copies kept
//! with their headers parsed, all at once in a pool far smaller than the
//! tree, so frames are given to other pages while they run; and a table
//! and its indexes changed so, checkpointed and crashed, found again.

use std::ops::Bound;
use std::path::Path;
use std::sync::Arc;
use std::thread;

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::Pager;
use turborel::engine::tree::{BLinkTree, Direction};

use crate::common::{Scratch, after_crash, crc32c, number, small_pool, sorted};

/// The frames of the pool the threads share: a small part of the tree's
/// pages, so owned and latched frames meet evictions.
const FRAMES: usize = 96;

fn key(n: u64) -> Vec<u8> {
    format!("key {n:08}").into_bytes()
}

/// A value of `n` that grows with `round`, so a replace may not fit its
/// leaf and splits it.
fn value(n: u64, round: u64) -> Vec<u8> {
    format!("{n} {}", "v".repeat(usize::try_from((n + round * 13) % 60).unwrap())).into_bytes()
}

/// `0..n` in a scattered order: 7919 is prime, so coprime to `n` unless a
/// multiple.
fn scattered(n: u64) -> impl Iterator<Item = u64> {
    (0..n).map(move |i| (i * 7919) % n)
}

/// Verifies: [DESIGN] RULES.md D840, D31, D814: in a pool of a few frames
/// of a tree larger than the pool, writers insert their own keys in a scattered
/// order, replace them with longer and shorter values, so leaves split
/// and are built again, and remove most again, so leaves merge and are
/// taken out, each finding its leaf from its parent and writing through
/// the frame it owns; meanwhile readers find keys that never change, and
/// scan both ways, seeing keys strictly in order, never one twice. At the
/// end the tree holds exactly the keys and values left.
#[test]
fn writers_through_owned_frames_split_and_merge_under_readers() {
    let scratch = Scratch::new("tree-paths-threads");
    let pager = Arc::new(Pager::create(&OsFileSystem, &scratch.path("tree"), crc32c(), Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(FRAMES, Box::new(Clock::new(FRAMES))).unwrap());
    let file = pool.add_pager(pager);
    let tree = Arc::new(BLinkTree::create(pool, file).unwrap());
    let (writers, per) = (4_u64, 2500_u64);
    // Keys that stay, one in each writer's stretch, which the readers find.
    let stays = |n: u64| n.is_multiple_of(100);
    for n in (0..writers * per).filter(|&n| stays(n)) {
        assert!(tree.insert(&key(n), &value(n, 0)).unwrap());
    }
    let mut handles = Vec::new();
    for w in 0..writers {
        let tree = Arc::clone(&tree);
        handles.push(thread::spawn(move || {
            let mine = move |i: u64| w * per + i;
            let own = |i: &u64| !stays(mine(*i));
            for i in scattered(per).filter(own) {
                assert!(tree.insert(&key(mine(i)), &value(mine(i), 0)).unwrap());
            }
            for round in 1..4 {
                for i in scattered(per).filter(own) {
                    assert!(tree.replace(&key(mine(i)), &value(mine(i), round)).unwrap().is_some(), "key {}", mine(i));
                }
            }
            for i in scattered(per).filter(own).filter(|i| i % 5 != 0) {
                assert_eq!(tree.delete(&key(mine(i))).unwrap(), Some(value(mine(i), 3)));
            }
        }));
    }
    for reader in 0..3_u64 {
        let tree = Arc::clone(&tree);
        handles.push(thread::spawn(move || {
            for pass in 0..30_u64 {
                for n in (0..writers * per).filter(|&n| stays(n)).skip(usize::try_from(pass % 7).unwrap()).step_by(3) {
                    assert_eq!(tree.get(&key(n)).unwrap(), Some(value(n, 0)), "a key that stays");
                }
                let direction = if (pass + reader) % 2 == 0 { Direction::Forward } else { Direction::Backward };
                let keys: Vec<Vec<u8>> = tree.scan(Bound::Unbounded, Bound::Unbounded, direction).map(|e| e.unwrap().0).collect();
                let ordered = keys.windows(2).all(|p| if direction == Direction::Forward { p[0] < p[1] } else { p[0] > p[1] });
                assert!(ordered, "a scan saw keys out of order or twice");
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let counts = tree.counts();
    assert!(counts.splits > 0 && counts.unlinked > 0, "{counts:?}");
    assert!(tree.height().unwrap() >= 2, "a tree of inner nodes and leaves");
    let expected: Vec<(Vec<u8>, Vec<u8>)> = (0..writers * per)
        .filter(|&n| stays(n) || (n % per) % 5 == 0)
        .map(|n| (key(n), if stays(n) { value(n, 0) } else { value(n, 3) }))
        .collect();
    let found: Vec<(Vec<u8>, Vec<u8>)> = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).map(Result::unwrap).collect();
    assert_eq!(found.len(), expected.len());
    assert_eq!(found, expected);
}

/// The database in `dir`, with a small pool, so pages go to the data file
/// as it runs, and checkpoints only when asked.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 32);
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}

/// Rows `ids` of S, with quantities of `round`.
fn fill(s: &Session, ids: std::ops::Range<i32>, round: i32) {
    for id in ids.step_by(50) {
        let rows: Vec<String> = (id..id + 50)
            .map(|i| format!("({i}, {}, 'data {i:05} {}')", (i * 7 + round) % 100, "d".repeat(usize::try_from((i + round) % 40).unwrap())))
            .collect();
        s.run(&format!("INSERT INTO S VALUES {}", rows.join(", "))).unwrap();
    }
    s.commit().unwrap();
}

/// What the queries of the test answer in `s`.
fn answers(s: &Session) -> Vec<Vec<String>> {
    [
        "SELECT Q, D FROM S WHERE I = 777",
        "SELECT I FROM S WHERE I BETWEEN 1990 AND 2010",
        "SELECT I FROM S WHERE Q = 42 AND I < 600",
        "SELECT COUNT(*) FROM S",
        "SELECT SUM(Q) FROM S",
        "SELECT COUNT(*) FROM S WHERE Q < 10",
    ]
    .iter()
    .map(|q| sorted(s, q))
    .collect()
}

/// Verifies: [DESIGN] RULES.md D840, D25, D814: a table and its index
/// filled, so their leaves split, checkpointed; then updated in place and
/// to longer values, through the frames their writers own, rows deleted
/// so leaves merge, and more inserted, and checkpointed again mid-way;
/// then crashed. Opened again and recovered, every query answers as
/// before the crash, and a checkpoint of the recovered database and a
/// clean reopening answer the same.
#[test]
fn trees_changed_through_owned_frames_recover_after_checkpoints_and_a_crash() {
    let scratch = Scratch::new("tree-paths-crash");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    crashed.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE S (I INTEGER NOT NULL PRIMARY KEY, Q INTEGER, D VARCHAR(80))").unwrap();
    crashed.create_index("S_Q", "APP.S", &["Q"], "btree").unwrap();
    let s = crashed.session("APP");
    fill(&s, 0..3000, 0);
    crashed.checkpoint().unwrap();
    s.run("UPDATE S SET Q = Q + 1 WHERE I < 1500").unwrap();
    s.run("UPDATE S SET D = D || 'longer and longer' WHERE MOD(I, 3) = 0").unwrap();
    s.commit().unwrap();
    s.run("DELETE FROM S WHERE I BETWEEN 400 AND 1400").unwrap();
    s.commit().unwrap();
    crashed.checkpoint().unwrap();
    fill(&s, 3000..4000, 1);
    s.run("UPDATE S SET Q = 42 WHERE MOD(I, 11) = 0").unwrap();
    s.commit().unwrap();
    let expected = answers(&s);
    assert_eq!(expected[3], ["2999"], "4000 rows less 1001 deleted");

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let r = db.session("APP");
    assert_eq!(answers(&r), expected, "recovered");
    db.checkpoint().unwrap();
    drop((r, db));
    let (again, _) = open(&dir);
    assert_eq!(answers(&again.session("APP")), expected, "checkpointed after recovery and opened again");
    drop((s, crashed));
}
