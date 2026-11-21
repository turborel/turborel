//! \[DESIGN\] Index entries an update keeps (RULES.md D443): an update
//! whose new version holds every column an index's key reads as the
//! version it replaced shares that version's entry, so the index keeps one
//! entry per run of versions with a key, which every snapshot finds the row
//! through, and which collection, a rollback, an index built beside the
//! writers and recovery from a checkpoint all leave exact.

use std::path::Path;
use std::thread;

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::params::Params;

use crate::common::{Scratch, after_crash, count, number, rows, small_pool};

/// The rows of T.
const ROWS: usize = 200;

/// Schema APP: T (ID, X, Y), an index on X, `ROWS` rows of X = ID, Y = 0.
fn define(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER, Y INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    let values: Vec<String> = (0..ROWS).map(|i| format!("({i}, {i}, 0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
}

/// A database with `define`'s schema, in memory.
fn setup() -> Database {
    let db = Database::new();
    define(&db);
    db
}

/// A session reading at one snapshot from its first statement.
fn reader(db: &Database) -> Session {
    let s = db.session("APP");
    s.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    s
}

/// Ends a transaction on T with none other open, so every version no one
/// reads is collected, with its index entries.
fn settle(db: &Database) {
    let s = db.session("APP");
    rows(&s, "SELECT ID FROM T WHERE ID = 0").unwrap();
    s.commit().unwrap();
}

/// The Y of the rows `s` finds through T_X with X = `x`, the plan checked
/// to read the index.
fn by_x(s: &Session, x: i64) -> Vec<String> {
    let sql = format!("SELECT Y FROM T WHERE X = {x}");
    let plan = s.physical_plan(&sql, &Params::new()).unwrap();
    assert!(plan.contains("T_X"), "the index is read: {plan}");
    rows(s, &sql).unwrap()
}

/// Verifies: [DESIGN] RULES.md D443, D34: updates keeping X leave row 7
/// found through T_X by a reader at the snapshot before them and by one
/// after; once both end and the old versions are collected, the row has one
/// entry, and a change of X then leaves one entry, of the new key, as each
/// update had added its own.
#[test]
fn a_kept_key_is_found_from_every_snapshot() {
    let db = setup();
    let (old, s) = (reader(&db), db.session("APP"));
    assert_eq!(by_x(&old, 7), vec!["0".to_owned()]);
    for _ in 0..2 {
        s.run("UPDATE T SET Y = Y + 1 WHERE ID = 7").unwrap();
        s.commit().unwrap();
    }
    assert_eq!(by_x(&old, 7), vec!["0".to_owned()], "the old snapshot, through the shared entry");
    assert_eq!(by_x(&s, 7), vec!["2".to_owned()]);
    old.commit().unwrap();
    settle(&db);
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS);
    assert_eq!(by_x(&s, 7), vec!["2".to_owned()], "the entry outlives the versions that shared it");
    s.run("UPDATE T SET X = 1000 WHERE ID = 7").unwrap();
    s.commit().unwrap();
    settle(&db);
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS, "no entry of the old key left");
    assert_eq!(by_x(&s, 7), Vec::<String>::new());
    assert_eq!(by_x(&s, 1000), vec!["2".to_owned()]);
}

/// Verifies: [DESIGN] RULES.md D443, D34: a key changed, changed back and
/// then kept, across three transactions: each snapshot finds the row under
/// the key it had then, and under no other; after all end, one entry.
#[test]
fn a_key_changed_and_back_is_found_by_each_snapshot() {
    let db = setup();
    let s = db.session("APP");
    let mut readers = Vec::new();
    for sql in ["UPDATE T SET X = 8000 WHERE ID = 7", "UPDATE T SET X = 7 WHERE ID = 7", "UPDATE T SET Y = 5 WHERE ID = 7"] {
        let r = reader(&db);
        count(&r, "SELECT ID FROM T WHERE ID = 7");
        readers.push(r);
        s.run(sql).unwrap();
        s.commit().unwrap();
    }
    for (r, expected) in readers.iter().zip([(1, 0), (0, 1), (1, 0)]) {
        assert_eq!((by_x(r, 7).len(), by_x(r, 8000).len()), expected);
    }
    for r in &readers {
        r.commit().unwrap();
    }
    settle(&db);
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS);
    assert_eq!((by_x(&s, 7), by_x(&s, 8000).len()), (vec!["5".to_owned()], 0));
}

/// Verifies: [DESIGN] RULES.md D443, [X3.135-1986 §8.9 GR1]: an update
/// keeping X rolled back, and one rolled back to a savepoint after a change
/// of X, leave the index as it was: each kept key's entry stays, each added
/// one goes.
#[test]
fn kept_keys_survive_rollbacks() {
    let db = setup();
    let s = db.session("APP");
    s.run("UPDATE T SET Y = 1 WHERE ID = 7").unwrap();
    s.rollback().unwrap();
    s.run("UPDATE T SET Y = 2 WHERE ID = 7").unwrap();
    s.run("SAVEPOINT P").unwrap();
    s.run("UPDATE T SET X = 9000 WHERE ID = 7").unwrap();
    s.run("UPDATE T SET Y = 3 WHERE ID = 7").unwrap();
    s.run("ROLLBACK TO SAVEPOINT P").unwrap();
    s.run("UPDATE T SET Y = 4 WHERE ID = 7").unwrap();
    s.commit().unwrap();
    settle(&db);
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS);
    assert_eq!((by_x(&s, 7), by_x(&s, 9000).len()), (vec!["4".to_owned()], 0));
}

/// Verifies: [DESIGN] RULES.md D443, D27: an index built after updates
/// that kept its columns, while a reader still holds their old versions,
/// counts each version, and collection takes each back; an index that was
/// there shares them. A change of the key then leaves one entry in each.
/// Without the fill's mark, the new index kept one entry too many.
#[test]
fn an_index_built_over_kept_versions_counts_each() {
    let db = setup();
    let (old, s) = (reader(&db), db.session("APP"));
    count(&old, "SELECT ID FROM T WHERE ID = 7");
    for _ in 0..2 {
        s.run("UPDATE T SET Y = Y + 1 WHERE ID = 7").unwrap();
        s.commit().unwrap();
    }
    db.create_index("T_X2", "APP.T", &["X"], "btree").unwrap();
    old.commit().unwrap();
    settle(&db);
    s.run("UPDATE T SET X = 1000 WHERE ID = 7").unwrap();
    s.commit().unwrap();
    settle(&db);
    assert_eq!((db.index_entries("T_X").unwrap(), db.index_entries("T_X2").unwrap()), (ROWS, ROWS));
}

/// Verifies: [DESIGN] RULES.md D443, D27: an index built while writers go
/// on updating a column it does not read finds, once they end, exactly the
/// rows a scan does, with one entry a row.
#[test]
fn an_index_built_beside_kept_updates_is_exact() {
    let db = setup();
    let threads: Vec<_> = (0..3)
        .map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("APP");
                for n in 0..150 {
                    let sql = format!("UPDATE T SET Y = Y + 1 WHERE ID = {}", (n * 7 + t) % ROWS);
                    while s.run(&sql).and_then(|_| s.commit()).is_err() {
                        let _ = s.rollback();
                    }
                }
            })
        })
        .collect();
    db.create_index("T_X2", "APP.T", &["X"], "btree").unwrap();
    for t in threads {
        t.join().unwrap();
    }
    settle(&db);
    let s = db.session("APP");
    s.run("UPDATE T SET X = X + 1000").unwrap();
    s.commit().unwrap();
    settle(&db);
    assert_eq!((db.index_entries("T_X").unwrap(), db.index_entries("T_X2").unwrap()), (ROWS, ROWS));
    assert_eq!(rows(&s, "SELECT SUM(Y) FROM T"), Ok(vec!["450".to_owned()]));
}

/// The database in `dir`, with a small buffer pool, checkpointed only when
/// asked.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 32);
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    (db, opened)
}

/// Verifies: [DESIGN] RULES.md D443, D25: a checkpoint taken while a
/// reader holds versions that share their entries logs none of those
/// entries as stale, so recovery from it takes back none a live version
/// holds: every row is found through the index after the crash.
#[test]
fn a_crash_keeps_shared_entries() {
    let scratch = Scratch::new("kept-entries-crash");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    define(&crashed);
    let old = reader(&crashed);
    count(&old, "SELECT ID FROM T WHERE ID = 7");
    let s = crashed.session("APP");
    for _ in 0..3 {
        s.run("UPDATE T SET Y = Y + 1 WHERE ID < 50").unwrap();
        s.commit().unwrap();
    }
    crashed.checkpoint().unwrap();
    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS);
    let s = db.session("APP");
    for x in [0, 7, 49] {
        assert_eq!(by_x(&s, x), vec!["3".to_owned()]);
    }
    drop((db, old, crashed));
}

/// Verifies: [DESIGN] RULES.md D447: a UNIQUE constraint added in a
/// transaction, while an older snapshot keeps row 7's versions, X changed
/// away and back, and row 8 changed by the transaction itself, keeps an
/// entry of each row once the old versions are collected.
#[test]
fn a_constraint_added_in_a_transaction_keeps_its_entries_after_collection() {
    let db = setup();
    let (old, s) = (reader(&db), db.session("APP"));
    assert_eq!(by_x(&old, 7), vec!["0".to_owned()]);
    for x in [1007, 7] {
        s.run(&format!("UPDATE T SET X = {x} WHERE ID = 7")).unwrap();
        s.commit().unwrap();
    }
    s.run("UPDATE T SET Y = 5 WHERE ID = 8").unwrap();
    s.run("ALTER TABLE T ADD CONSTRAINT T_X_UNIQUE UNIQUE (X)").unwrap();
    s.commit().unwrap();
    old.commit().unwrap();
    settle(&db);
    let (sql, name) = ("SELECT Y FROM T WHERE X = 8", "APP.T.UNIQUE1");
    assert_eq!(db.index_entries(name).unwrap(), ROWS, "one entry of each row");
    assert_eq!(rows(&s, sql).unwrap(), vec!["5".to_owned()]);
    let err = s.run("INSERT INTO T VALUES (500, 7, 0)").unwrap_err();
    assert!(err.to_string().contains("23505") || err.to_string().to_lowercase().contains("unique"), "{err}");
}
