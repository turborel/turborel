//! Older row versions kept in memory (RULES.md D31, D745, D746): in a
//! database kept in files, readers of long snapshots and readers taking a
//! new one at each statement read the versions they see while writers on
//! many threads commit over them, whether the undo store's batches are
//! still in memory or were let go past the budget's share and are read from
//! the log; the batches kept are reserved of the budget within its share;
//! and a crash, with batches in memory lost, leaves what was committed.

use std::thread;

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, after_crash, exec, rows};

/// Rows of the table, and the writers, each changing its own share of them.
const ROWS: usize = 400;
const WRITERS: usize = 4;
/// The rows of each writer's share.
const PER: usize = 100;
/// Rounds each writer commits, each an update of every row of its share.
const ROUNDS: usize = 16;
/// What the undo store's reservation is listed as holding.
const KEPT: &str = "the undo store''s batches kept in memory";

/// A number setting's value.
fn number(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0))
}

/// A database kept in `dir`, its budget `budget` bytes with a pool of a
/// mebibyte, and a table `T (K, R, V)` of `ROWS` rows, `R` 0, made if
/// `make`.
fn database(dir: &std::path::Path, budget: usize, make: bool) -> (Database, Opened) {
    let db = Database::new();
    db.set_setting("MEMORY.BUDGET_BYTES", &number(budget)).unwrap();
    db.set_setting("STORAGE.POOL_BYTES", &number(1 << 20)).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    let _ = db.create_user("OWNER", b"x");
    let _ = db.set_owner("OWNER");
    if make {
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, R INTEGER, V CHARACTER(100))").unwrap();
        let s = db.session("A");
        let values: Vec<String> = (0..ROWS).map(|k| format!("({k}, 0, 'v')")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
        s.commit().unwrap();
    }
    (db, opened)
}

/// The rounds `s` sees in each writer's share of `T`: the least and
/// greatest `R` of each, which one snapshot sees equal.
fn rounds(s: &Session) -> Vec<String> {
    let per = PER;
    let share = |w: usize| format!("SELECT MIN(R), MAX(R), COUNT(*) FROM T WHERE K BETWEEN {} AND {}", w * per, (w + 1) * per - 1);
    (0..WRITERS).map(|w| rows(s, &share(w)).unwrap().concat()).collect()
}

/// Each share seen at round `round`.
fn at_round(round: usize) -> Vec<String> {
    vec![format!("{round},{round},{}", PER); WRITERS]
}

/// The bytes the undo store's batches kept hold of `db`'s budget.
fn kept_bytes(db: &Database) -> usize {
    let sql = format!("SELECT COALESCE(SUM(BYTES), 0) FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS WHERE RESERVED_BY = '{KEPT}'");
    rows(&db.session("OWNER"), &sql).unwrap()[0].parse().unwrap()
}

/// Runs `WRITERS` writers, each committing `ROUNDS` updates of its share
/// of `T`, `R` the round past `base`, while a reader taking a new snapshot at each
/// statement checks each share is seen at one round.
fn write_rounds(db: &Database, base: usize) {
    let per = PER;
    let writers: Vec<_> = (0..WRITERS)
        .map(|w| {
            let s = db.session("A");
            thread::spawn(move || {
                for round in base + 1..=base + ROUNDS {
                    let (from, to) = (w * per, (w + 1) * per - 1);
                    s.run(&format!("UPDATE T SET R = {round}, V = 'w{w} r{round}' WHERE K BETWEEN {from} AND {to}")).unwrap();
                    s.commit().unwrap();
                }
            })
        })
        .collect();
    let reader = db.session("A");
    while writers.iter().any(|w| !w.is_finished()) {
        for share in rounds(&reader) {
            let read: Vec<&str> = share.split(',').collect();
            assert_eq!((read[0], read[2]), (read[1], per.to_string().as_str()), "a share seen at one round: {share}");
        }
        reader.commit().unwrap();
    }
    for w in writers {
        w.join().unwrap();
    }
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D29, D745, D746: a
/// reader whose snapshot began before writers on four threads committed
/// sixteen rounds over every row sees the rows as they were, through
/// versions the undo store kept in memory and versions past the share of
/// a budget of 8 MiB, half a mebibyte, read back from the log; readers taking
/// a new snapshot at each statement see each commit whole; the batches kept
/// are reserved of the budget, never past its share; and once the reader
/// ends, it sees the last round.
#[test]
fn long_snapshots_read_old_versions_in_memory_and_from_the_log() {
    let scratch = Scratch::new("undo-memory-snapshots");
    let budget = 8 << 20;
    let (db, _) = database(&scratch.path("db"), budget, true);
    let long = db.session("A");
    exec(&long, "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    let before = rounds(&long);
    assert_eq!(before, at_round(0));
    write_rounds(&db, 0);
    let kept = kept_bytes(&db);
    assert!(kept > 0 && kept <= budget >> 4, "kept within the share: {kept}");
    assert_eq!(rounds(&long), before, "the long snapshot's rows, through every round");
    assert_eq!(rows(&long, "SELECT COUNT(*) FROM T WHERE V = 'v'").unwrap(), [ROWS.to_string()]);
    long.commit().unwrap();
    assert_eq!(rounds(&long), at_round(ROUNDS), "the last round");
}

/// Verifies: [X3.135-1986 §4.16, §8.2 GR3]; [DESIGN] RULES.md D25, D746:
/// a database that crashes while the undo store holds batches in memory,
/// a long snapshot open and a writer's change uncommitted, recovers with
/// every committed round and without the uncommitted change; the store
/// begins again with nothing kept, and new snapshots read old versions
/// again.
#[test]
fn a_crash_with_batches_kept_recovers_what_was_committed() {
    let scratch = Scratch::new("undo-memory-crash");
    let dir = scratch.path("db");
    let (db, _) = database(&dir, 64 << 20, true);
    let long = db.session("A");
    exec(&long, "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    rounds(&long);
    write_rounds(&db, 0);
    let open = db.session("A");
    open.run("UPDATE T SET R = -1 WHERE K < 10").unwrap();
    assert!(kept_bytes(&db) > 0, "batches kept as it crashes");
    // Still open at the crash: their sessions kept, as the files are
    // reopened.
    drop(db);
    let (db, opened) = database(&dir, 64 << 20, false);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let s = db.session("A");
    assert_eq!(rounds(&s), at_round(ROUNDS), "committed, and only that");
    s.commit().unwrap();
    exec(&s, "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    rounds(&s);
    write_rounds(&db, ROUNDS);
    assert_eq!(rounds(&s), at_round(ROUNDS), "its snapshot after recovery");
    s.commit().unwrap();
    assert_eq!(rounds(&s), at_round(2 * ROUNDS));
    drop((long, open));
}
