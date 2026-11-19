//! The shape an R-link tree is left in (RULES.md D139): node count, bytes,
//! average fill and the total overlap of node envelopes, per level, after
//! a packed load, after an insert-by-insert load, and after a long mix of
//! inserts, moves and deletes on each. Also the packed load's rate beside
//! the insert-by-insert one. Set TURBOREL_SPATIAL_SHAPE=only to run just
//! this part.

use std::sync::Arc;

use super::harness::{self, Bench, Case};
use super::{FRAMES, coordinate, fresh_named};
use turborel::engine::rtree::RTree;
use turborel::memory::MemoryBudget;
use turborel::spill::{Area, WorkAreas};

/// A work area that holds the whole load, and its spill batches.
const WORK: usize = 512 << 20;
const BATCH: usize = 64 << 10;
/// Operations in the aged mix, per entry loaded.
const AGE: u64 = 3;

pub fn report(bench: &Bench, rows: u64) {
    for distribution in ["uniform", "cluster", "boxes"] {
        let title = format!("{distribution}, {rows}");
        bench.measure_fresh(
            Case::new(format!("packed load {title}"), "entries"),
            || fresh_named(bench, FRAMES, "packed-rate"),
            |tree| {
                load(&tree, distribution, rows);
                rows
            },
        );
        bench.measure_fresh(
            Case::new(format!("inserted load {title}"), "entries"),
            || fresh_named(bench, FRAMES, "inserted-rate"),
            |tree| {
                (0..rows).for_each(|row| tree.insert(coordinate(distribution, row), row).unwrap());
                rows
            },
        );
        let packed = fresh_named(bench, FRAMES, "packed");
        load(&packed, distribution, rows);
        let inserted = fresh_named(bench, FRAMES, "inserted");
        (0..rows).for_each(|row| inserted.insert(coordinate(distribution, row), row).unwrap());
        print(&format!("packed {title}"), &packed);
        print(&format!("inserted {title}"), &inserted);
        age(&packed, distribution, rows);
        age(&inserted, distribution, rows);
        print(&format!("packed, aged {title}"), &packed);
        print(&format!("inserted, aged {title}"), &inserted);
    }
}

fn load(tree: &RTree, distribution: &str, rows: u64) {
    let work = WorkAreas::new(MemoryBudget::new(usize::MAX));
    let mut entries = (0..rows).map(|row| Ok((coordinate(distribution, row), row)));
    tree.load(&mut entries, (&Arc::new(work), Area::new(WORK, BATCH))).unwrap();
}

/// Of every five operations, three move a row, one deletes a row and one
/// inserts a new one, so the tree keeps its size while it ages. Rows are
/// visited in a scattered order, as a workload's updates would be.
fn age(tree: &RTree, distribution: &str, rows: u64) {
    let mut place: Vec<Option<u64>> = (0..rows).map(Some).collect();
    let mut next = rows;
    let mut deleted = Vec::new();
    for step in 0..rows * AGE {
        let row = usize::try_from(step.wrapping_mul(0x9e37_79b9) % rows).unwrap();
        match step % 5 {
            0 => {
                if let Some(at) = place[row].take() {
                    tree.remove(coordinate(distribution, at), row as u64).unwrap();
                    deleted.push(row);
                }
            }
            1 => {
                if let Some(back) = deleted.pop() {
                    next += 1;
                    tree.insert(coordinate(distribution, next), back as u64).unwrap();
                    place[back] = Some(next);
                }
            }
            _ => {
                if let Some(at) = place[row] {
                    next += 1;
                    tree.insert(coordinate(distribution, next), row as u64).unwrap();
                    tree.remove(coordinate(distribution, at), row as u64).unwrap();
                    place[row] = Some(next);
                }
            }
        }
    }
}

fn print(label: &str, tree: &RTree) {
    let (reclaimed, narrowed) = tree.reclaimed();
    harness::note(&format!("{label}: reclaimed {reclaimed} nodes, narrowed {narrowed} entries"));
    for level in tree.shape().unwrap() {
        harness::note(&format!(
            "  level {}: {} nodes, {} bytes in pages, {} used, fill {:.3}, overlap {:.1}",
            level.level, level.nodes, level.bytes, level.used, level.fill, level.overlap
        ));
    }
}
