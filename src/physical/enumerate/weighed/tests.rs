//! Tests of a hash join's cost in the enumeration: past its grant, in passes,
//! at its part's degree, and after its runtime filter (RULES.md D651 to D653,
//! D1240).

use super::super::bits::bit;
use super::super::conflicts::{Kind, Shape};
use super::super::graph::Graph;
use super::super::model::{Conj, Model, Order, Rel};
use super::super::space::{Entry, Space};
use crate::physical::cardinality::Classes;
use crate::physical::cost::parallel::ROW_STORE_GAIN;
use crate::physical::robust::Linear;
use crate::physical::{CostModel, Degrees};

/// Rows of F, the fact, and its key's distinct values.
const FACT: f64 = 1_000_000.0;
/// Rows of D, the dimension, each of its own key.
const DIMENSION: f64 = 100.0;

/// F of `FACT` rows, joined with D of `DIMENSION` rows on a key of which
/// F has `FACT` values, each relation's stream holding its rows and each
/// row 24 bytes, its part's threads gaining `gain` each, priced by `cost`.
fn star(cost: CostModel, gain: f64) -> Model {
    let rel = |rows: f64| Rel {
        rows,
        upper: f64::INFINITY,
        most: Vec::new(),
        reads: vec![(Order::None, rows)],
        plans: Vec::new(),
        probes: Vec::new(),
        held: rows,
        bytes: 24.0_f64,
        gain,
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

/// The cost of the hash join of F, the left, and D, the right, of `kind`,
/// of tables of the column store.
fn hashed(cost: CostModel, kind: Kind) -> Linear {
    hashed_gaining(cost, kind, 1.0_f64)
}

/// `hashed`, each thread past the first of a part gaining `gain`.
fn hashed_gaining(cost: CostModel, kind: Kind, gain: f64) -> Linear {
    hashed_of(cost, kind, (gain, false))
}

/// `hashed_gaining`, or of D the left and F the right if `flipped`.
fn hashed_of(cost: CostModel, kind: Kind, (gain, flipped): (f64, bool)) -> Linear {
    let model = star(cost, gain);
    let (l, r) = if flipped { (1, 0) } else { (0, 1) };
    let shapes = [Shape { kind, left: bit(l), right: bit(r), reads: 0, strong: true }];
    let graph = Graph::new(2, &shapes, &[(0, bit(0) | bit(1))]);
    let space = Space::new(&graph, &model);
    let entry = |rows: f64| Entry { rows, upper: f64::INFINITY, cands: Vec::new() };
    let (fact, dimension) = (entry(FACT), entry(DIMENSION));
    let (left, right) = if flipped { (&dimension, &fact) } else { (&fact, &dimension) };
    let (read, probe) = if flipped { (FACT, DIMENSION) } else { (DIMENSION, FACT) };
    space.hash_linear((left, right), (bit(l), bit(r)), (kind, read, probe))
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
/// threads over the column store costs each row hashed an eighth; over
/// the row store, whose parts gain nothing, each row whole, and its start
/// more.
#[test]
fn a_parallel_part_shares_the_hashing() {
    let alone = hashed(plain(1e9_f64), Kind::Inner);
    let degrees = Degrees { most: 8, morsel: 1_000, least: 0 };
    let shared = hashed(CostModel { degrees, ..plain(1e9_f64) }, Kind::Inner);
    assert!((shared.per_row * 8.0_f64 - alone.per_row).abs() < 1e-12_f64, "{alone:?} {shared:?}");
    let rows = hashed_gaining(CostModel { degrees, ..plain(1e9_f64) }, Kind::Inner, ROW_STORE_GAIN);
    assert!((rows.per_row - alone.per_row).abs() < 1e-12_f64 && rows.fixed > alone.fixed, "{alone:?} {rows:?}");
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

/// Verifies: [DESIGN] (RULES.md D1240) Regression test (R-TEST-7): an
/// antijoin built past its grant on eight threads was weighed probed in
/// passes, which no pipeline runs it in, and chosen over its index's
/// probes, then run partitioned. Only an inner join and a LEFT one its
/// left probing cost passes, less than both operands spilled; a semi
/// join, an antijoin and a LEFT join whose left is built cost the spill.
/// MC/DC of `kind == Kind::Inner || (kind == Kind::Left && probes_left)`:
/// the inner join, the LEFT one probing and the LEFT one built.
#[test]
fn only_a_join_a_pipeline_builds_in_parts_costs_passes() {
    let degrees = Degrees { most: 8, morsel: 1_000, least: 0 };
    let small = CostModel { degrees, ..plain(1_800.0_f64) };
    let [inner, left, semi, anti] = [Kind::Inner, Kind::Left, Kind::Semi, Kind::Anti].map(|kind| hashed(small.clone(), kind).fixed);
    let [inner_built, left_built] = [Kind::Inner, Kind::Left].map(|kind| hashed_of(small.clone(), kind, (1.0_f64, true)).fixed);
    assert!(inner + FACT < anti, "{inner} {anti}");
    assert!((left - inner).abs() < f64::EPSILON && (semi - anti).abs() < f64::EPSILON, "{left} {semi}");
    assert!(inner_built + FACT < left_built, "{inner_built} {left_built}");
}
