//! FreshDiskANN updates: an index built by inserts, counted entries,
//! tombstones and consolidation, failure leftovers, concurrent writers.

use super::*;
use crate::disk::pager::PageId;
use node::Node;

fn empty(dims: u32, degree: u32, pq: bool) -> DiskAnn {
    let (pool, file, budget) = pool();
    DiskAnn::create(pool, file, layout(dims, degree), (tuning(), plugins(pq)), budget).unwrap()
}

/// Verifies: [EXTENSION] An index built only by inserts (as online CREATE
/// INDEX fills one) reaches 95% recall@10; once it holds
/// `Tuning::train_at` nodes, maintenance trains its codes and recall stays.
#[test]
fn an_index_built_by_inserts_reaches_recall_and_trains_its_codes() {
    let (points, queries) = dataset(1500, 60, (16, 15), 2);
    let index = empty(16, 24, true);
    for (row, p) in (0_u64..).zip(&points) {
        index.insert(p, row).unwrap();
    }
    let (before, _) = recall(&index, &points, &queries, &ten());
    assert!(before >= 0.95_f64, "inserted recall@10 {before}");
    assert!(index.maintenance_due());
    assert!(index.maintain().unwrap());
    assert!(read(&index.inner.book).unwrap().codes.is_some());
    let (after, _) = recall(&index, &points, &queries, &ten());
    assert!(after >= 0.95_f64, "recall@10 after training {after}");
    assert!(!index.maintain().unwrap());
}

/// Verifies: [DESIGN] Entries are counted: a repeat is one node of count
/// two, removed by two removes; removing an absent entry changes nothing;
/// inserting a removed entry brings its tombstone back to life.
#[test]
fn entries_are_counted_and_tombstones_come_back() {
    let index = empty(4, 8, false);
    let (a, b) = ([1.0_f32, 2.0, 3.0, 4.0], [4.0_f32, 3.0, 2.0, 1.0]);
    index.insert(&a, 7).unwrap();
    index.insert(&a, 7).unwrap();
    index.insert(&b, 7).unwrap();
    index.insert(&a, 8).unwrap();
    assert_eq!(index.counts().live, 3);
    assert_eq!(index.rows(&a).unwrap().len(), 2);
    index.remove(&a, 99).unwrap();
    index.remove(&[9.0_f32; 4], 7).unwrap();
    index.remove(&a, 7).unwrap();
    assert!(index.rows(&a).unwrap().contains(&7));
    index.remove(&a, 7).unwrap();
    index.remove(&a, 7).unwrap();
    assert_eq!(index.rows(&a).unwrap(), [8]);
    assert_eq!((index.counts().live, index.counts().dead), (2, 1));
    let found = index.search(&a, &ten()).unwrap().0;
    assert!(found.iter().all(|c| !(c.row == 7 && c.vector == a)));
    index.insert(&a, 7).unwrap();
    assert_eq!((index.counts().live, index.counts().dead, index.counts().allocated), (3, 0, 3));
    assert!(index.search(&a, &ten()).unwrap().0.iter().any(|c| c.row == 7 && c.vector == a));
}

/// Verifies: [EXTENSION] (FreshDiskANN Algorithm 4) Tombstoned rows are
/// never returned; once a fifth of the nodes are dead consolidation is
/// due, frees them all, repairs the edges into them so recall over the
/// rest stays at 95%, and new inserts reuse the freed slots.
#[test]
fn consolidation_frees_tombstones_and_keeps_recall() {
    let (points, queries) = dataset(1500, 60, (16, 15), 4);
    let index = loaded(&points, layout(16, 24), tuning(), true);
    let gone = |row: u64| row.is_multiple_of(3);
    for (row, p) in (0_u64..).zip(&points).filter(|(r, _)| gone(*r)) {
        index.remove(p, row).unwrap();
    }
    assert!(index.maintenance_due());
    let left: Vec<Vec<f32>> = points.iter().cloned().zip(0_u64..).filter(|(_, r)| !gone(*r)).map(|(p, _)| p).collect();
    let rows: Vec<u64> = (0_u64..1500).filter(|r| !gone(*r)).collect();
    let check = |searched: &DiskAnn| {
        let mut hit = 0;
        for q in &queries {
            let (found, _) = searched.search(q, &ten()).unwrap();
            assert!(found.iter().all(|c| !gone(c.row)));
            let expected: Vec<u64> = truth(&left, q, 10).into_iter().map(|i| rows[usize::try_from(i).unwrap()]).collect();
            hit = found.iter().filter(|c| expected.contains(&c.row)).count().checked_add(hit).unwrap();
        }
        f64::from(u32::try_from(hit).unwrap()) / 600.0_f64
    };
    let tombstoned = check(&index);
    assert!(index.maintain().unwrap());
    assert_eq!(index.counts().dead, 0);
    let consolidated = check(&index);
    assert!(tombstoned >= 0.95_f64 && consolidated >= 0.95_f64, "{tombstoned} {consolidated}");
    let allocated = index.counts().allocated;
    for (row, p) in (0_u64..).zip(&points).filter(|(r, _)| gone(*r)) {
        index.insert(p, row).unwrap();
    }
    assert_eq!(index.counts().allocated, allocated);
    let (all, _) = recall(&index, &points, &queries, &ten());
    assert!(all >= 0.95_f64, "recall@10 after reinserting {all}");
}

/// Verifies: [DESIGN] (D19) A node an insert left unfinished, as a failure
/// before its row map entry and live flag would, is never returned nor
/// found by `rows`; consolidation frees it, while a node an insert still
/// holds is left alone.
#[test]
fn unfinished_nodes_are_invisible_and_freed() {
    let index = empty(4, 8, false);
    for row in 0..20_u64 {
        index.insert(&[f32::from(u8::try_from(row).unwrap()); 4], row).unwrap();
    }
    let orphan = index.allocate().unwrap();
    lock_builtin(&index.inner.pending).remove(&orphan);
    let held = index.allocate().unwrap();
    let vector = vec![3.5_f32; 4];
    for id in [orphan, held] {
        index.put(id, &Node { flags: 0, row: 99, count: 1, vector: vector.clone(), neighbours: vec![0] }).unwrap();
        index.link(0, id, &vector).unwrap();
    }
    assert!(index.search(&vector, &ten()).unwrap().0.iter().all(|c| c.row != 99));
    assert_eq!(index.rows(&vector).unwrap(), Vec::<u64>::new());
    index.consolidate().unwrap();
    let mut node = Node::default();
    index.read_node(orphan, &mut node).unwrap();
    assert!(node.free());
    index.read_node(held, &mut node).unwrap();
    assert!(!node.free());
    index.read_node(0, &mut node).unwrap();
    assert!(!node.neighbours.contains(&orphan));
    let _ = PageId(0);
}

/// Verifies: [DESIGN] Concurrent writers each insert their rows through
/// shared nodes' optimistic edge updates and per-node latches; every row
/// is then found by `rows` and the graph keeps 90% recall. Checked after
/// joining, however the threads were scheduled (R-TEST-5).
#[test]
fn concurrent_inserts_keep_every_row() {
    let (points, queries) = dataset(1200, 40, (8, 12), 6);
    let index = empty(8, 16, false);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4_usize)
            .map(|t| {
                let (index, points) = (&index, &points);
                std::thread::Builder::new()
                    .name(format!("diskann-writer-{t}"))
                    .spawn_scoped(scope, move || {
                        for (i, p) in points.iter().enumerate().skip(t).step_by(4) {
                            index.insert(p, u64::try_from(i).unwrap()).unwrap();
                        }
                    })
                    .unwrap()
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    assert_eq!(index.counts().live, 1200);
    for (i, p) in points.iter().enumerate().step_by(37) {
        assert!(index.rows(p).unwrap().contains(&u64::try_from(i).unwrap()));
    }
    let (rate, _) = recall(&index, &points, &queries, &ten());
    assert!(rate >= 0.9_f64, "concurrent recall@10 {rate}");
}

/// Verifies: [DESIGN] A search's list grows by the share of dead nodes it
/// will pass over, so with four in five nodes tombstoned the first search
/// still holds the ten rows asked, never doubling its list.
#[test]
fn a_search_list_grows_past_the_dead() {
    let (points, queries) = dataset(1000, 10, (8, 10), 7);
    let index = empty(8, 16, false);
    for (row, p) in (0_u64..).zip(&points) {
        index.insert(p, row).unwrap();
    }
    // One row in five of each cluster (a row's cluster is its number
    // modulo 10) is left.
    for (row, p) in (0_u64..).zip(&points).filter(|(r, _)| !(r / 10).is_multiple_of(5)) {
        index.remove(p, row).unwrap();
    }
    for q in &queries {
        let (found, stats) = index.search(q, &Search { list: Some(20), ..ten() }).unwrap();
        assert_eq!(found.len(), 10);
        assert_eq!(stats.rounds, 1, "a list grown past the dead is searched once, of {}", stats.list);
    }
}
