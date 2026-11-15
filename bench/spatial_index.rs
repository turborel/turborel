//! Paged spatial index workloads, independent of SQL parse, plan and row
//! materialization costs. Fixed seeds, checked answers and drained joins;
//! warmup and five measured runs on the shared harness (R-BENCH-1).
//! Set TURBOREL_SPATIAL_ROWS for scale (default 100000), and optionally
//! TURBOREL_SPATIAL_DISTRIBUTION to one of uniform, cluster, diagonal,
//! coincident or boxes. Coincident input is capped at 5000 entries to
//! expose tie and duplicate-placement costs without a quadratic setup.

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use harness::{Bench, Case, sampled_threads};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use turborel::engine::rtree::RTree;
use turborel::memory::MemoryBudget;
use turborel::spatial::Bounds;

/// Scale and query counts. Enough samples for tail percentiles, including
/// 5000 samples for single-thread cases over the five measured runs.
const ROWS: u64 = 100_000;
const QUERIES: usize = 1_000;
const THREADS: [usize; 4] = [1, 4, 8, 16];
const FRAMES: usize = 16_384;
const SMALL_FRAMES: usize = 64;
const JOIN_ROWS: u64 = 2_000;
/// Queries use a different deterministic portion of the coordinate stream.
const QUERY_OFFSET: u64 = 1_u64 << 32;
const DISTRIBUTIONS: [&str; 5] = ["uniform", "cluster", "diagonal", "coincident", "boxes"];

fn main() {
    let bench = Bench::new("spatial_index");
    let rows = std::env::var("TURBOREL_SPATIAL_ROWS").ok().map_or(ROWS, |s| s.parse().unwrap());
    let selected = std::env::var("TURBOREL_SPATIAL_DISTRIBUTION").ok();
    for distribution in DISTRIBUTIONS {
        if selected.as_ref().is_some_and(|s| s != distribution) {
            continue;
        }
        let count = if distribution == "coincident" { rows.min(5_000) } else { rows };
        suite(&bench, distribution, count);
    }
    if selected.is_none() || selected.as_deref() == Some("uniform") {
        small_pool(&bench, rows);
    }
}

fn suite(bench: &Bench, distribution: &str, rows: u64) {
    assert!(rows >= 16_000 || distribution == "coincident");
    let title = format!("{distribution}, {rows}");
    bench.measure_fresh(
        Case::new(format!("insert {title}"), "entries"),
        || fresh(bench, FRAMES),
        |empty| {
            fill(&empty, distribution, rows);
            rows
        },
    );
    let tree = fresh(bench, FRAMES);
    fill(&tree, distribution, rows);
    check(&tree, distribution, rows);
    let budget = MemoryBudget::new(256 * 1024 * 1024);
    for threads in THREADS {
        for width in [10.0_f64, 250.0_f64] {
            bench.measure_sampled(Case::new(format!("window{width} {title}"), "queries").threads(threads), || {
                sampled_threads(threads, QUERIES, |t, n| {
                    let window = window((t * QUERIES + n) as u64, width);
                    black_box(tree.scan(Some(window)).map(|e| e.unwrap().1).fold(0_u64, u64::wrapping_add));
                })
            });
        }
        for k in [1_usize, 10, 100] {
            bench.measure_sampled(Case::new(format!("nearest{k} {title}"), "queries").threads(threads), || {
                sampled_threads(threads, QUERIES, |t, n| {
                    let query = coordinate("uniform", QUERY_OFFSET + (t * QUERIES + n) as u64);
                    let found = tree.nearest(query, &budget).unwrap().take(k).map(|e| e.unwrap().2).fold(0_u64, u64::wrapping_add);
                    black_box(found);
                })
            });
        }
    }
    let other = fresh_named(bench, FRAMES, "join");
    for n in 0..JOIN_ROWS {
        other.insert(window(n, 10.0_f64), n).unwrap();
    }
    let expected: usize = (0..JOIN_ROWS).map(|n| tree.scan(Some(window(n, 10.0_f64))).map(Result::unwrap).count()).sum();
    bench.measure(Case::new(format!("join {title}"), "joins"), || {
        let count = tree.join(&other, &budget).unwrap().map(Result::unwrap).count();
        assert_eq!(count, expected);
        1
    });
    harness::note(&format!("join matches {expected}; splits {}, right-link traversals {}", tree.counts().0, tree.counts().1));
    if distribution != "coincident" {
        mixed(bench, &tree, distribution, rows);
    }
}

fn mixed(bench: &Bench, tree: &RTree, distribution: &str, rows: u64) {
    let generations: Vec<_> = (0..rows).map(|_| AtomicU64::new(0)).collect();
    let budget = MemoryBudget::new(256 * 1024 * 1024);
    for threads in THREADS {
        bench.measure_sampled(
            Case::new(format!("40% windows/40% nearest/20% moves {distribution}, {rows}"), "operations").threads(threads),
            || {
                sampled_threads(threads, QUERIES, |t, n| {
                    let row = (t * QUERIES + n) as u64 % rows;
                    if n % 5 == 0 {
                        let generation = generations[usize::try_from(row).unwrap()].fetch_add(1, Ordering::Relaxed);
                        let old = coordinate(distribution, row + generation * rows);
                        let new = coordinate(distribution, row + (generation + 1) * rows);
                        tree.insert(new, row).unwrap();
                        tree.remove(old, row).unwrap();
                    } else if n % 2 == 0 {
                        let query = coordinate("uniform", QUERY_OFFSET + row);
                        black_box(tree.nearest(query, &budget).unwrap().take(10).map(Result::unwrap).count());
                    } else {
                        black_box(tree.scan(Some(window(row, 10.0_f64))).map(Result::unwrap).count());
                    }
                })
            },
        );
    }
    let mut found: Vec<_> = tree.scan(None).map(|e| e.unwrap().1).collect();
    found.sort_unstable();
    assert_eq!(found, (0..rows).collect::<Vec<_>>());
    bench.measure_sampled(Case::new(format!("aged window10 {distribution}, {rows}"), "queries"), || {
        sampled_threads(1, QUERIES, |_, n| {
            black_box(tree.scan(Some(window(n as u64, 10.0_f64))).map(Result::unwrap).count());
        })
    });
}

fn small_pool(bench: &Bench, rows: u64) {
    let tree = fresh(bench, SMALL_FRAMES);
    fill(&tree, "uniform", rows);
    for threads in [1_usize, 4] {
        bench.measure_sampled(Case::new(format!("64 frames window10 uniform, {rows}"), "queries").threads(threads), || {
            sampled_threads(threads, QUERIES, |t, n| {
                black_box(tree.scan(Some(window((t * QUERIES + n) as u64, 10.0_f64))).map(Result::unwrap).count());
            })
        });
    }
}

fn fresh(bench: &Bench, frames: usize) -> RTree {
    fresh_named(bench, frames, "tree")
}

fn fresh_named(bench: &Bench, frames: usize, name: &str) -> RTree {
    let pool = harness::pool(frames);
    let file = pool.add_pager(Arc::new(bench.pager(name)));
    RTree::create(pool, file).unwrap()
}

fn fill(tree: &RTree, distribution: &str, rows: u64) {
    for row in 0..rows {
        tree.insert(coordinate(distribution, row), row).unwrap();
    }
}

fn window(n: u64, width: f64) -> Bounds {
    let lower = coordinate("uniform", QUERY_OFFSET + n).lower();
    Bounds::new(lower, lower.map(|x| x + width)).unwrap()
}

/// SplitMix64's fixed finalizer mixes each axis independently. Repeated
/// benchmark runs get identical datasets and query positions, no RNG.
fn unit(mut n: u64) -> f64 {
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    n ^= n >> 31_u32;
    (n >> 11) as f64 / (1_u64 << 53) as f64
}

fn coordinate(distribution: &str, n: u64) -> Bounds {
    let x = unit(n.wrapping_add(0x9e37_79b9_7f4a_7c15)) * 1_000.0_f64;
    let y = unit(n.wrapping_add(0x6a09_e667_f3bc_c909)) * 1_000.0_f64;
    let (lower, upper) = match distribution {
        "cluster" => {
            let centre = (n % 16) as f64 * 60.0_f64;
            let p = [centre + x / 100.0_f64, centre + y / 100.0_f64];
            (p, p)
        }
        "diagonal" => ([x, x], [x, x]),
        "coincident" => ([500.0_f64, 500.0_f64], [500.0_f64, 500.0_f64]),
        "boxes" => ([x, y], [x + 50.0_f64, y + 50.0_f64]),
        other => {
            assert_eq!(other, "uniform");
            ([x, y], [x, y])
        }
    };
    Bounds::new(lower, upper).unwrap()
}

fn check(tree: &RTree, distribution: &str, rows: u64) {
    let budget = MemoryBudget::new(usize::MAX);
    for n in 0..10 {
        let query = window(n, 10.0_f64);
        let expected: Vec<_> = (0..rows).filter(|&id| query.intersects(coordinate(distribution, id))).collect();
        let mut found: Vec<_> = tree.scan(Some(query)).map(|e| e.unwrap().1).collect();
        found.sort_unstable();
        assert_eq!(found, expected);
        let mut nearest: Vec<_> = (0..rows).map(|id| (query.distance(coordinate(distribution, id)), id)).collect();
        nearest.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let neighbours: Vec<_> = tree.nearest(query, &budget).unwrap().take(100).map(|e| e.map(|(d, _, id)| (d, id)).unwrap()).collect();
        assert_eq!(neighbours, nearest[..nearest.len().min(100)]);
    }
}
