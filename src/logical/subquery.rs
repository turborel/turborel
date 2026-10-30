//! [X3.135-1986 §5.24] A bound `<subquery>`: a logical plan of its own, run
//! for each row (or group) its predicate is applied to [X3.135-1986 §5.21
//! GR3, §5.23 GR4], with the values of the outer references it makes
//! [X3.135-1986 §5.7 SR4]. Split from logical.rs (RULES.md R-SIZE-2).
//!
//! An outer reference is bound as a slot, `Expr::Outer(i)`, and the
//! subquery keeps the expression, over the enclosing row, that fills it:
//! `outer[i]`. Before the plan runs for a row, every slot is replaced by its
//! value (`bound`), so the plan is an ordinary one, whose comparisons with
//! the values an index can answer. The outer expressions of a subquery
//! nested inside are in this plan's rows, so they are replaced too, and its
//! own plan is not.

use super::{AggCall, Cond, Expr, LogicalPlan, and_all, map_cond_exprs, split_and};
use crate::value::{CompOp, Value};

/// \[DESIGN\] A bound subquery.
#[derive(Debug, Clone)]
pub struct Subquery {
    /// Its plan: one column, its `<result specification>`; a row
    /// subquery's, of the degree of the row it is compared with or read
    /// from; or any under EXISTS or UNIQUE.
    pub plan: LogicalPlan,
    /// The expressions, over the rows it is applied to, whose values its
    /// outer reference slots take.
    pub outer: Vec<Expr>,
}

/// `e` with every outer reference slot replaced by its value in `values`;
/// a slot with none is left, and fails when evaluated.
fn bound_expr(e: Expr, values: &[Value]) -> Expr {
    if let Expr::Outer(i, name) = e {
        return values.get(i).cloned().map_or(Expr::Outer(i, name), Expr::Const);
    }
    e.map_children(&mut |child| bound_expr(child, values))
}

/// `plan` with every outer reference slot replaced by its value: in its
/// conditions, among them the outer expressions of the subqueries nested in
/// it, which are over its rows; in its select list; in its set
/// functions' arguments; and in the rows of its VALUES.
fn bound_plan(plan: LogicalPlan, values: &[Value]) -> LogicalPlan {
    let bound = |e: Expr| bound_expr(e, values);
    let node = match plan {
        LogicalPlan::Filter { cond, input } => LogicalPlan::Filter { cond: map_cond_exprs(cond, &bound), input },
        LogicalPlan::Project { exprs, input } => LogicalPlan::Project { exprs: exprs.into_iter().map(bound).collect(), input },
        LogicalPlan::Aggregate { groups, aggs, input } => {
            let aggs = aggs.into_iter().map(|a| AggCall { arg: a.arg.map(bound), ..a }).collect();
            LogicalPlan::Aggregate { groups, aggs, input }
        }
        LogicalPlan::OuterJoin { kind, cond, left, right } => {
            LogicalPlan::OuterJoin { kind, cond: map_cond_exprs(cond, &bound), left, right }
        }
        LogicalPlan::Values { rows, width } => {
            LogicalPlan::Values { rows: rows.into_iter().map(|r| r.into_iter().map(bound).collect()).collect(), width }
        }
        other @ (LogicalPlan::Scan { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Sort { .. }) => other,
    };
    node.map_inputs(|p| bound_plan(p, values))
}

/// Whether running `plan` can raise a run-time error: through a condition
/// or expression that can, or a set function, which can overflow.
fn plan_can_raise(plan: &LogicalPlan) -> bool {
    let own = match plan {
        LogicalPlan::Filter { cond, .. } | LogicalPlan::OuterJoin { cond, .. } => cond.can_raise(),
        LogicalPlan::Project { exprs, .. } => exprs.iter().any(Expr::can_raise),
        LogicalPlan::Aggregate { aggs, .. } => !aggs.is_empty(),
        LogicalPlan::Values { rows, .. } => rows.iter().flatten().any(Expr::can_raise),
        LogicalPlan::Scan { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Sort { .. } => false,
    };
    own || plan.inputs().into_iter().any(plan_can_raise)
}

impl Subquery {
    /// It, its outer expressions replaced by `f` of each.
    pub fn map_outer(&self, f: &mut dyn FnMut(Expr) -> Expr) -> std::sync::Arc<Subquery> {
        std::sync::Arc::new(Subquery { plan: self.plan.clone(), outer: self.outer.iter().cloned().map(f).collect() })
    }

    /// Whether running it can raise a run-time error, or computing its
    /// outer expressions can.
    pub fn can_raise(&self) -> bool {
        self.outer.iter().any(Expr::can_raise) || plan_can_raise(&self.plan)
    }

    /// Its plan, for a row whose outer expressions have `values`: every
    /// slot replaced by its value.
    pub fn bound(&self, values: &[Value]) -> LogicalPlan {
        bound_plan(self.plan.clone(), values)
    }
}

/// \[DESIGN\] A subquery run once for every row it is applied to (RULES.md
/// D35): its plan without the equalities of its columns with outer
/// references, which yields its rows followed by those columns' values, so
/// the rows for any outer values are those whose columns equal them.
#[derive(Debug, Clone)]
pub struct Decorrelated {
    /// The plan: the subquery's columns, then one for each key.
    pub plan: LogicalPlan,
    /// The subquery's columns, the first of each row.
    pub width: usize,
    /// For each key column, the outer reference slot it equals.
    pub slots: Vec<usize>,
}

/// Whether `e` has an outer reference slot, also among the outer
/// expressions of the subqueries nested in it.
fn expr_has_outer(e: &Expr) -> bool {
    let mut found = matches!(e, Expr::Outer(..));
    e.for_each_child(&mut |child| found = found || expr_has_outer(child));
    found
}

/// Whether `c` has an outer reference slot, also among the outer
/// expressions of the subqueries nested in it.
fn cond_has_outer(c: &Cond) -> bool {
    let mut found = false;
    c.for_each_expr(&mut |e| found = found || expr_has_outer(e));
    found
}

/// Whether `plan` has an outer reference slot anywhere.
pub fn plan_has_outer(plan: &LogicalPlan) -> bool {
    let own = match plan {
        LogicalPlan::Filter { cond, .. } | LogicalPlan::OuterJoin { cond, .. } => cond_has_outer(cond),
        LogicalPlan::Project { exprs, .. } => exprs.iter().any(expr_has_outer),
        LogicalPlan::Aggregate { aggs, .. } => aggs.iter().any(|a| a.arg.as_ref().is_some_and(expr_has_outer)),
        LogicalPlan::Values { rows, .. } => rows.iter().flatten().any(expr_has_outer),
        LogicalPlan::Scan { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Sort { .. } => false,
    };
    own || plan.inputs().into_iter().any(plan_has_outer)
}

/// "column = slot" or "slot = column": the column and the slot.
fn keyed(c: &Cond) -> Option<(usize, usize)> {
    let Cond::Compare(a, CompOp::Eq, b) = c else { return None };
    if let (Expr::Column(col), Expr::Outer(slot, _)) = (a, b) {
        return Some((*col, *slot));
    }
    if let (Expr::Outer(slot, _), Expr::Column(col)) = (a, b) {
        return Some((*col, *slot));
    }
    None
}

impl Subquery {
    /// It decorrelated, if it can be: its plan a projection, under DISTINCT
    /// or not, of a filter whose conjuncts that use outer references are
    /// all equalities of a column with one, and whose select list cannot
    /// raise an error, as it is then evaluated on the rows those
    /// equalities would have removed (RULES.md D15). The rest of the
    /// condition is evaluated on every row either way.
    pub fn decorrelated(&self) -> Option<Decorrelated> {
        let (distinct, projected) = if let LogicalPlan::Distinct { input } = &self.plan { (true, &**input) } else { (false, &self.plan) };
        let LogicalPlan::Project { exprs, input } = projected else { return None };
        let LogicalPlan::Filter { cond, input: rows } = &**input else { return None };
        if exprs.iter().any(|e| e.can_raise() || expr_has_outer(e)) || plan_has_outer(rows) {
            return None;
        }
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut keys, mut rest) = (Vec::new(), Vec::new());
        for c in conjuncts {
            if let Some(key) = keyed(&c) {
                keys.push(key);
            } else if cond_has_outer(&c) {
                return None;
            } else {
                rest.push(c);
            }
        }
        if keys.is_empty() {
            return None;
        }
        let filtered = match and_all(rest) {
            Some(remaining) => LogicalPlan::Filter { cond: remaining, input: rows.clone() },
            None => (**rows).clone(),
        };
        let columns = exprs.iter().cloned().chain(keys.iter().map(|&(col, _)| Expr::Column(col))).collect();
        let project = LogicalPlan::Project { exprs: columns, input: Box::new(filtered) };
        let plan = if distinct { LogicalPlan::Distinct { input: Box::new(project) } } else { project };
        Some(Decorrelated { plan, width: exprs.len(), slots: keys.into_iter().map(|(_, slot)| slot).collect() })
    }
}

// Unit tests: which subqueries are decorrelated, and into what (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use std::sync::Arc;

    use crate::logical::Quantifier;
    use crate::value::{ArithOp, Decimal};

    fn scan() -> LogicalPlan {
        LogicalPlan::Scan {
            table: QualifiedName { auth: "A".into(), ident: "U".into() },
            names: vec!["K".into(), "W".into()],
            needed: vec![0, 1],
        }
    }

    fn outer() -> Expr {
        Expr::Outer(0, "T.K".into())
    }

    fn one() -> Expr {
        Expr::Const(Value::Exact(Decimal::new(1, 0)))
    }

    /// The subquery selecting `select` from U where `cond`.
    fn subquery(select: Expr, cond: Cond) -> Subquery {
        let filter = LogicalPlan::Filter { cond, input: Box::new(scan()) };
        Subquery { plan: LogicalPlan::Project { exprs: vec![select], input: Box::new(filter) }, outer: vec![Expr::Column(0)] }
    }

    /// Verifies: [DESIGN] RULES.md D35: a subquery whose outer references
    /// are only in equalities with its columns is decorrelated: its select
    /// list, then the keyed columns, over the rest of its condition; one
    /// whose outer reference is elsewhere, in a disjunction or its select
    /// list, or whose select list can raise an error, is not.
    #[test]
    fn subqueries_equating_columns_with_outer_values_are_decorrelated() {
        let key = Cond::Compare(outer(), CompOp::Eq, Expr::Column(0));
        let rest = Cond::Compare(Expr::Column(1), CompOp::Gt, one());
        let d = subquery(Expr::Column(1), Cond::And(Box::new(key.clone()), Box::new(rest.clone()))).decorrelated().unwrap();
        assert_eq!((d.width, d.slots.clone()), (1, vec![0]));
        let LogicalPlan::Project { exprs, input } = &d.plan else { panic!("{:?}", d.plan) };
        assert!(matches!(exprs.as_slice(), [Expr::Column(1), Expr::Column(0)]), "{exprs:?}");
        assert!(matches!(&**input, LogicalPlan::Filter { cond: Cond::Compare(..), .. }), "{input:?}");
        let refused = [
            subquery(Expr::Column(1), Cond::Or(Box::new(key.clone()), Box::new(rest.clone()))),
            subquery(Expr::Column(1), Cond::Compare(outer(), CompOp::Lt, Expr::Column(0))),
            subquery(outer(), key.clone()),
            subquery(Expr::Arith(Box::new(Expr::Column(1)), ArithOp::Add, Box::new(one())), key),
            subquery(Expr::Column(1), rest),
        ];
        for s in refused {
            assert!(s.decorrelated().is_none(), "{:?}", s.plan);
        }
    }

    /// Verifies: [DESIGN] RULES.md D35: an outer reference anywhere but an
    /// equality with a column keeps a subquery from being decorrelated:
    /// in a null test, LIKE, NOT, a nested subquery's outer expressions,
    /// a quantified comparison, under arithmetic or a call, or in the plan
    /// below its filter; under DISTINCT, "column = slot" either way round
    /// is decorrelated, keeping DISTINCT.
    #[test]
    fn outer_references_elsewhere_keep_a_subquery_correlated() {
        let key = Cond::Compare(Expr::Column(0), CompOp::Eq, outer());
        let with = |c: Cond| Cond::And(Box::new(key.clone()), Box::new(c));
        let nested = Arc::new(Subquery { plan: scan(), outer: vec![outer()] });
        let neg = Expr::Neg(Box::new(outer()));
        let arith = Expr::Arith(Box::new(Expr::Column(1)), ArithOp::Add, Box::new(outer()));
        let conds = [
            Cond::IsNull(outer(), false),
            Cond::Like(Expr::Column(1), outer(), None),
            Cond::Like(Expr::Column(1), one(), Some(outer())),
            Cond::Not(Box::new(Cond::Compare(neg, CompOp::Lt, one()))),
            Cond::Exists(Arc::clone(&nested)),
            Cond::Quantified(vec![Expr::Column(1)], CompOp::Eq, Quantifier::Some, nested),
            Cond::Quantified(vec![outer()], CompOp::Eq, Quantifier::Some, Arc::new(Subquery { plan: scan(), outer: vec![] })),
            Cond::Compare(arith, CompOp::Gt, Expr::Const(Value::Null)),
        ];
        for c in conds {
            let s = subquery(Expr::Column(1), with(c));
            assert!(s.decorrelated().is_none(), "{:?}", s.plan);
        }
        let below = |plan: LogicalPlan| {
            let filter = LogicalPlan::Filter { cond: key.clone(), input: Box::new(plan) };
            Subquery { plan: LogicalPlan::Project { exprs: vec![Expr::Column(1)], input: Box::new(filter) }, outer: vec![] }
        };
        let inner_filter = LogicalPlan::Filter { cond: Cond::IsNull(outer(), false), input: Box::new(scan()) };
        let plans = [
            LogicalPlan::Product { left: Box::new(scan()), right: Box::new(inner_filter.clone()) },
            LogicalPlan::Project { exprs: vec![outer()], input: Box::new(scan()) },
            LogicalPlan::Aggregate { groups: vec![], aggs: vec![], input: Box::new(inner_filter.clone()) },
            LogicalPlan::Distinct { input: Box::new(inner_filter) },
        ];
        for plan in plans {
            assert!(below(plan.clone()).decorrelated().is_none(), "{plan:?}");
        }
        let reversed = Cond::Compare(outer(), CompOp::Eq, Expr::Column(0));
        let distinct =
            Subquery { plan: LogicalPlan::Distinct { input: Box::new(subquery(Expr::Column(1), reversed).plan) }, outer: vec![] };
        let d = distinct.decorrelated().unwrap();
        let LogicalPlan::Distinct { input } = &d.plan else { panic!("{:?}", d.plan) };
        let LogicalPlan::Project { input: rows, .. } = &**input else { panic!("{input:?}") };
        assert!(matches!(**rows, LogicalPlan::Scan { .. }), "no condition is left: {rows:?}");
    }
}
