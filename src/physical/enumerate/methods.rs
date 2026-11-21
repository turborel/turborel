//! \[DESIGN\] How each join may be made, and what it costs (RULES.md
//! D303), chosen inside the enumeration, so the order and the methods are
//! chosen together. Costs are in rows of a scan, as the cost model's
//! (`physical/cost.rs`), and each join adds the rows it gives, as Cout
//! does (Neumann and Radke, 2018, §2).
//!
//! * A hash join, on the equalities of a column of each operand: both read,
//!   each row hashed (`HASH_ROW`); the build side, chosen as the plan is
//!   made, is the operand of fewer rows (`enumerate/realize.rs`). It keeps
//!   the order of the side probing it.
//! * An index nested-loop join, of an inner, LEFT or semi join or an
//!   antijoin whose right operand is one relation an index of whose table
//!   answers an equality: each left row probes it (`physical/indexed.rs`),
//!   a semi join's stopping at the first match. It keeps the left's order.
//! * A merge join, of an inner or LEFT join on an equality of columns
//!   whose types order as they compare, if one operand has a plan in the
//!   key's order, as Selinger et al., "Access Path Selection in a
//!   Relational Database Management System", 1979, §5, keep: the other
//!   from a plan of that order too or sorted, then each row compared once
//!   (`CostModel::sort_compare`). Its rows are in the key's order. Where
//!   both would be sorted the hash join is taken, which reads each once.
//! * A nested loop, which pairs every row, when no equality joins them.

use super::bits::Set;
use super::conflicts::Kind;
use super::model::Order;
use super::space::{Applied, Cand, Entry, Method, Space, Tree};
use crate::physical::cost::HASH_ROW;

/// A join's operands as it is costed: each side's entry.
struct Sides<'e> {
    left: &'e Entry,
    right: &'e Entry,
}

impl Space<'_> {
    /// The plans of the join of the sets `l` and `r`, the left and right
    /// operands, applying `applied`, giving `rows` rows: the cheapest of
    /// each method for each plan of the left.
    pub(super) fn offers(&self, applied: &Applied, (l, r): (Set, Set), rows: f64) -> Vec<(Cand, Tree)> {
        let (Some(left), Some(right)) = (self.table.get(&l), self.table.get(&r)) else { return Vec::new() };
        let sides = Sides { left, right };
        let kind = applied.op.map_or(Kind::Inner, |o| self.model.kind_of(o));
        let keys = self.keys(applied, (l, r));
        let mut out = Vec::new();
        let Some(build) = right.best() else { return out };
        let tree = |how: Method, (a, b): (usize, usize)| Tree::Join { how, op: applied.op, left: a, right: b, sets: (l, r) };
        for cand in &left.cands {
            let order = if kind == Kind::Full { Order::None } else { cand.order };
            let base = cand.cost + rows;
            if keys.is_empty() {
                let cost = base + build.cost + left.rows * right.rows;
                out.push((Cand { cost, order, tree: 0 }, tree(Method::Nested, (cand.tree, build.tree))));
                continue;
            }
            let cost = base + build.cost + HASH_ROW * (left.rows + right.rows);
            out.push((Cand { cost, order, tree: 0 }, tree(Method::Hash, (cand.tree, build.tree))));
            if let Some(probe) = self.probe_cost(kind, &keys, r) {
                out.push((Cand { cost: base + left.rows * probe, order, tree: 0 }, tree(Method::Index, (cand.tree, build.tree))));
            }
        }
        out.extend(
            self.merges(&sides, kind, &keys, rows).into_iter().map(|(cand, class, trees)| (cand, tree(Method::Merge(class), trees))),
        );
        out
    }

    /// The equalities of a column of `l` and one of `r` the join judges,
    /// each as its left and right column, an inner join's those its keys
    /// imply among them (RULES.md D387).
    fn keys(&self, applied: &Applied, (l, r): (Set, Set)) -> Vec<(usize, usize)> {
        let pairs: Vec<(usize, usize)> = if let Some(pred) = applied.op.and_then(|o| self.model.preds.get(o)).and_then(Option::as_ref) {
            pred.keys.clone()
        } else {
            let stated: Vec<(usize, usize)> = applied.conjuncts.iter().filter_map(|&c| self.model.conjuncts.get(c)?.key).collect();
            // A join of the FROM clause's tree is planned as written, and
            // one a conjunct reads is no product (RULES.md D387).
            if self.written || !applied.conjuncts.is_empty() { stated } else { self.model.implied_keys((l, r)) }
        };
        pairs
            .into_iter()
            .filter_map(|(a, b)| {
                if self.model.holds(l, a) && self.model.holds(r, b) {
                    Some((a, b))
                } else if self.model.holds(l, b) && self.model.holds(r, a) {
                    Some((b, a))
                } else {
                    None
                }
            })
            .collect()
    }

    /// The cost of a probe of an index of `r`'s table by one of `keys`, if
    /// the join is inner or LEFT and `r` is one relation one answers.
    fn probe_cost(&self, kind: Kind, keys: &[(usize, usize)], r: Set) -> Option<f64> {
        if !matches!(kind, Kind::Inner | Kind::Left | Kind::Semi | Kind::Anti) {
            return None;
        }
        let rel = Space::single(r)?;
        keys.iter().filter_map(|&(_, b)| self.model.probe(rel, b)).reduce(f64::min)
    }

    /// The merge joins of `sides` on each of `keys` whose columns a merge
    /// join may join by, of an inner or LEFT join: each with its class and
    /// the trees of its operands.
    fn merges(&self, sides: &Sides<'_>, kind: Kind, keys: &[(usize, usize)], rows: f64) -> Vec<(Cand, usize, (usize, usize))> {
        if !matches!(kind, Kind::Inner | Kind::Left) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for &(a, b) in keys {
            let (Some((ka, true)), Some((kb, true))) = (self.model.order_of(a), self.model.order_of(b)) else { continue };
            let (Some(left), Some(right)) = (self.ordered(sides.left, ka), self.ordered(sides.right, kb)) else { continue };
            if !(left.2 || right.2) {
                continue;
            }
            let compared = self.model.cost.sort_compare * (sides.left.rows + sides.right.rows);
            let cost = left.0 + right.0 + compared + rows;
            out.push((Cand { cost, order: Order::Class(ka), tree: 0 }, ka, (left.1, right.1)));
        }
        out
    }

    /// The cheapest way to have the rows of `entry` in the order of class
    /// `class`: a plan of that order, or its best sorted; its cost, tree,
    /// and whether it is in the order unsorted.
    fn ordered(&self, entry: &Entry, class: usize) -> Option<(f64, usize, bool)> {
        let best = entry.best()?;
        let sorted = (best.cost + self.model.sort(entry.rows), best.tree, false);
        Some(entry.of_order(Order::Class(class)).filter(|c| c.cost < sorted.0).map_or(sorted, |c| (c.cost, c.tree, true)))
    }
}
