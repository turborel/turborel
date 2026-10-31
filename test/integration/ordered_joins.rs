//! \[DESIGN\] An ORDER BY answered by a join's left operand's order, no
//! sort planned (RULES.md D35, D31): an inner hash join, an index
//! nested-loop join, a LEFT outer join and a product each give their rows
//! in their left operand's order, in memory, partitioned past the work
//! area or moved to the spill area, read row by row or a batch at a time;
//! so do a grouping and a DISTINCT over them.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// O, of the column store, its groups sorted by S (COLUMNSTORE.ORDER), so
/// a scan of it gives S in order; R, of the row store, alike, with an
/// index of S, so a range of S read through it does: 6000 rows each of K,
/// a key; G, one of 7; S, a scramble of K, nearly as distinct; B. D, of
/// the row store, and DC, of the column store, two names for G = 1 and one
/// each for 0, 2 to 5 and a null G.
fn tables(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE O (K INTEGER PRIMARY KEY, G INTEGER, S INTEGER, B INTEGER) USING columnstore
             WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'S')
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, S INTEGER, B INTEGER)
           CREATE TABLE D (G INTEGER, NAME VARCHAR(10))
           CREATE TABLE DC (G INTEGER, NAME VARCHAR(10)) USING columnstore",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE INDEX RS ON R (S)").unwrap();
    for table in ["O", "R"] {
        for first in (0..6000_i64).step_by(1000) {
            let values: Vec<String> =
                (first..first + 1000).map(|k: i64| format!("({k}, {}, {}, {})", k % 7, k * 7919 % 6007, k * 3 % 1000)).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    for table in ["D", "DC"] {
        a.run(&format!("INSERT INTO {table} VALUES (0, 'zero'), (1, 'one'), (1, 'uno'), (2, 'two'), (3, 'three'), (4, 'four'), (5, 'five'), (NULL, 'none')")).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    a
}

/// Queries in order of `{t}.S`, their first column, `{t}` O or R: each
/// reads S < 300 of it, in order, as the left operand of an inner hash
/// join of a small right operand and of a large one of either store, of
/// an index nested-loop join, of a LEFT outer join, of a product; grouped
/// by it over a join or a scan; DISTINCT over a join or a scan.
const ORDERED: [&str; 12] = [
    "SELECT {t}.S, D.NAME FROM {t}, D WHERE {t}.G = D.G AND {t}.S < 300 ORDER BY {t}.S",
    "SELECT {t}.S, Y.K FROM {t}, R Y WHERE {t}.K = Y.B AND {t}.S < 300 ORDER BY {t}.S",
    "SELECT {t}.S, Y.K FROM {t}, O Y WHERE {t}.K = Y.B AND {t}.S < 300 ORDER BY {t}.S",
    "SELECT {t}.S, Y.K FROM {t}, R Y WHERE {t}.K = Y.K AND {t}.S < 300 ORDER BY {t}.S",
    "SELECT {t}.S, Y.K FROM {t} LEFT JOIN R Y ON {t}.K = Y.B WHERE {t}.S < 300 ORDER BY {t}.S",
    "SELECT {t}.S, Y.K FROM {t} LEFT JOIN O Y ON {t}.K = Y.B WHERE {t}.S < 300 ORDER BY {t}.S",
    "SELECT {t}.S, D.NAME FROM {t}, D WHERE D.G = 1 AND {t}.S < 300 ORDER BY {t}.S",
    "SELECT {t}.S, COUNT(*), SUM(Y.B) FROM {t}, O Y WHERE {t}.K = Y.B AND {t}.S < 300 GROUP BY {t}.S ORDER BY {t}.S",
    "SELECT {t}.S, COUNT(Y.K) FROM {t} LEFT JOIN O Y ON {t}.K = Y.B WHERE {t}.S < 300 GROUP BY {t}.S ORDER BY {t}.S",
    "SELECT S, COUNT(*) FROM {t} WHERE S < 300 GROUP BY S ORDER BY S",
    "SELECT DISTINCT {t}.S, DC.NAME FROM {t}, DC WHERE {t}.G = DC.G AND {t}.S < 300 ORDER BY {t}.S",
    "SELECT DISTINCT S FROM {t} WHERE S < 300 ORDER BY S",
];

/// The plan `s` makes of `sql`, as optimized.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &turborel::params::Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// The first column of each row of `sql`, an integer, in the rows' order.
fn firsts(s: &Session, sql: &str) -> Vec<i64> {
    rows(s, sql).unwrap().iter().map(|r| r.split(',').next().unwrap_or_default().parse::<i64>().unwrap()).collect()
}

/// Verifies: [DESIGN] RULES.md D35, D31, D38 [X3.135-1986 §8.3 GR3b]: an
/// ORDER BY of a join's left operand's column, that operand read in its
/// order (a column store table's ordered scan, a row store table's index),
/// is planned with no sort, and its rows come in that order: of an inner
/// hash join, its right operand held or partitioned past the work area,
/// its larger right operand not swapped for the left; of an index
/// nested-loop join; of a LEFT outer join, its right operand held or moved
/// to the spill area; of a product; grouped or DISTINCT over a join or a
/// scan; each read row by row (EXECUTOR.BATCHES OFF) and by batches, and
/// alike of either table.
///
/// Regression test (R-TEST-7): a grouping or a DISTINCT read by batches
/// read a column store table's ordered scan, alone or as a join's left
/// operand, by batches out of its order, so its groups came unsorted
/// though no sort was planned.
#[test]
fn joins_give_their_left_operands_order() {
    let db = Database::new();
    let a = tables(&db);
    // The default work area, then one the large right operands outgrow.
    for work in [None, Some("SET MEMORY.WORK_BYTES = 65536")] {
        if let Some(set) = work {
            exec(&a, set).unwrap();
        }
        for batches in ["ON", "OFF"] {
            exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
            for query in ORDERED {
                let of = |t: &str| query.replace("{t}", t);
                for sql in [of("O"), of("R")] {
                    let plan = physical(&a, &sql);
                    assert!(!plan.contains("Sort"), "{sql}: {plan}");
                    let keys = firsts(&a, &sql);
                    assert!(!keys.is_empty() && keys.is_sorted(), "{work:?} {batches} {sql}: {plan}");
                }
                let mut alike = (rows(&a, &of("O")).unwrap(), rows(&a, &of("R")).unwrap());
                alike.0.sort();
                alike.1.sort();
                assert_eq!(alike.0, alike.1, "{query}");
            }
        }
    }
}
