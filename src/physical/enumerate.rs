//! \[DESIGN\] Join enumeration (RULES.md D300-D309): the joins of a chain of
//! products, filters and outer joins planned in the order, the shape and
//! by the methods estimated cheapest, bushy trees among them, rather than
//! greedily left-deep (`physical/order.rs`, still taken when the setting
//! PLANNER.JOIN_ENUMERATION is GREEDY).
//!
//! The chain is taken apart into its initial operator tree
//! (`enumerate/extract.rs`), whose query hypergraph and conflict rules
//! (`enumerate/graph.rs`, `enumerate/conflicts.rs`) say which joins of
//! two sets of relations keep its result, so outer joins are reordered
//! where that is sound. The search adapts to the graph, as Neumann and
//! Radke, "Adaptive Optimization of Very Large Join Queries", 2018, §4.4,
//! Fig. 8, do: below `ALWAYS_EXACT` relations, or while the graph has no
//! more connected subgraphs than PLANNER.DP_BUDGET, exact dynamic
//! programming, DPccp of a graph of simple edges and inner joins alone,
//! DPhyp of any (`enumerate/dphyp.rs`); else, of a graph of simple edges
//! and inner joins of at most PLANNER.LINEARIZED_RELATIONS relations,
//! linearized DP (`enumerate/linear.rs`); else GOO guiding linearized DP,
//! or DPhyp of a hypergraph, over subtrees of at most that many relations,
//! or `HYPER_SUBTREE` (`enumerate/goo.rs`). Each join's method is chosen
//! with its order (`enumerate/methods.rs`), and the plan is made of the
//! choice (`enumerate/realize.rs`).
//!
//! A plan is taken if it is estimated cheaper than the FROM clause's
//! tree, planned by the same methods, by the gain PLANNER.REORDER_GAIN
//! gives; else the chain is planned as written. A chain of fewer than
//! `MIN_RELATIONS` relations, or of more than 128, is left to the greedy
//! planner. EXPLAIN shows
//! the order taken and the algorithm that found it on the plan's top join.

mod bits;
mod conflicts;
mod dphyp;
mod extract;
mod goo;
mod graph;
mod linear;
mod methods;
mod model;
mod modeled;
mod realize;
mod space;

use bits::{MAX_RELATIONS, Set, bit};
use extract::{Node, Query};
use goo::Inner;
use graph::Graph;
use model::{Model, Order};
use realize::{Context, Realized};
use space::{Cand, Space};

use super::built::Built;
use super::{PhysicalPlanner, Plan};
use crate::logical::{Expr, LogicalPlan, SortKey};
use crate::settings::{DP_BUDGET, JOIN_ENUMERATION, LINEARIZED_RELATIONS};

/// PLANNER.JOIN_ENUMERATION's words: the adaptive enumeration, or the
/// greedy left-deep order.
pub const ADAPTIVE: &str = "ADAPTIVE";
/// See [`ADAPTIVE`].
pub const GREEDY: &str = "GREEDY";
/// See [`ADAPTIVE`].
pub const ENUMERATIONS: &[&str] = &[ADAPTIVE, GREEDY];
/// \[DESIGN\] The default of PLANNER.DP_BUDGET: the connected subgraphs
/// past which exact DP gives way, Neumann and Radke's 10,000 (§4.1).
pub const DEFAULT_DP_BUDGET: usize = 10_000;
/// \[DESIGN\] The default of PLANNER.LINEARIZED_RELATIONS: the most
/// relations linearized DP plans whole, and the subtrees GOO-DP plans
/// again by it, the paper's 100 (§4.2, §4.3).
pub const DEFAULT_LINEARIZED: usize = 100;
/// \[DESIGN\] Below this many relations exact DP is taken without counting
/// subgraphs: a clique of 13 has fewer than the budget (§4.1).
const ALWAYS_EXACT: usize = 14;
/// \[DESIGN\] The relations of the subtrees GOO-DP plans again by DPhyp,
/// of a hypergraph, which cannot be linearized: the paper's 10 (§4.3).
const HYPER_SUBTREE: usize = 10;
/// \[DESIGN\] The fewest relations a chain is enumerated of: two have one
/// join, which the greedy planner already chooses the method and the
/// sides of.
const MIN_RELATIONS: usize = 3;

/// What became of a chain: planned, planned as written, or left to the
/// greedy order.
enum Outcome {
    Planned(Box<Built>),
    Kept,
    Greedy,
}

/// The algorithm that found a plan, as EXPLAIN names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Algorithm {
    DPccp,
    DPhyp,
    Linearized,
    GooDp,
}

impl Algorithm {
    fn name(self) -> &'static str {
        match self {
            Algorithm::DPccp => "DPccp",
            Algorithm::DPhyp => "DPhyp",
            Algorithm::Linearized => "linearized DP",
            Algorithm::GooDp => "GOO-DP",
        }
    }
}

/// Plans every subset of `space`'s graph the adaptive switch reaches, by
/// the budget and the linearized limit; the algorithm taken.
fn adaptive(space: &mut Space<'_>, (budget, linear): (usize, usize)) -> Algorithm {
    let all = space.graph.all();
    let n = space.graph.n;
    let hyper = space.graph.is_hyper();
    if n < ALWAYS_EXACT || space.count(all, budget) <= budget {
        space.dphyp(all);
        return if hyper { Algorithm::DPhyp } else { Algorithm::DPccp };
    }
    if !hyper && n <= linear && space.linearized(all) {
        return Algorithm::Linearized;
    }
    let (inner, k) = if hyper { (Inner::Hyper, HYPER_SUBTREE) } else { (Inner::Linearized, linear) };
    if !space.goo_dp(all, (k, budget), inner) {
        space.dphyp(all);
    }
    Algorithm::GooDp
}

impl PhysicalPlanner<'_> {
    /// `plan` built, its joins enumerated if they may be, else ordered
    /// greedily (`physical/order.rs`).
    pub(super) fn ordered_joins(&self, plan: &LogicalPlan, want: &[SortKey]) -> Built {
        match self.enumerated(plan, want) {
            Outcome::Planned(built) => *built,
            Outcome::Kept => self.build_node(plan, want),
            Outcome::Greedy => match self.join_order(plan, want) {
                Some(reordered) => self.build_node(&reordered, want),
                None => self.build_node(plan, want),
            },
        }
    }

    /// `plan`, a chain, planned by join enumeration, if
    /// PLANNER.JOIN_ENUMERATION is ADAPTIVE and the chain may be.
    fn enumerated(&self, plan: &LogicalPlan, want: &[SortKey]) -> Outcome {
        if !extract::is_chain(plan) || self.db.settings.word(JOIN_ENUMERATION) == GREEDY {
            return Outcome::Greedy;
        }
        let Some(query) = extract::extract(plan, &*self.db.catalog) else { return Outcome::Greedy };
        let n = query.leaves.len();
        if !(MIN_RELATIONS..=MAX_RELATIONS).contains(&n) {
            return Outcome::Greedy;
        }
        let Some(model) = self.model(&query, want) else { return Outcome::Greedy };
        let conjuncts: Vec<(usize, Set)> = model.conjuncts.iter().map(|c| (c.op, c.reads)).collect();
        let graph = Graph::new(n, &query.shapes, &conjuncts);
        let wanted = if want.is_empty() { Order::None } else { Order::Want };
        let Some(written) = written_cost(&graph, &model, &query, wanted) else { return Outcome::Greedy };
        let mut space = Space::new(&graph, &model);
        let limits = (self.setting(DP_BUDGET), self.setting(LINEARIZED_RELATIONS));
        let algorithm = adaptive(&mut space, limits);
        let Some(best) = space.chosen(graph.all(), wanted) else { return Outcome::Greedy };
        if best.cost >= self.weights().reorder_gain * written {
            return Outcome::Kept;
        }
        let cx = Context { query: &query, model: &model, space: &space, want };
        match self.realize(&cx, best.tree, 0) {
            Some(done) => Outcome::Planned(Box::new(self.restored(&cx, done, algorithm))),
            None => Outcome::Kept,
        }
    }

    /// A whole-number setting as a count.
    fn setting(&self, key: crate::settings::Key) -> usize {
        usize::try_from(self.db.settings.integer(key)).unwrap_or(usize::MAX)
    }

    /// The realized chain under a projection putting its columns back in
    /// the FROM clause's order, its top join telling the order and the
    /// algorithm, if the plan is shown.
    fn restored(&self, cx: &Context<'_>, done: Realized, algorithm: Algorithm) -> Built {
        let Realized { mut plan, layout, order, shape } = done;
        if self.shown {
            annotate(&mut plan, &format!("  (join order by {}: {shape})", algorithm.name()));
        }
        let exprs: Vec<Expr> = (0..cx.query.width).map(|c| Expr::Column(cx.place_of(&layout, c).unwrap_or(c))).collect();
        let inner_names: Option<Vec<String>> =
            self.shown.then(|| layout.iter().filter_map(|&r| cx.query.leaves.get(r)).flat_map(|l| l.plan.names()).collect());
        let detail = inner_names
            .map(|n| exprs.iter().map(|e| crate::logical::render_expr(e, &n)).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        Built { plan: Plan::Project { exprs, detail, input: Box::new(plan) }, ordered: !cx.want.is_empty() && cx.model.answers(order) }
    }
}

/// The cost of the FROM clause's tree, each of its joins by its cheapest
/// method, if it can be planned: none past any plan, if it has a product
/// of two sets a class of the inner joins' keys joins, which the model
/// counts as joined but the tree, planned as written, makes a product of
/// (RULES.md D387).
fn written_cost(graph: &Graph, model: &Model, query: &Query, wanted: Order) -> Option<f64> {
    let mut space = Space::new(graph, model);
    space.written = true;
    let root = query.operands.len().checked_sub(1)?;
    let (all, product) = written(&mut space, query, Node::Op(root), 0)?;
    let cost = space.chosen(all, wanted).map(|c: Cand| c.cost)?;
    Some(if product { f64::INFINITY } else { cost })
}

/// The set of `node` of the FROM clause's tree, its joins made in `space`,
/// and whether one is a product of sets a class joins.
fn written(space: &mut Space<'_>, query: &Query, node: Node, depth: usize) -> Option<(Set, bool)> {
    if depth > MAX_RELATIONS {
        return None;
    }
    match node {
        Node::Leaf(r) => Some((bit(r), false)),
        Node::Op(o) => {
            let &(l, r) = query.operands.get(o)?;
            let (sl, pl) = written(space, query, l, depth.saturating_add(1))?;
            let (sr, pr) = written(space, query, r, depth.saturating_add(1))?;
            let s = sl | sr;
            let read = space.model.conjuncts.iter().any(|c| c.reads & !s == 0 && c.reads & !sl != 0 && c.reads & !sr != 0);
            let product = !read && !space.model.implied_keys((sl, sr)).is_empty();
            space.join(sl, sr).then_some((s, pl || pr || product))
        }
    }
}

/// Adds `note` to the description of `plan`'s top join, or of the filter
/// over it.
fn annotate(plan: &mut Plan, note: &str) {
    match plan {
        Plan::HashJoin { detail, .. } | Plan::OuterJoin { detail, .. } | Plan::Filter { detail, .. } => {
            detail.push_str(note);
        }
        Plan::MergeJoin(m) => m.detail.push_str(note),
        Plan::IndexJoin { detail, .. } => *detail = format!("{detail}{note}").into(),
        Plan::Project { input, .. } => annotate(input, note),
        Plan::SeqScan { .. }
        | Plan::Parallel(_)
        | Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::Product { .. }
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Recursive { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Recognize { .. }
        | Plan::Polymorphic(..)
        | Plan::Counted(_) => {}
    }
}

// Unit tests: the enumerations' plans and the conflict rules on synthetic
// graphs, and the adaptive switch (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
