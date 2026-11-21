//! Unit tests of join enumeration on synthetic graphs: the pairs DPccp
//! enumerates, the connected subgraphs it counts, the conflict rules'
//! effect, the estimates of classes, and the adaptive switch (RULES.md
//! R-TEST-1, R-TEST-8).

use super::bits::{Set, bit, through};
use super::conflicts::{Kind, Shape};
use super::graph::Graph;
use super::model::{Conj, Model, Order, Pred, Rel};
use super::space::{Space, Tree};
use super::{Algorithm, adaptive};
use crate::physical::CostModel;
use crate::physical::cardinality::Classes;

/// A query of `rows.len()` relations of one column each, the FROM clause
/// left-deep, and an equality of each pair of `edges` of the given
/// distinct values, each its own class.
fn inner(rows: &[f64], edges: &[(usize, usize, f64)]) -> (Vec<Shape>, Model) {
    let n = rows.len();
    let joins = n.saturating_sub(1);
    let shapes: Vec<Shape> =
        (1..n).map(|k| Shape { kind: Kind::Inner, left: through(k.saturating_sub(1)), right: bit(k), reads: 0, strong: true }).collect();
    let conjuncts = edges
        .iter()
        .enumerate()
        .map(|(c, &(a, b, _))| Conj {
            reads: bit(a) | bit(b),
            op: a.max(b).saturating_sub(1),
            fraction: 1.0_f64,
            key: Some((a, b)),
            class: Some(c),
        })
        .collect();
    let classes = Classes::new(edges.iter().map(|&(a, b, d)| vec![(a, d), (b, d)]).collect());
    let rels = rows
        .iter()
        .map(|&r| Rel {
            rows: r,
            upper: f64::INFINITY,
            most: Vec::new(),
            reads: vec![(Order::None, r)],
            probes: Vec::new(),
            plans: Vec::new(),
        })
        .collect();
    let model = Model {
        rels,
        conjuncts,
        preds: vec![None; joins],
        kinds: vec![Kind::Inner; joins],
        classes,
        orders: Vec::new(),
        spans: (0..n).map(|r| (r, 1)).collect(),
        cost: CostModel::default(),
        want_class: None,
        injected: Vec::new(),
        implied: Vec::new(),
        ranged: super::ranged::Ranges::default(),
    };
    let model = Model { implied: model.implied_classes(), ..model };
    (shapes, model)
}

/// The graph of `shapes` and `model`'s conjuncts.
fn graph(shapes: &[Shape], model: &Model) -> Graph {
    let conjuncts: Vec<(usize, Set)> = model.conjuncts.iter().map(|c| (c.op, c.reads)).collect();
    Graph::new(model.rels.len(), shapes, &conjuncts)
}

/// The pairs joined by DPhyp of a graph of `edges` over `n` relations.
fn pairs(n: usize, edges: &[(usize, usize, f64)]) -> usize {
    let (shapes, model) = inner(&vec![10.0_f64; n], edges);
    let g = graph(&shapes, &model);
    let mut space = Space::new(&g, &model);
    space.dphyp(g.all());
    assert!(space.has(g.all()));
    space.pairs
}

/// Verifies: [DESIGN] RULES.md D300: DPccp enumerates each csg-cmp pair
/// once, as many as Moerkotte and Neumann, "Analysis of Two Existing and
/// One New Dynamic Programming Algorithm for the Generation of Optimal
/// Bushy Join Trees without Cross Products", 2006, §2.3 count: of a chain
/// (n³ − n)/6, of a star (n − 1)·2^(n − 2), of a clique
/// (3^n − 2^(n + 1) + 1)/2; and Neumann and Radke's countCC, 2018, Fig.
/// 3, counts a chain's n(n + 1)/2 connected subgraphs, stopping past the
/// budget.
#[test]
fn dpccp_enumerates_each_pair_once() {
    let n = 6_usize;
    let chain: Vec<_> = (1..n).map(|k| (k.saturating_sub(1), k, 10.0_f64)).collect();
    let star: Vec<_> = (1..n).map(|k| (0_usize, k, 10.0_f64)).collect();
    let clique: Vec<_> = (0..n).flat_map(|a| (a.saturating_add(1)..n).map(move |b| (a, b, 10.0_f64))).collect();
    // The counts for n = 6, (216 − 6)/6, 5·16 and (729 − 128 + 1)/2.
    assert_eq!(pairs(n, &chain), 35_usize);
    assert_eq!(pairs(n, &star), 80_usize);
    assert_eq!(pairs(n, &clique), 301_usize);
    let (shapes, model) = inner(&[10.0_f64; 6], &chain);
    let chained = graph(&shapes, &model);
    assert_eq!(Space::new(&chained, &model).count(chained.all(), 1000_usize), 21_usize);
    assert!(Space::new(&chained, &model).count(chained.all(), 5_usize) > 5_usize);
}

/// Verifies: [DESIGN] RULES.md D300, D302: of a chain A - B - C - D whose
/// ends each match one row of their neighbour, which match each other
/// widely, the cheapest plan joins A to B and C to D first, a bushy tree
/// no left-deep order gives.
#[test]
fn the_cheapest_tree_may_be_bushy() {
    let rows = [10.0_f64, 1e4_f64, 1e4_f64, 10.0_f64];
    let (shapes, model) = inner(&rows, &[(0_usize, 1_usize, 1e4_f64), (1, 2, 10.0_f64), (2, 3, 1e4_f64)]);
    let g = graph(&shapes, &model);
    let mut space = Space::new(&g, &model);
    space.dphyp(g.all());
    let best = space.chosen(g.all(), Order::None).unwrap();
    let Some(&Tree::Join { sets: (l, r), .. }) = space.trees.get(best.tree) else { panic!("no join") };
    let halves = [bit(0_usize) | bit(1_usize), bit(2_usize) | bit(3_usize)];
    assert!(halves.contains(&l) && halves.contains(&r), "{l:b} {r:b}");
}

/// Verifies: [DESIGN] RULES.md D302: equalities chained through one value,
/// `a = b`, `b = c` and `a = c`, are counted once, the implied one not
/// again; a join gives at least one row of a product that has one.
#[test]
fn a_class_of_equalities_is_counted_once() {
    let (shapes, mut model) = inner(&[100.0_f64; 3], &[(0_usize, 1_usize, 100.0_f64), (1, 2, 100.0_f64), (0, 2, 100.0_f64)]);
    model.classes = Classes::new(vec![vec![(0_usize, 100.0_f64), (1, 100.0_f64), (2, 100.0_f64)]]);
    for c in &mut model.conjuncts {
        c.class = Some(0_usize);
    }
    let classed = graph(&shapes, &model);
    let mut space = Space::new(&classed, &model);
    space.dphyp(classed.all());
    assert_eq!(space.table.get(&classed.all()).map(|e| e.rows), Some(100.0_f64));
    let (few_shapes, few) = inner(&[2.0_f64; 3], &[(0_usize, 1_usize, 1e6_f64), (1, 2, 1e6_f64)]);
    let sparse = graph(&few_shapes, &few);
    let mut floored = Space::new(&sparse, &few);
    floored.dphyp(sparse.all());
    assert_eq!(floored.table.get(&sparse.all()).map(|e| e.rows), Some(1.0_f64));
}

/// Verifies: [DESIGN] RULES.md D301: of `(R0 ⟕ R1) ⋈ R2`, the inner join
/// on R1 and R2, no plan joins R1 and R2 before R0, which would lose R0's
/// rows R1 does not match; of `R0 ⟕ (R1 ⋈ R2)` the LEFT join's right is
/// not taken apart; the rows of a LEFT join are at least its left's.
#[test]
fn outer_joins_reorder_only_where_sound() {
    let (r0, r1, r2) = (bit(0_usize), bit(1_usize), bit(2_usize));
    let rows = [1000.0_f64, 10.0_f64, 10.0_f64];
    let (mut shapes, mut model) = inner(&rows, &[(1_usize, 2_usize, 10.0_f64)]);
    if let Some(first) = shapes.first_mut() {
        *first = Shape { kind: Kind::Left, left: r0, right: r1, reads: r0 | r1, strong: true };
    }
    model.kinds = vec![Kind::Left, Kind::Inner];
    model.preds = vec![Some(Pred { fraction: 0.001_f64, keys: vec![(0_usize, 1_usize)] }), None];
    let left_first = graph(&shapes, &model);
    let mut space = Space::new(&left_first, &model);
    space.dphyp(left_first.all());
    assert!(!space.has(r1 | r2), "R1 joined to R2 without R0");
    assert!(space.has(left_first.all()));
    assert!(space.table.get(&(r0 | r1)).is_some_and(|e| e.rows >= 1000.0_f64));
    let swapped = [
        Shape { kind: Kind::Left, left: r0, right: r1 | r2, reads: r0 | r1, strong: true },
        Shape { kind: Kind::Inner, left: r1, right: r2, reads: r1 | r2, strong: true },
    ];
    let mut nested = inner(&rows, &[(1_usize, 2_usize, 10.0_f64)]).1;
    nested.kinds = vec![Kind::Left, Kind::Inner];
    nested.preds = vec![Some(Pred { fraction: 0.001_f64, keys: vec![(0_usize, 1_usize)] }), None];
    if let Some(c) = nested.conjuncts.first_mut() {
        c.op = 1_usize;
    }
    let right_nested = graph(&swapped, &nested);
    let mut kept = Space::new(&right_nested, &nested);
    kept.dphyp(right_nested.all());
    assert!(!kept.has(r0 | r1), "the LEFT join's right taken apart");
    assert!(kept.has(right_nested.all()));
}

/// Verifies: [DESIGN] RULES.md D304: past the budget, a chain of simple
/// edges is planned by linearized DP, a plan no cheaper than exact DP's;
/// larger than the linearized limit, by GOO-DP; each plans the whole.
#[test]
fn the_switch_adapts_to_the_budget() {
    let rows: Vec<f64> = (0..16_u32).map(|r| 10.0_f64 + f64::from(r.wrapping_mul(37) % 1000)).collect();
    let chain: Vec<_> = (1..16_usize).map(|k| (k.saturating_sub(1), k, 50.0_f64)).collect();
    let (shapes, model) = inner(&rows, &chain);
    let g = graph(&shapes, &model);
    let mut exact = Space::new(&g, &model);
    assert_eq!(adaptive(&mut exact, (10_000_usize, 100_usize)), Algorithm::DPccp);
    let optimum = exact.chosen(g.all(), Order::None).unwrap().cost;
    let mut linear = Space::new(&g, &model);
    assert_eq!(adaptive(&mut linear, (10_usize, 100_usize)), Algorithm::Linearized);
    let found = linear.chosen(g.all(), Order::None).unwrap().cost;
    assert!(found >= optimum * 0.999_999_f64, "{found} below the optimum {optimum}");
    let mut guided = Space::new(&g, &model);
    assert_eq!(adaptive(&mut guided, (10_usize, 8_usize)), Algorithm::GooDp);
    assert!(guided.chosen(g.all(), Order::None).unwrap().cost >= optimum * 0.999_999_f64);
}

/// Verifies: [DESIGN] RULES.md D387: the keys of a chain of inner joins
/// make one class of the columns they make equal, and two relations it
/// joins through a third take the key it implies, the left's column first;
/// a chain with an outer join takes none.
#[test]
fn a_class_implies_keys_of_relations_no_conjunct_joins() {
    let (_, model) = inner(&[10.0_f64, 10.0_f64, 1000.0_f64], &[(0, 2, 10.0_f64), (1, 2, 10.0_f64)]);
    assert_eq!(model.implied, vec![(vec![0, 1, 2], bit(0) | bit(1) | bit(2))]);
    assert_eq!(model.implied_keys((bit(1), bit(0))), vec![(1, 0)]);
    assert_eq!(model.implied_keys((bit(0), bit(1) | bit(2))), vec![(0, 1)]);
    let mut outer = model;
    outer.kinds = vec![Kind::Left, Kind::Inner];
    outer.implied = outer.implied_classes();
    assert_eq!(outer.implied_keys((bit(0), bit(1))), Vec::new());
}
