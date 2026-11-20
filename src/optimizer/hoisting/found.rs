//! \[DESIGN\] The subexpressions of a condition that `hoist_raising_expressions`
//! makes columns of an operand (RULES.md D373): each maximal one that reads
//! the columns of one operand alone and can raise an error, or is a sort
//! key, and gives the same value of the same row each time.
//!
//! The search goes down only through what is evaluated whenever the
//! condition is: the operands of comparisons, AND, OR and NOT
//! [X3.135-1986 §5.18 GR2], arithmetic, the arguments of a function, a
//! CAST, a sort key and a truth value. It does not go into a CASE, whose
//! results and later conditions are evaluated only when the ones before
//! select them [ISO/IEC 9075:1992 §6.9 GR1, GR2], nor into a subquery or an
//! invoked routine. An expression that draws a value of a sequence
//! generator, invokes a routine, is a subquery, calls a function that is
//! not deterministic, or raises a warning is never taken: computed once
//! for each row of its operand, it would give its value fewer times than
//! for each row of the product.

use crate::catalog::Catalog;
use crate::logical::{
    Cond, Expr, LogicalPlan, MAX_JUDGED_DEPTH, can_raise_typed, cond_columns, expr_columns, map_expr_columns, plan_column_type,
};

/// The operands of a join and where their columns begin among its own.
pub(super) struct Operands<'p> {
    /// The operands, left to right.
    pub(super) plans: Vec<&'p LogicalPlan>,
    /// Of each, its first column among the join's.
    pub(super) starts: Vec<usize>,
    /// The join's columns.
    pub(super) width: usize,
}

impl<'p> Operands<'p> {
    /// The operands `plans`, side by side.
    pub(super) fn new(plans: Vec<&'p LogicalPlan>) -> Operands<'p> {
        let mut starts = Vec::with_capacity(plans.len());
        let mut width = 0_usize;
        for p in &plans {
            starts.push(width);
            width = width.saturating_add(p.width());
        }
        Operands { plans, starts, width }
    }

    /// The operand whose column the join's column `column` is.
    pub(super) fn of(&self, column: usize) -> Option<usize> {
        let mut at = None;
        for (n, (start, plan)) in self.starts.iter().zip(&self.plans).enumerate() {
            if column >= *start && column < start.saturating_add(plan.width()) {
                at = Some(n);
            }
        }
        at
    }

    /// The one operand whose columns `columns` are, if they are of one.
    pub(super) fn one(&self, columns: &[usize]) -> Option<usize> {
        let first = self.of(*columns.first()?)?;
        columns.iter().all(|&c| self.of(c) == Some(first)).then_some(first)
    }

    /// Whether the search looks into the conjunct `c`: it reads two
    /// operands or more, and can raise or compares sort keys.
    pub(super) fn searched(&self, c: &Cond) -> bool {
        let mut keyed = false;
        c.for_each_expr(&mut |e| keyed = keyed || has_sort_key(e, 0));
        (keyed || c.can_raise()) && self.read_by(c) > 1
    }

    /// Whether the part `e` of operand `at` is worth a column: a sort key,
    /// or one that can raise by its columns' declared types. A part that
    /// cannot raise by them is left where it is, as the enumeration judges
    /// it so too (`physical/enumerate/extract.rs`).
    pub(super) fn needed(&self, at: usize, e: &Expr, catalog: &dyn Catalog) -> bool {
        let (Some(plan), Some(start)) = (self.plans.get(at), self.starts.get(at)) else { return false };
        let own = map_expr_columns(e.clone(), &|c| c.saturating_sub(*start));
        matches!(e, Expr::SortKey(..)) || can_raise_typed(&own, &|c| plan_column_type(plan, c, catalog))
    }

    /// How many operands the columns of `c` are of.
    pub(super) fn read_by(&self, c: &Cond) -> usize {
        let mut columns = Vec::new();
        cond_columns(c, &mut columns);
        let mut of: Vec<usize> = columns.iter().filter_map(|&col| self.of(col)).collect();
        of.sort_unstable();
        of.dedup();
        of.len()
    }
}

/// A subexpression taken: its operand, and it, over the join's columns.
pub(super) struct Found {
    pub(super) at: usize,
    pub(super) expr: Expr,
}

/// Which subexpressions of an operand may be taken.
pub(super) type Allowed<'a> = &'a dyn Fn(usize, &Expr) -> bool;

/// The search: the operands, which subexpressions may be taken, and those
/// taken so far, the `k`th read where it was as the join's column
/// `width + k`.
pub(super) struct Finder<'a, 'p> {
    operands: &'a Operands<'p>,
    allowed: Allowed<'a>,
    pub(super) found: Vec<Found>,
    depth: usize,
}

impl<'a, 'p> Finder<'a, 'p> {
    /// A search over `operands`, taking what `allowed` lets it.
    pub(super) fn new(operands: &'a Operands<'p>, allowed: Allowed<'a>) -> Finder<'a, 'p> {
        Finder { operands, allowed, found: Vec::new(), depth: 0 }
    }

    /// `c`, each subexpression taken replaced by its column.
    pub(super) fn cond(&mut self, c: Cond) -> Cond {
        c.map_exprs(&mut |e| self.expr(e))
    }

    /// `e`, taken whole if it may be, else each of its parts the search
    /// goes into.
    fn expr(&mut self, e: Expr) -> Expr {
        if let Some(at) = self.takes(&e) {
            let column = self.operands.width.saturating_add(self.found.len());
            self.found.push(Found { at, expr: e });
            return Expr::Column(column);
        }
        if !descends(&e) || self.depth >= MAX_JUDGED_DEPTH {
            return e;
        }
        self.depth = self.depth.saturating_add(1);
        let mapped = e.map_children(&mut |child| self.expr(child));
        self.depth = self.depth.saturating_sub(1);
        mapped
    }

    /// The operand `e` is taken as a column of, if it is taken.
    fn takes(&self, e: &Expr) -> Option<usize> {
        if matches!(e, Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) | Expr::Dynamic(_)) {
            return None;
        }
        let worth = e.can_raise() || matches!(e, Expr::SortKey(..));
        if !worth || !pure(e, 0) {
            return None;
        }
        let mut columns = Vec::new();
        expr_columns(e, &mut columns);
        let at = self.operands.one(&columns)?;
        (self.allowed)(at, e).then_some(at)
    }
}

/// Whether `e` is or has a sort key.
fn has_sort_key(e: &Expr, depth: usize) -> bool {
    let mut keyed = matches!(e, Expr::SortKey(..));
    if depth <= MAX_JUDGED_DEPTH {
        e.for_each_child(&mut |child| keyed = keyed || has_sort_key(child, depth.saturating_add(1)));
    }
    keyed
}

/// Whether the search goes into `e`'s parts: those evaluated whenever it is.
fn descends(e: &Expr) -> bool {
    match e {
        Expr::Neg(_) | Expr::Arith(..) | Expr::Call(..) | Expr::Cast(..) | Expr::Domain(..) | Expr::SortKey(..) | Expr::Truth(_) => true,
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Subquery(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_)
        | Expr::Warned(..) => false,
    }
}

/// Whether `e` gives the same value of the same row each time, and raises
/// no warning: no sequence generator, routine, subquery or function that
/// is not deterministic is in it.
pub(super) fn pure(e: &Expr, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let own = match e {
        Expr::Call(f, _) => f.0.deterministic(),
        Expr::NextValue(_) | Expr::Invoked(_) | Expr::Subquery(..) | Expr::Warned(..) => false,
        Expr::Truth(c) => c.subqueries().is_empty(),
        Expr::Case(whens, _) => whens.iter().all(|(c, _)| c.subqueries().is_empty()),
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..) => true,
    };
    let mut all = own;
    e.for_each_child(&mut |child| all = all && pure(child, depth.saturating_add(1)));
    all
}
