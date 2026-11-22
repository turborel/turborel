//! \[DESIGN\] A filter over a product planned as a range join (RULES.md
//! D580), and the plan node made of what the search answers: the operand
//! of fewer estimated rows sorted, unless the left's order is the order
//! asked for, if the range join is estimated cheaper than the nested loop,
//! the hash join on the condition's equalities, and an index probed by its
//! comparisons, whichever the condition allows (`physical/range/cost.rs`).

use super::{RangeJoin, Ranged, expanded_periods, range_cost, ranged};
use std::fmt::Write;

use crate::logical::{Cond, Expr, LogicalPlan, and_all, cond_can_raise_typed, judged_by_sets};
use crate::logical::{map_expr_columns, plan_column_type, render_cond, render_expr, split_and};
use crate::physical::cost::HASH_ROW;
use crate::physical::join::Flip;
use crate::physical::shown::Shown;
use crate::physical::{Built, PhysicalPlanner, Plan};
use crate::settings::RANGE_JOINS;

/// One way round of a range join: what its search answers, its cost, and
/// whether it is the join of its operands swapped.
pub(super) struct Way {
    pub(super) ranged: Ranged,
    pub(super) cost: f64,
    pub(super) swapped: bool,
}

/// What makes a range join's plan node: whether it keeps the left rows
/// matching none, its operands' degrees, its whole condition over its
/// pairs, how it is described, and its estimated rows.
pub(in crate::physical) struct Made<'s> {
    pub(in crate::physical) outer: bool,
    pub(in crate::physical) widths: (usize, usize),
    pub(in crate::physical) full: Cond,
    pub(in crate::physical) shown: Shown<'s>,
    pub(in crate::physical) est: f64,
}

impl PhysicalPlanner<'_> {
    /// `cond` over `input` as a range join, if `input` is a product,
    /// PLANNER.RANGE_JOINS is ON, `cond` cannot raise and compares a value
    /// of each operand by order, and it is estimated cheapest.
    /// Its rows keep no order: past the work area they come a part of the
    /// sorted operand at a time (`exec/join/range.rs`).
    pub(in crate::physical) fn range_join(&self, cond: &Cond, input: &LogicalPlan) -> Option<Built> {
        let LogicalPlan::Product { left, right } = input else { return None };
        if !self.searchable(cond, left.width()) {
            return None;
        }
        let est = self.estimating(|e| e.filter_card(cond, input)).rows;
        let way = self.range_way(cond, (left, right), (est, false))?;
        if way.cost >= self.joined_otherwise(cond, (left, right), &way.ranged) {
            return None;
        }
        let names = self.names_of(input);
        let shown = Shown::of(names.as_deref());
        let widths = (left.width(), right.width());
        if way.swapped {
            let flip = Flip { width: widths.0, right_width: widths.1 };
            let flipped = flip.names(shown);
            let of_flipped = Shown::of(flipped.as_deref());
            let (l, r) = (self.build_node(right, &[]), self.build_node(left, &[]));
            let made = Made { outer: false, widths: (widths.1, widths.0), full: flip.cond(cond.clone()), shown: of_flipped, est };
            return Some(Built::unordered(flip.restored(range_plan(way.ranged, (l.plan, r.plan), made), of_flipped)));
        }
        let (l, r) = (self.build_node(left, &[]), self.build_node(right, &[]));
        let made = Made { outer: false, widths, full: cond.clone(), shown, est };
        Some(Built::unordered(range_plan(way.ranged, (l.plan, r.plan), made)))
    }

    /// Whether PLANNER.RANGE_JOINS is ON and a search answers part of
    /// `cond`, over pairs whose first `width` columns are the left's. It
    /// reads the condition's text alone, so a join no search answers is
    /// planned with no estimate more.
    pub(super) fn searchable(&self, cond: &Cond, width: usize) -> bool {
        let mut parts = Vec::new();
        split_and(cond.clone(), &mut parts);
        self.db.settings.is_on(RANGE_JOINS) && ranged(expanded_periods(parts), width).is_some()
    }

    /// The cheaper way round of a range join of `left` and `right` on
    /// `cond`, estimated to give `est` rows, if it may be one: not swapped,
    /// if `kept`, the left's order being kept.
    pub(super) fn range_way(&self, cond: &Cond, (left, right): (&LogicalPlan, &LogicalPlan), (est, kept): (f64, bool)) -> Option<Way> {
        if !self.db.settings.is_on(RANGE_JOINS) || self.cond_raises(cond, (left, right)) {
            return None;
        }
        let (width, right_width) = (left.width(), right.width());
        let flip = Flip { width, right_width };
        let back = Flip { width: right_width, right_width: width };
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let conjuncts = expanded_periods(conjuncts);
        let swapped_conjuncts = conjuncts.iter().map(|c| flip.cond(c.clone())).collect();
        let rows = (self.rows_of(left), self.rows_of(right));
        let product = LogicalPlan::Product { left: Box::new(left.clone()), right: Box::new(right.clone()) };
        let costed = |r: Ranged, swapped: bool| {
            let searched: Vec<Cond> = r.searched.iter().map(|c| if swapped { back.cond(c.clone()) } else { c.clone() }).collect();
            let candidates = and_all(searched).map_or(rows.0 * rows.1, |c| self.estimating(|e| e.filter_card(&c, &product)).rows);
            let sides = if swapped { (rows.1, rows.0) } else { rows };
            Way { cost: range_cost(&self.weights(), sides, (candidates, est), r.pruned.is_some()), ranged: r, swapped }
        };
        let normal = ranged(conjuncts, width).map(|r| costed(r, false));
        let swapped = if kept { None } else { ranged(swapped_conjuncts, right_width).map(|r| costed(r, true)) };
        match (normal, swapped) {
            (Some(n), Some(s)) => Some(if s.cost < n.cost { s } else { n }),
            (Some(one), None) | (None, Some(one)) => Some(one),
            (None, None) => None,
        }
    }

    /// The cost of the cheapest other plan of the join of `left` and
    /// `right` on `cond`, whose search `ranged` is: the hash join on its
    /// equalities, if it has any and no conjunct can raise as the hash
    /// join judges it (`physical/join.rs`); the nested loop; an index
    /// probed by its comparisons; or one probed by its equalities and
    /// bounds of the key's next column (RULES.md D776).
    pub(super) fn joined_otherwise(&self, cond: &Cond, (left, right): (&LogicalPlan, &LogicalPlan), ranged: &Ranged) -> f64 {
        let rows = (self.rows_of(left), self.rows_of(right));
        let nested = rows.0 * rows.1;
        let product = LogicalPlan::Product { left: Box::new(left.clone()), right: Box::new(right.clone()) };
        let keys: Vec<Cond> =
            ranged.searched.iter().filter(|c| matches!(c, Cond::Compare(_, crate::value::CompOp::Eq, _))).cloned().collect();
        let keys = if cond.can_raise() { Vec::new() } else { keys };
        let hashed = and_all(keys).map(|k| HASH_ROW * (rows.0 + rows.1) + self.estimating(|e| e.filter_card(&k, &product)).rows);
        let probed = self.comparison_cost(cond, (left, right));
        let bounded = self.bounded_join_cost(cond, (left, right));
        [Some(nested), hashed, probed, bounded].into_iter().flatten().fold(f64::INFINITY, f64::min)
    }

    /// Whether `cond`, over the pairs of `left` and `right`, can raise an
    /// exception condition by its columns' declared types (`logical/raises.rs`).
    pub(in crate::physical) fn cond_raises(&self, cond: &Cond, (left, right): (&LogicalPlan, &LogicalPlan)) -> bool {
        let catalog = &*self.db.catalog;
        let width = left.width();
        let types = |i: usize| match i.checked_sub(width) {
            Some(at) => plan_column_type(right, at, catalog),
            None => plan_column_type(left, i, catalog),
        };
        cond_can_raise_typed(cond, &types)
    }
}

/// The range join of `left` and `right` searching as `ranged` says, made
/// as `made` says.
pub(in crate::physical) fn range_plan(ranged: Ranged, (left, right): (Plan, Plan), made: Made<'_>) -> Plan {
    let Ranged { keys, sorted, lower, upper, pruned, rest, .. } = ranged;
    let Made { outer, widths, full, shown, est } = made;
    let width = widths.0;
    let paired = |e: &Expr| map_expr_columns(e.clone(), &|i| i.saturating_add(width));
    let detail = shown.text(|names| {
        let mut how = format!("sorted on {}", render_expr(&paired(&sorted), names));
        for k in &keys {
            // Writing to a String cannot fail.
            let _ = write!(how, " within {}", render_expr(&Expr::Column(k.right.saturating_add(width)), names));
        }
        if let Some(p) = &pruned {
            let _ = write!(how, ", pruned by {}", render_expr(&paired(&p.build), names));
        }
        format!("{}  ({how})", render_cond(&full, names))
    });
    let cond = and_all(rest).map(judged_by_sets);
    let full = judged_by_sets(full);
    let join = RangeJoin { outer, left, right, keys, sorted, lower, upper, pruned, cond, full, widths, detail, est };
    Plan::RangeJoin(Box::new(join))
}
