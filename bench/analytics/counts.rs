//! How fast a row-store table of 200,000 rows is counted and summed
//! (RULES.md D350, D351): COUNT(*) of its zones settled, each counted
//! from its zone, and of every zone unsettled by a change left open, its
//! rows read in place; SUM of a column, read in place either way
//! (RULES.md R-BENCH-1).

use super::harness::{Bench, Case};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// Rows in the table T.
const ROWS: u64 = 200_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 500;
/// The row ids of a zone (`engine/rows/zones.rs`): a change of one row
/// of each leaves every zone unsettled.
const ZONE_ROWS: u64 = 1_024;

/// Measures the counts and the sum of T, its zones settled, then not.
pub fn workloads(bench: &Bench) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (K INTEGER NOT NULL, A INTEGER, S VARCHAR(20))").unwrap();
    let s = db.session("B");
    for start in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (start..start + PER_INSERT).map(|k| format!("({k}, {}, 's{}')", k % 1_000, k % 97)).collect();
        s.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    measured(bench, &s, "settled");
    let open = db.session("B");
    open.run(&format!("UPDATE T SET A = A WHERE MOD(K, {ZONE_ROWS}) = 0")).unwrap();
    measured(bench, &s, "unsettled");
    open.run("ROLLBACK WORK").unwrap();
}

/// Measures COUNT(*) and SUM(A) of T, its zones as `zones` says.
fn measured(bench: &Bench, s: &Session, zones: &str) {
    for (name, sql) in [("COUNT(*)", "SELECT COUNT(*) FROM T"), ("SUM of 1 of 3 columns", "SELECT SUM(A) FROM T")] {
        bench.measure(Case::new(format!("{name} of 200,000 rows, zones {zones}"), "rows"), || {
            super::fetched(s, sql);
            ROWS
        });
    }
}
