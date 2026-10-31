//! How fast prepared statements run: a query of one row by an indexed
//! key, an UPDATE of one row by it, and an INSERT of one row, each
//! prepared once and executed many times with new values; beside the
//! same query as direct SQL text, planned anew each time; a positioned
//! UPDATE prepared, through a cursor opened on the row; a dynamic
//! cursor's query opened by the key; and the query in a session that
//! declared a local temporary table. Run with `cargo xtask bench`, on the
//! harness (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::cell::Cell;

use harness::{Bench, Case};
use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

/// Rows in the table.
const ROWS: u64 = 100_000;
/// Executions in one run of a measurement.
const EXECUTIONS: u64 = 20_000;

fn main() {
    let bench = Bench::new("prepared");
    let db = database();
    let s = db.session("B");
    by_key(&bench, &s);
    through_cursors(&bench, &s);
    declared(&bench, &db);
    inserts(&bench, &s);
}

/// The query and the UPDATE of one row by its key, prepared, and the
/// query as direct SQL text.
fn by_key(bench: &Bench, s: &Session) {
    let query = s.prepare("SELECT NAME FROM T WHERE K = ?").unwrap();
    bench.measure(Case::new("query one row by its key", "executions"), || {
        for i in 0..EXECUTIONS {
            let mut cursor = s.open_prepared(&query, &[int(harness::scatter(i, ROWS))]).unwrap();
            cursor.fetch().unwrap().unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
    bench.measure(Case::new("query one row by its key, direct", "executions"), || {
        for i in 0..EXECUTIONS {
            let k = harness::scatter(i, ROWS);
            let mut cursor = s.open_cursor(&format!("SELECT NAME FROM T WHERE K = {k}")).unwrap();
            cursor.fetch().unwrap().unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
    let update = s.prepare("UPDATE T SET N = N + 1 WHERE K = ?").unwrap();
    bench.measure(Case::new("update one row by its key", "executions"), || {
        for i in 0..EXECUTIONS {
            s.execute_prepared(&update, &[int(harness::scatter(i, ROWS))]).unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
}

/// A positioned UPDATE prepared, through a cursor opened on the row; a
/// dynamic cursor's query, opened by the key.
fn through_cursors(bench: &Bench, s: &Session) {
    let mut p = host();
    s.declare_cursor("DECLARE P CURSOR FOR SELECT K FROM T WHERE K = KV").unwrap();
    let positioned = s.prepare("UPDATE T SET N = N + 1 WHERE CURRENT OF P").unwrap();
    bench.measure(Case::new("update one row through a cursor", "executions"), || {
        for i in 0..EXECUTIONS {
            p.set("KV", int(harness::scatter(i, ROWS)));
            s.execute("OPEN P", &mut p).unwrap();
            s.execute("FETCH P INTO FK", &mut p).unwrap();
            s.execute_prepared(&positioned, &[]).unwrap();
            s.execute("CLOSE P", &mut p).unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
    p.set("TEXT", Value::Char("SELECT NAME FROM T WHERE K = ?".to_owned()));
    s.execute("PREPARE S FROM TEXT", &mut p).unwrap();
    s.declare_cursor("DECLARE C CURSOR FOR S").unwrap();
    bench.measure(Case::new("open a dynamic cursor by its key", "executions"), || {
        for i in 0..EXECUTIONS {
            p.set("KV", int(harness::scatter(i, ROWS)));
            s.execute("OPEN C USING KV", &mut p).unwrap();
            s.execute("FETCH C INTO NM", &mut p).unwrap();
            s.execute("CLOSE C", &mut p).unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
}

/// The query prepared, in a session that declared a local temporary
/// table.
fn declared(bench: &Bench, db: &Database) {
    let declared = db.session("B");
    declared.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (K INTEGER)").unwrap();
    let by_key = declared.prepare("SELECT NAME FROM T WHERE K = ?").unwrap();
    bench.measure(Case::new("query one row by its key, a table declared", "executions"), || {
        for i in 0..EXECUTIONS {
            let mut cursor = declared.open_prepared(&by_key, &[int(harness::scatter(i, ROWS))]).unwrap();
            cursor.fetch().unwrap().unwrap();
        }
        declared.commit().unwrap();
        EXECUTIONS
    });
}

/// An INSERT of one row, prepared.
fn inserts(bench: &Bench, s: &Session) {
    let insert = s.prepare("INSERT INTO U VALUES (?, ?)").unwrap();
    let next = Cell::new(0_u64);
    bench.measure(Case::new("insert one row", "executions"), || {
        for _ in 0..EXECUTIONS {
            let k = next.get();
            next.set(k + 1);
            s.execute_prepared(&insert, &[int(k), Value::Char(format!("name {k}"))]).unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
}

/// The parameters of the statements in SQL text: TEXT, a statement's;
/// KV, a key; FK and NM, a FETCH's targets.
fn host() -> Params {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    Params::new()
        .declare("TEXT", DataType::Varying { length: 100 }, Value::Char(String::new()))
        .declare("KV", integer, int(0))
        .declare("FK", integer, int(0))
        .declare("NM", DataType::Character { length: 20 }, Value::Char(String::new()))
}

/// `k` as an INTEGER's value.
fn int(k: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(k), 0))
}

/// Schema B with table T of `ROWS` rows, indexed by its key, and an
/// empty table U, in memory.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B \
         CREATE TABLE T (K INTEGER NOT NULL, NAME CHAR(20), N INTEGER) \
         CREATE TABLE U (K INTEGER NOT NULL, NAME CHAR(20))",
    )
    .unwrap();
    let s = db.session("B");
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'name {k}', 0)")).unwrap();
    }
    s.commit().unwrap();
    db.create_index("T_K", "B.T", &["K"], "btree").unwrap();
    db
}
