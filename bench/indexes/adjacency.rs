//! How fast an adjacency index (`index/adjacency.rs`) is built and
//! followed beside a btree on the source column: a million edges among a
//! hundred thousand vertices, inserted through the `Index` trait, the
//! adjacency index then compacted (`Index::maintain`), and made by
//! CREATE INDEX of a table of them, a bulk load; and a vertex's
//! steps (`Adjacency::steps`) read of its compacted adjacency, of its
//! tree alone, and as the btree's entries of an equality of the source
//! (RULES.md R-BENCH-1).

use std::hint::black_box;
use std::sync::Arc;

use super::harness::{self, Bench, Case, scatter};
use super::number;
use turborel::db::Database;
use turborel::graph::adjacency::{Adjacency, Direction};
use turborel::index::{AdjacencyMethod, BTreeMethod, Index, IndexMethod, Key, KeyOrder, Probe};
use turborel::memory::MemoryBudget;
use turborel::value::CompOp;

/// Edges indexed.
const EDGES: u64 = 1_000_000;
/// Vertices they join: ten edges each way of a vertex, on average.
const VERTICES: u64 = 100_000;
/// Vertices whose steps a run reads.
const LOOKUPS: u64 = 100_000;
/// Frames: enough for either index.
const FRAMES: usize = 98_304;
/// The memory budget of a compacted adjacency.
const BUDGET: usize = 1 << 32;

/// Edge `n`: its source and destination, scattered.
fn edge(n: u64) -> (u64, u64) {
    (scatter(n, VERTICES), scatter(n.wrapping_mul(7).wrapping_add(3), VERTICES))
}

/// A new, empty adjacency index on (source, destination), of a budget if
/// `compacted`, in a new pager file.
fn adjacency(bench: &Bench, compacted: bool) -> Arc<dyn Index> {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("adjacency")));
    let method = AdjacencyMethod::new(pool, file);
    let method = if compacted { method.with_budget(MemoryBudget::new(BUDGET)) } else { method };
    Arc::from(method.create(&[super::COLUMN, super::COLUMN]).unwrap())
}

/// A new, empty btree on the source, in a new pager file.
fn btree(bench: &Bench) -> Arc<dyn Index> {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("btree")));
    Arc::from(BTreeMethod::new(pool, file).create(&[super::COLUMN]).unwrap())
}

/// Inserts every edge into `index`, of key (source, destination), or
/// (source) alone if `source`; its row its number.
fn fill(index: &dyn Index, source: bool) -> u64 {
    for n in 0..EDGES {
        let (s, d) = edge(n);
        let key = if source { Key(vec![number(s)]) } else { Key(vec![number(s), number(d)]) };
        index.insert(key, n).unwrap();
    }
    EDGES
}

/// The steps of `LOOKUPS` vertices: each one's edges counted.
fn steps(adjacency: &dyn Adjacency) -> u64 {
    let mut out = Vec::new();
    for n in 0..LOOKUPS {
        out.clear();
        adjacency.steps(&Key(vec![number(scatter(n, VERTICES))]), Direction::Out, &mut out).unwrap();
        black_box(&out);
    }
    LOOKUPS
}

/// The entries of an equality of the source of `LOOKUPS` vertices, of `index`.
fn probes(index: &Arc<dyn Index>) -> u64 {
    for n in 0..LOOKUPS {
        let probe = Probe::Compare { op: CompOp::Eq, value: number(scatter(n, VERTICES)) };
        black_box(Arc::clone(index).search(&probe, KeyOrder::Ascending).map(Result::unwrap).count());
    }
    LOOKUPS
}

/// A table E of every edge, (ID, SRC, DST), in schema B.
fn table() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE E (ID INTEGER NOT NULL, SRC INTEGER NOT NULL, DST INTEGER NOT NULL)").unwrap();
    let s = db.session("B");
    let edges: Vec<String> = (0..EDGES)
        .map(|n| {
            let (src, dst) = edge(n);
            format!("({n}, {src}, {dst})")
        })
        .collect();
    for chunk in edges.chunks(2_000) {
        s.run(&format!("INSERT INTO E VALUES {}", chunk.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db
}

/// CREATE INDEX of every edge of `db`'s table, by `method` on `columns`,
/// a bulk load, after the index of the run before is dropped, untimed.
fn created(bench: &Bench, db: &Database, method: &str, columns: &str) {
    let s = db.session("B");
    bench.measure_fresh(
        Case::new(format!("CREATE INDEX of 1M edges, {method}"), "edges"),
        || {
            let _ = s.run("DROP INDEX E_I");
            s.commit().unwrap();
        },
        |()| {
            s.run(&format!("CREATE INDEX E_I ON E ({columns}) USING {method}")).unwrap();
            EDGES
        },
    );
}

/// The workloads: building each index, then reading each.
pub fn workloads(bench: &Bench) {
    let db = table();
    created(bench, &db, "adjacency", "SRC, DST");
    created(bench, &db, "btree", "SRC");
    drop(db);
    bench.measure_fresh(
        Case::new("adjacency: insert 1M edges, then compact", "edges"),
        || adjacency(bench, true),
        |index| {
            fill(&*index, false);
            assert!(index.maintain().unwrap(), "compacted");
            EDGES
        },
    );
    bench.measure_fresh(
        Case::new("adjacency: insert 1M edges, tree only", "edges"),
        || adjacency(bench, false),
        |index| fill(&*index, false),
    );
    bench.measure_fresh(Case::new("btree on source: insert 1M edges", "edges"), || btree(bench), |index| fill(&*index, true));
    let (compacted, tree, source) = (adjacency(bench, true), adjacency(bench, false), btree(bench));
    fill(&*compacted, false);
    fill(&*tree, false);
    fill(&*source, true);
    assert!(compacted.maintain().unwrap());
    let (followed, alone) = (Arc::clone(&compacted).adjacency().unwrap(), Arc::clone(&tree).adjacency().unwrap());
    let mut a = Vec::new();
    followed.steps(&Key(vec![number(5)]), Direction::Out, &mut a).unwrap();
    assert_eq!(a.len(), Arc::clone(&source).search(&Probe::Compare { op: CompOp::Eq, value: number(5) }, KeyOrder::Ascending).count());
    bench.measure(Case::new("adjacency steps, compacted", "lookups"), || steps(&*followed));
    bench.measure(Case::new("adjacency steps, tree only", "lookups"), || steps(&*alone));
    bench.measure(Case::new("adjacency equality search, compacted", "lookups"), || probes(&compacted));
    bench.measure(Case::new("btree on source: equality search", "lookups"), || probes(&source));
    harness::note(&format!("{} edges, {} vertices, {} steps of vertex 5", followed.edges(), VERTICES, a.len()));
}
