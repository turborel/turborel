//! How fast the built-in index methods work through the `Index` trait the
//! database uses: "btree" (`index/blink.rs`) and "hash" (`index/hash.rs`),
//! entries added under scattered integer keys, found again by equality as
//! a query's probe and as a UNIQUE check, on one and on several threads,
//! in a buffer pool that holds every index. Run with `cargo xtask bench`,
//! on the harness (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::sync::Arc;

use harness::{Bench, Case, on_threads, scatter};
use turborel::index::{BTreeMethod, HashMethod, Index, IndexMethod, Key, KeyOrder, Probe};
use turborel::value::Spelled;
use turborel::value::{CompOp, DataType, Decimal, Value};

/// Entries per thread.
const ENTRIES: u64 = 200_000;
/// Frames: enough for every index.
const FRAMES: usize = 32_768;
/// Thread counts.
const THREADS: [usize; 3] = [1, 4, 8];
/// The indexed column: an INTEGER.
const COLUMN: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };

fn main() {
    let bench = Bench::new("indexes");
    for method in ["btree", "hash"] {
        for threads in THREADS {
            workloads(&bench, method, threads);
        }
    }
}

/// The workloads of `method` on `threads` threads.
fn workloads(bench: &Bench, method: &str, threads: usize) {
    let total = threads as u64 * ENTRIES;
    let scattered = |t: usize, n: u64| scatter(t as u64 * ENTRIES + n, total);
    bench.measure_fresh(
        Case::new(format!("insert, keys scattered, {method}"), "entries").threads(threads),
        || index(bench, method),
        |index| insert(&*index, threads, scattered),
    );
    let full: Arc<dyn Index> = Arc::from(index(bench, method));
    insert(&*full, threads, scattered);
    let probed = |t: usize| {
        (0..ENTRIES).all(|n| {
            let probe = Probe::Compare { op: CompOp::Eq, value: number(scattered(t, n)) };
            Arc::clone(&full).search(&probe, KeyOrder::Ascending).map(Result::unwrap).count() == 1
        })
    };
    bench.measure(Case::new(format!("search, equality, {method}"), "probes").threads(threads), || {
        on_threads(threads, |t| assert!(probed(t)));
        total
    });
    let checked = |t: usize| (0..ENTRIES).all(|n| full.rows(&key(total + scattered(t, n))).unwrap().is_empty());
    bench.measure(Case::new(format!("rows, key absent (a UNIQUE check), {method}"), "probes").threads(threads), || {
        on_threads(threads, |t| assert!(checked(t)));
        total
    });
}

/// A new, empty index of `method`, in a new pager file and a pool that holds it.
fn index(bench: &Bench, method: &str) -> Box<dyn Index> {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("indexes")));
    let made: Box<dyn IndexMethod> = if method == "hash" {
        Box::new(HashMethod::new(Arc::clone(&pool), file))
    } else {
        Box::new(BTreeMethod::new(Arc::clone(&pool), file))
    };
    made.create(&[COLUMN]).unwrap()
}

/// Inserts `ENTRIES` entries from each of `threads` threads, thread `t`'s
/// `n`th under key `key_of(t, n)`, its row the key; the entries inserted.
fn insert(index: &dyn Index, threads: usize, key_of: impl Fn(usize, u64) -> u64 + Sync) -> u64 {
    on_threads(threads, |t| {
        for n in 0..ENTRIES {
            let k = key_of(t, n);
            index.insert(key(k), k).unwrap();
        }
    });
    threads as u64 * ENTRIES
}

fn number(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

fn key(n: u64) -> Key {
    Key(vec![number(n)])
}
