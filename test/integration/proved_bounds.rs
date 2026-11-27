//! A comparison of a column with a constant that the bounds of a column
//! store's parts prove true of every row frozen in them is not judged by
//! the scan [X3.135-1986 §5.11; RULES.md D1293]: the rows given are those
//! the comparison is true of all the same, of rows as frozen and of rows
//! changed, deleted and added since.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// The rows written to each table before the freeze, four groups of the
/// column store's, each of two parts.
const MANY: i64 = 8192;

/// Tables R, of the row store, and C, of the column store in groups of
/// 2,048 rows, alike: V the row's key K, so each part holds V from its
/// first key to its last, N null of one row in 3,000 and else K, and P a
/// text of the row's own. Written, then frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "(K INTEGER PRIMARY KEY, V INTEGER, N INTEGER, P VARCHAR(40))";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R {columns}
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..MANY).step_by(512) {
            let values: Vec<String> = (first..first + 512).map(|k| format!("({k}, {k}, {}, 'payload of row {k}')", n_of(k))).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
    }
    db.maintain().unwrap();
    (db, a)
}

/// N of row `k` as written: null of one row in 3,000.
fn n_of(k: i64) -> String {
    if k % 3000 == 1500 { "NULL".to_owned() } else { k.to_string() }
}

/// The ways a filter over a scan runs: by batches, a row at a time, and
/// in parallel; each the settings of a session of its own.
const WAYS: [&[&str]; 3] = [
    &[],
    &["SET EXECUTOR.BATCHES = 'OFF'"],
    &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"],
];

/// The comparisons asked, each with what it is true of a value: the
/// bounds of every part prove the first five of the rows as frozen; a
/// strict one fails the part's first or last row alone; the others are
/// proved of some parts, judged of one and false of the rest.
type Case = (&'static str, fn(i64) -> bool);
const CASES: [Case; 10] = [
    ("{c} >= 0", |v| v >= 0),
    ("{c} <= 8191", |v| v <= 8191),
    ("{c} > -1 AND {c} < 8192", |v| v > -1 && v < 8192),
    ("{c} <> -5", |v| v != -5),
    ("{c} <> 8192", |v| v != 8192),
    ("{c} > 0", |v| v > 0),
    ("{c} < 8191", |v| v < 8191),
    ("{c} >= 2048 AND {c} < 4096", |v| (2048..4096).contains(&v)),
    ("{c} > 2048 AND {c} <= 5000", |v| v > 2048 && v <= 5000),
    ("{c} = 3000", |v| v == 3000),
];

/// Asserts each case, of V and of N, gives of each table, each way a
/// filter runs, the keys of `held`, each row's key, V and N, of which it
/// is true, none of a null.
fn each_case(db: &Database, held: &[(i64, Option<i64>, Option<i64>)], when: &str) {
    for (column, (text, keeps)) in ["V", "N"].iter().flat_map(|c| CASES.iter().map(move |case| (*c, case))) {
        let value = |row: &(i64, Option<i64>, Option<i64>)| if column == "V" { row.1 } else { row.2 };
        let wanted: Vec<String> = held.iter().filter(|row| value(row).is_some_and(keeps)).map(|row| row.0.to_string()).collect();
        for (table, set) in ["R", "C"].iter().flat_map(|t| WAYS.iter().map(move |set| (*t, *set))) {
            let a = db.session("A");
            for setting in set {
                exec(&a, setting).unwrap();
            }
            let sql = format!("SELECT K FROM {table} WHERE {} ORDER BY K", text.replace("{c}", column));
            assert_eq!(rows(&a, &sql).unwrap(), wanted, "{when}, {table} {set:?}: {sql}");
        }
    }
}

/// Verifies: [X3.135-1986 §5.11 GR1, GR2] (RULES.md D1293, D360) a
/// comparison the bounds of a part prove of its frozen rows gives the
/// rows it is true of, as the row store gives them and each way a filter
/// runs: at the least and the greatest value a part holds, strict and
/// not, of a column with a null in a part, and of a range that holds
/// some groups whole, cuts one and leaves the rest out. Then, with rows
/// of parts whose bounds prove the comparison changed since the freeze,
/// the same: a row updated to a value the comparison is false of is not
/// given, though its part's stored bounds still prove it of the frozen
/// row; one updated to a value past every bound is given where that
/// value satisfies; a row deleted is not given; a row updated to null is
/// not; and rows added, of values inside and outside the bounds, are
/// judged as they are.
#[test]
fn a_comparison_proved_by_a_parts_bounds_gives_the_rows_it_is_true_of_5_11() {
    let (db, a) = tables();
    let null_of = |k: i64| (k % 3000 != 1500).then_some(k);
    let mut held: Vec<(i64, Option<i64>, Option<i64>)> = (0..MANY).map(|k| (k, Some(k), null_of(k))).collect();
    each_case(&db, &held, "as frozen");
    for table in ["R", "C"] {
        for change in [
            "UPDATE {t} SET V = -5, N = -5 WHERE K = 100",
            "UPDATE {t} SET V = 99999, N = 99999 WHERE K = 300",
            "UPDATE {t} SET V = NULL, N = NULL WHERE K = 2500",
            "UPDATE {t} SET V = 8192, N = 8192 WHERE K = 5000",
            "DELETE FROM {t} WHERE K = 200 OR K = 3000 OR K = 8191",
            "INSERT INTO {t} VALUES (9000, -1, -1, 'added'), (9001, 3000, 3000, 'added'), (9002, 8191, NULL, 'added')",
        ] {
            a.run(&change.replace("{t}", table)).unwrap();
        }
        a.commit().unwrap();
    }
    held.retain(|row| ![200, 3000, 8191].contains(&row.0));
    for (k, v) in [(100, Some(-5)), (300, Some(99999)), (2500, None), (5000, Some(8192))] {
        let row = held.iter_mut().find(|row| row.0 == k).unwrap();
        (row.1, row.2) = (v, v);
    }
    held.extend([(9000, Some(-1), Some(-1)), (9001, Some(3000), Some(3000)), (9002, Some(8191), None)]);
    each_case(&db, &held, "changed since the freeze");
}
