//! \[DESIGN\] (RULES.md D660) How each set function of a grouping over a
//! chain is computed when one relation, X, is grouped first, as Yan and
//! Larson, "Eager Aggregation and Lazy Aggregation" (VLDB 1995), sort
//! them:
//!
//! * pushed, of X's columns alone: its part made of each early group's
//!   rows, the parts merged above the join (`functions/partial.rs`). Each
//!   row of the join stands for an early group's rows of X with one row
//!   of the rest, so the parts merged are the function of the join's
//!   rows. Only of COUNT, SUM, AVG, MAX and MIN, of a state merged
//!   whatever its order (never of approximate numbers or DECFLOAT, whose
//!   sum depends on it, `functions/sum.rs`); of an argument and a filter
//!   that cannot raise (RULES.md D15) and an argument never null, since
//!   the early grouping takes rows the join drops, and a null of one
//!   would raise the warning of a null eliminated the join's rows never
//!   raise [ISO/IEC 9075:1992 §6.5 GR1b];
//! * plain, duplicate blind (Yan and Larson's class D): MAX, MIN or a
//!   DISTINCT function, computed above the join of the values it reads,
//!   which the early grouping passes up as grouping columns;
//! * scaled, duplicate sensitive (class C): COUNT, or SUM or AVG of exact
//!   numbers, computed above the join of values each of the rows its
//!   early group counts (`functions/partial/scaled.rs`).
//!
//! X's grouping columns are those the joins read, those the grouping
//! groups by, and those a function computed above reads. Each is of a
//! type whose equal values are alike, as exact numbers, dates and times
//! of no zone, and truth values, so a group's first value stands for
//! every row of it in what is computed above. Nothing is grouped first of
//! an aggregation of an inverse distribution function, or of an
//! expression that is no column of X's to move, or of a condition reading
//! X and another relation that can raise, as it would be judged of fewer
//! pairs (RULES.md D15).

use super::chain::Chain;
use crate::catalog::{Catalog, ConstraintKind, TableDesc};
use crate::functions::{ColumnarKind, Split, split_of};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, Types, can_raise_typed, cond_can_raise_typed, cond_columns, expr_columns};
use crate::value::{DataType, Value};

/// \[DESIGN\] How deep an expression or a relation is looked into for
/// nulls (RULES.md R-CF-6).
const MAX_DEPTH: usize = 64;

/// How a set function is computed (as the module says).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Pushed,
    Plain,
    Scaled(ColumnarKind),
}

/// The functions' roles, X's grouping columns of the chain's, in order,
/// and its conjuncts of X alone and of the rest.
#[derive(Debug, Clone)]
pub(super) struct Shape {
    pub(super) roles: Vec<Role>,
    pub(super) keys: Vec<usize>,
    pub(super) own: Vec<Cond>,
    pub(super) rest: Vec<Cond>,
}

/// What a classification reads: the chain, X, the grouping, the chain's
/// column types, and the catalog.
pub(super) struct Cx<'a> {
    pub(super) chain: &'a Chain,
    pub(super) x: usize,
    pub(super) groups: &'a [usize],
    pub(super) aggs: &'a [AggCall],
    pub(super) types: Types<'a>,
    pub(super) catalog: &'a dyn Catalog,
}

impl Cx<'_> {
    /// Whether the chain's column `c` is X's.
    fn in_x(&self, c: usize) -> bool {
        self.chain.leaves.get(self.x).is_some_and(|l| l.holds(c))
    }

    /// The shape of grouping X first, if it may be (as the module says).
    pub(super) fn shape(&self) -> Option<Shape> {
        let leaf = self.chain.leaves.get(self.x)?;
        if matches!(leaf.plan, LogicalPlan::Aggregate { .. }) {
            return None;
        }
        let (mut keys, mut own, mut rest) = (Vec::new(), Vec::new(), Vec::new());
        for c in &self.chain.conds {
            let mut reads = Vec::new();
            cond_columns(c, &mut reads);
            let of_x: Vec<usize> = reads.iter().copied().filter(|&r| self.in_x(r)).collect();
            if of_x.is_empty() {
                rest.push(c.clone());
                continue;
            }
            if !cond_movable(c, 0) || cond_can_raise_typed(c, self.types) {
                return None;
            }
            if of_x.len() == reads.len() {
                own.push(c.clone());
            } else {
                keys.extend(of_x);
                rest.push(c.clone());
            }
        }
        keys.extend(self.groups.iter().copied().filter(|&g| self.in_x(g)));
        let mut roles = Vec::with_capacity(self.aggs.len());
        for a in self.aggs {
            roles.push(self.role(a, &mut keys)?);
        }
        keys.sort_unstable();
        keys.dedup();
        let alike = keys.iter().all(|&k| (self.types)(k).as_ref().is_some_and(alike));
        (alike && !keys.is_empty()).then_some(Shape { roles, keys, own, rest })
    }

    /// The role of `a`, the columns of X it needs above added to `keys`.
    fn role(&self, a: &AggCall, keys: &mut Vec<usize>) -> Option<Role> {
        let exprs = a.exprs();
        if a.distribution.is_some() || !exprs.iter().all(|e| movable(e, 0)) {
            return None;
        }
        let mut reads = Vec::new();
        for e in &exprs {
            expr_columns(e, &mut reads);
        }
        let of_x = reads.iter().filter(|&&r| self.in_x(r)).count();
        if of_x == reads.len() && self.pushed(a) {
            return Some(Role::Pushed);
        }
        if of_x > 0 && of_x < reads.len() {
            return None;
        }
        keys.extend(reads.into_iter().filter(|&r| self.in_x(r)));
        above(a)
    }

    /// Whether `a`, of X's columns alone, may be computed in parts of the
    /// early groups (as the module says).
    fn pushed(&self, a: &AggCall) -> bool {
        let Some(kind) = a.func.0.columnar().map(|c| c.0) else { return false };
        let blind = matches!(kind, ColumnarKind::Min | ColumnarKind::Max);
        let summed = matches!(kind, ColumnarKind::Sum | ColumnarKind::Avg);
        if (a.distinct && !blind) || (summed && !exact(&a.result)) || split_of(&a.func, &a.result) != Some(Split::State) {
            return false;
        }
        let filtered = a.filter.as_ref().is_none_or(|f| !cond_can_raise_typed(f, self.types));
        let taken = a.arg.as_ref().is_none_or(|e| !can_raise_typed(e, self.types) && self.never_null(e, 0));
        filtered && taken
    }

    /// Whether `e`, over the chain's columns, is never null: of columns of
    /// X its table holds not null, and constants not null, by arithmetic.
    fn never_null(&self, e: &Expr, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let deeper = depth.saturating_add(1);
        match e {
            Expr::Column(c) => self
                .chain
                .leaves
                .get(self.x)
                .is_some_and(|l| c.checked_sub(l.start).is_some_and(|at| column_never_null(&l.plan, at, self.catalog, 0))),
            Expr::Const(v) => !matches!(v, Value::Null),
            Expr::Neg(x) | Expr::Cast(x, _) => self.never_null(x, deeper),
            Expr::Arith(l, _, r) => self.never_null(l, deeper) && self.never_null(r, deeper),
            Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::NextValue(_)
            | Expr::Invoked(_)
            | Expr::Warned(..) => false,
        }
    }
}

/// How `a`, not pushed, is computed above the join, if it may be: plain
/// if duplicates do not change it, scaled if it counts or sums exact
/// numbers.
fn above(a: &AggCall) -> Option<Role> {
    let kind = a.func.0.columnar()?.0;
    let summed = matches!(kind, ColumnarKind::Sum | ColumnarKind::Avg);
    if summed && !exact(&a.result) {
        return None;
    }
    if a.distinct || matches!(kind, ColumnarKind::Min | ColumnarKind::Max) {
        return Some(Role::Plain);
    }
    Some(Role::Scaled(kind))
}

/// Whether `t` is of exact numbers.
fn exact(t: &DataType) -> bool {
    matches!(t, DataType::Exact { .. })
}

/// Whether equal values of type `t` are alike: exact numbers, truth
/// values, dates, and times and timestamps of no time zone.
fn alike(t: &DataType) -> bool {
    match t {
        DataType::Exact { .. } | DataType::Boolean | DataType::Date => true,
        DataType::Time { zoned, .. } | DataType::Timestamp { zoned, .. } => !zoned,
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Xml(_)
        | DataType::Extension(_) => false,
    }
}

/// Whether column `at` of `plan`'s rows is a column its table holds not
/// null, by a constraint not deferred, under filters alone.
fn column_never_null(plan: &LogicalPlan, at: usize, catalog: &dyn Catalog, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    if let LogicalPlan::Filter { input, .. } = plan {
        return column_never_null(input, at, catalog, depth.saturating_add(1));
    }
    let LogicalPlan::Scan { table, .. } = plan else { return false };
    catalog.table(table).is_some_and(|desc| not_null(desc, at))
}

/// Whether `desc` holds column `at` not null, by a NOT NULL or a PRIMARY
/// KEY not deferrable.
fn not_null(desc: &TableDesc, at: usize) -> bool {
    desc.constraints.iter().any(|c| {
        let held = match &c.kind {
            ConstraintKind::NotNull(column) => *column == at,
            ConstraintKind::Unique { columns, primary, .. } => *primary && columns.contains(&at),
            ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } | ConstraintKind::Connection(_) => false,
        };
        held && !c.attributes.deferrable
    })
}

/// Whether `e` may be computed in another plan than its own: no
/// subquery, no value of a sequence generator, no routine invoked.
fn movable(e: &Expr, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let mut moves = !matches!(e, Expr::Subquery(..) | Expr::NextValue(_) | Expr::Invoked(_));
    e.for_each_child(&mut |c| moves = moves && movable(c, depth.saturating_add(1)));
    moves
}

/// Whether `c` may be judged in another plan than its own: no subquery
/// and no expression that may not move.
fn cond_movable(c: &Cond, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let deeper = depth.saturating_add(1);
    match c {
        Cond::Const(_) => true,
        Cond::Or(a, b) | Cond::And(a, b) => cond_movable(a, deeper) && cond_movable(b, deeper),
        Cond::Not(a) | Cond::Is(a, ..) => cond_movable(a, deeper),
        Cond::Compare(..) | Cond::IsNull(..) | Cond::Distinct(..) | Cond::Like(..) | Cond::Value(_) | Cond::In(..) => {
            let mut moves = true;
            c.for_each_expr(&mut |e| moves = moves && movable(e, deeper));
            moves
        }
        Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(..) | Cond::Match { .. } => false,
    }
}

// Unit tests: the roles and the shapes refused (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests;
