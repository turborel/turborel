//! \[DESIGN\] Window functions, EXCEPT and INTERSECT run in parallel at
//! the smallest MEMORY.WORK_BYTES (RULES.md D1227): each partition's
//! result held in a list of its share of the grant that spills, and the
//! lists merged back as the statement reads them, the rows those of the
//! plan run alone, in its order where one is asked of them (RULES.md
//! D1245).

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows, spill_bytes};

/// The smallest MEMORY.WORK_BYTES, 64 KiB.
const SMALL: i128 = 64 << 10;

/// Table W of the column store in groups of 2048: K, a key; G, one of
/// 3000, null for every 41st; T, a text; N, a number.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE W (K INTEGER PRIMARY KEY, G BIGINT, T VARCHAR(20), N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0_i64..48_000)
        .map(|k| {
            let g = k * 7919 % 3000;
            let shown = if k % 41 == 0 { "NULL".to_owned() } else { g.to_string() };
            format!("({k}, {shown}, 'text {}', {})", k % 977, k % 13)
        })
        .collect();
    a.run(&format!("INSERT INTO W VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Runs `sql` alone, a row at a time on one thread.
fn alone(s: &Session, sql: &str) -> Vec<String> {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Runs `sql` in parallel on 4 threads in morsels of 256 rows, a row at a
/// time, as a scan of the column store by batch is kept serial, its plan
/// asserted to run a part in parallel.
fn parallel(s: &Session, sql: &str) -> Vec<String> {
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap();
    assert!(shown.contains("Parallel"), "{sql}: run in parallel\n{shown}");
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Windows whose calls the rows' form alone computes, a filtered count and
/// a DISTINCT one, so their partitions are split by row, the first read
/// in its own order; and EXCEPT and INTERSECT of every row's columns.
const QUERIES: [&str; 5] = [
    "SELECT K, G, COUNT(*) FILTER (WHERE N > 5) OVER (PARTITION BY G ORDER BY K) FROM W ORDER BY G, K",
    "SELECT K, G, COUNT(*) FILTER (WHERE N > 5) OVER (PARTITION BY G ORDER BY K) FROM W",
    "SELECT K, T, COUNT(DISTINCT N) OVER (PARTITION BY T) FROM W",
    "SELECT G, N, T FROM W EXCEPT ALL SELECT G, N, T FROM W WHERE MOD(K, 3) = 0",
    "SELECT G, N FROM W INTERSECT ALL SELECT G, N FROM W WHERE MOD(K, 5) = 0",
];

/// Verifies: [DESIGN] (RULES.md D1227) windows split by row, EXCEPT and
/// INTERSECT run in parallel at the smallest MEMORY.WORK_BYTES, their
/// partitions' results held in lists that spill past their shares and
/// merged back as they are read, give the rows they give run alone, in
/// their order: a set operation's, and a window's read in its order; a
/// window read in no order gives its partitions' lists one after another
/// (RULES.md D1245), the same rows.
#[test]
fn partitions_past_the_grant_give_their_results_in_order() {
    let (db, a) = table();
    let in_memory: Vec<Vec<String>> = QUERIES.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    for (q, expected) in QUERIES.iter().zip(&in_memory) {
        assert!(!expected.is_empty(), "{q}");
        let (mut found, mut wanted) = (parallel(&a, q), expected.clone());
        if q.contains(" OVER ") && !q.contains("ORDER BY G, K") {
            found.sort();
            wanted.sort();
        }
        assert_eq!(found, wanted, "{q}");
    }
}

/// Verifies: [DESIGN] RULES.md D44, D1227, D1245: a window split by row
/// at the smallest MEMORY.WORK_BYTES, cancelled past its
/// STATEMENT.TIMEOUT_MS wherever that lands, while its rows are split,
/// while its partitions are sorted and computed on their threads, or as
/// their lists are read, is cancelled (SQLCODE -920) or gives its rows
/// whole; and either way nothing of its lists is left held as it
/// returns.
#[test]
fn a_window_split_by_row_cancelled_anywhere_leaves_nothing_held() {
    let (db, a) = table();
    let q = "SELECT K, T, COUNT(DISTINCT N) OVER (PARTITION BY T) FROM W";
    let mut expected = alone(&a, q);
    expected.sort();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    parallel(&a, q);
    let mut cancelled = 0_usize;
    for timeout in [1, 5, 25, 100, 400] {
        exec(&a, &format!("SET STATEMENT.TIMEOUT_MS = {timeout}")).unwrap();
        match rows(&a, q) {
            Ok(mut found) => {
                found.sort();
                assert_eq!(found, expected, "{timeout} ms");
            }
            Err(code) => {
                assert_eq!(code, Code::Cancelled, "{timeout} ms");
                cancelled += 1;
            }
        }
        // Its workers' lists let go before it returned, not as they next
        // woke.
        assert_eq!(spill_bytes(&db), 0, "{timeout} ms: nothing left held");
        exec(&a, "SET STATEMENT.TIMEOUT_MS = 0").unwrap();
    }
    assert!(cancelled > 0, "cancelled at least once");
}
