//! Rows inserted together (RULES.md D530): an INSERT of many rows puts
//! them in its table a batch at a time, logged as one record a batch, its
//! index entries in key order. What it does is as if each row were
//! inserted alone: its constraints checked, its failure undoing all of it,
//! a rollback taking every row back; and a crash keeps the batches
//! committed and none other, rows and index entries alike.

use std::path::Path;

use turborel::db::{Database, Opened, Session};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, contents, count, exec, reopened_after_crash, rows};

/// More rows than a batch holds, so a statement inserts more than one.
const MANY: usize = 10_000;

/// The schema A: N, the digits; T, with a PRIMARY KEY, a NOT NULL column,
/// a CHECK and an index of a column whose values come in no order; and
/// `using`, an engine clause, for its store.
fn schema(db: &Database, using: &str) {
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    s.run("CREATE TABLE N (D INTEGER)").unwrap();
    s.run("INSERT INTO N VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9)").unwrap();
    let t = format!("CREATE TABLE T (K INTEGER PRIMARY KEY, V VARCHAR(20) NOT NULL, W INTEGER CHECK (W >= 0)) {using}");
    s.run(&t).unwrap();
    s.commit().unwrap();
    s.run("CREATE INDEX T_W ON T (W)").unwrap();
    s.commit().unwrap();
}

/// An INSERT of `MANY` rows of T, their keys from `from`: W a scramble of
/// the key, so its index is written out of key order.
fn insert_many(from: usize) -> String {
    format!(
        "INSERT INTO T SELECT {from} + a.D * 1000 + b.D * 100 + c.D * 10 + d.D, 'v' || CAST(d.D AS VARCHAR(2)), \
         MOD((a.D * 1000 + b.D * 100 + c.D * 10 + d.D) * 7919, 10007) FROM N a, N b, N c, N d"
    )
}

/// The rows of T found through its index of W, and by a scan.
fn found(s: &Session) -> (usize, usize) {
    (count(s, "SELECT K FROM T WHERE W >= 0"), count(s, "SELECT K FROM T"))
}

/// Verifies: [X3.135-1986 §8.7 GR1c, §4.5]; [EXTENSION] RULES.md D530:
/// the rows of an INSERT of a query and of many rows of values go in a
/// batch at a time, each found by a scan and through each index; a
/// duplicate key, within the rows or against one there, a null in a NOT
/// NULL column and a CHECK false each fail the statement, every row of it
/// undone; a rollback takes every row back. Of each store.
#[test]
fn rows_inserted_together_are_as_if_inserted_one_at_a_time() {
    for using in ["", "USING columnstore"] {
        let db = Database::new();
        schema(&db, using);
        let s = db.session("A");
        s.run(&insert_many(0)).unwrap();
        assert_eq!(found(&s), (MANY, MANY), "{using}");
        assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 4321").unwrap(), ["v1"], "{using}");
        s.run("INSERT INTO T VALUES (-1, 'a', 1), (-2, 'b', 2), (-3, 'c', 3)").unwrap();
        s.commit().unwrap();
        let before = found(&s);
        assert_eq!(exec(&s, &insert_many(5000)).err(), Some(Code::Constraint), "{using}: keys taken");
        assert_eq!(exec(&s, "INSERT INTO T VALUES (-4, 'a', 1), (-4, 'b', 2)").err(), Some(Code::Constraint), "{using}");
        assert_eq!(exec(&s, "INSERT INTO T VALUES (-5, 'a', 1), (-6, NULL, 2)").err(), Some(Code::Constraint), "{using}");
        assert_eq!(exec(&s, "INSERT INTO T VALUES (-7, 'a', 1), (-8, 'b', -2)").err(), Some(Code::Constraint), "{using}");
        assert_eq!(found(&s), before, "{using}: each failed statement undone whole");
        s.run(&insert_many(MANY)).unwrap();
        assert_eq!(found(&s), (MANY * 2 + 3, MANY * 2 + 3), "{using}");
        s.rollback().unwrap();
        assert_eq!(found(&s), before, "{using}: rolled back");
        s.run("INSERT INTO T VALUES (-4, 'a', 1)").unwrap();
        s.commit().unwrap();
    }
}

/// The load of `dir`'s database: two INSERTs of `MANY` rows committed,
/// then a third left open as it crashes. Its contents once the two are
/// committed, and its session, kept until it is reopened: a session
/// dropped first would commit the third.
fn load_crashing(dir: &Path, using: &str, checkpoint: bool) -> (Database, Session, Vec<String>) {
    let (crashed, _) = reopened_after_crash(dir);
    schema(&crashed, using);
    let s = crashed.session("A");
    s.run(&insert_many(0)).unwrap();
    s.commit().unwrap();
    if checkpoint {
        crashed.checkpoint().unwrap();
    }
    s.run(&insert_many(MANY)).unwrap();
    s.commit().unwrap();
    let committed = contents(&crashed, &["A"]);
    s.run(&insert_many(MANY * 2)).unwrap();
    (crashed, s, committed)
}

/// Verifies: [DESIGN] RULES.md D25, D530: a load crashed with a batch's
/// transaction open is recovered to its batches committed, their rows and
/// index entries, and none of the open one's, whose keys are free again;
/// recovered twice, from the log alone and from a checkpoint and the log
/// after it; of each store.
#[test]
fn a_load_crashed_midway_keeps_its_committed_batches() {
    for (using, checkpoint) in [("", false), ("", true), ("USING columnstore", false), ("USING columnstore", true)] {
        let scratch = Scratch::new("bulk-insert-crash");
        let dir = scratch.path("db");
        let (crashed, open, committed) = load_crashing(&dir, using, checkpoint);
        for _ in 0..2 {
            let (db, opened) = reopened_after_crash(&dir);
            assert!(matches!(opened, Opened::Recovered(_)), "{using}: {opened:?}");
            assert_eq!(contents(&db, &["A"]), committed, "{using}, checkpoint {checkpoint}");
            assert_eq!(found(&db.session("A")), (MANY * 2, MANY * 2), "{using}");
        }
        let (db, _) = reopened_after_crash(&dir);
        let s = db.session("A");
        s.run(&insert_many(MANY * 2)).unwrap();
        s.commit().unwrap();
        assert_eq!(found(&s), (MANY * 3, MANY * 3), "{using}: the open batch's keys free");
        drop((db, open, crashed));
    }
}

/// `n` as an exact numeric value.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Rows of T of keys `keys`, each its key's V and W.
fn given(keys: std::ops::Range<i128>) -> Vec<Vec<Value>> {
    keys.map(|k| vec![int(k), Value::Char(format!("g{}", k % 10)), int(k % 97)]).collect()
}

/// Verifies: [ISO/IEC 9075:1992 §17.9 GR5a, §13.8]; [EXTENSION] RULES.md
/// D530: a prepared INSERT of one row of dynamic parameters executed once
/// for many rows inserts each as its own execution would, each value cast
/// to its parameter's type, found by a scan
/// and through each index; a duplicate key or a CHECK false among them
/// fails it whole; a row of the wrong degree is refused, as is an
/// execution for no rows, and one of a statement of any other form. Of
/// each store.
#[test]
fn a_prepared_insert_given_many_rows_inserts_each() {
    for using in ["", "USING columnstore"] {
        let db = Database::new();
        schema(&db, using);
        let s = db.session("A");
        let insert = s.prepare("INSERT INTO T (K, V, W) VALUES (?, ?, ?)").unwrap();
        s.execute_prepared_rows(&insert, &given(0..10_000)).unwrap();
        assert_eq!(found(&s), (MANY, MANY), "{using}");
        assert_eq!(rows(&s, "SELECT V, W FROM T WHERE K = 4321").unwrap(), ["g1,53"], "{using}");
        // Cast to each parameter's type, as an execution casts its values.
        let spelled = [vec![Value::Char("77777".into()), Value::Char("c".into()), Value::Char("3".into())]];
        s.execute_prepared_rows(&insert, &spelled).unwrap();
        assert_eq!(rows(&s, "SELECT V, W FROM T WHERE K = 77777").unwrap(), ["c,3"], "{using}");
        let mut broken = given(20_000..20_010);
        broken.push(vec![int(5), Value::Char("dup".into()), int(1)]);
        let failed = s.execute_prepared_rows(&insert, &broken).map_err(|e| e.code);
        assert_eq!(failed, Err(Code::Constraint), "{using}: key 5 taken");
        let negative = vec![vec![int(30_000), Value::Char("n".into()), int(-1)]];
        assert_eq!(s.execute_prepared_rows(&insert, &negative).map_err(|e| e.code), Err(Code::Constraint), "{using}");
        assert_eq!(s.execute_prepared_rows(&insert, &[vec![int(1)]]).map_err(|e| e.code), Err(Code::Dynamic), "{using}");
        assert_eq!(s.execute_prepared_rows(&insert, &[]).map_err(|e| e.code), Err(Code::Dynamic), "{using}");
        let two = s.prepare("INSERT INTO T (K, V, W) VALUES (?, ?, ?), (?, ?, ?)").unwrap();
        let six = given(40_000..40_002).concat();
        assert_eq!(s.execute_prepared_rows(&two, &[six]).map_err(|e| e.code), Err(Code::Api), "{using}");
        let update = s.prepare("UPDATE T SET W = ? WHERE K = ?").unwrap();
        assert_eq!(s.execute_prepared_rows(&update, &[vec![int(1), int(1)]]).map_err(|e| e.code), Err(Code::Api), "{using}");
        assert_eq!(found(&s), (MANY + 1, MANY + 1), "{using}: each failed execution undone whole");
        s.commit().unwrap();
    }
}
