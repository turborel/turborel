//! \[DESIGN\] Outer joins for `hoist_raising_expressions` (RULES.md D373):
//! a LEFT join of a chain of products whose condition reads one operand of
//! the chain moved onto that operand, and the parts of an ON condition
//! over both operands made columns of one (`hoisting.rs`).
//!
//! The chain's rows are each a row of every operand. A LEFT join keeps
//! each of them once for each row of the other operand its condition is
//! true of, or once with nulls [ISO/IEC 9075:1992 §7.5 GR1c, GR5]; when
//! the condition reads, of the chain, the columns of operand X alone, the
//! rows it is true of depend on X's part of the row alone, so the join is
//! the chain with X replaced by X's LEFT join, its columns in their
//! order. That is how a scalar subquery the optimizer unnested
//! (`unnesting.rs`) is joined to the chain of its outer query, and its
//! `SINGLE`, which raises of a group of more than one row, then reads one
//! operand of the chain, the group's, and is hoisted. The condition is
//! then evaluated once for each row of X and the other's, not once for
//! each of them and each row of the other operands, so it must neither
//! raise nor change its value from one evaluation to the next: of the
//! declared types it cannot raise (`logical/raises.rs`), and it has no
//! subquery and nothing `found.rs` finds impure.

use super::found::{Finder, Operands, pure};
use super::{Hoisted, operands_of, replaced};
use crate::catalog::Catalog;
use crate::logical::{
    Cond, Expr, LogicalPlan, OuterJoin, and_all, can_raise_typed, cond_can_raise_typed, cond_columns, map_cond_columns, map_expr_columns,
    plan_column_type, split_and,
};

/// An outer join, as the rule finds it.
pub(super) struct Join<'a> {
    pub(super) kind: OuterJoin,
    pub(super) cond: &'a Cond,
    pub(super) left: &'a LogicalPlan,
    pub(super) right: &'a LogicalPlan,
}

impl Join<'_> {
    /// The LEFT join moved onto the operand of the chain of products it
    /// joins, seen through a projection of its columns, whose columns its
    /// condition reads, as the module says.
    pub(super) fn moved(&self, catalog: &dyn Catalog) -> Option<LogicalPlan> {
        if self.kind != OuterJoin::Left || !self.steady(catalog) {
            return None;
        }
        let width = self.left.width();
        let projected = if let LogicalPlan::Project { exprs, input } = self.left { Some((exprs, &**input)) } else { None };
        let (exprs, chain): (&[Expr], &LogicalPlan) = match projected {
            Some((exprs, input)) => (exprs.as_slice(), input),
            None => (&[], self.left),
        };
        let of_chain = |c: usize| if projected.is_some() { exprs.get(c).and_then(column_of) } else { Some(c) };
        if !matches!(chain, LogicalPlan::Product { .. }) || (0..width).any(|c| of_chain(c).is_none()) {
            return None;
        }
        let mut plans = Vec::new();
        operands_of(chain, &mut plans);
        let operands = Operands::new(plans);
        let mut read = Vec::new();
        cond_columns(self.cond, &mut read);
        let of_left: Vec<usize> = read.iter().filter(|&&c| c < width).filter_map(|&c| of_chain(c)).collect();
        let at = operands.one(&of_left)?;
        let (start, own) = (operands.starts.get(at).copied()?, (*operands.plans.get(at)?).clone());
        let (own_width, joined) = (own.width(), self.right.width());
        let cond = map_cond_columns(self.cond.clone(), &|c| match c.checked_sub(width) {
            Some(r) => own_width.saturating_add(r),
            None => of_chain(c).unwrap_or(c).saturating_sub(start),
        });
        let x = LogicalPlan::OuterJoin { kind: OuterJoin::Left, cond, left: Box::new(own), right: Box::new(self.right.clone()) };
        let mut moved = operands.plans.iter().enumerate().map(|(n, o)| if n == at { x.clone() } else { (*o).clone() });
        let product = replaced(chain, &mut moved)?;
        // The chain's columns after X's move past the other operand's.
        let end = start.saturating_add(own_width);
        let placed = |k: usize| if k >= end { k.saturating_add(joined) } else { k };
        let kept = (0..width).map(|c| Expr::Column(placed(of_chain(c).unwrap_or(c))));
        let columns = kept.chain((0..joined).map(|r| Expr::Column(end.saturating_add(r)))).collect();
        Some(LogicalPlan::Project { exprs: columns, input: Box::new(product) })
    }

    /// Whether the condition, evaluated fewer times, has the same effect:
    /// it cannot raise, has no subquery, and is pure.
    fn steady(&self, catalog: &dyn Catalog) -> bool {
        let join = LogicalPlan::OuterJoin {
            kind: self.kind,
            cond: self.cond.clone(),
            left: Box::new(self.left.clone()),
            right: Box::new(self.right.clone()),
        };
        let mut all = self.cond.subqueries().is_empty();
        self.cond.for_each_expr(&mut |e| all = all && pure(e, 0));
        all && !cond_can_raise_typed(self.cond, &|c| plan_column_type(&join, c, catalog))
    }

    /// The join with the parts of its condition over both operands made
    /// columns of one, if one has a part to take and the condition is then
    /// left raising nothing. A part of the operand that supplies nulls is
    /// taken over a guard of the other; of a preserved one, only a part
    /// that cannot raise, which needs none.
    pub(super) fn hoisted(&self, catalog: &dyn Catalog) -> Option<LogicalPlan> {
        let kind = self.kind;
        let operands = Operands::new(vec![self.left, self.right]);
        let safe = |at: usize, e: &Expr| {
            let (plan, start) = (*operands.plans.get(at)?, operands.starts.get(at).copied()?);
            let own = map_expr_columns(e.clone(), &|c| c.saturating_sub(start));
            Some(!can_raise_typed(&own, &|c| plan_column_type(plan, c, catalog)))
        };
        // A guard of a preserved operand would drop its rows.
        let supplies =
            |at: usize| if at == 0 { kind.keeps_right() && !kind.keeps_left() } else { kind.keeps_left() && !kind.keeps_right() };
        let allowed = |at: usize, e: &Expr| supplies(at) || safe(at, e).unwrap_or(false);
        let mut finder = Finder::new(&operands, &allowed);
        let mut conjuncts = Vec::new();
        split_and(self.cond.clone(), &mut conjuncts);
        let conjuncts: Vec<Cond> = conjuncts.into_iter().map(|c| if operands.searched(&c) { finder.cond(c) } else { c }).collect();
        let mut hoisted = Hoisted::of(&operands, finder.found, catalog)?;
        let [l, r] = <[LogicalPlan; 2]>::try_from(std::mem::take(&mut hoisted.operands)).ok()?;
        let on = and_all(conjuncts.into_iter().map(|c| hoisted.cond(c)).collect())?;
        let join = LogicalPlan::OuterJoin { kind, cond: on.clone(), left: Box::new(l), right: Box::new(r) };
        if cond_can_raise_typed(&on, &|c| plan_column_type(&join, c, catalog)) {
            return None;
        }
        Some(LogicalPlan::Project { exprs: hoisted.kept(), input: Box::new(join) })
    }
}

/// The column `e` is, if it is one.
fn column_of(e: &Expr) -> Option<usize> {
    if let Expr::Column(c) = e { Some(*c) } else { None }
}
