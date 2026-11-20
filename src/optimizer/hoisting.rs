//! \[DESIGN\] The parts of a condition over two operands that can raise an
//! error, or are sort keys, made columns of the one operand each reads
//! (RULES.md D373, D15). A conjunct of a filter over a chain of products
//! that can raise and reads two operands stays whole over the chain, and
//! the chain is then joined in the order written. Rarely does all of it
//! raise: in `S.SALARY > SINGLE(M.CNT, M.AVG)` only the call does, and it
//! reads one operand. The filter's condition is evaluated on every row of
//! the product [X3.135-1986 §5.20 GR2b, §5.21 GR2], and each row of an
//! operand is in some row of it exactly when every other operand has a
//! row; a part that reads that operand alone, and gives the same value of
//! the same row each time (`hoisting/found.rs`), has the same value in each
//! of them. So it is computed once for each row of its operand, as a
//! column after the operand's, over the guard that
//! `push_raising_filter_into_product` uses, a row of no columns if every
//! other operand has a row: it raises exactly when the product would. A
//! part that cannot raise by the declared types (`logical/raises.rs`), as
//! a sort key, needs no guard. The conjunct reads the column in its place,
//! no longer raises, and moves as any. A projection over the chain keeps
//! its own columns.
//!
//! An ON condition is applied to every row of the product of its operands
//! [ISO/IEC 9075:1992 §7.5 GR1c]. A part of the operand that supplies
//! nulls is computed over a guard of the other, as of a product: the
//! join's rows are then all made of a row of each or of the preserved
//! operand alone, which has rows only when the join does. A part of a
//! preserved operand is taken only when it cannot raise, as a guard would
//! drop that operand's rows when the other has none. A part reading both
//! operands is left, and so is the condition.

mod found;
mod outer;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::views::substitute;
use super::{Rule, RuleContext, wrap};
use crate::catalog::Catalog;
use crate::logical::{
    Cond, Expr, LogicalPlan, Subquery, can_raise_typed, cond_can_raise_typed, map_cond_columns, map_cond_exprs, map_expr_columns,
    plan_column_type, plan_has_outer, split_and,
};
use found::{Finder, Found, Operands};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const HOIST_RAISING_EXPRESSIONS: &str = "hoist_raising_expressions";

/// Parts of conditions over two operands made columns of the operand
/// each reads, as the module says.
pub struct HoistRaisingExpressions;

impl Rule for HoistRaisingExpressions {
    fn name(&self) -> &'static str {
        HOIST_RAISING_EXPRESSIONS
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let catalog = &*ctx.db.catalog;
        if let LogicalPlan::OuterJoin { kind, cond, left, right } = plan {
            let join = outer::Join { kind: *kind, cond, left, right };
            return join.moved(catalog).or_else(|| join.hoisted(catalog));
        }
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        // A projection of columns gives one row of each of its input's,
        // so the condition is evaluated on the same rows below it.
        let projected = if let LogicalPlan::Project { exprs, input: chain } = &**input { Some((exprs, &**chain)) } else { None };
        let (exprs, chain) = match projected.filter(|(exprs, _)| exprs.iter().all(|e| matches!(e, Expr::Column(_)))) {
            Some((exprs, chain)) => (Some(exprs), chain),
            None => (None, &**input),
        };
        if !matches!(chain, LogicalPlan::Product { .. }) {
            return None;
        }
        let Some(exprs) = exprs else { return product_hoisted(cond, chain, catalog) };
        let below = map_cond_exprs(cond.clone(), &|e| substitute(e, exprs));
        Some(LogicalPlan::Project { exprs: exprs.clone(), input: Box::new(product_hoisted(&below, chain, catalog)?) })
    }
}

/// The filter of `cond` over the chain of products `chain`, hoisted, if a
/// conjunct over two operands has a part to take and none is left raising.
fn product_hoisted(cond: &Cond, chain: &LogicalPlan, catalog: &dyn Catalog) -> Option<LogicalPlan> {
    let mut plans = Vec::new();
    operands_of(chain, &mut plans);
    let operands = Operands::new(plans);
    let everywhere = |_: usize, _: &Expr| true;
    let mut finder = Finder::new(&operands, &everywhere);
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let conjuncts: Vec<Cond> = conjuncts.into_iter().map(|c| if operands.searched(&c) { finder.cond(c) } else { c }).collect();
    let mut hoisted = Hoisted::of(&operands, finder.found, catalog)?;
    let mut moved = std::mem::take(&mut hoisted.operands).into_iter();
    let product = replaced(chain, &mut moved)?;
    let conds: Vec<Cond> = conjuncts.into_iter().map(|c| hoisted.cond(c)).collect();
    let types = |c: usize| plan_column_type(&product, c, catalog);
    if conds.iter().any(|c| operands.read_by(c) > 1 && cond_can_raise_typed(c, &types)) {
        return None;
    }
    Some(LogicalPlan::Project { exprs: hoisted.kept(), input: Box::new(wrap(product, conds)) })
}

/// The operands of a join with the parts taken as their columns: where
/// each operand's columns began and begin now, the join's columns before,
/// of each part taken its operand and its place there, and the operands so
/// made.
struct Hoisted {
    was: Vec<usize>,
    starts: Vec<usize>,
    width: usize,
    parts: Vec<(usize, usize)>,
    operands: Vec<LogicalPlan>,
}

impl Hoisted {
    /// The operands of `operands` with the parts `found` taken, each over
    /// a guard of the others if one of its parts can raise; none if none
    /// was found, or if a guard is needed and an operand has no columns or
    /// an outer reference, which the guard's subquery would leave unfilled.
    fn of(operands: &Operands<'_>, found: Vec<Found>, catalog: &dyn Catalog) -> Option<Hoisted> {
        if found.is_empty() {
            return None;
        }
        let count = operands.plans.len();
        let (mut lists, mut guarded) = (vec![Vec::new(); count], vec![false; count]);
        let mut parts = Vec::with_capacity(found.len());
        for Found { at, expr } in found {
            let (plan, start) = (*operands.plans.get(at)?, operands.starts.get(at).copied()?);
            let own = map_expr_columns(expr, &|c| c.saturating_sub(start));
            *guarded.get_mut(at)? |= can_raise_typed(&own, &|c| plan_column_type(plan, c, catalog));
            let list = lists.get_mut(at)?;
            parts.push((at, plan.width().saturating_add(list.len())));
            list.push(own);
        }
        if guarded.contains(&true) && operands.plans.iter().any(|o| o.width() == 0 || plan_has_outer(o)) {
            return None;
        }
        let made = lists.into_iter().zip(guarded).enumerate().map(|(at, (list, guard))| hoisted_operand(operands, at, list, guard));
        let made: Vec<LogicalPlan> = made.collect::<Option<_>>()?;
        let starts = made.iter().scan(0_usize, |at, o| Some(std::mem::replace(at, at.saturating_add(o.width())))).collect();
        Some(Hoisted { was: operands.starts.clone(), starts, width: operands.width, parts, operands: made })
    }

    /// The join's column `column`, among the columns of the operands as
    /// they are: of the operands as they were, or past them, the
    /// `column - width`th part taken.
    fn column(&self, column: usize) -> usize {
        let (at, offset) = if let Some(k) = column.checked_sub(self.width) {
            self.parts.get(k).copied().unwrap_or_default()
        } else {
            let at = self.was.iter().rposition(|&start| start <= column).unwrap_or_default();
            (at, column.saturating_sub(self.was.get(at).copied().unwrap_or_default()))
        };
        self.starts.get(at).copied().unwrap_or_default().saturating_add(offset)
    }

    /// `c`, over the operands as they are.
    fn cond(&self, c: Cond) -> Cond {
        map_cond_columns(c, &|col| self.column(col))
    }

    /// The columns of the operands as they were, of the join as it is.
    fn kept(&self) -> Vec<Expr> {
        (0..self.width).map(|c| Expr::Column(self.column(c))).collect()
    }
}

/// The operand `at` of `operands`, its parts `parts` after its columns,
/// over a guard of the others if `guarded`.
fn hoisted_operand(operands: &Operands<'_>, at: usize, parts: Vec<Expr>, guarded: bool) -> Option<LogicalPlan> {
    let operand = (*operands.plans.get(at)?).clone();
    if parts.is_empty() {
        return Some(operand);
    }
    let width = operand.width();
    let input = if guarded {
        let others = operands.plans.iter().enumerate().filter(|(n, _)| *n != at);
        let exists = others.map(|(_, o)| Cond::Exists(Arc::new(Subquery::new((*o).clone(), Vec::new())))).collect();
        let guard = wrap(LogicalPlan::Values { rows: vec![Vec::new()], types: Vec::new() }, exists);
        LogicalPlan::Product { left: Box::new(operand), right: Box::new(guard) }
    } else {
        operand
    };
    let exprs = (0..width).map(Expr::Column).chain(parts).collect();
    Some(LogicalPlan::Project { exprs, input: Box::new(input) })
}

/// The operands of the chain of products `plan`, left to right.
fn operands_of<'p>(plan: &'p LogicalPlan, out: &mut Vec<&'p LogicalPlan>) {
    if let LogicalPlan::Product { left, right } = plan {
        operands_of(left, out);
        operands_of(right, out);
    } else {
        out.push(plan);
    }
}

/// The chain of products `plan`, its operands replaced, left to right, by
/// those `moved` gives.
fn replaced(plan: &LogicalPlan, moved: &mut impl Iterator<Item = LogicalPlan>) -> Option<LogicalPlan> {
    if let LogicalPlan::Product { left, right } = plan {
        let l = replaced(left, moved)?;
        let r = replaced(right, moved)?;
        return Some(LogicalPlan::Product { left: Box::new(l), right: Box::new(r) });
    }
    moved.next()
}
