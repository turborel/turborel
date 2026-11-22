//! The measurement behind `PROBE_ROWS` (RULES.md D791): a correlated
//! scalar subquery's run as one probe of a unique key, against a row of
//! its decorrelated build, over inner tables of 1,000, 10,000 and 100,000
//! rows, as an order line reads its item's price: the outer rows an
//! UNNEST's, so the subquery stays one the runner runs. The probes are
//! 100 outer rows' runs, never decorrelated; the build is one outer
//! row's run of a table with no index, decorrelated at its first run, so
//! it costs its scan and keyed table. The ratio of
//! the rates is the rows built that cost one probe.

use super::harness::{Bench, Case};
use super::joined;
use turborel::db::Database;
use turborel::value::{Decimal, Value};

/// The inner tables' sizes swept.
const INNER: [u64; 3] = [1_000, 10_000, 100_000];
/// The outer rows of the probing query, each one probe.
const OUTER: u64 = 100;
/// Rows inserted by one statement as a table is filled.
const BATCH: u64 = 1_000;

/// Measures the probes and the build at each inner size.
pub fn sweep(bench: &Bench) {
    for inner in INNER {
        let keys: Vec<String> = (0..OUTER).map(|n| (n * (inner / OUTER)).to_string()).collect();
        let probing = database(inner, true);
        let probes = probing.session("P");
        let query = format!("SELECT L.N * (SELECT W FROM X WHERE X.K = L.N) FROM UNNEST(ARRAY[{}]) AS L (N)", keys.join(", "));
        bench.measure(Case::new(format!("scalar subquery probed by key, {inner} rows"), "probes"), || joined(&probes, &query));
        let building = database(inner, false);
        let builds = building.session("P");
        bench.measure(Case::new(format!("scalar subquery built, {inner} rows"), "rows"), || {
            joined(&builds, "SELECT L.N * (SELECT W FROM X WHERE X.K = L.N) FROM UNNEST(ARRAY[0]) AS L (N)");
            inner
        });
    }
}

/// Schema P: X of `inner` rows, K its primary key if `keyed`; a keyed X's
/// subquery never decorrelated, another's at its first run.
fn database(inner: u64, keyed: bool) -> Database {
    let db = Database::new();
    let key = if keyed { ", PRIMARY KEY (K)" } else { "" };
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION P CREATE TABLE X (K INTEGER NOT NULL, W INTEGER{key})")).unwrap();
    let after: i128 = if keyed { 1_000_000 } else { 0 };
    db.set_setting("EXECUTOR.DECORRELATE_AFTER_RUNS", &Value::Exact(Decimal::new(after, 0))).unwrap();
    let s = db.session("P");
    for first in (0..inner).step_by(BATCH as usize) {
        let rows: Vec<String> = (first..(first + BATCH).min(inner)).map(|k| format!("({k}, {k})")).collect();
        s.run(&format!("INSERT INTO X VALUES {}", rows.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db
}
