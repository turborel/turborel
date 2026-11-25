//! \[DESIGN\] A grouping over a LEFT outer join grouped first below it
//! (RULES.md D1112), the eager aggregation of Yan and Larson, "Eager
//! Aggregation and Lazy Aggregation" (VLDB 1995), of the operand the join
//! fills with nulls. Of a grouping by the left operand's columns whose
//! set functions each take one column of the right operand, over a join
//! on equalities of a column of each alone, the right operand is grouped
//! first by its columns of those equalities, each function computed of
//! each of its groups. A left row then matches one group at most, the
//! join holds a row a distinct key in place of every right row, and the
//! grouping above reads a row a left row. That grouping merges what the
//! groups below computed: MAX the greatest and MIN the least of theirs,
//! and COUNT their sum, which is 0 of a left row no group matched, as the
//! count of that row's null was.
//!
//! The functions are COUNT, MAX and MIN of a column declared to hold no
//! null, of a table's rows as they are or filtered by a condition that
//! cannot raise. So the grouping below warns of no null, and the one
//! above of the null of a left row no group matched, as the function
//! warned of it before [ISO/IEC 9075-2:2023 §10.9 GR4b], and nothing is
//! evaluated of a row it was not before that could raise (RULES.md D15).
//! MAX and MIN are taken of exact numbers alone, whose values alike in
//! one column are one value, so which of them a group keeps shows
//! nowhere.
//!
//! Grouping the right operand costs a row grouped for each of its rows,
//! and spares the join each row beyond one a key, and the grouping above
//! each row beyond one a left row. So the rewrite is made where the right
//! operand's rows less its keys, as the planner estimates them, outnumber
//! the left operand's rows.

use super::extremes::table_rows;
use super::{Rule, RuleContext};
use crate::functions::Standard;
use crate::lexer::kw;
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, OuterJoin, split_and};
use crate::physical::estimated_rows;
use crate::value::{CompOp, DataType, Decimal, Value};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const GROUP_BEFORE_OUTER_JOIN: &str = "group_before_outer_join";

/// A grouping over a LEFT outer join grouped first below it, as the
/// module says.
pub struct GroupBeforeOuterJoin;

/// What the rule reads of a plan it applies to: the join's operands, the
/// equalities of its condition as a left column and a right one, and the
/// right operand's columns among them, each once.
struct Shape<'a> {
    left: &'a LogicalPlan,
    right: &'a LogicalPlan,
    pairs: Vec<(usize, usize)>,
    keys: Vec<usize>,
}

impl Rule for GroupBeforeOuterJoin {
    fn name(&self) -> &'static str {
        GROUP_BEFORE_OUTER_JOIN
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = plan else { return None };
        let LogicalPlan::OuterJoin { kind: OuterJoin::Left, cond, left, right } = &**input else { return None };
        let width = left.width();
        if !seeds.is_empty() || aggs.is_empty() || groups.iter().any(|&g| g >= width) {
            return None;
        }
        let shape = shape_of(cond, left, right)?;
        let below = aggs.iter().map(|a| below_of(a, &shape, ctx)).collect::<Option<Vec<_>>>()?;
        if !pays(&shape, ctx) {
            return None;
        }
        let first = shape.keys.len();
        let grouped = LogicalPlan::Aggregate {
            groups: shape.keys.clone(),
            collations: Vec::new(),
            aggs: below,
            seeds: Vec::new(),
            input: Box::new(shape.right.clone()),
        };
        let at = |key: usize| shape.keys.iter().position(|&k| k == key).map_or(width, |p| width.saturating_add(p));
        let equal = |&(l, r): &(usize, usize)| Cond::Compare(Expr::Column(l), CompOp::Eq, Expr::Column(at(r)));
        let on = shape.pairs.iter().map(equal).reduce(|a, b| Cond::And(Box::new(a), Box::new(b)))?;
        let joined =
            LogicalPlan::OuterJoin { kind: OuterJoin::Left, cond: on, left: Box::new(shape.left.clone()), right: Box::new(grouped) };
        let part = |j: usize| width.saturating_add(first).saturating_add(j);
        let above = aggs.iter().enumerate().map(|(j, a)| above_of(a, part(j), ctx)).collect::<Option<Vec<_>>>()?;
        let merged = LogicalPlan::Aggregate {
            groups: groups.clone(),
            collations: collations.clone(),
            aggs: above,
            seeds: Vec::new(),
            input: Box::new(joined),
        };
        Some(counted(merged, groups.len(), aggs, ctx))
    }
}

/// The shape of a LEFT join of `left` and `right` on `cond`, if every
/// conjunct of `cond` equates a column of each.
fn shape_of<'a>(cond: &Cond, left: &'a LogicalPlan, right: &'a LogicalPlan) -> Option<Shape<'a>> {
    let width = left.width();
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut pairs, mut keys) = (Vec::new(), Vec::new());
    for c in conjuncts {
        let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
        let (l, r) = match (a.checked_sub(width), b.checked_sub(width)) {
            (None, Some(r)) => (a, r),
            (Some(r), None) => (b, r),
            (None, None) | (Some(_), Some(_)) => return None,
        };
        pairs.push((l, r));
        if !keys.contains(&r) {
            keys.push(r);
        }
    }
    (!pairs.is_empty()).then_some(Shape { left, right, pairs, keys })
}

/// Which standard set function `a` is, if the rule takes it: one neither
/// DISTINCT nor filtered, of a column alone.
fn taken(a: &AggCall, ctx: &RuleContext<'_>) -> Option<(Standard, usize)> {
    let Some(Expr::Column(c)) = &a.arg else { return None };
    let plain = !a.distinct && a.filter.is_none() && a.distribution.is_none();
    let standard = ctx.db.functions.standard(&a.func)?;
    plain.then_some((standard, *c))
}

/// The set function the grouping below computes for `a`, over the right
/// operand's own columns, if the rule takes `a`: COUNT, or MAX or MIN of
/// exact numbers, of a column of the right operand's table declared to
/// hold no null.
fn below_of(a: &AggCall, shape: &Shape<'_>, ctx: &RuleContext<'_>) -> Option<AggCall> {
    let (standard, column) = taken(a, ctx)?;
    let own = column.checked_sub(shape.left.width())?;
    let (table, _) = table_rows(shape.right)?;
    if !ctx.db.catalog.table(table)?.columns.get(own)?.not_null {
        return None;
    }
    let exact = matches!(a.result, DataType::Exact { .. });
    let merges = match standard {
        Standard::Count => true,
        Standard::Max | Standard::Min => exact,
        Standard::Sum | Standard::Avg => false,
    };
    merges.then(|| AggCall { arg: Some(Expr::Column(own)), ..a.clone() })
}

/// The set function the grouping above computes for `a` of the column
/// `part`, what the groups below computed for it: MAX and MIN themselves,
/// and for COUNT the sum.
fn above_of(a: &AggCall, part: usize, ctx: &RuleContext<'_>) -> Option<AggCall> {
    let (standard, _) = taken(a, ctx)?;
    let arg = Some(Expr::Column(part));
    if standard != Standard::Count {
        return Some(AggCall { arg, ..a.clone() });
    }
    let func = ctx.db.functions.aggregate(kw::SUM)?;
    if ctx.db.functions.standard(&func) != Some(Standard::Sum) {
        return None;
    }
    let result = func.0.result_type(Some(a.result.clone())).ok()?;
    Some(AggCall { func, arg, result, label: format!("{}({})", kw::SUM, a.label), ..a.clone() })
}

/// `merged`, the grouping above, of `groups` grouping columns, each sum
/// it computes for a COUNT of `aggs` given as that count: 0 of a group
/// whose sum is null, no group below matched, and of COUNT's type.
fn counted(merged: LogicalPlan, groups: usize, aggs: &[AggCall], ctx: &RuleContext<'_>) -> LogicalPlan {
    let zero = || Expr::Const(Value::Exact(Decimal::new(0, 0)));
    let mut exprs: Vec<Expr> = (0..groups).map(Expr::Column).collect();
    for (j, a) in aggs.iter().enumerate() {
        let column = groups.saturating_add(j);
        if taken(a, ctx).is_some_and(|(standard, _)| standard == Standard::Count) {
            let sum = Expr::Case(vec![(Cond::IsNull(Expr::Column(column), false), zero())].into(), Box::new(Expr::Column(column)));
            exprs.push(Expr::Cast(Box::new(sum), a.result.clone()));
        } else {
            exprs.push(Expr::Column(column));
        }
    }
    LogicalPlan::Project { exprs, input: Box::new(merged) }
}

/// Whether grouping the right operand first pays: its rows less its
/// distinct keys, as estimated, outnumber the left operand's rows.
fn pays(shape: &Shape<'_>, ctx: &RuleContext<'_>) -> bool {
    let keyed = LogicalPlan::Aggregate {
        groups: shape.keys.clone(),
        collations: Vec::new(),
        aggs: Vec::new(),
        seeds: Vec::new(),
        input: Box::new(shape.right.clone()),
    };
    let (rows, keys) = (estimated_rows(ctx.db, shape.right), estimated_rows(ctx.db, &keyed));
    rows - keys > estimated_rows(ctx.db, shape.left)
}
