//! \[DESIGN\] A hash join's table and its runtime filters (RULES.md D368,
//! D369): over random rows of a fixed seed, inner, outer, semi and anti
//! joins by batch give what the row path gives, on both stores, with the
//! filters on and off, at degree 1 and in parallel, held and spilled; a
//! filter never hides an error a row it would drop raises.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Rng, both_ways, exec, failed, sorted};

/// Schema A: F, of the column store, and R, of the row store, alike, of
/// random rows of `seed`; B, a build of keys of other types: G a decimal
/// of another scale, S a longer CHAR, N a name of a collation of no case.
fn tables(seed: u64) -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER, D DECIMAL(9, 2), S CHAR(3), N VARCHAR(8) COLLATE CASE_INSENSITIVE) USING columnstore
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, D DECIMAL(9, 2), S CHAR(3), N VARCHAR(8) COLLATE CASE_INSENSITIVE)
           CREATE TABLE B (G DECIMAL(12, 3), D DECIMAL(5, 1), S CHAR(6), N VARCHAR(8) COLLATE CASE_INSENSITIVE, TAG INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let mut rng = Rng::new(seed);
    let rows: Vec<String> = (0..5000).map(|k| row(&mut rng, k)).collect();
    for table in ["F", "R"] {
        for part in rows.chunks(1000) {
            a.run(&format!("INSERT INTO {table} VALUES {}", part.join(", "))).unwrap();
        }
    }
    let built: Vec<String> = (0..300).map(|t| built_row(&mut rng, t)).collect();
    a.run(&format!("INSERT INTO B VALUES {}", built.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// A random row of F and R of key `k`: G one of 40 or null, D of cents,
/// S a short text or null, N a name of mixed case.
fn row(rng: &mut Rng, k: usize) -> String {
    let group = if rng.chance(10) { "NULL".to_owned() } else { rng.below(40).to_string() };
    let cents = format!("{}.{:02}", rng.below(20), rng.below(4) * 25);
    let text = if rng.chance(5) { "NULL".to_owned() } else { format!("'{}'", rng.pick(&["a", "b", "ab", "c "])) };
    let name = format!("'{}'", rng.pick(&["Ann", "ann", "BOB", "cy", "Dee"]));
    format!("({k}, {group}, {cents}, {text}, {name})")
}

/// A random row of B, tag `t`: some keys more than once, some none of F's.
fn built_row(rng: &mut Rng, t: usize) -> String {
    let group = if rng.chance(5) { "NULL".to_owned() } else { format!("{}.000", rng.below(60)) };
    let cents = format!("{}.{}", rng.below(20), rng.pick(&["0", "5"]));
    let text = format!("'{}'", rng.pick(&["a", "b  ", "ab", "c", "zz"]));
    let name = format!("'{}'", rng.pick(&["ANN", "bob", "Cy", "eve"]));
    format!("({group}, {cents}, {text}, {name}, {t})")
}

/// Joins of `{t}`, F or R, with B: inner, outer, semi and anti, on keys
/// of a decimal of another scale, of CHARs of other lengths, of names
/// collated, of two keys.
const JOINS: [&str; 12] = [
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G",
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.D = B.D WHERE {t}.K < 3000",
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.S = B.S",
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.N = B.N AND {t}.G = B.G",
    "SELECT {t}.K, B.TAG FROM {t} LEFT JOIN B ON {t}.G = B.G",
    "SELECT {t}.K, B.TAG FROM B LEFT JOIN {t} ON {t}.G = B.G AND {t}.K < 200",
    "SELECT {t}.K, B.TAG FROM {t} RIGHT JOIN B ON {t}.S = B.S AND {t}.K < 100",
    "SELECT {t}.K, B.TAG FROM {t} FULL JOIN B ON {t}.G = B.G AND {t}.K < 500",
    "SELECT {t}.K FROM {t} WHERE EXISTS (SELECT * FROM B WHERE B.G = {t}.G)",
    "SELECT {t}.K FROM {t} WHERE NOT EXISTS (SELECT * FROM B WHERE B.G = {t}.G)",
    "SELECT {t}.K FROM {t} WHERE {t}.D IN (SELECT B.D FROM B WHERE B.TAG < 30)",
    "SELECT B.TAG, COUNT(*), SUM({t}.D) FROM {t} JOIN B ON {t}.G = B.G GROUP BY B.TAG",
];

/// Each of `JOINS` of F and R, by batch, gives what the row path gives.
fn joins_answer_as_rows_do(a: &Session, how: &str) {
    for query in JOINS {
        for table in ["F", "R"] {
            let sql = query.replace("{t}", table);
            let (batched, rows) = both_ways(a, &sql);
            assert_eq!(batched, rows, "{sql}, {how}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D368, D369 (R-TEST-5): over random rows
/// of seeds fixed, every join of `JOINS` gives by batch the rows it gives
/// row by row: with runtime filters on and off, at degree 1 and 4, the
/// build held and spilled; nulls match none; 2 and 2.000 match; 'b' and
/// 'b  ' match; 'Ann' and 'ANN' match of a collation of no case.
#[test]
fn joins_by_batch_answer_as_rows_do() {
    for seed in [7_u64, 1_234_567] {
        let (_db, a) = tables(seed);
        joins_answer_as_rows_do(&a, "filters on");
        exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
        joins_answer_as_rows_do(&a, "filters off");
        exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'ON'").unwrap();
        for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
            exec(&a, set).unwrap();
        }
        joins_answer_as_rows_do(&a, "degree 4");
        exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
        exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
        joins_answer_as_rows_do(&a, "spilled");
    }
}

/// Verifies: [DESIGN] RULES.md D369 (R-TEST-1): runtime filters on and
/// off give the same rows, a build of a few keys of a range, its filters
/// judged at the scan, and one of every key, which they stop judging.
#[test]
fn filters_change_no_answer() {
    let (_db, a) = tables(99);
    let queries = [
        "SELECT F.K, B.TAG FROM F JOIN B ON F.G = B.G WHERE B.TAG < 5",
        "SELECT R.K, B.TAG FROM R JOIN B ON R.G = B.G WHERE B.TAG < 5",
        "SELECT COUNT(*) FROM F JOIN (SELECT G FROM R WHERE R.K < 4000) AS X ON F.G = X.G",
        "SELECT F.K FROM F JOIN B ON F.G = B.G JOIN R ON F.K = R.K WHERE B.TAG < 9",
    ];
    for sql in queries {
        let on = sorted(&a, sql);
        exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
        let off = sorted(&a, sql);
        exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'ON'").unwrap();
        assert_eq!(on, off, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D369, D15 (MC/DC): a filter judged below
/// a condition that can raise would hide its error, so it is judged above:
/// `1 / (G - 5)` of a row of G = 5, a key the build has none of, raises by
/// batch as it does row by row; of a condition that cannot raise, the
/// filter goes down and the rows are the same.
#[test]
fn a_filter_hides_no_error() {
    let (_db, a) = tables(5);
    exec(&a, "DELETE FROM B WHERE G = 5").unwrap();
    for table in ["F", "R"] {
        let raising =
            format!("SELECT {table}.K FROM (SELECT * FROM {table} WHERE 1 / ({table}.G - 5) > -9) AS {table} JOIN B ON {table}.G = B.G");
        assert_eq!(failed(&a, &raising).code, Code::Data, "{raising}");
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(failed(&a, &raising).code, Code::Data, "{raising}, row by row");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        let safe = format!("SELECT {table}.K FROM (SELECT * FROM {table} WHERE {table}.G <> 5) AS {table} JOIN B ON {table}.G = B.G");
        let (batched, rows) = both_ways(&a, &safe);
        assert_eq!(batched, rows, "{safe}");
    }
}

/// Verifies: [DESIGN] RULES.md D368, D292: a probe gives a row's matches
/// in the order the build read them, in parallel as alone, so a join of
/// many matches a row gives its rows in one order at degree 4 and 1.
#[test]
fn matches_come_in_build_order_in_parallel() {
    let (_db, a) = tables(31);
    let sql = "SELECT F.K, B.TAG FROM F JOIN B ON F.G = B.G";
    let alone = crate::common::rows(&a, sql).unwrap();
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(&a, set).unwrap();
    }
    assert_eq!(crate::common::rows(&a, sql).unwrap(), alone);
}
