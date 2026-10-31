//! \[DESIGN\] Equality joins: a filter over a product whose condition
//! equates a column of each operand is planned as a hash join, which reads
//! the right operand once into a hash table and pairs each row of the left
//! with only the rows whose keys are equal, rather than with every row.
//! Split from physical.rs (RULES.md R-SIZE-2).
//!
//! The rows are those the filter keeps, in the same order: each left row's
//! matches in the order of the right operand. The equalities are evaluated
//! only on the pairs they could hold for, and the rest of the condition only
//! on the pairs they keep, so the condition must not be able to raise an
//! error (RULES.md D15): a conjunct that can, reading one operand's columns
//! alone, is moved onto that operand before, over a guard
//! (`optimizer::PushRaisingFilterIntoProduct`), and the rest is then this
//! filter's. An outer join is planned the same way, on the equalities of
//! its `<join condition>`. Of an inner or a LEFT outer join, few left rows
//! each probe an index of the right operand's table instead, if that is
//! estimated cheaper than reading it whole (`physical/indexed.rs`).

use super::indexed::{Equijoin, index_join};
use super::{Built, PhysicalPlanner, Plan, of_left};
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin, SortKey, and_all, column_name, judged_by_sets, render_cond, split_and};
use crate::value::CompOp;

impl PhysicalPlanner<'_> {
    /// `cond` over `input` as a hash join, if `input` is a product and
    /// `cond` equates a column of each of its operands and cannot raise an
    /// error. The conjuncts that are not such equalities filter its rows.
    pub(super) fn hash_join(&self, cond: &Cond, input: &LogicalPlan, want: &[SortKey]) -> Option<Built> {
        let LogicalPlan::Product { left, right } = input else { return None };
        if cond.can_raise() {
            return None;
        }
        let width = left.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut keys, mut rest) = (Vec::new(), Vec::new());
        for c in conjuncts {
            match equated(&c, width) {
                Some(key) => keys.push(key),
                None => rest.push(c),
            }
        }
        if keys.is_empty() {
            return None;
        }
        let names = input.names();
        let detail = keys
            .iter()
            .map(|&(l, r)| format!("{} = {}", column_name(&names, l), column_name(&names, r.saturating_add(width))))
            .collect::<Vec<_>>()
            .join(" AND ");
        // As in a product, the rows come in the order of the left operand.
        let want_left = of_left(want, width);
        let (l, r) = (self.build_node(left, want_left), self.build_node(right, &[]));
        let ordered = !want_left.is_empty() && l.ordered;
        // A few left rows each probing an index of the right's table.
        if let Some((probing, rows)) = self.probed_instead((left, right), &keys, &r.plan) {
            let join = Equijoin { width, keys: &keys, rest, outer: false };
            return Some(Built { plan: index_join(probing, join, (l.plan, rows), &names), ordered });
        }
        let join = Plan::HashJoin { left: Box::new(l.plan), right: Box::new(r.plan), keys, detail };
        let plan = match and_all(rest).map(judged_by_sets) {
            Some(c) => Plan::Filter { detail: render_cond(&c, &names), cond: c, est: None, input: Box::new(join) },
            None => join,
        };
        Some(Built { plan, ordered })
    }
}

/// The columns `c` equates, one of the left operand of a product whose
/// left operand is `width` columns wide and one of the right, numbered in
/// it: "left = right" or "right = left".
fn equated(c: &Cond, width: usize) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    let (a, b) = (*a, *b);
    if a < width && b >= width {
        Some((a, b.saturating_sub(width)))
    } else if b < width && a >= width {
        Some((b, a.saturating_sub(width)))
    } else {
        None
    }
}

impl PhysicalPlanner<'_> {
    /// [ISO/IEC 9075:1992 §7.5] The outer join of `left` and `right` on
    /// `cond`: on the equalities of a column of each, through a hash
    /// table, the rest of the condition on the pairs they keep, unless the
    /// condition can raise an error, when all of it is evaluated on every
    /// pair (RULES.md D15). Its rows come in the order of `left`, unless
    /// the unmatched rows of a preserved `right` follow them.
    #[inline(never)]
    pub(super) fn outer_join(&self, (kind, cond): (OuterJoin, &Cond), left: &LogicalPlan, right: &LogicalPlan, want: &[SortKey]) -> Built {
        let width = left.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut keys, mut rest) = (Vec::new(), Vec::new());
        for c in conjuncts {
            match equated(&c, width).filter(|_| !cond.can_raise()) {
                Some(key) => keys.push(key),
                None => rest.push(c),
            }
        }
        let names: Vec<String> = left.names().into_iter().chain(right.names()).collect();
        let detail = render_cond(cond, &names);
        let want_left = if kind.keeps_right() { &[] } else { of_left(want, width) };
        let (l, r) = (self.build_node(left, want_left), self.build_node(right, &[]));
        let ordered = !want_left.is_empty() && l.ordered;
        // Of a LEFT join, a few left rows each probing an index of the right's table.
        let probed = (kind == OuterJoin::Left && !keys.is_empty()).then(|| self.probed_instead((left, right), &keys, &r.plan)).flatten();
        if let Some((probing, rows)) = probed {
            let join = Equijoin { width, keys: &keys, rest, outer: true };
            return Built { plan: index_join(probing, join, (l.plan, rows), &names), ordered };
        }
        let plan = Plan::OuterJoin {
            kind,
            left: Box::new(l.plan),
            right: Box::new(r.plan),
            keys,
            cond: and_all(rest).map(judged_by_sets),
            widths: (width, right.width()),
            detail,
        };
        Built { plan, ordered }
    }
}
