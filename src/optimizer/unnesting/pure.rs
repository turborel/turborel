//! \[DESIGN\] What unnesting asks of the plans and expressions it moves
//! (RULES.md D318). Each must give the same rows every time it is
//! evaluated, so a domain of bindings made of one evaluation of the outer
//! rows matches another. Unnesting also asks whether one can raise an
//! error, so a part evaluated for bindings no row has is never one that
//! could.

use crate::functions::{Registry, Standard};
use crate::logical::raises::{Judge, can_raise_judged, cond_can_raise_judged, total_fits};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, Search, Subquery, map_plan_exprs, plan_column_type};
use crate::optimizer::RuleContext;
use crate::optimizer::folding::fold_expr;
use crate::parser::MAX_NESTING_DEPTH;
use crate::value::Value;

/// Whether `e`'s own operators, not its subqueries' plans, are all
/// deterministic and change nothing, so there is no NEXT VALUE FOR,
/// which draws [ISO/IEC 9075-2:2023 §6.14 GR1]; no function call that is
/// possibly non-deterministic [§4.35.1]; no invocation that possibly
/// modifies SQL-data, is possibly non-deterministic or is written in
/// Rust. A worklist, so no depth bounds it.
fn own_pure(e: &Expr) -> bool {
    let mut work = vec![e];
    while let Some(next) = work.pop() {
        let pure = match next {
            Expr::NextValue(_) => false,
            Expr::Call(f, _) => f.0.deterministic(),
            Expr::Invoked(i) => i.deterministic && !i.modifies && i.rust.is_none(),
            Expr::Column(_)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::Warned(..) => true,
        };
        if !pure {
            return false;
        }
        next.for_each_child(&mut |child| work.push(child));
    }
    true
}

/// Whether `e` is deterministic and changes nothing, its subqueries too.
pub(crate) fn expr_pure(e: &Expr) -> bool {
    own_pure(e) && e.all_subqueries().into_iter().all(|s| subquery_pure(s, 0))
}

/// Whether `s` is deterministic and changes nothing, both its outer
/// expressions and its plan, `depth` subqueries deep.
fn subquery_pure(s: &Subquery, depth: usize) -> bool {
    s.outer.iter().all(own_pure) && plan_pure_at(&s.plan, depth.saturating_add(1))
}

/// Whether `plan` gives the same rows each time it runs and changes
/// nothing, so none of its expressions is otherwise, nor its subqueries,
/// and it has no polymorphic table function, whose component procedures
/// may do anything [ISO/IEC 9075-2:2023 §9.25], nor row pattern
/// recognition, which unnesting leaves alone. Past `MAX_NESTING_DEPTH`
/// subqueries deep it is taken as not (RULES.md R-CF-6).
pub(super) fn plan_pure(plan: &LogicalPlan) -> bool {
    plan_pure_at(plan, 0)
}

/// `plan_pure`, of a plan `depth` subqueries deep.
fn plan_pure_at(plan: &LogicalPlan, depth: usize) -> bool {
    if depth > MAX_NESTING_DEPTH {
        return false;
    }
    let mut pure = true;
    // The expressions of every node, read through the one walk of them.
    drop(map_plan_exprs(plan.clone(), &mut |e| {
        pure = pure && own_pure(&e);
        e
    }));
    pure && nodes_pure(plan, depth)
}

/// Whether no node of `plan` is one `plan_pure` refuses, and each
/// node's subqueries are pure.
fn nodes_pure(plan: &LogicalPlan, depth: usize) -> bool {
    let own = match plan {
        LogicalPlan::Polymorphic(_) | LogicalPlan::Recognize(_) => false,
        LogicalPlan::Scan { located, .. } => !located,
        LogicalPlan::Filter { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_) => true,
    };
    own && plan.subqueries().into_iter().all(|s| subquery_pure(s, depth)) && plan.inputs().all(|p| nodes_pure(p, depth))
}

/// Whether running `plan` can raise an error, as `Subquery::can_raise`
/// judges it, but each condition, expression and set function by the
/// declared types of the columns it reads (`logical/raises.rs`, RULES.md
/// D1060). A set function the registry knows as COUNT, MAX or MIN, of an
/// argument and filter that cannot, raises none, as no count reaches its
/// type's limit and an extreme only compares; nor SUM or AVG of exact
/// values whose sum stays within the digits an exact number has
/// (`total_fits`); nor one that raises nothing of one row, of an input of
/// one row at most; nor a collection of an UNNEST that is a subquery of
/// one row at most raising nothing, as a lateral derived table's is
/// (RULES.md D779). A scalar subquery of a condition or expression raises
/// what its plan raises, so judged, if its shape gives one row at most,
/// and else its cardinality error too [ISO/IEC 9075:1992 §7.11 GR1].
pub(super) fn plan_raises(plan: &LogicalPlan, ctx: &RuleContext<'_>) -> bool {
    judged(plan, ctx, 0)
}

/// `plan_raises`, of a plan `depth` subqueries deep, which no rule has
/// walked yet: its expressions of constants alone are computed first, as
/// `constant_folding` computes them once the plan is unnested, so one
/// that computes, as a date and an interval added, is seen to raise
/// nothing. Past `MAX_NESTING_DEPTH` the plan is taken as raising
/// (RULES.md R-CF-6).
fn judged(plan: &LogicalPlan, ctx: &RuleContext<'_>, depth: usize) -> bool {
    if depth > MAX_NESTING_DEPTH {
        return true;
    }
    let folded = map_plan_exprs(plan.clone(), &mut |e| fold_expr(&e, &ctx.db.memory).unwrap_or(e));
    raises_at(&folded, ctx, depth)
}

/// `judged`, of a plan whose constants are computed.
fn raises_at(plan: &LogicalPlan, ctx: &RuleContext<'_>, depth: usize) -> bool {
    let catalog = &*ctx.db.catalog;
    let deeper = depth.saturating_add(1);
    let subquery = |s: &Subquery, one: bool| (one && !s.plan.one_row_at_most()) || judged(&s.plan, ctx, deeper);
    let over = |input: &LogicalPlan, asks: &dyn Fn(&Judge<'_>) -> bool| {
        asks(&Judge { types: &|c| plan_column_type(input, c, catalog), subquery: &subquery })
    };
    let own = match plan {
        LogicalPlan::Filter { cond, input } => over(input, &|j| cond_raises(cond, j)),
        LogicalPlan::OuterJoin { cond, .. } => over(plan, &|j| cond_raises(cond, j)),
        LogicalPlan::SemiJoin { cond, left, right, .. } => {
            let product = LogicalPlan::Product { left: left.clone(), right: right.clone() };
            cond.can_raise() && over(&product, &|j| cond_raises(cond, j))
        }
        LogicalPlan::MarkJoin(m) => m.conds().any(Cond::can_raise),
        LogicalPlan::Project { exprs, input } => over(input, &|j| exprs.iter().any(|e| expr_raises(e, j))),
        LogicalPlan::Aggregate { aggs, groups, input, .. } => {
            let once = groups.is_empty() && input.one_row_at_most();
            let functions = &ctx.db.functions;
            over(input, &|j| aggs.iter().any(|a| agg_raises(a, functions, j) && !(once && raise_free_once(a, j))))
        }
        LogicalPlan::Values { rows, .. } => rows.iter().flatten().any(Expr::can_raise),
        LogicalPlan::Unnest(u) => {
            let special = u.json.is_some() || u.graph.is_some();
            special || u.collections.iter().any(|c| collection_raises(c, ctx, deeper)) || u.outer.as_ref().is_some_and(Cond::can_raise)
        }
        LogicalPlan::Window(w) => w.can_raise(),
        LogicalPlan::Limit { limit, .. } => {
            let fetch = limit.fetch.as_ref().is_some_and(|f| !counted(&f.count) || f.percent || !matches!(f.search, Search::Unstated));
            fetch || limit.offset.as_ref().is_some_and(|o| !counted(o))
        }
        LogicalPlan::Recognize(_) | LogicalPlan::Recursive { .. } | LogicalPlan::Polymorphic(_) => true,
        LogicalPlan::Scan { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. } => false,
    };
    own || plan.inputs().any(|p| raises_at(p, ctx, depth))
}

/// Whether evaluating `c`, a condition over the rows of `input`, can
/// raise an error, its constants computed and its subqueries judged as
/// `plan_raises` judges a plan's.
pub(super) fn cond_raises_over(c: &Cond, input: &LogicalPlan, ctx: &RuleContext<'_>) -> bool {
    let folded = c.clone().map_exprs(&mut |e| fold_expr(&e, &ctx.db.memory).unwrap_or(e));
    let catalog = &*ctx.db.catalog;
    let subquery = |s: &Subquery, one: bool| (one && !s.plan.one_row_at_most()) || judged(&s.plan, ctx, 1);
    cond_raises(&folded, &Judge { types: &|column| plan_column_type(input, column, catalog), subquery: &subquery })
}

/// Whether evaluating `c` can raise an error: as `Cond::can_raise` judges
/// it, and as its columns' types and its subqueries' plans do not disprove.
fn cond_raises(c: &Cond, judge: &Judge<'_>) -> bool {
    c.can_raise() && cond_can_raise_judged(c, judge)
}

/// Whether evaluating `e` can raise an error, as `cond_raises` judges a
/// condition.
fn expr_raises(e: &Expr, judge: &Judge<'_>) -> bool {
    e.can_raise() && can_raise_judged(e, judge)
}

/// Whether computing `a` can raise an error.
fn agg_raises(a: &AggCall, functions: &Registry, judge: &Judge<'_>) -> bool {
    let own = match functions.standard(&a.func) {
        Some(Standard::Count | Standard::Max | Standard::Min) => false,
        Some(Standard::Sum | Standard::Avg) => !total_fits(a, judge.types),
        None => true,
    };
    own || parts_raise(a, judge)
}

/// Whether `a`, of one row at most, raises nothing (RULES.md D779).
fn raise_free_once(a: &AggCall, judge: &Judge<'_>) -> bool {
    a.func.0.gathers() && !parts_raise(a, judge)
}

/// Whether computing the argument or filter of `a` can raise an error, or
/// its distribution, which is taken as raising.
fn parts_raise(a: &AggCall, judge: &Judge<'_>) -> bool {
    let argument = a.arg.as_ref().is_some_and(|e| expr_raises(e, judge));
    argument || a.filter.as_ref().is_some_and(|c| cond_raises(c, judge)) || a.distribution.is_some()
}

/// Whether computing the collection `e` of an UNNEST can raise an error,
/// `depth` subqueries deep. A subquery of one row at most raises what its
/// outer expressions and plan raise, as it raises no cardinality violation
/// [ISO/IEC 9075:1992 §7.11 GR1].
fn collection_raises(e: &Expr, ctx: &RuleContext<'_>, depth: usize) -> bool {
    if let Expr::Subquery(s, _) = e {
        !s.plan.one_row_at_most() || s.outer.iter().any(Expr::can_raise) || judged(&s.plan, ctx, depth)
    } else {
        e.can_raise()
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 GR5] Whether `e` is a row count that raises
/// no error, a constant exact number of scale 0 that is not negative.
fn counted(e: &Expr) -> bool {
    let Expr::Const(Value::Exact(d)) = e else { return false };
    d.scale == 0 && d.units >= 0
}
