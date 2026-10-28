//! How fast the pager reads and writes pages (RULES.md D30): writes and
//! reads of default-size pages, in order and scattered, on one and on
//! several threads; contents that overflow one page; and writes each
//! followed by a sync. Run with `cargo xtask bench`, on the harness
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

use harness::{Bench, Case, on_threads, scatter};
use turborel::disk::io::Durability;
use turborel::disk::pager::{DEFAULT_PAGE_SIZE, PageId, Pager};

/// Pages in the file: 256 MiB of default-size pages.
const PAGES: u64 = 32 * 1024;
/// Thread counts.
const THREADS: [usize; 2] = [1, 4];
/// Pages written, each followed by a sync, per run.
const SYNCED_WRITES: u64 = 100;
/// Content sizes that overflow, and the bytes of each written per run.
const OVERFLOW_SIZES: [usize; 2] = [64 * 1024, 1024 * 1024];
const OVERFLOW_BYTES: usize = 256 * 1024 * 1024;

/// A pass over every page.
#[derive(Clone, Copy)]
struct Workload {
    name: &'static str,
    scattered: bool,
    write: bool,
}

const WORKLOADS: [Workload; 4] = [
    Workload { name: "write in order", scattered: false, write: true },
    Workload { name: "read in order", scattered: false, write: false },
    Workload { name: "write scattered", scattered: true, write: true },
    Workload { name: "read scattered", scattered: true, write: false },
];

fn main() {
    let bench = Bench::new("pages");
    let pager = bench.pager("pages");
    pager.allocate(PAGES);
    let page = DEFAULT_PAGE_SIZE as usize;
    for threads in THREADS {
        for workload in WORKLOADS {
            bench.measure(Case::new(workload.name, "pages").threads(threads).bytes(page), || pass(&pager, threads, workload));
        }
    }
    for size in OVERFLOW_SIZES {
        // The pages may come in several extents.
        let extents = pager.allocate((OVERFLOW_BYTES / size) as u64);
        let ids: Vec<PageId> = extents.iter().flat_map(|e| (0..e.count).map(move |i| PageId(e.start.0 + i))).collect();
        let body = vec![3_u8; size];
        bench.measure(Case::new("write, overflowing content", "contents").bytes(size), || {
            for &id in &ids {
                pager.write(id, &body).unwrap();
            }
            ids.len() as u64
        });
        let mut content = Vec::new();
        bench.measure(Case::new("read, overflowing content", "contents").bytes(size), || {
            for &id in &ids {
                pager.read(id, &mut content).unwrap();
            }
            ids.len() as u64
        });
    }
    let payload = vec![1_u8; pager.inline_capacity()];
    for durability in [Durability::Data, Durability::Full] {
        bench.measure(Case::new(format!("write + sync {durability:?}"), "pages").bytes(page), || {
            for n in 0..SYNCED_WRITES {
                pager.write(PageId(n), &payload).unwrap();
                pager.sync(durability).unwrap();
            }
            SYNCED_WRITES
        });
    }
}

/// Runs `workload` over every page once, split between `threads` threads;
/// the pages read or written.
fn pass(pager: &Pager, threads: usize, workload: Workload) -> u64 {
    on_threads(threads, |t| {
        let mut payload = vec![7_u8; pager.inline_capacity()];
        for n in (t as u64..PAGES).step_by(threads) {
            let id = PageId(if workload.scattered { scatter(n, PAGES) } else { n });
            if workload.write {
                pager.write(id, &payload).unwrap();
            } else {
                pager.read(id, &mut payload).unwrap();
            }
        }
    });
    PAGES
}
