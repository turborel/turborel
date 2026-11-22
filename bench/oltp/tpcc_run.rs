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
        "old" | "native" => [tpcc_procs::NEWORD, tpcc_procs::PAYMENT, tpcc_procs::DELIVERY, tpcc_procs::OSTAT, tpcc_procs::SLEV],
        "new" => [tpcc_procs_new::NEWORD, tpcc_procs::PAYMENT, tpcc_procs_new::DELIVERY, tpcc_procs::OSTAT, tpcc_procs_new::SLEV],
        other => panic!("TURBOREL_OLTP_PROCEDURES names old, new or native, not {other}"),
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
            Terminal::new(connect.connect(USER), seed, set == "native").run(until)
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
    consistency(&mut connect.connect(USER));
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

/// The TPROC-C consistency conditions, checked by SQL once the runs are
/// done: W_YTD is the sum of D_YTD; of each district, D_NEXT_O_ID - 1 is
/// the greatest O_ID and the greatest NO_O_ID, the orders are numbered
/// with no gap, the new orders' numbers likewise, and the order lines are
/// as many as the orders' O_OL_CNT add to.
fn consistency(client: &mut impl crate::client::Client) {
    let mut query = |sql: &str| -> Vec<Vec<f64>> {
        client.direct(sql, true).unwrap().into_iter().map(|r| r.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect()).collect()
    };
    let mut failed = Vec::new();
    let w = query("SELECT W_YTD FROM WAREHOUSE");
    let d = query("SELECT SUM(D_YTD) FROM DISTRICT");
    if (w[0][0] - d[0][0]).abs() > 0.001 {
        failed.push(format!("W_YTD {} <> SUM(D_YTD) {}", w[0][0], d[0][0]));
    }
    let next = query("SELECT D_ID, D_NEXT_O_ID FROM DISTRICT ORDER BY D_ID");
    let orders = query("SELECT O_D_ID, MAX(O_ID), COUNT(*), SUM(O_OL_CNT) FROM ORDERS GROUP BY O_D_ID ORDER BY O_D_ID");
    let new = query("SELECT NO_D_ID, MAX(NO_O_ID), MIN(NO_O_ID), COUNT(*) FROM NEW_ORDER GROUP BY NO_D_ID ORDER BY NO_D_ID");
    let lines = query("SELECT OL_D_ID, COUNT(*) FROM ORDER_LINE GROUP BY OL_D_ID ORDER BY OL_D_ID");
    let (mut order_count, mut line_count) = (0.0_f64, 0.0_f64);
    for (i, n) in next.iter().enumerate() {
        let (o, l) = (&orders[i], &lines[i]);
        order_count += o[2];
        line_count += l[1];
        if n[1] - 1.0_f64 != o[1] {
            failed.push(format!("district {}: D_NEXT_O_ID - 1 {} <> MAX(O_ID) {}", n[0], n[1] - 1.0_f64, o[1]));
        }
        if o[2] != o[1] {
            failed.push(format!("district {}: COUNT(ORDERS) {} <> MAX(O_ID) {}", n[0], o[2], o[1]));
        }
        if o[3] != l[1] {
            failed.push(format!("district {}: SUM(O_OL_CNT) {} <> COUNT(ORDER_LINE) {}", n[0], o[3], l[1]));
        }
        match new.iter().find(|r| r[0] == n[0]) {
            Some(r) => {
                if r[1] != n[1] - 1.0_f64 {
                    failed.push(format!("district {}: MAX(NO_O_ID) {} <> D_NEXT_O_ID - 1 {}", n[0], r[1], n[1] - 1.0_f64));
                }
                if r[1] - r[2] + 1.0_f64 != r[3] {
                    failed.push(format!("district {}: NEW_ORDER numbers {}..{} <> COUNT {}", n[0], r[2], r[1], r[3]));
                }
            }
            None => failed.push(format!("district {}: no NEW_ORDER rows", n[0])),
        }
    }
    let total: f64 = next.iter().map(|n| n[1] - 1.0_f64).sum();
    if failed.is_empty() {
        note(&format!(
            "consistency: all hold (W_YTD {:.2}, {} orders, {} order lines, {} new orders)",
            w[0][0],
            order_count,
            line_count,
            new.iter().map(|r| r[3]).sum::<f64>()
        ));
    } else {
        note(&format!("consistency: {} FAILED of {total} orders: {}", failed.len(), failed.join("; ")));
    }
}
