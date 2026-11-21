//! \[DESIGN\] Hash joins of dense keys (RULES.md D601): a join whose one
//! key is of whole numbers dense in their span finds a probe's rows at
//! its key's place, and gives the rows, in the order, it gives hashed,
//! of keys negative, of another scale, of fractions, duplicated and
//! sparse, of nulls that match, at degree 1 and 4, held and spilled.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Rng, exec, rows};

/// Schema A: P, a probe of 4000 random rows; D, a dimension of keys -20
/// to 99, each twice, and two null; S, of 50 keys spread over a million; X, of keys of
/// scale 2, whole and not.
fn tables(seed: u64) -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (K INTEGER, G INTEGER, M DECIMAL(9, 2), F DOUBLE PRECISION) USING columnstore
           CREATE TABLE D (K INTEGER, TAG INTEGER)
           CREATE TABLE S (K INTEGER, TAG INTEGER)
           CREATE TABLE X (K DECIMAL(9, 2), TAG INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let mut rng = Rng::new(seed);
    let probe: Vec<String> = (0..4000).map(|k| probe_row(&mut rng, k)).collect();
    for part in probe.chunks(1000) {
        a.run(&format!("INSERT INTO P VALUES {}", part.join(", "))).unwrap();
    }
    let dims: Vec<String> = (-20_i64..100).flat_map(|k| [format!("({k}, {})", k.rem_euclid(7)), format!("({k}, {})", 100 + k)]).collect();
    a.run(&format!("INSERT INTO D VALUES {}, (NULL, 7), (NULL, 8)", dims.join(", "))).unwrap();
    let sparse: Vec<String> = (0..50).map(|k| format!("({}, {k})", k * 20_011)).collect();
    a.run(&format!("INSERT INTO S VALUES {}", sparse.join(", "))).unwrap();
    let scaled: Vec<String> = (0..60).map(|k| format!("({}.{}, {k})", k, if k % 3 == 0 { "50" } else { "00" })).collect();
    a.run(&format!("INSERT INTO X VALUES {}", scaled.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// A random row of P of number `k`: G of -30 to 129 or null, M of scale
/// 2 and F a double, both of G, whole or not.
fn probe_row(rng: &mut Rng, k: usize) -> String {
    let g = if rng.chance(15) { None } else { Some(i64::try_from(rng.below(160)).unwrap() - 30) };
    let (fraction, exponent) = if rng.chance(4) { ("50", ".5E0") } else { ("00", "E0") };
    let (g, m, f) = g.map_or_else(
        || ("NULL".to_owned(), "NULL".to_owned(), "NULL".to_owned()),
        |g| (g.to_string(), format!("{g}.{fraction}"), format!("{g}{exponent}")),
    );
    format!("({k}, {g}, {m}, {f})")
}

/// Joins of P with the dimensions by one key: of integers, of a decimal
/// of scale 2 and a double against integers, of integers against a
/// decimal dimension, a sparse build, and in a semi join, an outer join
/// and a grouping; of nulls that match, by IS NOT DISTINCT FROM and a
/// correlated subquery unnested into a join on its outer reference.
const QUERIES: [&str; 10] = [
    "SELECT P.K, D.TAG FROM P JOIN D ON P.G = D.K",
    "SELECT P.K, D.TAG FROM P JOIN D ON P.M = D.K",
    "SELECT P.K, D.TAG FROM P JOIN D ON P.F = D.K",
    "SELECT P.K, X.TAG FROM P JOIN X ON P.M = X.K",
    "SELECT P.K, S.TAG FROM P JOIN S ON P.G = S.K",
    "SELECT P.K FROM P WHERE P.G IN (SELECT D.K FROM D WHERE D.TAG < 50)",
    "SELECT P.K, D.TAG FROM P LEFT JOIN D ON P.G = D.K",
    "SELECT D.TAG, COUNT(*), MIN(P.K) FROM P JOIN D ON P.G = D.K GROUP BY D.TAG",
    "SELECT P.K, D.TAG FROM P JOIN D ON P.G IS NOT DISTINCT FROM D.K",
    "SELECT D.K, D.TAG FROM D WHERE D.TAG > (SELECT COUNT(*) FROM P WHERE P.G IS NOT DISTINCT FROM D.K)",
];

/// Each of `QUERIES` gives the rows, in the order, it gives with
/// EXECUTOR.DENSE_JOINS OFF.
fn queries_answer_alike(a: &Session, how: &str) {
    for sql in QUERIES {
        let dense = rows(a, sql).unwrap();
        exec(a, "SET EXECUTOR.DENSE_JOINS = 'OFF'").unwrap();
        let hashed = rows(a, sql).unwrap();
        exec(a, "SET EXECUTOR.DENSE_JOINS = 'ON'").unwrap();
        let (mut dense_sorted, mut hashed_sorted) = (dense.clone(), hashed.clone());
        dense_sorted.sort();
        hashed_sorted.sort();
        assert_eq!(dense_sorted, hashed_sorted, "{sql}, {how}");
        if how == "degree 1" {
            assert_eq!(dense, hashed, "{sql}: the order, {how}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D601 (R-TEST-5): over random rows of seeds
/// fixed, every query of `QUERIES` gives the same rows with
/// EXECUTOR.DENSE_JOINS on and off, in the same order at degree 1, at
/// degree 4 and spilled; 6, 6.00 and 6E0 match a key of 6, and 6.50
/// none; a key of each build row's twice found in build order.
#[test]
fn dense_joins_answer_as_hashed() {
    for seed in [3_u64, 77_001] {
        let (_db, a) = tables(seed);
        queries_answer_alike(&a, "degree 1");
        for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
            exec(&a, set).unwrap();
        }
        queries_answer_alike(&a, "degree 4");
        exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
        exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
        queries_answer_alike(&a, "spilled");
    }
}
