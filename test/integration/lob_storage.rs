//! Large objects kept by a database in a directory (RULES.md D25, D79):
//! a value of many chunks logged a chunk at a time and recovered after a
//! crash, before and after a checkpoint; a transaction open at the crash
//! leaves nothing; a value written back unchanged shares its chunks; and a
//! value much larger than the buffer pool is written and read through it;
//! and a snapshot reads the value it saw, though it was replaced.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, after_crash, number, rows, small_pool};

/// The database in `dir`, of a pool of 64 pages, checkpointing only when
/// asked.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 64);
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    (db, opened)
}

/// A text of `n` characters, a three-octet one every seventh, so chunks'
/// ends fall within characters; `seed` the others.
fn text(n: usize, seed: char) -> String {
    (0..n).map(|i| if i % 7 == 0 { '€' } else { seed }).collect()
}

/// Inserts row `k` of T holding `c`, as a parameter.
fn insert(s: &Session, k: i64, c: &str) {
    let p = s.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    s.execute_prepared(&p, &[Value::Exact(Decimal::new(i128::from(k), 0)), Value::Lob(Lob::of_text(c))]).unwrap();
}

/// The text of row `k`'s CLOB.
fn read(s: &Session, k: i64) -> String {
    let row = s.open_cursor(&format!("SELECT C FROM T WHERE K = {k}")).unwrap().fetch().unwrap().unwrap();
    let Value::Lob(c) = &row[0] else { panic!("{row:?}") };
    String::from_utf8(c.reader().read(0, c.octets()).unwrap()).unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, D79: values of many chunks committed
/// before a checkpoint and after it are back after a crash, whole; one
/// updated after it to another value is the new one, and one written back
/// unchanged keeps its value; a row whose transaction was open at the
/// crash is not there, nor one rolled back; the database reopened again
/// gives the same.
#[test]
fn large_objects_are_recovered_after_a_crash() {
    let scratch = Scratch::new("lob-crash");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    crashed.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB)").unwrap();
    let a = crashed.session("A");
    let (before, after, changed) = (text(700_000, 'a'), text(500_000, 'b'), text(300_000, 'c'));
    insert(&a, 1, &before);
    insert(&a, 2, &before);
    a.commit().unwrap();
    crashed.checkpoint().unwrap();
    insert(&a, 3, &after);
    a.commit().unwrap();
    let p = a.prepare("UPDATE T SET C = ? WHERE K = 2").unwrap();
    a.execute_prepared(&p, &[Value::Lob(Lob::of_text(&changed))]).unwrap();
    a.run("UPDATE T SET K = K + 10 WHERE K = 1").unwrap();
    a.commit().unwrap();
    insert(&a, 4, &after);
    a.rollback().unwrap();
    let open_tx = crashed.session("A");
    insert(&open_tx, 5, &after);

    for _ in 0..2 {
        let (db, opened) = open(&dir);
        assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
        let s = db.session("A");
        assert_eq!(rows(&s, "SELECT K, CHAR_LENGTH(C) FROM T ORDER BY K").unwrap(), ["2,300000", "3,500000", "11,700000"]);
        assert_eq!(read(&s, 11), before, "written back unchanged");
        assert_eq!(read(&s, 2), changed, "updated after the checkpoint");
        assert_eq!(read(&s, 3), after, "inserted after the checkpoint");
        drop((s, db));
    }
    drop((a, open_tx, crashed));
}

/// Verifies: [DESIGN] RULES.md D79: a value of 24 MiB, 96 chunks, is
/// written and read through a buffer pool of 512 KiB, never held whole in
/// it, and read back by ranges; a checkpoint and a reopen keep it; it is
/// appended to, and folded to the temporary store, a chunk at a time.
#[test]
fn a_value_larger_than_the_pool_passes_through_it() {
    let scratch = Scratch::new("lob-large");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB)").unwrap();
    let a = db.session("A");
    let big = text(24 << 20, 'z');
    insert(&a, 1, &big);
    a.commit().unwrap();
    db.checkpoint().unwrap();
    drop((a, db));
    let (db, _) = open(&dir);
    let s = db.session("A");
    assert_eq!(rows(&s, "SELECT OCTET_LENGTH(C) FROM T").unwrap(), [big.len().to_string()]);
    let row = s.open_cursor("SELECT C FROM T").unwrap().fetch().unwrap().unwrap();
    let Value::Lob(c) = &row[0] else { panic!("{row:?}") };
    let mut reader = c.reader();
    for at in [0_u64, 262_143, 5 << 20, c.octets() - 7] {
        let start = usize::try_from(at).unwrap();
        assert_eq!(reader.read(at, 7).unwrap(), big.as_bytes()[start..(start + 7).min(big.len())], "at {at}");
    }
    drop(row);
    // Appended, its chunks shared with the old value's; folded, written a
    // chunk at a time to the temporary store.
    s.run("UPDATE T SET C = C || 'end'").unwrap();
    s.commit().unwrap();
    let tail = "SELECT OCTET_LENGTH(C), CAST(SUBSTRING(C FROM CHAR_LENGTH(C) - 4) AS VARCHAR(5)) FROM T";
    assert_eq!(rows(&s, tail).unwrap(), [format!("{},zzend", big.len() + 3)]);
    assert_eq!(rows(&s, "SELECT OCTET_LENGTH(UPPER(C)) FROM T WHERE UPPER(C) LIKE '€Z%ZZEND'").unwrap(), [(big.len() + 3).to_string()]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.41.6] (RULES.md D29, D79) a
/// SERIALIZABLE transaction reads the value of many chunks its snapshot saw,
/// though another replaced it, committed, and later commits collected what no
/// other snapshot reads; once it ends, the old value's objects are collected
/// and a new transaction reads the new value.
#[test]
fn a_snapshot_reads_the_value_it_saw() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB)").unwrap();
    let (old, new) = (text(500_000, 'o'), text(400_000, 'n'));
    let writer = db.session("A");
    insert(&writer, 1, &old);
    writer.commit().unwrap();
    let reader = db.session("A");
    reader.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    assert_eq!(read(&reader, 1), old);
    let p = writer.prepare("UPDATE T SET C = ? WHERE K = 1").unwrap();
    writer.execute_prepared(&p, &[Value::Lob(Lob::of_text(&new))]).unwrap();
    writer.commit().unwrap();
    writer.run("UPDATE T SET K = K").unwrap();
    writer.commit().unwrap();
    db.maintain().unwrap();
    assert_eq!(read(&reader, 1), old, "its snapshot's");
    reader.commit().unwrap();
    writer.run("UPDATE T SET K = K").unwrap();
    writer.commit().unwrap();
    assert_eq!(read(&reader, 1), new);
}
