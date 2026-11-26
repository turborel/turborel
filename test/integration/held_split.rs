//! \[DESIGN\] A semi join or an antijoin that builds its left starts a
//! pipeline of its own (RULES.md D1156): its held rows, marked, are split
//! into morsels, and the operators over the join take each morsel's rows
//! on the part's threads. Each row the join gives comes of one morsel
//! alone, once.

use std::collections::BTreeSet;

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// The rows of O, of K from 0 to 2999, and of L.
const ORDERS: i64 = 3000;
const LINES: i64 = 20_000;

/// The F of order `k`: null for every 7th, else one of 0 to 3999.
fn order_f(k: i64) -> Option<i64> {
    (k % 7 != 0).then_some(k * 13 % 4000)
}

/// The K of line `n`: null for every 19th, else an even number below
/// 8000, each of them some line's.
fn line_k(n: i64) -> Option<i64> {
    (n % 19 != 0).then_some(n * 31 % 4000 * 2)
}

/// Tables O, orders, and L, their lines, of the row store, and OC and LC
/// alike of the column store, in groups of 2048.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE O (K INTEGER PRIMARY KEY, F INTEGER, D INTEGER)
           CREATE TABLE L (K INTEGER, A INTEGER)
           CREATE TABLE OC (K INTEGER PRIMARY KEY, F INTEGER, D INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE LC (K INTEGER, A INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let shown = |v: Option<i64>| v.map_or_else(|| "NULL".to_owned(), |v| v.to_string());
    for table in ["O", "OC"] {
        let values: Vec<String> = (0..ORDERS).map(|k| format!("({k}, {}, {})", shown(order_f(k)), k % 10)).collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    for table in ["L", "LC"] {
        let values: Vec<String> = (0..LINES).map(|n| format!("({}, {})", shown(line_k(n)), n % 50)).collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The K of each order a line matches, or of `anti` each no line
/// matches, an order of a null F among the latter.
fn expected(anti: bool) -> BTreeSet<i64> {
    let lines: BTreeSet<i64> = (0..LINES).filter_map(line_k).collect();
    (0..ORDERS).filter(|&k| order_f(k).is_some_and(|f| lines.contains(&f)) != anti).collect()
}

/// Whether `shown`, a plan, runs a semi join or an antijoin that builds
/// its left within a part: a `Parallel` above it.
fn in_a_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().collect();
    let indent = |l: &str| l.len().saturating_sub(l.trim_start().len());
    let Some(at) = lines.iter().position(|l| l.contains("HashRightSemiJoin") || l.contains("HashRightAntiJoin")) else { return false };
    let depth = lines.get(at).map_or(0, |l| indent(l));
    lines.iter().take(at).any(|l| l.contains("Parallel") && indent(l) < depth)
}

/// The numbers `sql` gives, one a row, or the code it failed with.
fn numbers(s: &Session, sql: &str) -> Vec<i64> {
    let given = rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    given.iter().map(|r| r.trim().parse().unwrap_or_else(|e| panic!("{sql}: {r}: {e}"))).collect()
}

/// Verifies: [X3.135-1986 §5.17 GR2, GR3] (RULES.md D1156, D1070) of a
/// semi join and an antijoin that builds its left, run within a part as
/// EXPLAIN shows, its held rows split into morsels of 16 rows on 4
/// threads, each held row a line matches is given once by the semi join,
/// and each none matches once by the antijoin, an order of a null key
/// among them, as a grouping by the orders' key over the join counts, of
/// both engines.
#[test]
fn held_rows_split_into_morsels_are_each_given_once() {
    let (_db, a) = tables();
    for setting in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 16", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(&a, setting).unwrap();
    }
    for (o, l) in [("O", "L"), ("OC", "LC")] {
        for anti in [false, true] {
            let not = if anti { "NOT " } else { "" };
            let joined = format!("FROM {o} WHERE {not}EXISTS (SELECT * FROM {l} WHERE {l}.K = {o}.F)");
            let keys = format!("SELECT {o}.K {joined} GROUP BY {o}.K");
            let shown = a.physical_plan(&keys, &Params::new()).unwrap_or_else(|e| panic!("{keys}: {e}"));
            assert!(in_a_part(&shown), "{keys}\n{shown}");
            let twice = format!("SELECT {o}.K {joined} GROUP BY {o}.K HAVING COUNT(*) <> 1");
            assert_eq!(numbers(&a, &twice), Vec::<i64>::new(), "{twice}");
            let given: BTreeSet<i64> = numbers(&a, &keys).into_iter().collect();
            let want = expected(anti);
            assert!(!want.is_empty() && want.len() < usize::try_from(ORDERS).unwrap_or(0), "{keys}");
            assert_eq!(given, want, "{keys}");
            let count = format!("SELECT COUNT(*) {joined}");
            assert_eq!(numbers(&a, &count), vec![i64::try_from(want.len()).unwrap_or(0)], "{count}");
        }
    }
}
