//! The "btree" method by key pattern (RULES.md D500): entries inserted in
//! key order per thread, scattered, and at a hot right edge every thread
//! appends to at once; then found by equality, and read by short ranges of
//! a hundred keys, on one and on several threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::harness::{Bench, Case, on_threads, scatter};
use super::{ENTRIES, index, key, number};
use turborel::index::{Index, KeyOrder, Probe};
use turborel::value::CompOp;

/// Thread counts.
const THREADS: [usize; 2] = [1, 8];
/// Keys a range reads.
const RANGE: usize = 100;

/// Each pattern's workloads, on each thread count.
pub fn workloads(bench: &Bench) {
    for threads in THREADS {
        let total = threads as u64 * ENTRIES;
        let in_order = move |t: usize, n: u64| t as u64 * ENTRIES + n;
        let scattered = move |t: usize, n: u64| scatter(t as u64 * ENTRIES + n, total);
        pattern(bench, "in order", threads, in_order);
        pattern(bench, "scattered", threads, scattered);
        let edge = AtomicU64::new(0);
        bench.measure_fresh(
            Case::new("insert, hot right edge, btree", "entries").threads(threads),
            || index(bench, "btree"),
            |index| {
                edge.store(0, Ordering::Relaxed);
                on_threads(threads, |_| {
                    for _ in 0..ENTRIES {
                        let k = edge.fetch_add(1, Ordering::Relaxed);
                        index.insert(key(k), k).unwrap();
                    }
                });
                total
            },
        );
    }
}

/// Inserts, lookups and ranges of keys `key_of` gives, on `threads` threads.
fn pattern(bench: &Bench, name: &str, threads: usize, key_of: impl Fn(usize, u64) -> u64 + Sync + Copy) {
    let total = threads as u64 * ENTRIES;
    let fill = |index: &dyn Index| {
        on_threads(threads, |t| {
            for n in 0..ENTRIES {
                let k = key_of(t, n);
                index.insert(key(k), k).unwrap();
            }
        });
    };
    bench.measure_fresh(
        Case::new(format!("insert, {name}, btree"), "entries").threads(threads),
        || index(bench, "btree"),
        |index| {
            fill(&*index);
            total
        },
    );
    let built: Arc<dyn Index> = Arc::from(index(bench, "btree"));
    fill(&*built);
    let found = |op: CompOp, k: u64, take: usize| {
        let probe = Probe::Compare { op, value: number(k) };
        Arc::clone(&built).search(&probe, KeyOrder::Ascending).take(take).map(Result::unwrap).count()
    };
    bench.measure(Case::new(format!("lookup, {name}, btree"), "probes").threads(threads), || {
        on_threads(threads, |t| assert!((0..ENTRIES).all(|n| found(CompOp::Eq, key_of(t, n), 1) == 1)));
        total
    });
    let ranges = ENTRIES / 10;
    bench.measure(Case::new(format!("range of {RANGE}, {name}, btree"), "ranges").threads(threads), || {
        on_threads(threads, |t| {
            let read: usize = (0..ranges).map(|n| found(CompOp::Ge, scatter(t as u64 * ranges + n, total - RANGE as u64), RANGE)).sum();
            assert_eq!(read, ranges as usize * RANGE);
        });
        threads as u64 * ranges
    });
}
