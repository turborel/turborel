//! The keys of a hash join's runtime filter asked by a column store's
//! scan of the join's other operand, once a run of its key's column and
//! before its other columns are decoded [RULES.md D1294]: what the scan
//! gives, and that the statement gives the rows it gave without it.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// The rows of the fact tables, eight groups of the column store's.
const FACTS: usize = 16_384;
/// The rows of the dimension, one for every four facts.
const DIMS: usize = 4_096;

/// Schema A: F, of the column store in groups of 2,048 rows, and R, of
/// the row store, alike: K the row's number, D the key of its dimension
/// row, the same of four rows running, V and W two numbers and P a text
/// of the row's own; and D, a dimension of `DIMS` keys, TAG one of a
/// hundred. Written, then frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    // No table has an index, so each join is hashed and its scans are told.
    let columns = "(K INTEGER NOT NULL, D INTEGER, V INTEGER, W INTEGER, P VARCHAR(40))";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R {columns}
           CREATE TABLE D (K INTEGER NOT NULL, TAG INTEGER)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["F", "R"] {
        for first in (0..FACTS).step_by(512) {
            let values: Vec<String> =
                (first..first + 512).map(|k| format!("({k}, {}, {}, {}, 'payload of row {k}')", k.div_euclid(4), k % 7, k % 5)).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    for first in (0..DIMS).step_by(512) {
        let values: Vec<String> = (first..first + 512).map(|k| format!("({k}, {})", k % 100)).collect();
        a.run(&format!("INSERT INTO D VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows the scans of table `table` in `sql` gave, as EXPLAIN ANALYZE
/// counts them.
fn scanned(s: &Session, sql: &str, table: &str) -> u64 {
    let analyzed = s.analyzed(sql, &Params::new()).unwrap();
    let label = format!("SeqScan A.{table}");
    let scans: Vec<u64> = analyzed.operators.iter().filter(|o| o.label.starts_with(&label)).map(|o| o.rows).collect();
    assert!(!scans.is_empty(), "no scan of {table} in\n{}", analyzed.text);
    scans.iter().sum()
}

/// The ways a statement runs: its plan alone, and in parallel by
/// morsels; each the settings of a session of its own.
const WAYS: [&[&str]; 2] =
    [&["SET EXECUTOR.DEGREE = 1"], &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]];

/// A session of `db` with `settings` set.
fn session(db: &Database, settings: &[&str]) -> Session {
    let a = db.session("A");
    for setting in settings {
        exec(&a, setting).unwrap();
    }
    a
}

/// The statements asked, `{t}` the fact table: a join with few
/// dimension rows, the same with a condition of two of the fact's
/// columns, a semi join that holds the dimension's rows and reads the
/// facts once, a join of a fact column that is no run, and a join with
/// most dimension rows.
const FEW: &str = "SELECT T.K, T.P FROM {t} AS T, D WHERE T.D = D.K AND D.TAG = 7 ORDER BY T.K";
const FEW_FILTERED: &str = "SELECT T.K FROM {t} AS T, D WHERE T.D = D.K AND D.TAG = 7 AND T.V < T.W ORDER BY T.K";
const SEMI: &str = "SELECT D.K FROM D WHERE D.TAG = 7 AND EXISTS (SELECT * FROM {t} AS T WHERE T.D = D.K AND T.V < T.W) ORDER BY D.K";
const BY_ROW: &str = "SELECT T.K FROM {t} AS T, D WHERE T.K = D.K AND D.TAG = 7 ORDER BY T.K";
const MOST: &str = "SELECT COUNT(*), SUM(T.V) FROM {t} AS T, D WHERE T.D = D.K AND D.TAG < 90";

/// Verifies: [DESIGN] RULES.md D1294 [X3.135-1986 §5.20 GR2b, §5.17 GR2]
/// each statement gives, of the column store's table, the rows it gives
/// of the row store's, and those it gives with runtime filters off, its
/// plan alone and in parallel: a join with few built keys, of a key
/// column kept as runs and of one that is not, with a condition of two
/// columns beside it, a semi join that reads the facts once, and a join
/// with most keys built.
#[test]
fn a_join_whose_keys_a_scan_asks_gives_the_rows_it_gave_5_20() {
    let (db, _) = tables();
    for (sql, way) in [FEW, FEW_FILTERED, SEMI, BY_ROW, MOST].iter().flat_map(|sql| WAYS.iter().map(move |way| (*sql, *way))) {
        let a = session(&db, way);
        let wanted = rows(&a, &sql.replace("{t}", "R")).unwrap();
        assert!(!wanted.is_empty(), "{sql}");
        assert_eq!(rows(&a, &sql.replace("{t}", "F")).unwrap(), wanted, "{way:?}: {sql}");
        exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
        assert_eq!(rows(&a, &sql.replace("{t}", "F")).unwrap(), wanted, "filters off, {way:?}: {sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D1294: a scan told the keys of a join
/// with few built rows gives the rows whose key they hold and no others,
/// where with runtime filters off it gives every row of the table: of 41
/// keys of 4,096, each of four rows, 164 rows of 16,384, its plan alone
/// and in parallel, where a morsel's scan gave every row of the parts
/// the keys' spans reach before it asked the keys; with a condition of
/// two of its columns beside the join, which then judges those rows
/// alone; and under a semi join that holds the other operand.
#[test]
fn a_scan_told_few_keys_gives_their_rows_alone() {
    let (db, _) = tables();
    let keys = u64::try_from(DIMS.div_ceil(100)).unwrap();
    let held = keys * 4;
    let all = u64::try_from(FACTS).unwrap();
    for way in WAYS {
        let a = session(&db, way);
        // The runtime filters alone are measured, without a phase of
        // predicate transfer before the joins (RULES.md D665).
        exec(&a, "SET PLANNER.PREDICATE_TRANSFER = 'OFF'").unwrap();
        for sql in [FEW, FEW_FILTERED, SEMI] {
            assert_eq!(scanned(&a, &sql.replace("{t}", "F"), "F"), held, "{way:?}: {sql}");
        }
        assert_eq!(scanned(&a, &BY_ROW.replace("{t}", "F"), "F"), keys, "{way:?}: a key a row");
        exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
        assert_eq!(scanned(&a, &FEW.replace("{t}", "F"), "F"), all, "{way:?}: filters off");
    }
}
