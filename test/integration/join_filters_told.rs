//! \[DESIGN\] Runtime filters below operators run row by row (RULES.md
//! D600): an index scan judges a filter and bounds its probe by it, an
//! inner index join and an inner merge join take one to their operands,
//! and a grouping whose set functions raise nothing (RULES.md D1062)
//! takes one of a grouping column to its input, warning as it would of
//! every row. The rows are those of the filters off, at degree 1 and 4.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Rng, exec, rows, sorted};

/// Schema A: F, of the column store, and R, of the row store, of random
/// rows of `seed`, R indexed by G; L and M, column tables kept in the
/// order of K; D, a dimension of 40 keys.
fn tables(seed: u64) -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER) USING columnstore
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER)
           CREATE TABLE L (K INTEGER, V INTEGER) USING columnstore WITH (COLUMNSTORE.ORDER = 'K')
           CREATE TABLE M (K INTEGER, V INTEGER) USING columnstore WITH (COLUMNSTORE.ORDER = 'K')
           CREATE TABLE D (K INTEGER PRIMARY KEY, TAG INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE INDEX RG ON R (G)").unwrap();
    let mut rng = Rng::new(seed);
    let rows: Vec<String> = (0..6000).map(|k| row(&mut rng, k)).collect();
    for table in ["F", "R"] {
        for part in rows.chunks(1000) {
            a.run(&format!("INSERT INTO {table} VALUES {}", part.join(", "))).unwrap();
        }
    }
    for (table, m) in [("L", 300), ("M", 400)] {
        let ordered: Vec<String> = (0..3000).map(|k| format!("({}, {})", (k * 7) % m, k % 11)).collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", ordered.join(", "))).unwrap();
    }
    let dims: Vec<String> = (0..40).map(|k| format!("({k}, {})", k % 7)).collect();
    a.run(&format!("INSERT INTO D VALUES {}", dims.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// A random row of F and R of key `k`: G one of 200 or null, H one of 9
/// or null.
fn row(rng: &mut Rng, k: usize) -> String {
    let group = if rng.chance(10) { "NULL".to_owned() } else { rng.below(200).to_string() };
    let h = if rng.chance(20) { "NULL".to_owned() } else { rng.below(9).to_string() };
    format!("({k}, {group}, {h})")
}

/// Queries whose filters go below an index scan, an index join, a merge
/// join, a grouping and a semi join, of `{t}`, F or R: groupings of each
/// set function that takes a filter, SUM of an INTEGER column among them, a join
/// by R's index of G, merge joins of L and M in K's order, IN and EXISTS,
/// and NOT IN of a subquery with a null, which an antijoin's filter would
/// change, and of one without.
const QUERIES: [&str; 12] = [
    "SELECT X.G, X.C FROM (SELECT G, COUNT(*) AS C FROM {t} GROUP BY G) AS X JOIN D ON X.G = D.K WHERE D.TAG < 2",
    "SELECT X.G, X.C, X.N FROM (SELECT G, COUNT(H) AS C, MIN(H) AS N FROM {t} GROUP BY G) AS X JOIN D ON X.G = D.K WHERE D.TAG = 3",
    "SELECT X.G, X.M FROM (SELECT G, MAX(K) AS M FROM {t} GROUP BY G) AS X JOIN D ON X.G = D.K WHERE D.TAG > 4",
    "SELECT X.G, X.S FROM (SELECT G, SUM(H) AS S FROM {t} GROUP BY G) AS X JOIN D ON X.G = D.K WHERE D.TAG = 1",
    "SELECT T.K, R.K FROM {t} AS T JOIN R ON T.H = R.G JOIN D ON T.G = D.K WHERE D.TAG < 6 AND T.K < 300",
    "SELECT R.K, R.H FROM R JOIN D ON R.G = D.K WHERE D.TAG = 6",
    "SELECT L.K, M.V FROM L JOIN M ON L.K = M.K JOIN D ON L.V = D.K WHERE D.TAG < 3 ORDER BY L.K",
    "SELECT L.K, M.V FROM L JOIN M ON L.K = M.K JOIN D ON M.K = D.K WHERE D.TAG < 3 ORDER BY L.K",
    "SELECT T.K FROM {t} AS T JOIN D ON T.G = D.K WHERE D.TAG = 2 AND T.G IN (SELECT R.G FROM R WHERE R.H < 5)",
    "SELECT T.K FROM {t} AS T JOIN D ON T.G = D.K WHERE D.TAG < 3 AND EXISTS (SELECT 1 FROM R WHERE R.G = T.G AND R.H = 3)",
    "SELECT T.K FROM {t} AS T JOIN D ON T.G = D.K WHERE D.TAG = 4 AND T.G NOT IN (SELECT R.H FROM R)",
    "SELECT T.K FROM {t} AS T JOIN D ON T.G = D.K WHERE D.TAG = 4 AND T.G NOT IN (SELECT R.H FROM R WHERE R.H IS NOT NULL)",
];

/// Each of `QUERIES` of F and R gives the rows it gives with the filters
/// off.
fn queries_answer_alike(a: &Session, how: &str) {
    for query in QUERIES {
        for t in ["F", "R"] {
            let sql = query.replace("{t}", t);
            let on = sorted(a, &sql);
            exec(a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
            let off = sorted(a, &sql);
            exec(a, "SET EXECUTOR.RUNTIME_FILTERS = 'ON'").unwrap();
            assert_eq!(on, off, "{sql}, {how}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D600 (R-TEST-5): over random rows of
/// seeds fixed, every query of `QUERIES` gives the same rows with the
/// filters on and off, read by batches and row by row, at degree 1 and 4.
#[test]
fn filters_below_rows_made_row_by_row_answer_alike() {
    for seed in [5_u64, 90_210] {
        let (_db, a) = tables(seed);
        queries_answer_alike(&a, "degree 1");
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        queries_answer_alike(&a, "row by row");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
            exec(&a, set).unwrap();
        }
        queries_answer_alike(&a, "degree 4");
    }
}

/// The rows the operators of `sql` whose labels begin with `label` gave
/// as EXPLAIN ANALYZE ran it, and its text.
fn given(a: &Session, sql: &str, label: &str) -> (u64, String) {
    let analyzed = a.analyzed(sql, &Params::new()).unwrap();
    (analyzed.operators.iter().filter(|o| o.label.starts_with(label)).map(|o| o.rows).sum(), analyzed.text)
}

/// Verifies: [DESIGN] RULES.md D600, D1062, D15: a grouping of COUNT(*)
/// alone takes the filter of the few keys of D a filter of TAG leaves to
/// its scan of F, so the scan gives the rows of those keys, where with the
/// filters off it gives every row; a grouping of SUM of an INTEGER
/// column declared NOT NULL, whose sum always fits and which eliminates
/// no null, takes it to its scan too, and gives the sums it gives with
/// the filters off, as it does under a projection of `0.5 * SUM(H)`,
/// which the types prove fits; a grouping of SUM of a column of 38
/// digits, which can overflow, takes none, so its scan gives every row
/// either way.
#[test]
fn a_filter_reaches_the_scan_below_a_grouping() {
    let (_db, a) = tables(17);
    exec(&a, "CREATE TABLE N (G INTEGER, H INTEGER NOT NULL) USING columnstore").unwrap();
    exec(&a, "CREATE TABLE W (G INTEGER, H DECIMAL(38, 0) NOT NULL) USING columnstore").unwrap();
    for table in ["N", "W"] {
        exec(&a, &format!("INSERT INTO {table} SELECT G, COALESCE(H, 0) FROM F")).unwrap();
    }
    a.commit().unwrap();
    let counted = "SELECT X.G, X.C FROM (SELECT G, COUNT(*) AS C FROM F GROUP BY G) AS X JOIN D ON X.G = D.K WHERE D.TAG = 3";
    let summed =
        |t: &str| format!("SELECT X.G, X.S FROM (SELECT G, SUM(H) AS S FROM {t} GROUP BY G) AS X JOIN D ON X.G = D.K WHERE D.TAG = 3");
    let ((counted_on, plan), sums_on) = (given(&a, counted, "SeqScan A.F"), sorted(&a, &summed("N")));
    let ((summed_on, summed_plan), (wide_on, _)) = (given(&a, &summed("N"), "SeqScan A.N"), given(&a, &summed("W"), "SeqScan A.W"));
    // A projection of arithmetic the types prove fits passes the filter on.
    let halved = summed("N").replace("SUM(H) AS S", "0.5 * SUM(H) AS S");
    let (halved_on, halved_plan) = given(&a, &halved, "SeqScan A.N");
    exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
    let (counted_off, _) = given(&a, counted, "SeqScan A.F");
    assert_eq!(counted_off, 6000, "{plan}");
    assert!(counted_on < 600, "F gave {counted_on} of\n{plan}");
    assert!(summed_on < 600, "N gave {summed_on} of\n{summed_plan}");
    assert_eq!(sums_on, sorted(&a, &summed("N")), "the sums with the filters off");
    assert!(halved_on < 600, "N gave {halved_on} of\n{halved_plan}");
    assert_eq!(wide_on, 6000, "SUM of 38 digits takes no filter");
}

/// Verifies: [DESIGN] RULES.md D600 [ISO/IEC 9075-2:2023 §10.9 GR4b]: a
/// grouping of COUNT(H) warns of the null H it would eliminate of a row
/// the filter drops, with the filters on as off; with no null H, it
/// warns of none either way.
#[test]
fn a_grouping_told_a_filter_warns_as_of_every_row() {
    let (_db, a) = tables(23);
    exec(&a, "UPDATE F SET H = 1 WHERE H IS NULL").unwrap();
    exec(&a, "INSERT INTO F VALUES (90001, 150, NULL)").unwrap();
    a.commit().unwrap();
    let sql = "SELECT X.G, X.C FROM (SELECT G, COUNT(H) AS C FROM F GROUP BY G) AS X JOIN D ON X.G = D.K WHERE D.TAG = 3";
    for filters in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.RUNTIME_FILTERS = '{filters}'")).unwrap();
        rows(&a, sql).unwrap();
        assert_eq!(a.warning(), SqlState::parse("01003"), "filters {filters}");
    }
    exec(&a, "DELETE FROM F WHERE K = 90001").unwrap();
    a.commit().unwrap();
    for filters in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.RUNTIME_FILTERS = '{filters}'")).unwrap();
        rows(&a, sql).unwrap();
        assert_eq!(a.warning(), None, "filters {filters}, no null");
    }
}
