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

/// A query of `rows.len()` relations of one column each, the FROM clause
/// left-deep, and an equality of each pair of `edges` of the given
/// distinct values, each its own class.
fn inner(rows: &[f64], edges: &[(usize, usize, f64)]) -> (Vec<Shape>, Model) {
    let n = rows.len();
    let shapes: Vec<Shape> =
        (1..n).map(|k| Shape { kind: Kind::Inner, left: through(k - 1), right: bit(k), reads: 0, strong: true }).collect();
    let conjuncts = edges
        .iter()
        .enumerate()
        .map(|(c, &(a, b, _))| Conj { reads: bit(a) | bit(b), op: a.max(b) - 1, fraction: 1.0, key: Some((a, b)), class: Some(c) })
        .collect();
    let classes = edges.iter().map(|&(a, b, d)| vec![(a, d), (b, d)]).collect();
    let rels = rows.iter().map(|&r| Rel { rows: r, reads: vec![(Order::None, r)], probes: Vec::new() }).collect();
    let model = Model {
        rels,
        conjuncts,
        preds: vec![None; n - 1],
        kinds: vec![Kind::Inner; n - 1],
        classes,
        orders: Vec::new(),
        spans: (0..n).map(|r| (r, 1)).collect(),
        cost: CostModel::default(),
        want_class: None,
    };
    (shapes, model)
}

/// The graph of `shapes` and `model`'s conjuncts.
fn graph(shapes: &[Shape], model: &Model) -> Graph {
    let conjuncts: Vec<(usize, Set)> = model.conjuncts.iter().map(|c| (c.op, c.reads)).collect();
    Graph::new(model.rels.len(), shapes, &conjuncts)
}

/// The pairs joined by DPhyp of a graph of `edges` over `n` relations.
fn pairs(n: usize, edges: &[(usize, usize, f64)]) -> usize {
    let (shapes, model) = inner(&vec![10.0; n], edges);
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
    let n = 6;
    let chain: Vec<_> = (1..n).map(|k| (k - 1, k, 10.0)).collect();
    let star: Vec<_> = (1..n).map(|k| (0, k, 10.0)).collect();
    let clique: Vec<_> = (0..n).flat_map(|a| (a + 1..n).map(move |b| (a, b, 10.0))).collect();
    assert_eq!(pairs(n, &chain), (n * n * n - n) / 6);
    assert_eq!(pairs(n, &star), (n - 1) * (1 << (n - 2)));
    assert_eq!(pairs(n, &clique), (3_usize.pow(6) - (1 << 7) + 1) / 2);
    let (shapes, model) = inner(&[10.0; 6], &chain);
    let g = graph(&shapes, &model);
    assert_eq!(Space::new(&g, &model).count(g.all(), 1000), n * (n + 1) / 2);
    assert!(Space::new(&g, &model).count(g.all(), 5) > 5);
}

/// Verifies: [DESIGN] RULES.md D300, D302: of a chain A - B - C - D whose
/// ends each match one row of their neighbour, which match each other
/// widely, the cheapest plan joins A to B and C to D first, a bushy tree
/// no left-deep order gives.
#[test]
fn the_cheapest_tree_may_be_bushy() {
    let (shapes, model) = inner(&[10.0, 1e4, 1e4, 10.0], &[(0, 1, 1e4), (1, 2, 10.0), (2, 3, 1e4)]);
    let g = graph(&shapes, &model);
    let mut space = Space::new(&g, &model);
    space.dphyp(g.all());
    let best = space.chosen(g.all(), Order::None).unwrap();
    let Some(&Tree::Join { sets: (l, r), .. }) = space.trees.get(best.tree) else { panic!("no join") };
    let halves = [bit(0) | bit(1), bit(2) | bit(3)];
    assert!(halves.contains(&l) && halves.contains(&r), "{l:b} {r:b}");
}

/// Verifies: [DESIGN] RULES.md D302: equalities chained through one value,
/// `a = b`, `b = c` and `a = c`, are counted once, the implied one not
/// again; a join gives at least one row of a product that has one.
#[test]
fn a_class_of_equalities_is_counted_once() {
    let (shapes, mut model) = inner(&[100.0; 3], &[(0, 1, 100.0), (1, 2, 100.0), (0, 2, 100.0)]);
    model.classes = vec![vec![(0, 100.0), (1, 100.0), (2, 100.0)]];
    for c in &mut model.conjuncts {
        c.class = Some(0);
    }
    let g = graph(&shapes, &model);
    let mut space = Space::new(&g, &model);
    space.dphyp(g.all());
    assert_eq!(space.table.get(&g.all()).map(|e| e.rows), Some(100.0));
    let (shapes, model) = inner(&[2.0, 2.0, 2.0], &[(0, 1, 1e6), (1, 2, 1e6)]);
    let g = graph(&shapes, &model);
    let mut space = Space::new(&g, &model);
    space.dphyp(g.all());
    assert_eq!(space.table.get(&g.all()).map(|e| e.rows), Some(1.0));
}

/// Verifies: [DESIGN] RULES.md D301: of `(R0 ⟕ R1) ⋈ R2`, the inner join
/// on R1 and R2, no plan joins R1 and R2 before R0, which would lose R0's
/// rows R1 does not match; of `R0 ⟕ (R1 ⋈ R2)` the LEFT join's right is
/// not taken apart; the rows of a LEFT join are at least its left's.
#[test]
fn outer_joins_reorder_only_where_sound() {
    let (mut shapes, mut model) = inner(&[1000.0, 10.0, 10.0], &[(1, 2, 10.0)]);
    if let Some(first) = shapes.first_mut() {
        *first = Shape { kind: Kind::Left, left: bit(0), right: bit(1), reads: bit(0) | bit(1), strong: true };
    }
    model.kinds = vec![Kind::Left, Kind::Inner];
    model.preds = vec![Some(Pred { fraction: 0.001, keys: vec![(0, 1)] }), None];
    let g = graph(&shapes, &model);
    let mut space = Space::new(&g, &model);
    space.dphyp(g.all());
    assert!(!space.has(bit(1) | bit(2)), "R1 joined to R2 without R0");
    assert!(space.has(g.all()));
    assert!(space.table.get(&(bit(0) | bit(1))).is_some_and(|e| e.rows >= 1000.0));
    let swapped = [
        Shape { kind: Kind::Left, left: bit(0), right: bit(1) | bit(2), reads: bit(0) | bit(1), strong: true },
        Shape { kind: Kind::Inner, left: bit(1), right: bit(2), reads: bit(1) | bit(2), strong: true },
    ];
    let mut model = inner(&[1000.0, 10.0, 10.0], &[(1, 2, 10.0)]).1;
    model.kinds = vec![Kind::Left, Kind::Inner];
    model.preds = vec![Some(Pred { fraction: 0.001, keys: vec![(0, 1)] }), None];
    if let Some(c) = model.conjuncts.first_mut() {
        c.op = 1;
    }
    let g = graph(&swapped, &model);
    let mut space = Space::new(&g, &model);
    space.dphyp(g.all());
    assert!(!space.has(bit(0) | bit(1)), "the LEFT join's right taken apart");
    assert!(space.has(g.all()));
}

/// Verifies: [DESIGN] RULES.md D304: past the budget, a chain of simple
/// edges is planned by linearized DP, a plan no cheaper than exact DP's;
/// larger than the linearized limit, by GOO-DP; each plans the whole.
#[test]
fn the_switch_adapts_to_the_budget() {
    let n = 16;
    let rows: Vec<f64> = (0..n).map(|r| 10.0 + ((r * 37) % 1000) as f64).collect();
    let chain: Vec<_> = (1..n).map(|k| (k - 1, k, 50.0)).collect();
    let (shapes, model) = inner(&rows, &chain);
    let g = graph(&shapes, &model);
    let mut exact = Space::new(&g, &model);
    assert_eq!(adaptive(&mut exact, (10_000, 100)), Algorithm::DPccp);
    let optimum = exact.chosen(g.all(), Order::None).unwrap().cost;
    let mut linear = Space::new(&g, &model);
    assert_eq!(adaptive(&mut linear, (10, 100)), Algorithm::Linearized);
    let found = linear.chosen(g.all(), Order::None).unwrap().cost;
    assert!(found >= optimum * 0.999_999, "{found} below the optimum {optimum}");
    let mut guided = Space::new(&g, &model);
    assert_eq!(adaptive(&mut guided, (10, 8)), Algorithm::GooDp);
    assert!(guided.chosen(g.all(), Order::None).unwrap().cost >= optimum * 0.999_999);
}
