//! How fast the B-link tree works (RULES.md D31): entries added in key
//! order (as row ids are) and in a scattered order, found again, and
//! scanned, on one and on several threads, in a buffer pool that holds the
//! whole tree; then the same in a small pool, a tenth of the tree's pages
//! or so, so most reads miss and read their page from the file, as a
//! table larger than STORAGE.POOL_BYTES is read; its scans with read-ahead
//! and without (`BufferPool::read_ahead`). Run with `cargo xtask bench`, on
//! the harness (RULES.md R-BENCH-1).

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
use turborel::disk::buffer::{BufferPool, READ_AHEAD_PAGES};
use turborel::engine::tree::{BLinkTree, Direction};

/// Entries per thread, and frames: enough for the whole tree.
const ENTRIES: u64 = 200_000;
const FRAMES: usize = 32_768;
/// Thread counts.
const THREADS: [usize; 3] = [1, 4, 8];
/// A value as long as a short row's record.
const VALUE: [u8; 40] = [7; 40];
/// Frames of the small pool: 1 MiB of 4 KiB pages, about a tenth of a
/// tree of `ENTRIES` entries.
const SMALL_FRAMES: usize = 256;
/// Thread counts in the small pool.
const SMALL_THREADS: [usize; 2] = [1, 4];

fn main() {
    let bench = Bench::new("tree");
    in_a_large_pool(&bench);
    in_a_small_pool(&bench);
}

/// The tree in a pool that holds it whole.
fn in_a_large_pool(bench: &Bench) {
    for threads in THREADS {
        let total = threads as u64 * ENTRIES;
        // Each thread its own block of keys in order, as sessions given
        // blocks of row ids are.
        let in_order = |t: usize, n: u64| t as u64 * ENTRIES + n;
        let scattered = |t: usize, n: u64| scatter(t as u64 * ENTRIES + n, total);
        bench.measure_fresh(
            Case::new("insert, keys in order", "entries").threads(threads),
            || tree(bench, FRAMES),
            |tree| insert(&tree, threads, in_order),
        );
        bench.measure_fresh(
            Case::new("insert, keys scattered", "entries").threads(threads),
            || tree(bench, FRAMES),
            |tree| insert(&tree, threads, scattered),
        );
        let tree = tree(bench, FRAMES);
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

/// The tree in a pool of `SMALL_FRAMES`, most of its pages on the file
/// alone: `ENTRIES` entries added in key order, scattered, found again
/// scattered, and scanned forward, each thread reading the whole tree.
fn in_a_small_pool(bench: &Bench) {
    let scattered = |_: usize, n: u64| scatter(n, ENTRIES);
    bench.measure_fresh(
        Case::new("small pool: insert, keys in order", "entries"),
        || tree(bench, SMALL_FRAMES),
        |tree| insert(&tree, 1, |_, n| n),
    );
    bench.measure_fresh(
        Case::new("small pool: insert, keys scattered", "entries"),
        || tree(bench, SMALL_FRAMES),
        |tree| insert(&tree, 1, scattered),
    );
    let pool = harness::pool(SMALL_FRAMES);
    let tree = tree_in(bench, &pool);
    insert(&tree, 1, |_, n| n);
    for threads in SMALL_THREADS {
        pool.set_read_ahead(0);
        bench.measure(Case::new("small pool: scan, no read-ahead", "entries").threads(threads), || {
            on_threads(threads, |_| {
                tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).count();
            });
            threads as u64 * ENTRIES
        });
        pool.set_read_ahead(READ_AHEAD_PAGES);
        bench.measure(Case::new("small pool: get, keys scattered", "entries").threads(threads), || {
            on_threads(threads, |t| {
                for n in 0..ENTRIES {
                    tree.get(&key(scatter(n + t as u64, ENTRIES))).unwrap().unwrap();
                }
            });
            threads as u64 * ENTRIES
        });
        bench.measure(Case::new("small pool: scan forward", "entries").threads(threads), || {
            on_threads(threads, |_| {
                tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).count();
            });
            threads as u64 * ENTRIES
        });
    }
}

/// A new, empty tree, in a new pager file and a pool of `frames`.
fn tree(bench: &Bench, frames: usize) -> BLinkTree {
    tree_in(bench, &harness::pool(frames))
}

/// A new, empty tree, in a new pager file in `pool`.
fn tree_in(bench: &Bench, pool: &Arc<BufferPool>) -> BLinkTree {
    let file = pool.add_pager(Arc::new(bench.pager("tree")));
    BLinkTree::create(Arc::clone(pool), file).unwrap()
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
