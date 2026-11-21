//! \[DESIGN\] Joins that cannot change the rows removed (RULES.md D375;
//! Cheng et al., VLDB 1999). A projection that reads the columns of one
//! operand of a join alone is the projection of that operand when the join
//! gives each of its rows once.
//!
//! Of a LEFT join, the preserved operand's every row is in the result, once
//! for each row of the other it matches, or once with nulls if it matches
//! none [ISO/IEC 9075:1992 §7.5 GR1c, GR5]. It matches at most one when
//! the other operand is one table, filtered or not, and the `<join
//! condition>` equates a column of the preserved operand with each column
//! of a unique key of that table: no two of its rows are equal in all of
//! them [ISO/IEC 9075-2:2023 §11.7 GR2, §8.9]. Its key's columns must be
//! NOT NULL and the constraints not deferrable, as a deferred constraint
//! may be violated until it is checked [§4.25.3.1]; each compared with no
//! collation of its own, as the comparison of the condition is then the
//! one the key is unique by. The rest of the condition, and the table's
//! filter, are evaluated on no row once the join is gone, so they must not
//! raise (RULES.md D15) nor draw a value of a sequence generator. A RIGHT
//! join is the LEFT one of its operands swapped.
//!
//! Of an inner join on an enforced referential constraint
//! (`elimination/referential.rs`), each row of the referencing table
//! matches exactly one of the referenced table.

mod referential;
#[cfg(test)]
mod tests;

use super::views::substitute;
use super::{Rule, RuleContext};
use crate::catalog::{Catalog, ConstraintKind, TableDesc};
use crate::logical::{
    Cond, Expr, LogicalPlan, OuterJoin, Types, cond_can_raise_typed, expr_columns, map_expr_columns, plan_column_type, split_and,
};
use crate::value::CompOp;

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const ELIMINATE_JOINS: &str = "eliminate_joins";

/// Joins that cannot change the rows removed, as the module says.
pub struct EliminateJoins;

impl Rule for EliminateJoins {
    fn name(&self) -> &'static str {
        ELIMINATE_JOINS
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Project { exprs, input } = plan else { return None };
        let catalog = &*ctx.db.catalog;
        let (exprs, input) = peeled(exprs.clone(), input);
        let exprs = exprs.as_slice();
        match input {
            LogicalPlan::OuterJoin { kind, cond, left, right } => outer_eliminated(exprs, (*kind, cond), (left, right), catalog),
            LogicalPlan::Filter { cond, input: chain } => referential::eliminated(exprs, cond, chain, catalog),
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::SemiJoin { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Project { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::Recognize(..)
            | LogicalPlan::Polymorphic(..) => None,
        }
    }
}

/// \[DESIGN\] RULES.md R-CF-6: the most projections of columns seen
/// through below a projection.
const MAX_PEELED: usize = 64;

/// The projection `exprs` of `input` as a projection of what is below the
/// projections of columns `input` begins with: each gives a row of each
/// of its input's, so the projection's expressions are of those columns.
fn peeled(mut exprs: Vec<Expr>, mut input: &LogicalPlan) -> (Vec<Expr>, &LogicalPlan) {
    for _ in 0..MAX_PEELED {
        let LogicalPlan::Project { exprs: inner, input: below } = input else { break };
        if !inner.iter().all(|e| matches!(e, Expr::Column(_))) {
            break;
        }
        exprs = exprs.into_iter().map(|e| substitute(e, inner)).collect();
        input = below;
    }
    (exprs, input)
}

/// The projection `exprs` of the outer join `kind` on `cond` of `left` and
/// `right` as the projection of its preserved operand, if the join gives
/// each of that operand's rows once and `exprs` reads its columns alone.
fn outer_eliminated(
    exprs: &[Expr],
    (kind, cond): (OuterJoin, &Cond),
    (left, right): (&LogicalPlan, &LogicalPlan),
    catalog: &dyn Catalog,
) -> Option<LogicalPlan> {
    let width = left.width();
    // The operand kept, the one dropped and its first column, and how far
    // the kept one's columns move.
    let (kept, dropped, start, shift) = match kind {
        OuterJoin::Left => (left, right, width, 0),
        OuterJoin::Right => (right, left, 0, width),
        OuterJoin::Full => return None,
    };
    let mut read = Vec::new();
    for e in exprs {
        expr_columns(e, &mut read);
    }
    if !read.iter().all(|&c| (c < width) == (shift == 0)) {
        return None;
    }
    let (table, filter) = table_of(dropped)?;
    let desc = catalog.table(table)?;
    let join = LogicalPlan::OuterJoin { kind, cond: cond.clone(), left: Box::new(left.clone()), right: Box::new(right.clone()) };
    let unraised = quiet(cond, &|c| plan_column_type(&join, c, catalog));
    if !desc.periods.is_empty() || !unraised || !filter.is_none_or(|f| quiet(f, &|c| plan_column_type(dropped, c, catalog))) {
        return None;
    }
    let equated = equated_columns(cond, start, dropped.width());
    unique_key_covered(desc, &equated).then(|| {
        let exprs = exprs.iter().map(|e| map_expr_columns(e.clone(), &|c| c.saturating_sub(shift))).collect();
        LogicalPlan::Project { exprs, input: Box::new(kept.clone()) }
    })
}

/// The table `plan` reads and its filter, if it is a scan of one, which
/// gives no row ids, or a filter of such a scan.
fn table_of(plan: &LogicalPlan) -> Option<(&crate::catalog::QualifiedName, Option<&Cond>)> {
    match plan {
        LogicalPlan::Scan { table, located: false, .. } => Some((table, None)),
        LogicalPlan::Filter { cond, input } => {
            let LogicalPlan::Scan { table, located: false, .. } = &**input else { return None };
            Some((table, Some(cond)))
        }
        LogicalPlan::Scan { located: true, .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..) => None,
    }
}

/// The columns of the operand of `width` columns beginning at `start` that
/// a conjunct of `cond` equates with a value of the other operand alone,
/// a column or an expression of its columns, of no subquery, as that
/// operand's own: each row of the other gives one value, which at most one
/// row of a unique key equals.
fn equated_columns(cond: &Cond, start: usize, width: usize) -> Vec<usize> {
    let own = |c: usize| c.checked_sub(start).filter(|at| *at < width);
    let other = |e: &Expr| {
        let mut columns = Vec::new();
        expr_columns(e, &mut columns);
        columns.iter().all(|&c| own(c).is_none())
    };
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    conjuncts
        .iter()
        .filter(|c| c.subqueries().is_empty())
        .filter_map(|c| {
            let Cond::Compare(a, CompOp::Eq, b) = c else { return None };
            let column = |e: &Expr| if let Expr::Column(k) = e { own(*k) } else { None };
            column(a).filter(|_| other(b)).or_else(|| column(b).filter(|_| other(a)))
        })
        .collect()
}

/// Whether `columns` hold every column of a unique key of `desc` whose
/// constraint is not deferrable and of no period, each NOT NULL by a
/// constraint not deferrable and of no collation of its own.
fn unique_key_covered(desc: &TableDesc, columns: &[usize]) -> bool {
    desc.constraints.iter().any(|c| {
        let ConstraintKind::Unique { columns: key, period: None, .. } = &c.kind else { return false };
        !c.attributes.deferrable && key.iter().all(|k| columns.contains(k) && never_null(desc, *k) && uncollated(desc, *k))
    })
}

/// Whether a unique key of `desc` whose constraint is not deferrable and
/// of no period has its columns among `columns`: no two rows are equal in
/// all of `columns`, nulls aside.
pub(super) fn unique_within(desc: &TableDesc, columns: &[usize]) -> bool {
    desc.constraints.iter().any(|c| {
        let ConstraintKind::Unique { columns: key, period: None, .. } = &c.kind else { return false };
        !c.attributes.deferrable && key.iter().all(|k| columns.contains(k))
    })
}

/// Whether column `at` of `desc` holds no null whenever a statement reads
/// it: NOT NULL, or of the PRIMARY KEY, by a constraint not deferrable.
pub(super) fn never_null(desc: &TableDesc, at: usize) -> bool {
    desc.constraints.iter().any(|c| {
        let constrained = match &c.kind {
            ConstraintKind::NotNull(column) => *column == at,
            ConstraintKind::Unique { columns, primary: true, .. } => columns.contains(&at),
            ConstraintKind::Unique { primary: false, .. }
            | ConstraintKind::ForeignKey(_)
            | ConstraintKind::Check { .. }
            | ConstraintKind::Connection(_) => false,
        };
        constrained && !c.attributes.deferrable
    })
}

/// Whether column `at` of `desc` compares with no collation of its own.
pub(super) fn uncollated(desc: &TableDesc, at: usize) -> bool {
    desc.columns.get(at).is_some_and(|c| c.collation.is_none())
}

/// Whether `c`, its columns of the types `types` gives, can neither raise
/// an error nor draw a value of a sequence generator: so evaluating it on
/// no row has the same effect as on any.
pub(super) fn quiet(c: &Cond, types: Types<'_>) -> bool {
    let mut draws = false;
    c.for_each_expr(&mut |e| draws = draws || e.draws());
    !draws && !cond_can_raise_typed(c, types)
}
