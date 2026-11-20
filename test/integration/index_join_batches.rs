//! Index joins probing a unique index by batches of their outer rows
//! (RULES.md D491): the rows, their order and their failures those of
//! probing row by row.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{exec, rows};

/// Schema B: table T of 5000 rows keyed by its PRIMARY KEY K, and table O
/// of 600 rows whose keys K repeat, are null in some rows and missing
/// from T in others, in no order.
fn batches() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE T (K INTEGER NOT NULL, N VARCHAR(12), CONSTRAINT T_K PRIMARY KEY (K))
           CREATE TABLE O (K INTEGER, V INTEGER)",
    )
    .unwrap();
    let s = db.session("B");
    let t: Vec<String> = (0..5000).map(|k| format!("({k}, 'n{k}')")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", t.join(", "))).unwrap();
    let key = |v: i64| if v % 17 == 0 { "NULL".to_owned() } else { format!("{}", (v * 7919) % 6000) };
    let o: Vec<String> = (0..600).map(|v| format!("({}, {v})", key(v % 450))).collect();
    s.run(&format!("INSERT INTO O VALUES {}", o.join(", "))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The physical plan EXPLAIN shows of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    s.explain(sql, &Params::new()).unwrap().split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The rows of `sql` in order, or its failure, with index joins by batch
/// and row by row: both alike.
fn alike(s: &Session, sql: &str) -> Result<Vec<String>, turborel::error::Code> {
    let batched = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHED_INDEX_JOINS = 'OFF'").unwrap();
    let alone = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHED_INDEX_JOINS = 'ON'").unwrap();
    assert_eq!(batched, alone, "{sql}");
    batched
}

/// Joins of O's rows with T's by T's unique index, inner and LEFT, of a
/// residual condition, and of few rows asked for.
const JOINS: [&str; 4] = [
    "SELECT O.V, O.K, T.N FROM O JOIN T ON O.K = T.K WHERE O.V < 300",
    "SELECT O.V, O.K, T.N FROM O LEFT JOIN T ON O.K = T.K WHERE O.V < 300",
    "SELECT O.V, T.N FROM O JOIN T ON O.K = T.K AND T.N <> 'n7' WHERE O.V < 300",
    "SELECT O.V, T.N FROM O JOIN T ON O.K = T.K WHERE O.V < 300 FETCH FIRST 5 ROWS ONLY",
];

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.11 GR2] [ISO/IEC 9075:1992 §7.5]
/// (RULES.md D491) each outer row joined with the rows its key finds, in
/// the outer rows' order, a null or missing key finding none, a key that
/// repeats finding its row each time, as probing row by row joins them.
#[test]
fn batches_of_outer_rows_join_as_rows_alone() {
    let db = batches();
    let s = db.session("B");
    for sql in JOINS {
        assert!(physical(&s, sql).contains("IndexNestedLoop B.T using"), "{}", physical(&s, sql));
        let joined = alike(&s, sql).unwrap();
        assert!(!joined.is_empty(), "{sql}");
    }
    let left = alike(&s, JOINS[1]).unwrap();
    assert_eq!(left.len(), 300, "every outer row kept");
    assert!(left.iter().any(|r| r.ends_with(",NULL,NULL")), "a null key finds none: {left:?}");
}

/// Verifies: [ISO/IEC 9075:1992 §3.3.4.1] (RULES.md D491, D15) a failure
/// of a row's condition is raised at that row, the rows before it given
/// first, probing by batch as probing row by row.
#[test]
fn a_failure_is_raised_at_its_row() {
    let db = batches();
    let s = db.session("B");
    let sql = "SELECT O.V, T.N FROM O JOIN T ON O.K = T.K AND 100 / (O.V - 250) > -1000 WHERE O.V < 300";
    assert!(alike(&s, sql).is_err());
    let batched = delivered(&s, sql);
    exec(&s, "SET EXECUTOR.BATCHED_INDEX_JOINS = 'OFF'").unwrap();
    assert_eq!(batched, delivered(&s, sql), "the rows before the failure");
}

/// The rows of `sql` a cursor gives before it fails, or all of them.
fn delivered(s: &Session, sql: &str) -> usize {
    let Ok(mut cursor) = s.open_cursor(sql) else { return 0 };
    let mut fetched = 0;
    while let Ok(Some(_)) = cursor.fetch() {
        fetched += 1;
    }
    fetched
}
