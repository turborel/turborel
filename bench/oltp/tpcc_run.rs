//! A TPROC-C-style case: one warehouse loaded anew for each thread count,
//! as the measurement box restores it for each, its procedures those of
//! the set named, and its terminals run on the harness. Beside the rate
//! go NOPM, the New Orders committed a minute, counted as the driver
//! counts them, by the growth of the districts' next order numbers; each
//! type's share and mean latency; the failures by SQLCODE; and each CALL's
//! rows, as the database's statement statistics count them.

use std::cell::RefCell;

use turborel::db::Database;

use crate::client::Connect;
use crate::harness::{Bench, Case, note};
use crate::measure::{Statements, Totals, index_rows, index_use, on_threads, statement_rows, statements};
use crate::tpcc::{TYPES, Tally, Terminal, orders_taken};
use crate::{tpcc_load, tpcc_procs, tpcc_procs_new};

/// The user, and schema, the workload runs as.
pub const USER: &str = "TPCC";

/// The procedures of set `name`.
pub fn procedures(name: &str) -> [&'static str; 5] {
    match name {
        "old" => [tpcc_procs::NEWORD, tpcc_procs::PAYMENT, tpcc_procs::DELIVERY, tpcc_procs::OSTAT, tpcc_procs::SLEV],
        "new" => [tpcc_procs_new::NEWORD, tpcc_procs::PAYMENT, tpcc_procs_new::DELIVERY, tpcc_procs::OSTAT, tpcc_procs_new::SLEV],
        other => panic!("TURBOREL_OLTP_PROCEDURES names old or new, not {other}"),
    }
}

/// Creates the schema in `db` and loads it, with procedure set `set`.
pub fn build(db: &Database, set: &str) {
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION {USER}")).unwrap();
    tpcc_load::load(&db.session(USER), &procedures(set));
}

/// What the timed runs did together.
#[derive(Default)]
struct Figures {
    totals: Totals,
    tally: Tally,
    orders: i64,
    /// The CALLs' statistics before the first run and after each.
    seen: Vec<Statements>,
    /// The indexes' use after the untimed run and after the last.
    indexes: Vec<Statements>,
}

/// Measures the TPROC-C-style workload on `threads` terminals of `connect`,
/// for `secs` each run, its procedure set named `set`, the case named with
/// the store's settings `store`.
pub fn measure<K: Connect>(bench: &Bench, connect: &K, (set, threads, store): (&str, usize, &str), secs: std::time::Duration) {
    let figures = RefCell::new(Figures::default());
    figures.borrow_mut().seen.push(statements(&mut connect.connect(USER), CALLS));
    let label = format!("tpcc {set} {store} {}s", secs.as_secs_f64());
    bench.measure(Case::new(label, "transactions").threads(threads), || {
        let mut admin = connect.connect(USER);
        let before = orders_taken(&mut admin);
        let run = figures.borrow().totals.runs as u64;
        let (tallies, wall) = on_threads(threads, secs, |t, until| {
            let seed = run * 1_000 + t as u64 + 1;
            Terminal::new(connect.connect(USER), seed).run(until)
        });
        let mut tally = Tally::default();
        for t in &tallies {
            tally.add(t);
        }
        let orders = orders_taken(&mut admin) - before;
        let mut f = figures.borrow_mut();
        f.seen.push(statements(&mut admin, CALLS));
        if f.indexes.len() < 2 {
            f.indexes.push(index_use(&mut admin));
        } else {
            f.indexes[1] = index_use(&mut admin);
        }
        if f.totals.count(wall, tally.total()) {
            f.tally.add(&tally);
            f.orders += orders;
        }
        tally.total()
    });
    let f = figures.into_inner();
    report(&f, threads);
    let (first, last) = (f.seen.len().min(2), f.seen.len());
    statement_rows("rows a CALL in the first, untimed, run:", &f.seen[0], &f.seen[first - 1]);
    statement_rows("rows a CALL in the last run:", &f.seen[last.saturating_sub(2)], &f.seen[last - 1]);
    statement_rows("rows a CALL in the timed runs:", &f.seen[first - 1], &f.seen[last - 1]);
    if let [untimed, timed] = &f.indexes[..] {
        index_rows(untimed, timed, f.totals.transactions);
    }
}

/// The statements whose statistics are reported.
const CALLS: &str = "CALL%";

fn report(f: &Figures, threads: usize) {
    let minutes = f.totals.wall.as_secs_f64() / 60.0_f64;
    note(&format!("NOPM {:.0}, TPM {:.0}", f.orders as f64 / minutes, f.totals.transactions as f64 / minutes));
    f.totals.report(threads);
    let total = f.tally.total().max(1) as f64;
    let each: Vec<String> = TYPES
        .iter()
        .enumerate()
        .map(|(t, name)| {
            let done = f.tally.done[t];
            let mean = f.tally.time[t].as_secs_f64() * 1e3_f64 / done.max(1) as f64;
            format!("{name} {:.2}% {mean:.3} ms", done as f64 * 100.0_f64 / total)
        })
        .collect();
    note(&format!("share and mean latency: {}", each.join(", ")));
    if !f.tally.failed.is_empty() {
        let failed: Vec<String> = f.tally.failed.iter().map(|((t, code), n)| format!("{} SQLCODE {} x{n}", TYPES[*t], code.0)).collect();
        note(&format!("failed: {}", failed.join(", ")));
    }
}
