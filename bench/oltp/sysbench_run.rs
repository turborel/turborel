//! A sysbench-style case: the ten tables loaded anew for each script and
//! thread count, as the measurement box restores them for each, and the
//! script's terminals run on the harness. Beside the rate of events go
//! their cost, the failures by SQLCODE, and each statement's rows, as the
//! database's statement statistics count them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use turborel::db::Database;

use crate::client::{Connect, Failed};
use crate::harness::{Bench, Case, note};
use crate::measure::{Totals, on_threads, statement_rows};
use crate::sysbench::{Script, Terminal};
use crate::sysbench_load;

/// The user, and schema, the workloads run as.
pub const USER: &str = "SBTEST";

/// Creates the schema in `db` and loads its tables of `rows` rows.
pub fn build(db: &Database, rows: i64) {
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION {USER}")).unwrap();
    sysbench_load::load(&db.session(USER), rows);
}

/// Measures `script` on `threads` terminals of `connect`, on tables of
/// `rows` rows, for `secs` each run.
pub fn measure<K: Connect>(bench: &Bench, connect: &K, (script, threads, rows): (&Script, usize, i64), secs: Duration) {
    let totals = RefCell::new(Totals::default());
    let failed = RefCell::new(BTreeMap::<Failed, u64>::new());
    bench.measure(Case::new(format!("sysbench-style {}", script.name), "transactions").threads(threads), || {
        let run = totals.borrow().runs as u64;
        let (tallies, wall) =
            on_threads(threads, secs, |t, until| Terminal::new(connect.connect(USER), script, rows, run * 1_000 + t as u64 + 1).run(until));
        let events = tallies.iter().map(|t| t.events).sum();
        if totals.borrow_mut().count(wall, events) {
            for (code, n) in tallies.iter().flat_map(|t| t.failed.iter()) {
                *failed.borrow_mut().entry(*code).or_default() += n;
            }
        }
        events
    });
    totals.borrow().report(threads);
    let failed = failed.into_inner();
    if !failed.is_empty() {
        let each: Vec<String> = failed.iter().map(|(code, n)| format!("SQLCODE {} x{n}", code.0)).collect();
        note(&format!("failed: {}", each.join(", ")));
    }
    statement_rows(&mut connect.connect(USER), "%SBTEST%");
}
