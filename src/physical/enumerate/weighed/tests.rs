//! Tests of a hash join's cost in the enumeration: past its grant, at its
//! part's degree, and after its runtime filter (RULES.md D651 to D653).

use super::super::bits::bit;
use super::super::conflicts::{Kind, Shape};
use super::super::graph::Graph;
use super::super::model::{Conj, Model, Order, Rel};
use super::super::space::{Entry, Space};
use crate::physical::cardinality::Classes;
use crate::physical::robust::Linear;
use crate::physical::{CostModel, Degrees};

/// Rows of F, the fact, and its key's distinct values.
const FACT: f64 = 1_000_000.0;
/// Rows of D, the dimension, each of its own key.
const DIMENSION: f64 = 100.0;

/// F of `FACT` rows, joined with D of `DIMENSION` rows on a key of which
/// F has `FACT` values, each relation's stream holding its rows and each
/// row 24 bytes, priced by `cost`.
fn star(cost: CostModel) -> Model {
    let rel = |rows: f64| Rel {
        rows,
        upper: f64::INFINITY,
        most: Vec::new(),
        reads: vec![(Order::None, rows)],
        plans: Vec::new(),
        probes: Vec::new(),
        held: rows,
        bytes: 24.0_f64,
    };
    Model {
        rels: vec![rel(FACT), rel(DIMENSION)],
        conjuncts: vec![Conj { reads: bit(0) | bit(1), op: 0, fraction: 1.0_f64, key: Some((0, 1)), class: Some(0) }],
        preds: vec![None],
        kinds: vec![Kind::Inner],
        classes: Classes::new(vec![vec![(0, FACT), (1, DIMENSION)]]),
        orders: Vec::new(),
        spans: vec![(0, 1), (1, 1)],
        cost,
        want_class: None,
        injected: Vec::new(),
        implied: Vec::new(),
        ranged: super::super::ranged::Ranges::default(),
    }
}

/// The cost of the hash join of F, the left, and D, the right, of `kind`.
fn hashed(cost: CostModel, kind: Kind) -> Linear {
    let model = star(cost);
    let shapes = [Shape { kind, left: bit(0), right: bit(1), reads: 0, strong: true }];
    let graph = Graph::new(2, &shapes, &[(0, bit(0) | bit(1))]);
    let space = Space::new(&graph, &model);
    let entry = |rows: f64| Entry { rows, upper: f64::INFINITY, cands: Vec::new() };
    space.hash_linear((&entry(FACT), &entry(DIMENSION)), (bit(0), bit(1)), (kind, DIMENSION))
}

/// A cost model with no runtime filters, on one thread, of a grant of
/// `grant` bytes.
fn plain(grant: f64) -> CostModel {
    CostModel { runtime_filters: false, grant_bytes: grant, ..CostModel::default() }
}

/// Verifies: [DESIGN] (RULES.md D651) a hash join whose built rows outgrow
/// its grant costs more by both operands spilled; within it, nothing
/// more.
#[test]
fn a_build_past_its_grant_costs_its_spill() {
    let fits = hashed(plain(1e9_f64), Kind::Inner);
    let spills = hashed(plain(1_000.0_f64), Kind::Inner);
    assert!(spills.fixed > fits.fixed + FACT, "{fits:?} {spills:?}");
    assert!((spills.per_row - fits.per_row).abs() < f64::EPSILON);
}

/// Verifies: [DESIGN] (RULES.md D652) a hash join whose parts run on eight
/// threads costs each row hashed an eighth, and its start once.
#[test]
fn a_parallel_part_shares_the_hashing() {
    let alone = hashed(plain(1e9_f64), Kind::Inner);
    let degrees = Degrees { most: 8, morsel: 1_000, least: 0 };
    let shared = hashed(CostModel { degrees, ..plain(1e9_f64) }, Kind::Inner);
    assert!((shared.per_row * 8.0_f64 - alone.per_row).abs() < 1e-12_f64, "{alone:?} {shared:?}");
}

/// Verifies: [DESIGN] (RULES.md D653) an inner join's probing rows cost
/// less where its runtime filter keeps a hundred keys of a million; an
/// outer join's, or one with filters off, the whole hashing. MC/DC of
/// `kind == Kind::Inner && w.runtime_filters`: each alone false.
#[test]
fn a_selective_build_filters_its_probing_rows() {
    let filtered = hashed(CostModel { grant_bytes: 1e9_f64, ..CostModel::default() }, Kind::Inner);
    let off = hashed(plain(1e9_f64), Kind::Inner);
    let outer = hashed(CostModel { grant_bytes: 1e9_f64, ..CostModel::default() }, Kind::Left);
    assert!(filtered.per_row < off.per_row / 4.0_f64, "{filtered:?} {off:?}");
    assert!((outer.per_row - off.per_row).abs() < f64::EPSILON);
}
