//! \[EXTENSION\] Indexes created on tables that transactions go on reading
//! and writing (`Database::create_index`, RULES.md D27): the index has one
//! entry for every version a reader may read or garbage collection will
//! take back, however the build and the writers interleave.

use std::thread;

use turborel::db::Database;
use turborel::error::Code;
use turborel::params::Params;

use crate::common::{big, count, exec, rows, setup};

/// Ends a transaction on T of `db` with none other open, so every version
/// no one reads is collected, with its index entries.
fn settle(db: &Database) {
    let s = db.session("APP");
    rows(&s, "SELECT ID FROM T").unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] a transaction that wrote the table before its
/// index was created is refused (SQLCODE -911) at its next statement and at
/// its commit, which rolls it back, as the index has no entries for its
/// rows; one that began after maintains the index; readers and writers of
/// other tables go on.
#[test]
fn earlier_writers_run_again() {
    let db = setup();
    let (early, reader, other) = (db.session("APP"), db.session("APP"), db.session("APP"));
    early.run("UPDATE T SET F = 9 WHERE ID = 1").unwrap();
    other.run("INSERT INTO U VALUES (7, 'Z')").unwrap();
    assert_eq!(count(&reader, "SELECT ID FROM T"), 3);
    db.create_index("T_F", "APP.T", &["F"], "btree").unwrap();
    assert_eq!(exec(&early, "DELETE FROM U WHERE ID = 99").map(drop), Err(Code::Conflict), "its next statement, even on U");
    assert_eq!(early.commit().map_err(|e| e.code), Err(Code::Conflict), "its commit");
    assert_eq!(rows(&early, "SELECT F FROM T WHERE ID = 1"), Ok(vec!["1.5E0".into()]), "rolled back");
    early.run("UPDATE T SET F = 9 WHERE ID = 1").unwrap();
    early.commit().unwrap();
    other.commit().unwrap();
    assert_eq!(count(&reader, "SELECT ID FROM T WHERE F = 9"), 0, "the reader goes on, at its snapshot");
    reader.commit().unwrap();
    assert_eq!(count(&reader, "SELECT ID FROM T WHERE F = 9"), 1);
    settle(&db);
    assert_eq!(db.index_entries("T_F").unwrap(), 2, "rows 1 and 2; row 3's F is null");
}

/// Verifies: [EXTENSION] the build counts the versions that garbage
/// collection will take back but no reader can read any more: an old
/// version left behind when the reader that kept it ended on another table.
/// Regression test: the build counted only versions a reader could read,
/// so collecting the old version took back the entry of the row's newer
/// version, and the index lost the row.
#[test]
fn lingering_old_versions_are_counted() {
    let db = setup();
    let (s, reader) = (db.session("APP"), db.session("APP"));
    rows(&reader, "SELECT CODE FROM U").unwrap();
    s.run("UPDATE T SET NAME = 'NEW' WHERE ID = 1").unwrap();
    s.commit().unwrap(); // the reader may read row 1's old version
    reader.commit().unwrap(); // which no one reads now, but U's end did not collect it
    db.create_index("T_F", "APP.T", &["F"], "hash").unwrap();
    s.run("UPDATE T SET NAME = 'NEWER' WHERE ID = 2").unwrap();
    s.commit().unwrap(); // collects row 1's old version
    settle(&db);
    assert_eq!(db.index_entries("T_F").unwrap(), 2, "rows 1 and 2 each have their entry");
}

/// Verifies: [EXTENSION] an index built while threads update and insert
/// rows of its table, running again what is refused, finds every row by
/// its value, and once old versions are collected has exactly one entry
/// per row: none missing, none counted twice by the build and a writer.
#[test]
fn an_index_is_built_while_its_table_is_written() {
    let db = big();
    let threads: Vec<_> = (0..4_i64)
        .map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("APP");
                for n in 0..200_i64 {
                    let sql = if n % 4 == 0 {
                        format!("INSERT INTO T VALUES ({}, 'W', 0, {})", 1000 + t * 1000 + n, n % 9)
                    } else {
                        format!("UPDATE T SET F = {} WHERE ID = {}", (n + t) % 9, 1 + (n * 7 + t) % 60)
                    };
                    while s.run(&sql).and_then(|_| s.commit()).is_err() {
                        let _ = s.rollback();
                    }
                }
            })
        })
        .collect();
    db.create_index("T_F", "APP.T", &["F"], "btree").unwrap();
    for t in threads {
        t.join().unwrap();
    }
    settle(&db);
    let s = db.session("APP");
    let plan = s.physical_plan("SELECT ID FROM T WHERE F = 3", &Params::new()).unwrap();
    assert!(plan.contains("T_F"), "the index is read: {plan}");
    for f in 0..9 {
        let by_index = count(&s, &format!("SELECT ID FROM T WHERE F = {f}"));
        let by_scan = count(&s, &format!("SELECT ID FROM T WHERE F + 0 = {f}"));
        assert_eq!(by_index, by_scan, "F = {f}");
    }
    let with_f = count(&s, "SELECT ID FROM T WHERE F IS NOT NULL");
    assert_eq!(db.index_entries("T_F").unwrap(), with_f, "one entry per row");
}

/// Verifies: [EXTENSION] RULES.md D34, `Database::drop_index`: a dropped
/// index is no longer read, and the answers are the same; a reader that
/// planned with it before goes on; its name may be used again. Refused: an
/// index that does not exist, one enforcing a UNIQUE constraint
/// [X3.135-1986 §6.4 GR2], and, under no-wait locking, one whose table a
/// transaction is writing (SQLCODE -911), until it ends.
#[test]
fn indexes_are_dropped() {
    let db = big();
    let s = db.session("APP");
    let query = "SELECT ID FROM T WHERE NAME = 'N7'";
    assert!(s.physical_plan(query, &Params::new()).unwrap().contains("T_NAME"));
    let before = rows(&s, query);
    let reader = db.session("APP");
    let mut cursor = reader.open_cursor("SELECT ID FROM T WHERE NAME = 'N7'").unwrap();
    s.run("UPDATE T SET F = 1 WHERE ID = 1").unwrap();
    assert_eq!(db.drop_index("T_NAME").map_err(|e| e.code), Err(Code::Conflict), "T is being written");
    s.commit().unwrap();
    db.drop_index("T_NAME").unwrap();
    assert!(cursor.fetch().unwrap().is_some(), "the reader goes on");
    drop(cursor);
    assert!(!s.physical_plan(query, &Params::new()).unwrap().contains("T_NAME"));
    assert_eq!(rows(&s, query), before);
    assert_eq!(db.drop_index("T_NAME").map_err(|e| e.code), Err(Code::Api), "no longer exists");
    assert_eq!(db.drop_index("APP.T.UNIQUE0").map_err(|e| e.code), Err(Code::Api), "a UNIQUE constraint's");
    db.create_index("T_NAME", "APP.T", &["NAME"], "hash").unwrap();
    assert_eq!(rows(&s, query), before);
}
