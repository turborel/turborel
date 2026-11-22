//! \[DESIGN\] Runtime filters that reach every scan they can (RULES.md
//! D600): a hash join's filter goes down both operands of an inner hash
//! join below it, by the keys that equate them, so chains of joins give
//! the same rows with the filters on and off, at degree 1 and 4, held and
//! spilled; a build of few keys is judged as a set and told as spans; a
//! filter never hides an error a row it would drop raises.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Rng, exec, failed, sorted};

/// Schema A: F, of the column store, and R, of the row store, of random
/// rows of `seed`; D, a dimension of 40 keys, E, one of decimal keys of
/// another scale, and N, of names collated of no case.
fn tables(seed: u64) -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, N VARCHAR(8) COLLATE CASE_INSENSITIVE) USING columnstore
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, N VARCHAR(8) COLLATE CASE_INSENSITIVE)
           CREATE TABLE D (K INTEGER PRIMARY KEY, TAG INTEGER)
           CREATE TABLE E (G DECIMAL(9, 2), TAG INTEGER)
           CREATE TABLE N (N VARCHAR(8) COLLATE CASE_INSENSITIVE, TAG INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let mut rng = Rng::new(seed);
    let rows: Vec<String> = (0..6000).map(|k| row(&mut rng, k)).collect();
    for table in ["F", "R"] {
        for part in rows.chunks(1000) {
            a.run(&format!("INSERT INTO {table} VALUES {}", part.join(", "))).unwrap();
        }
    }
    let dims: Vec<String> = (0..40).map(|k| format!("({k}, {})", k % 7)).collect();
    a.run(&format!("INSERT INTO D VALUES {}", dims.join(", "))).unwrap();
    let decimals: Vec<String> = (0..60).map(|k| format!("({}.00, {})", k * 3, k % 5)).collect();
    a.run(&format!("INSERT INTO E VALUES {}", decimals.join(", "))).unwrap();
    a.run("INSERT INTO N VALUES ('ANN', 0), ('bob', 1), ('Cy', 2)").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// A random row of F and R of key `k`: G one of 200 or null, H one of 9,
/// N a name of mixed case.
fn row(rng: &mut Rng, k: usize) -> String {
    let group = if rng.chance(10) { "NULL".to_owned() } else { rng.below(200).to_string() };
    let name = rng.pick(&["Ann", "ann", "BOB", "cy", "Dee"]);
    format!("({k}, {group}, {}, '{name}')", rng.below(9))
}

/// Chains of joins of `{t}`, F or R, the other `{u}`, and the dimensions,
/// where a filter crosses a join's keys to the other operand or into a
/// build: by a right key, a left key, a decimal of another scale, names
/// collated, a join by two keys, and in a derived table.
const CHAINS: [&str; 7] = [
    "SELECT {t}.K, D.TAG FROM {t} JOIN {u} ON {t}.K = {u}.K JOIN D ON {u}.G = D.K WHERE D.TAG < 3",
    "SELECT COUNT(*), SUM({u}.H) FROM {t} JOIN {u} ON {t}.G = {u}.K JOIN D ON {t}.G = D.K WHERE D.TAG = 1",
    "SELECT {t}.K FROM {t} JOIN (SELECT {u}.K, {u}.G FROM {u} JOIN D ON {u}.G = D.K WHERE D.TAG < 2) AS X ON {t}.K = X.K",
    "SELECT {t}.K, E.TAG FROM {t} JOIN {u} ON {t}.K = {u}.K JOIN E ON {u}.G = E.G WHERE E.TAG = 2",
    "SELECT {t}.K, N.TAG FROM {t} JOIN {u} ON {t}.N = {u}.N AND {t}.K = {u}.K JOIN N ON {u}.N = N.N",
    "SELECT {t}.K FROM {t} JOIN {u} ON {t}.K = {u}.K AND {t}.G = {u}.G JOIN D ON {t}.G = D.K WHERE D.TAG > 4",
    "SELECT D.TAG, COUNT(*) FROM {t} JOIN {u} ON {t}.H = {u}.H AND {t}.K = {u}.K JOIN D ON {u}.G = D.K GROUP BY D.TAG",
];

/// Each of `CHAINS` of F and R gives the rows it gives with the filters
/// off.
fn chains_answer_alike(a: &Session, how: &str) {
    for query in CHAINS {
        for (t, u) in [("F", "R"), ("R", "F")] {
            let sql = query.replace("{t}", t).replace("{u}", u);
            let on = sorted(a, &sql);
            exec(a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
            let off = sorted(a, &sql);
            exec(a, "SET EXECUTOR.RUNTIME_FILTERS = 'ON'").unwrap();
            assert_eq!(on, off, "{sql}, {how}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D600 (R-TEST-5): over random rows of
/// seeds fixed, every chain of `CHAINS` gives the same rows with the
/// filters on and off, at degree 1 and 4, held and spilled; 6 and 6.00
/// match; 'Ann' and 'ANN' match of a collation of no case.
#[test]
fn chains_of_joins_answer_alike() {
    for seed in [11_u64, 424_242] {
        let (_db, a) = tables(seed);
        chains_answer_alike(&a, "degree 1");
        for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
            exec(&a, set).unwrap();
        }
        chains_answer_alike(&a, "degree 4");
        exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
        exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
        chains_answer_alike(&a, "spilled");
    }
}

/// Verifies: [DESIGN] RULES.md D600, D15 (R-TEST-7): a filter crossing a
/// join into a build stops above a condition that can raise there, so
/// `1 / (H - 5)` of a row of H = 5, whose G no key of D's matches,
/// raises with the filters on as off, at degree 1 and 4.
#[test]
fn a_filter_into_a_build_hides_no_error() {
    let (_db, a) = tables(3);
    exec(&a, "INSERT INTO F VALUES (90001, 9999, 5, 'x')").unwrap();
    exec(&a, "INSERT INTO R VALUES (90001, 9999, 5, 'x')").unwrap();
    for (t, u) in [("F", "R"), ("R", "F")] {
        let raising = format!(
            "SELECT {t}.K FROM {t} JOIN (SELECT * FROM {u} WHERE 1 / ({u}.H - 5) > -9) AS X ON {t}.K = X.K JOIN D ON X.G = D.K WHERE D.TAG < 2"
        );
        for degree in ["1", "4"] {
            exec(&a, &format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
            for filters in ["ON", "OFF"] {
                exec(&a, &format!("SET EXECUTOR.RUNTIME_FILTERS = '{filters}'")).unwrap();
                assert_eq!(failed(&a, &raising).code, Code::Data, "{raising}, filters {filters}, degree {degree}");
            }
        }
    }
}

/// The rows each scan of `table` gave as EXPLAIN ANALYZE ran `sql`.
fn scanned(a: &Session, sql: &str, table: &str) -> u64 {
    let analyzed = a.analyzed(sql, &Params::new()).unwrap();
    let label = format!("SeqScan A.{table}");
    analyzed.operators.iter().filter(|o| o.label.starts_with(&label)).map(|o| o.rows).sum()
}

/// Verifies: [DESIGN] RULES.md D600: the few keys of D a filter of
/// TAG leaves, told by their joins, keep the scans of FX and RX, tables
/// of F's and R's rows of no index, to the rows they can match, where
/// with the filters off each scan gives every row; EXPLAIN ANALYZE counts
/// the rows the filters left.
#[test]
fn a_filter_reaches_both_operands_of_a_join_below() {
    let (_db, a) = tables(17);
    exec(&a, "CREATE TABLE FX (K INTEGER, G INTEGER) USING columnstore").unwrap();
    exec(&a, "CREATE TABLE RX (K INTEGER, G INTEGER)").unwrap();
    exec(&a, "INSERT INTO FX SELECT K, G FROM F").unwrap();
    exec(&a, "INSERT INTO RX SELECT K, G FROM R").unwrap();
    a.commit().unwrap();
    // The runtime filters alone are measured: a phase of predicate
    // transfer (D665) would keep the scans to their matches before the joins.
    exec(&a, "SET PLANNER.PREDICATE_TRANSFER = 'OFF'").unwrap();
    let sql = "SELECT FX.K, D.TAG FROM FX JOIN RX ON FX.K = RX.K JOIN D ON RX.G = D.K WHERE D.TAG = 3";
    let (f_on, r_on) = (scanned(&a, sql, "FX"), scanned(&a, sql, "RX"));
    exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
    let (f_off, r_off) = (scanned(&a, sql, "FX"), scanned(&a, sql, "RX"));
    let plan = a.analyzed(sql, &Params::new()).unwrap().text;
    assert_eq!((f_off, r_off), (6000, 6000), "{plan}");
    assert!(f_on < 600 && r_on < 600, "FX gave {f_on}, RX gave {r_on}, of\n{plan}");
}
