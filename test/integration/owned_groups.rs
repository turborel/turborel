//! \[DESIGN\] A grouping run in parallel whose morsels own the groups of
//! values no other morsel's rows can hold (RULES.md D1121): its groups are
//! those the plan run alone gives, of a table frozen in about the order of
//! its key, whose morsels meet at their edges, overlap where rows came out
//! of order, and hold rows of the tree written since it was frozen.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Rows of L in the order of their keys, as a load writes them.
const ROWS: i64 = 24_000;

/// Table L of the column store, in groups of 2048 rows: K, a key of 3
/// rows each, written in its order, so a key lies across the edge of two
/// groups, but rows 12000 to 12599 of keys from 3000 to 3499 out of
/// order, so their morsel's bounds overlap those of the morsels of those
/// keys and the ones between; D, a date of K; P, an exact number of
/// scale 2 of K; T, a text of K; N, a number, null for every 11th; and
/// every 97th row's K null.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE L (R INTEGER PRIMARY KEY, K BIGINT, D DATE, P DECIMAL(9,2), T VARCHAR(12), N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let row = |r: i64| {
        let k = if (12_000..12_600).contains(&r) { 3000 + (r - 12_000) * 7 % 500 } else { r.div_euclid(3) };
        let key = if r % 97 == 0 { "NULL".to_owned() } else { k.to_string() };
        let n = if r % 11 == 0 { "NULL".to_owned() } else { (r % 23).to_string() };
        let day = k % 28 + 1;
        format!("({r}, {key}, DATE '2020-{:02}-{day:02}', {k}.25, 'k{k}', {n})", k.div_euclid(28) % 12 + 1)
    };
    for first in (0..ROWS).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(row).collect();
        a.run(&format!("INSERT INTO L VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of `sql`, sorted, run alone a row at a time, and run in
/// parallel by batch on 4 threads in morsels of at least 256 rows.
fn both(s: &Session, sql: &str) -> (Result<Vec<String>, Code>, Result<Vec<String>, Code>) {
    let sorted = |r: Result<Vec<String>, Code>| {
        r.map(|mut r| {
            r.sort();
            r
        })
    };
    for set in ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"] {
        exec(s, set).unwrap();
    }
    let alone = sorted(rows(s, sql));
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    (alone, sorted(rows(s, sql)))
}

/// Groupings by K, by an exact number and a date, by a text no morsel owns
/// a group of, under a filter the scan judges, and under a HAVING
/// condition judged of one set function's value and of a row.
const QUERIES: [&str; 7] = [
    "SELECT K, COUNT(*), SUM(N), MIN(R) FROM L GROUP BY K",
    "SELECT K, D, COUNT(*) FROM L GROUP BY K, D",
    "SELECT P, COUNT(N) FROM L GROUP BY P",
    "SELECT D, MAX(R) FROM L GROUP BY D",
    "SELECT T, K, COUNT(*) FROM L GROUP BY T, K",
    "SELECT K, SUM(N) FROM L WHERE N > 4 GROUP BY K HAVING SUM(N) > 30",
    "SELECT K, COUNT(*) FROM L GROUP BY K HAVING COUNT(*) > 2 AND MIN(R) > 100",
];

/// Verifies: [DESIGN] (RULES.md D1121) a grouping in parallel of a table
/// frozen in about the order of its key gives the groups the plan run
/// alone gives: of keys across the edges of morsels, which no morsel owns,
/// of rows written out of order into morsels whose bounds overlap others,
/// of null keys, of exact numbers of a scale and of dates, of a text, and
/// under a filter and a HAVING condition; one group of each value.
#[test]
fn groups_owned_by_their_morsels_come_as_alone() {
    let (_db, a) = table();
    for sql in QUERIES {
        let (alone, together) = both(&a, sql);
        assert!(alone.as_ref().is_ok_and(|r| r.len() > 50), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1121) rows written since the table was
/// frozen, of keys its frozen groups hold, in the tree past its groups and
/// as a frozen row changed within a group's range to a key another
/// morsel holds, give one group of each key, as the plan run alone gives:
/// a morsel's bounds take in the values of the tree's rows in its range,
/// so a morsel whose rows reach into another's owns none of them.
#[test]
fn rows_written_since_the_table_was_frozen_join_their_keys_groups() {
    let (_db, a) = table();
    let values: Vec<String> = (0..400_i64).map(|r| format!("({}, {}, DATE '2021-01-01', 1.00, 'x', 1)", ROWS + r, r * 9)).collect();
    a.run(&format!("INSERT INTO L VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    for sql in QUERIES.iter().take(2) {
        let (alone, together) = both(&a, sql);
        assert_eq!(alone, together, "{sql}, rows past the groups");
    }
    a.run(&format!("DELETE FROM L WHERE R >= {ROWS}")).unwrap();
    a.run("UPDATE L SET K = 3501 WHERE R = 10").unwrap();
    a.commit().unwrap();
    let (alone, together) = both(&a, QUERIES[0]);
    assert!(alone.as_ref().is_ok_and(|r| r.iter().any(|row| row.starts_with("3501,4,"))), "{alone:?}");
    assert_eq!(alone, together, "a frozen row changed to a key another morsel holds");
}

/// Verifies: [DESIGN] (RULES.md D1121, D38) a grouping whose morsels own
/// groups gives way where the grant runs out, at the groups owned or at
/// the partitions', its groups those the plan run alone gives.
#[test]
fn owned_groups_past_the_grant_give_way() {
    let (_db, a) = table();
    for bytes in [1_u32 << 16, 1 << 18, 1 << 20, 1 << 26] {
        exec(&a, &format!("SET MEMORY.WORK_BYTES = {bytes}")).unwrap();
        let (alone, together) = both(&a, QUERIES[0]);
        assert_eq!(alone, together, "under {bytes} bytes");
    }
}
