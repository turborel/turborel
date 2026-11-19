//! A broad spatial read taken apart into its phases (RULES.md D137): the
//! tree's traversal alone, then each candidate's row materialized, then
//! its geometry decoded, then tested exactly, then the row encoded as the
//! wire protocol's Rows reply encodes a value (`turborel-protocol`'s
//! `put_value`: a tag, and of a geometry, its kind's name, its text and
//! its bytes). Each phase repeats the ones before it, so the time a phase
//! adds is its rate's difference from the one before. The planner's
//! weights of a spatial call follow (`weights`): a row of a table scan,
//! the unit, against a geometry decoded of one vertex and of 129.

use std::hint::black_box;
use std::sync::Arc;

use super::harness::{Bench, Case};
use turborel::bytes::Put;
use turborel::engine::rtree::RTree;
use turborel::geometry::Geometry;
use turborel::spatial::Bounds;
use turborel::storage::{Near, TableStorage};
use turborel::txn::NO_TRANSACTION;
use turborel::value::Value;

/// The candidates a production cursor sorts by row id at once.
const BLOCK: usize = 4096;
/// The protocol's tags of an exact number and an extension value.
const EXACT: u8 = 2;
const EXTENSION: u8 = 4;

#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Phase {
    Traversal,
    Materialized,
    Decoded,
    Evaluated,
    Encoded,
}

/// Values decoded by a weight's case.
const DECODED: u64 = 100_000;
/// Vertices of the polygon decoded, closure included.
const POLYGON_VERTICES: u32 = 129;

/// Measures each phase over `windows`, `expected` candidates in all.
pub fn measure(bench: &Bench, (table, tree): (&dyn TableStorage, &RTree), windows: &[Bounds], expected: u64) {
    let shapes: Vec<Geometry> = windows.iter().map(|b| window(*b)).collect();
    for (name, phase) in [
        ("phase 1: tree traversal", Phase::Traversal),
        ("phase 2: + row materialization", Phase::Materialized),
        ("phase 3: + geometry decode", Phase::Decoded),
        ("phase 4: + exact test", Phase::Evaluated),
        ("phase 5: + wire encoding", Phase::Encoded),
    ] {
        bench.measure(Case::new(name, "rows"), || {
            let mut out = Vec::new();
            for (bounds, shape) in windows.iter().zip(&shapes) {
                read((table, tree), (*bounds, shape), phase, &mut out);
            }
            black_box(out.len());
            expected
        });
    }
}

fn window(bounds: Bounds) -> Geometry {
    let ([x0, y0], [x1, y1]) = (bounds.lower(), bounds.upper());
    Geometry::from_wkt(&format!("POLYGON (({x0} {y0}, {x1} {y0}, {x1} {y1}, {x0} {y1}, {x0} {y0}))"), 0).unwrap()
}

fn read((table, tree): (&dyn TableStorage, &RTree), (bounds, shape): (Bounds, &Geometry), phase: Phase, out: &mut Vec<u8>) {
    let mut scan = tree.scan(Some(bounds));
    if phase == Phase::Traversal {
        black_box(scan.map(Result::unwrap).count());
        return;
    }
    let mut near = Near::default();
    let mut held = Vec::with_capacity(BLOCK);
    loop {
        held.extend(scan.by_ref().take(BLOCK).map(Result::unwrap));
        if held.is_empty() {
            return;
        }
        held.sort_unstable_by_key(|entry| entry.1);
        for (_, id) in held.drain(..) {
            let row = table.get_near(NO_TRANSACTION, id, Some(&[0, 1]), &mut near).unwrap().unwrap();
            if phase >= Phase::Decoded {
                let geometry = Geometry::from_value(&row[1]).unwrap();
                if phase >= Phase::Evaluated {
                    black_box(geometry.intersects(shape).unwrap());
                }
            }
            if phase >= Phase::Encoded {
                out.clear();
                encode(&row, out);
            }
            black_box(row);
        }
    }
}

/// The planner's unit, a row of a table scan, and a geometry decoded of
/// one vertex and of `POLYGON_VERTICES`.
pub fn weights(bench: &Bench, table: &Arc<dyn TableStorage>) {
    bench.measure(Case::new("weight: table scan", "rows"), || {
        Arc::clone(table).scan(NO_TRANSACTION, &[0, 1]).map(Result::unwrap).count() as u64
    });
    let point = Geometry::point(1.0, 2.0, 0).unwrap().to_value();
    let ring: Vec<String> = (0..POLYGON_VERTICES)
        .map(|k| {
            let angle = std::f64::consts::TAU * f64::from(k % (POLYGON_VERTICES - 1)) / f64::from(POLYGON_VERTICES - 1);
            format!("{} {}", angle.cos(), angle.sin())
        })
        .collect();
    let polygon = Geometry::from_wkt(&format!("POLYGON (({}))", ring.join(", ")), 0).unwrap().to_value();
    for (name, value) in [("weight: decode a point", point), ("weight: decode a 129-vertex polygon", polygon)] {
        bench.measure(Case::new(name, "values"), || {
            for _ in 0..DECODED {
                black_box(Geometry::from_value(black_box(&value)).unwrap());
            }
            DECODED
        });
    }
}

/// `row` as the Rows reply writes its values.
fn encode(row: &[Value], out: &mut Vec<u8>) {
    for value in row {
        if let Value::Extension(e) = value {
            out.put_u8(EXTENSION);
            let mut bytes = Vec::new();
            e.kind.put_value(value, &mut bytes);
            out.put_str(e.kind.name());
            out.put_str(&e.kind.text(value));
            out.put_bytes(&bytes);
        } else {
            out.put_u8(EXACT);
            if let Some(kind) = value.kind() {
                kind.put_value(value, out);
            }
        }
    }
}
