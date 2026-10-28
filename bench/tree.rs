//! How fast the B-link tree works (RULES.md D31): entries added in key
//! order (as row ids are) and in a scattered order, found again, and
//! scanned, on one and on several threads, in a buffer pool that holds the
//! whole tree. Run with `cargo xtask bench`, on the harness (RULES.md
//! R-BENCH-1).

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

use std::ops::Bound;
use std::sync::Arc;

use harness::{Bench, Case, on_threads, scatter};
use turborel::engine::tree::{BLinkTree, Direction};

/// Entries per thread, and frames: enough for the whole tree.
const ENTRIES: u64 = 200_000;
const FRAMES: usize = 32_768;
/// Thread counts.
const THREADS: [usize; 3] = [1, 4, 8];
/// A value as long as a short row's record.
const VALUE: [u8; 40] = [7; 40];

fn main() {
    let bench = Bench::new("tree");
    for threads in THREADS {
        let total = threads as u64 * ENTRIES;
        // Each thread its own block of keys in order, as sessions given
        // blocks of row ids are.
        let in_order = |t: usize, n: u64| t as u64 * ENTRIES + n;
        let scattered = |t: usize, n: u64| scatter(t as u64 * ENTRIES + n, total);
        bench.measure_fresh(
            Case::new("insert, keys in order", "entries").threads(threads),
            || tree(&bench),
            |tree| insert(&tree, threads, in_order),
        );
        bench.measure_fresh(
            Case::new("insert, keys scattered", "entries").threads(threads),
            || tree(&bench),
            |tree| insert(&tree, threads, scattered),
        );
        let tree = tree(&bench);
        insert(&tree, threads, scattered);
        bench.measure(Case::new("get, keys scattered", "entries").threads(threads), || {
            on_threads(threads, |t| {
                for n in 0..ENTRIES {
                    tree.get(&key(scattered(t, n))).unwrap().unwrap();
                }
            });
            total
        });
        bench.measure(Case::new("scan forward, every entry", "entries").threads(threads), || {
            on_threads(threads, |_| {
                tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).count();
            });
            threads as u64 * total
        });
        if threads == 1 {
            bench.measure(Case::new("scan backward, every entry", "entries"), || {
                tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Backward).count() as u64
            });
        }
    }
}

/// A new, empty tree, in a new pager file and a pool that holds it.
fn tree(bench: &Bench) -> BLinkTree {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("tree")));
    BLinkTree::create(pool, file).unwrap()
}

/// Inserts `ENTRIES` entries from each of `threads` threads, thread `t`'s
/// `n`th under key `key_of(t, n)`; the entries inserted.
fn insert(tree: &BLinkTree, threads: usize, key_of: impl Fn(usize, u64) -> u64 + Sync) -> u64 {
    on_threads(threads, |t| {
        for n in 0..ENTRIES {
            tree.insert(&key(key_of(t, n)), &VALUE).unwrap();
        }
    });
    threads as u64 * ENTRIES
}

fn key(n: u64) -> [u8; 8] {
    n.to_be_bytes()
}
