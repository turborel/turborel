//! Spatial candidate generation and row-fetch locality, without SQL or
//! transaction bookkeeping. Every case reads the same envelope/row pairs.
//! The phases of a broad read, traversal, row materialization, exact
//! evaluation and wire encoding, are timed apart (`geometry_fetch/phases.rs`).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;
#[path = "geometry_fetch/phases.rs"]
mod phases;

use std::hint::black_box;
use std::sync::{Arc, Barrier};

use harness::{Bench, Case, on_threads};
use turborel::catalog::{ColumnDesc, QualifiedName, TableDesc};
use turborel::engine::rows::{RowStore, TableFiles, TableOptions};
use turborel::engine::rtree::RTree;
use turborel::geometry::{Geometry, data_type_with_srid};
use turborel::log::MemoryLog;
use turborel::spatial::Bounds;
use turborel::storage::{Near, StorageEngine, TableStorage};
use turborel::txn::NO_TRANSACTION;
use turborel::value::{DataType, Decimal, Spelled, Value};

const WINDOWS: u64 = 32;
const FRAMES: usize = 16384;

fn main() {
    let bench = Bench::new("geometry_fetch");
    let (_store, table, tree) = loaded(&bench);
    let windows: Vec<_> = (0..WINDOWS)
        .map(|n| {
            let (x, y) = harness::geometry::position(n);
            Bounds::new([coordinate(x), coordinate(y)], [coordinate(x + 500), coordinate(y + 500)]).unwrap()
        })
        .collect();
    let expected: u64 = (0..WINDOWS).map(|n| harness::geometry::ids(n, 500).len() as u64).sum();
    let actual: u64 = windows.iter().map(|bounds| tree.scan(Some(*bounds)).map(Result::unwrap).count() as u64).sum();
    assert_eq!(actual, expected);
    phases::measure(&bench, (&*table, &tree), &windows, expected);
    phases::weights(&bench, &table);
    if std::env::var("TURBOREL_FETCH_PHASES").is_ok() {
        return;
    }
    for threads in [1_usize, 4, 8] {
        for batch in [0_usize, 64, 256, 1024, 4096, 32768] {
            bench.measure(Case::new(format!("visible rows, sorted block {batch}"), "rows").threads(threads), || {
                run((&*table, &tree), &windows, (batch, threads));
                expected * threads as u64
            });
        }
    }
}

fn coordinate(n: u64) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}

fn run((table, tree): (&dyn TableStorage, &RTree), windows: &[Bounds], (batch, threads): (usize, usize)) {
    let start = Barrier::new(threads);
    on_threads(threads, |_| {
        start.wait();
        for &bounds in windows {
            fetched(table, tree, bounds, batch);
        }
    });
}

fn fetched(table: &dyn TableStorage, tree: &RTree, bounds: Bounds, batch: usize) {
    let mut near = Near::default();
    let mut scan = tree.scan(Some(bounds));
    let mut fetch = |id| {
        black_box(table.get_near(NO_TRANSACTION, id, Some(&[0, 1]), &mut near).unwrap().unwrap());
    };
    if batch == 0 {
        for entry in scan {
            fetch(entry.unwrap().1);
        }
        return;
    }
    let mut held = Vec::with_capacity(batch);
    loop {
        held.extend(scan.by_ref().take(batch).map(Result::unwrap));
        if held.is_empty() {
            return;
        }
        held.sort_unstable_by_key(|entry| entry.1);
        for (_, id) in held.drain(..) {
            fetch(id);
        }
    }
}

fn loaded(bench: &Bench) -> (RowStore, Arc<dyn TableStorage>, RTree) {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("rows")));
    let tree_file = pool.add_pager(Arc::new(bench.pager("rtree")));
    let tree = RTree::create(Arc::clone(&pool), tree_file).unwrap();
    let store = RowStore::new(TableFiles::new(pool, file, Arc::new(MemoryLog::new())), TableOptions::default());
    let columns = [("ID", DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }), ("G", data_type_with_srid(0).unwrap())];
    let desc = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: QualifiedName { auth: "B".into(), ident: "POINTS".into() },
        columns: columns
            .into_iter()
            .map(|(name, data_type)| ColumnDesc {
                name: name.into(),
                data_type,
                not_null: false,
                default: None,
                domain: None,
                collation: None,
                generated: None,
            })
            .collect(),
        constraints: vec![],
        engine: store.name().into(),
        options: vec![],
        generation: 0,
        temporary: None,
    };
    let table: Arc<dyn TableStorage> = store.create_table(&desc).unwrap().into();
    for n in 0..harness::geometry::ROWS {
        let (x, y) = harness::geometry::position(n);
        let point = Geometry::point(coordinate(x), coordinate(y), 0).unwrap();
        let id = table.insert(NO_TRANSACTION, vec![Value::Exact(Decimal::new(i128::from(n), 0)), point.to_value()]).unwrap();
        tree.insert(point.bounds().unwrap(), id).unwrap();
    }
    (store, table, tree)
}
