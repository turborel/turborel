//! How fast the linear hash table works (`engine/hash.rs`): entries added
//! under scattered keys, found again, and removed until it shrinks back,
//! on one and on several threads; in a buffer pool that holds the whole
//! table, and in one a sixteenth of its size, where most of it is in its
//! file, as when an operator spills. Run with `cargo xtask bench`, on the
//! harness (RULES.md R-BENCH-1).

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
use turborel::engine::hash::HashTable;

/// Entries per thread.
const ENTRIES: u64 = 200_000;
/// Frames: enough for the whole table at eight threads, of 8 KiB pages
/// about three quarters full of entries of 62 bytes.
const FRAMES: usize = 32_768;
/// The frames of a sixteenth of the table of `threads` threads.
fn few_frames(threads: usize) -> usize {
    (threads as u64 * ENTRIES * 62 * 4 / 3 / 8_192 / 16) as usize
}
/// Thread counts.
const THREADS: [usize; 3] = [1, 4, 8];
/// A value as long as a short row's record.
const VALUE: [u8; 40] = [7; 40];

fn main() {
    let bench = Bench::new("hashtable");
    for threads in THREADS {
        workloads(&bench, (FRAMES, "pool holds it"), threads);
    }
    for threads in THREADS {
        workloads(&bench, (few_frames(threads), "pool holds a sixteenth"), threads);
    }
}

/// The workloads on `threads` threads, in a pool of `frames` frames, which
/// `held` says how much of the table it holds.
fn workloads(bench: &Bench, (frames, held): (usize, &str), threads: usize) {
    let total = threads as u64 * ENTRIES;
    let scattered = |t: usize, n: u64| scatter(t as u64 * ENTRIES + n, total);
    bench.measure_fresh(
        Case::new(format!("insert, keys scattered, {held}"), "entries").threads(threads),
        || table(bench, frames),
        |table| insert(&table, threads, scattered),
    );
    let full = table(bench, frames);
    insert(&full, threads, scattered);
    let found = |t: usize| (0..ENTRIES).all(|n| full.get(&key(scattered(t, n))).unwrap().len() == 1);
    bench.measure(Case::new(format!("get, keys scattered, {held}"), "entries").threads(threads), || {
        on_threads(threads, |t| assert!(found(t)));
        total
    });
    let absent = |t: usize| (0..ENTRIES).all(|n| full.get(&key(total + scattered(t, n))).unwrap().is_empty());
    bench.measure(Case::new(format!("get, keys absent, {held}"), "entries").threads(threads), || {
        on_threads(threads, |t| assert!(absent(t)));
        total
    });
    if threads > 1 {
        return;
    }
    bench.measure_fresh(
        Case::new(format!("remove every entry, shrinking, {held}"), "entries"),
        || {
            let fresh = table(bench, frames);
            insert(&fresh, 1, scattered);
            fresh
        },
        |emptied| (0..ENTRIES).filter(|&n| emptied.remove(&key(scattered(0, n)), &VALUE).unwrap()).count() as u64,
    );
}

/// A new, empty table, in a new pager file and a pool of `frames` frames.
fn table(bench: &Bench, frames: usize) -> HashTable {
    let pool = harness::pool(frames);
    let file = pool.add_pager(Arc::new(bench.pager("hashtable")));
    HashTable::create(pool, file).unwrap()
}

/// Inserts `ENTRIES` entries from each of `threads` threads, thread `t`'s
/// `n`th under key `key_of(t, n)`; the entries inserted.
fn insert(table: &HashTable, threads: usize, key_of: impl Fn(usize, u64) -> u64 + Sync) -> u64 {
    on_threads(threads, |t| {
        for n in 0..ENTRIES {
            table.insert(&key(key_of(t, n)), &VALUE).unwrap();
        }
    });
    threads as u64 * ENTRIES
}

fn key(n: u64) -> [u8; 8] {
    n.to_be_bytes()
}
