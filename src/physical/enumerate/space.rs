//! \[DESIGN\] The search space every enumeration fills (RULES.md D300,
//! D302, D303): the dynamic programming table, of each connected set of
//! relations its estimated rows and its best plans, and the join of two
//! sets, made only if an edge joins them and the operators it applies
//! allow it (`enumerate/conflicts.rs`).
//!
//! A set keeps its cheapest plan, and the cheapest plan of each order a
//! later merge join or the order asked of the chain may use: Selinger's
//! interesting orders ("Access Path Selection in a Relational Database
//! Management System", 1979, §5), kept across the enumeration, so a plan
//! dearer than another may still win once its order spares a sort.

use crate::hash::xxhash::Xxh3BuildHasher;
use std::collections::HashMap;

use super::bits::{Set, count, lowest, within};
use super::conflicts::{Kind, obeyed};
use super::graph::Graph;
use super::model::{Model, Order};

/// How a join is made (`enumerate/methods.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Method {
    /// A hash join on its equalities.
    Hash,
    /// A merge join of operands ordered by the columns of the class.
    Merge(usize),
    /// Each left row probing an index of the right relation's table.
    Index,
    /// Every pair, judged.
    Nested,
}

/// A plan in the table: a relation read, in an order or not, or a join of
/// two plans, its operands' sets, and the non-inner operator it applies.
#[derive(Debug, Clone, Copy)]
pub(super) enum Tree {
    Leaf { rel: usize, order: Order },
    Join { how: Method, op: Option<usize>, left: usize, right: usize, sets: (Set, Set) },
}

/// A plan of a set: its cost, the order of its rows, and its tree.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cand {
    pub(super) cost: f64,
    pub(super) order: Order,
    pub(super) tree: usize,
}

/// A set's entry: its estimated rows, at most its bound (RULES.md D385),
/// and its plans, the cheapest of each order.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub(super) rows: f64,
    pub(super) upper: f64,
    pub(super) cands: Vec<Cand>,
}

impl Entry {
    /// Its cheapest plan.
    pub(super) fn best(&self) -> Option<Cand> {
        self.cands.iter().copied().min_by(|a, b| a.cost.total_cmp(&b.cost))
    }

    /// Its cheapest plan of order `order`, if it has one.
    pub(super) fn of_order(&self, order: Order) -> Option<Cand> {
        self.cands.iter().copied().filter(|c| c.order == order).min_by(|a, b| a.cost.total_cmp(&b.cost))
    }

    /// Whether `cand` is cheaper than its order's plan, or of an order it
    /// has none of.
    fn takes(&self, cand: &Cand) -> bool {
        self.cands.iter().find(|c| c.order == cand.order).is_none_or(|kept| cand.cost < kept.cost)
    }

    /// Adds `cand` if it is the cheapest of its order; whether it was.
    fn offer(&mut self, cand: Cand) -> bool {
        if let Some(kept) = self.cands.iter_mut().find(|c| c.order == cand.order) {
            if kept.cost <= cand.cost {
                return false;
            }
            *kept = cand;
            return true;
        }
        self.cands.push(cand);
        true
    }
}

/// What a join of two sets applies: the operator other than an inner join,
/// with the sets that are its left and right operands, if one; else the
/// conjuncts of inner joins it judges, either set the left.
#[derive(Debug, Clone)]
pub(super) struct Applied {
    pub(super) op: Option<usize>,
    pub(super) left: Set,
    pub(super) right: Set,
    pub(super) conjuncts: Vec<usize>,
}

/// The table and the plans in it.
pub(super) struct Space<'m> {
    pub(super) graph: &'m Graph,
    pub(super) model: &'m Model,
    pub(super) table: HashMap<Set, Entry, Xxh3BuildHasher>,
    pub(super) trees: Vec<Tree>,
    /// The joins tried that the operators allowed: the work done.
    pub(super) pairs: usize,
    /// Whether it costs the FROM clause's tree, which is planned as
    /// written, a product where no conjunct reads both operands, so no
    /// join of it judges the keys its classes imply (RULES.md D387).
    pub(super) written: bool,
}

impl<'m> Space<'m> {
    /// The table of `graph`'s relations alone, each with its reads.
    pub(super) fn new(graph: &'m Graph, model: &'m Model) -> Space<'m> {
        let mut space = Space { graph, model, table: HashMap::default(), trees: Vec::new(), pairs: 0, written: false };
        for rel in 0..graph.n {
            let upper = model.rels.get(rel).map_or(f64::INFINITY, |r| r.upper);
            let mut entry = Entry { rows: model.rows(rel), upper, cands: Vec::new() };
            for (order, cost) in model.reads(rel) {
                let tree = space.tree(Tree::Leaf { rel, order });
                entry.offer(Cand { cost, order, tree });
            }
            space.table.insert(super::bits::bit(rel), entry);
        }
        space
    }

    /// `tree`, kept, by its place.
    fn tree(&mut self, tree: Tree) -> usize {
        self.trees.push(tree);
        self.trees.len().saturating_sub(1)
    }

    /// Whether `s` has an entry.
    pub(super) fn has(&self, s: Set) -> bool {
        self.table.contains_key(&s)
    }

    /// What a join of `s1` and `s2` applies, if they are disjoint, an edge
    /// joins them, and its operators allow it: at most one operator other
    /// than an inner join, reading both, and no conjunct of an inner join
    /// with it; each operator whose edge joins them obeying its rules.
    pub(super) fn applies(&self, s1: Set, s2: Set) -> Option<Applied> {
        let s = s1 | s2;
        if s1 & s2 != 0 || !self.graph.connected(s1, s2) {
            return None;
        }
        let straddles = |reads: Set| within(reads, s) && !within(reads, s1) && !within(reads, s2);
        let conjuncts: Vec<usize> = (0..self.model.conjuncts.len()).filter(|&c| straddles(self.model.reads_of(c))).collect();
        let mut others = self.graph.edges.iter().filter(|e| e.conjunct.is_none() && straddles(e.left | e.right));
        let other = others.find(|e| self.graph.ops.get(e.op).is_some_and(|o| o.shape.kind != Kind::Inner));
        let used = self.graph.edges.iter().filter(|e| straddles(e.left | e.right)).map(|e| e.op);
        if !used.clone().all(|o| self.graph.ops.get(o).is_some_and(|op| obeyed(&op.rules, s))) {
            return None;
        }
        let Some(edge) = other else { return Some(Applied { op: None, left: s1, right: s2, conjuncts }) };
        let alone = used.filter(|&o| self.graph.ops.get(o).is_some_and(|op| op.shape.kind != Kind::Inner)).all(|o| o == edge.op);
        if !conjuncts.is_empty() || !alone {
            return None;
        }
        let (left, right) = if within(edge.left, s1) && within(edge.right, s2) {
            (s1, s2)
        } else if within(edge.left, s2) && within(edge.right, s1) {
            (s2, s1)
        } else {
            return None;
        };
        Some(Applied { op: Some(edge.op), left, right, conjuncts })
    }

    /// The rows of the join of `s1` and `s2` applying `applied`, and its
    /// bound, if both have entries: the set's, if it has one already, so
    /// that every plan of a set is costed by the same rows; else of this
    /// pair, the rows at most the bound.
    pub(super) fn rows_of(&self, applied: &Applied) -> Option<(f64, f64)> {
        let s = applied.left | applied.right;
        if let Some(entry) = self.table.get(&s) {
            return Some((entry.rows, entry.upper));
        }
        let (left, right) = (self.table.get(&applied.left)?, self.table.get(&applied.right)?);
        let rows = self.model.joined_rows((applied.left, left.rows), (applied.right, right.rows), applied);
        let kind = applied.op.map_or(Kind::Inner, |o| self.model.kind_of(o));
        // Only a side of one relation knows the most rows of a column's
        // value (`Model::joined_upper`), so the keys are found for it alone.
        let one = Space::single(applied.left).or(Space::single(applied.right)).is_some();
        let keys = if one { self.keys(applied, (applied.left, applied.right)) } else { Vec::new() };
        let upper = self.model.joined_upper((applied.left, left.upper), (applied.right, right.upper), (kind, &keys));
        Some((rows.min(upper), upper))
    }

    /// The rows of the join `applied` gives, as `rows_of` says, its bound
    /// found only where it could lower them: every bound of a join is at
    /// least the lesser of its sides' and their product, as a column's
    /// most rows of one value are at least one (`Model::joined_upper`).
    pub(super) fn estimate_of(&self, applied: &Applied) -> Option<f64> {
        let s = applied.left | applied.right;
        if let Some(entry) = self.table.get(&s) {
            return Some(entry.rows);
        }
        let (left, right) = (self.table.get(&applied.left)?, self.table.get(&applied.right)?);
        let rows = self.model.joined_rows((applied.left, left.rows), (applied.right, right.rows), applied);
        if rows <= left.upper.min(right.upper).min(left.upper * right.upper) {
            return Some(rows);
        }
        self.rows_of(applied).map(|(bounded, _)| bounded)
    }

    /// Joins `s1` and `s2` if they may be, offering each plan of the join
    /// to their union's entry (`enumerate/methods.rs`); whether they were.
    pub(super) fn join(&mut self, s1: Set, s2: Set) -> bool {
        let Some(applied) = self.applies(s1, s2) else { return false };
        let Some((rows, upper)) = self.rows_of(&applied) else { return false };
        self.pairs = self.pairs.saturating_add(1);
        let s = s1 | s2;
        self.table.entry(s).or_insert_with(|| Entry { rows, upper, cands: Vec::new() });
        let kind = applied.op.and_then(|o| self.graph.ops.get(o)).map_or(Kind::Inner, |o| o.shape.kind);
        let sides = [(applied.left, applied.right), (applied.right, applied.left)];
        for &(l, r) in sides.iter().take(if kind.commutes() { 2 } else { 1 }) {
            for (cand, tree) in self.offers(&applied, (l, r), rows) {
                if !self.table.get(&s).is_some_and(|entry| entry.takes(&cand)) {
                    continue;
                }
                let at = self.tree(tree);
                if let Some(entry) = self.table.get_mut(&s) {
                    entry.offer(Cand { tree: at, ..cand });
                }
            }
        }
        true
    }

    /// The best plan of the set `s`, and its cost; of the order `want`
    /// if it has one cheaper than its best and a sort of its rows.
    pub(super) fn chosen(&self, s: Set, want: Order) -> Option<Cand> {
        let entry = self.table.get(&s)?;
        let best = entry.best()?;
        let sorted = best.cost + self.model.sort(entry.rows);
        let answering =
            entry.cands.iter().filter(|c| want != Order::None && self.model.answers(c.order)).min_by(|a, b| a.cost.total_cmp(&b.cost));
        Some(answering.filter(|o| o.cost <= sorted).copied().unwrap_or(best))
    }

    /// Of a set of one relation, which.
    pub(super) fn single(s: Set) -> Option<usize> {
        (count(s) == 1).then(|| lowest(s)).flatten()
    }
}
