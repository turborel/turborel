//! How fast the buffer pool serves pages (RULES.md D30): reads of pages
//! that fit in the pool (hits) against the same reads from the pager, and
//! reads of more pages than the pool holds (misses and evictions), on one
//! and on several threads. Run with `cargo xtask bench`, on the harness
//! (RULES.md R-BENCH-1).

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
use turborel::disk::pager::{DEFAULT_PAGE_SIZE, PageId};

/// Pages in the file, frames in the pool, and reads per thread per run.
const PAGES: u64 = 4096;
const FRAMES: usize = 1024;
const READS: u64 = 100_000;
/// Pages that fit in the pool, with frames to spare.
const HOT: u64 = FRAMES as u64 - 24;
/// Thread counts.
const THREADS: [usize; 2] = [1, 4];

fn main() {
    let bench = Bench::new("buffer");
    let pager = Arc::new(bench.pager("pages"));
    pager.allocate(PAGES);
    let page = vec![5_u8; 4000];
    for id in 0..PAGES {
        pager.write(PageId(id), &page).unwrap();
    }
    let size = DEFAULT_PAGE_SIZE as usize;
    for threads in THREADS {
        let pool = harness::pool(FRAMES);
        let file = pool.add_pager(Arc::clone(&pager));
        // The harness's warm-up run brings the hot pages into the pool.
        bench.measure(Case::new("pool, pages that fit (hits)", "reads").threads(threads).bytes(size), || {
            reads(threads, |n| drop(pool.read(file, n % HOT).unwrap()))
        });
        bench.measure(Case::new("pager, the same pages", "reads").threads(threads).bytes(size), || {
            reads(threads, |n| pager.read(PageId(n % HOT), &mut Vec::new()).unwrap())
        });
        bench.measure(Case::new("pool, 4x the pages (misses)", "reads").threads(threads).bytes(size), || {
            reads(threads, |n| drop(pool.read(file, n % PAGES).unwrap()))
        });
    }
}

/// Calls `read` `READS` times on each of `threads` threads, with numbers in
/// a scattered order, which `read` takes modulo its page count; the reads
/// made.
fn reads(threads: usize, read: impl Fn(u64) + Sync) -> u64 {
    on_threads(threads, |t| {
        for n in 0..READS {
            read(scatter(n + t as u64 * READS, PAGES * HOT));
        }
    });
    threads as u64 * READS
}
