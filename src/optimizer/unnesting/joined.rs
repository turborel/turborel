//! \[DESIGN\] The node made of an unnested subquery (RULES.md D313). Its
//! rows are reduced to one for each binding and joined with the outer
//! rows, and each place the node used the subquery reads that row
//! instead.
//!
//! The join is a LEFT outer join of the outer rows, each with its
//! binding's keys, with a row for each binding the subquery has rows for,
//! so a row whose binding has none is kept, the row's columns null; a
//! WHERE of a marker that rejects those nulls makes it the join it is
//! (`optimizer::PushFilterIntoOuterJoin`). The row for each binding holds
//! the following.
//!
//! * of EXISTS, a marker [X3.135-1986 §5.17 GR2, GR3];
//! * of a scalar subquery, or a comparison with one row, the count of its
//!   rows and the first row's values. A row whose count is more than one
//!   raises SQLCODE -811 where the subquery stood, as that row would have
//!   [ISO/IEC 9075:1992 §7.11 GR1; X3.135-1986 §5.11 GR1], and a row whose
//!   binding has none reads nulls [§6.11 GR2];
//! * of a quantified comparison, the counts of the rows for which it is
//!   true, false or unknown, which give its truth value as §5.16 GR2 does,
//!   every row read.

use std::sync::Arc;

use super::calls::{COMPARED_ROWS, FirstValue, SCALAR_ROWS, Single};
use super::found::Node;
use super::{Bound, How, Target, compared};
use crate::functions::{AggregateRef, Registry, RowOf, ScalarRef, Standard};
use crate::lexer::kw;
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, OuterJoin, Truth, and_all};
use crate::value::{CompOp, DataType, Decimal, Value};

/// The node `node`, its subquery `target` unnested as `bound` gives it.
pub(super) fn rewritten(node: &Node<'_>, target: &Target, bound: Bound, functions: &Registry) -> Option<LogicalPlan> {
    let k = bound.keys.len();
    let rows = node.input();
    let w = rows.width();
    let per = if bound.unique && matches!(target.how, How::Value) { counted_once(bound.plan, k)? } else { per_binding(&target.how, bound.plan, k, functions)? };
    let mut exprs: Vec<Expr> = (0..w).map(Expr::Column).collect();
    exprs.extend(bound.keys.iter().cloned());
    let keyed = LogicalPlan::Project { exprs, input: Box::new(rows.clone()) };
    let on = and_all((0..k).map(|i| matched(w.saturating_add(i), w.saturating_add(k).saturating_add(i), bound.not_distinct)).collect())?;
    let join = LogicalPlan::OuterJoin { kind: OuterJoin::Left, cond: on, left: Box::new(keyed), right: Box::new(per) };
    let with = Replacing { target, base: w.saturating_add(k.saturating_mul(2)), defaults: bound.defaults.as_deref() };
    Some(match node {
        Node::Filter(cond, _) => {
            let filter = LogicalPlan::Filter { cond: with.cond((*cond).clone()), input: Box::new(join) };
            LogicalPlan::Project { exprs: (0..w).map(Expr::Column).collect(), input: Box::new(filter) }
        }
        Node::Project(exprs, _) => {
            LogicalPlan::Project { exprs: exprs.iter().map(|e| with.expr(e.clone())).collect(), input: Box::new(join) }
        }
    })
}

/// Columns `a` and `b` match, as IS NOT DISTINCT FROM if `not_distinct`,
/// else as =.
fn matched(a: usize, b: usize, not_distinct: bool) -> Cond {
    if not_distinct {
        Cond::Not(Box::new(Cond::Distinct(Expr::Column(a), Expr::Column(b))))
    } else {
        Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b))
    }
}

/// The rows of `plan`, its `k` key columns first, reduced to one for each
/// binding, with the keys, then what `how` reads.
fn per_binding(how: &How, plan: LogicalPlan, k: usize, functions: &Registry) -> Option<LogicalPlan> {
    let n = plan.width().checked_sub(k)?;
    let aggs = match how {
        How::Exists => {
            let mut exprs: Vec<Expr> = (0..k).map(Expr::Column).collect();
            exprs.push(Expr::Const(Value::Bool(true)));
            let marked = LogicalPlan::Project { exprs, input: Box::new(plan) };
            return Some(LogicalPlan::Distinct { input: Box::new(marked) });
        }
        How::Value => {
            let mut aggs = vec![counted(None, functions)?];
            aggs.extend((0..n).map(|j| first(k.saturating_add(j))));
            aggs
        }
        How::Quantified { op, all, slots } => {
            let xs = slots.iter().map(|&s| Expr::Column(s)).collect();
            let ys = (0..n).map(|j| Expr::Column(k.saturating_add(j))).collect();
            let c = compared(xs, *op, ys)?;
            let decisive = if *all { Cond::Is(Box::new(c.clone()), Truth::False, false) } else { c.clone() };
            vec![counted(Some(decisive), functions)?, counted(Some(Cond::Is(Box::new(c), Truth::Unknown, false)), functions)?]
        }
    };
    Some(LogicalPlan::Aggregate { groups: (0..k).collect(), collations: Vec::new(), aggs, seeds: Vec::new(), input: Box::new(plan) })
}

/// The rows of `plan`, its `k` key columns first and one row at most of
/// each binding, as `per_binding` gives them of a scalar subquery: the
/// keys, a count of 1, then the values.
fn counted_once(plan: LogicalPlan, k: usize) -> Option<LogicalPlan> {
    let width = plan.width();
    let mut exprs: Vec<Expr> = (0..k).map(Expr::Column).collect();
    exprs.push(Expr::Const(Value::Exact(Decimal::new(1, 0))));
    exprs.extend((k..width).map(Expr::Column));
    (k <= width).then(|| LogicalPlan::Project { exprs, input: Box::new(plan) })
}

/// COUNT(*) of the rows `filter` is true of, or of all.
fn counted(filter: Option<Cond>, functions: &Registry) -> Option<AggCall> {
    let func = functions.aggregate(kw::COUNT)?;
    if functions.standard(&func) != Some(Standard::Count) {
        return None;
    }
    let result = func.0.result_type(None).ok()?;
    let label = if filter.is_some() { format!("{}(*) FILTER", kw::COUNT) } else { format!("{}(*)", kw::COUNT) };
    Some(AggCall { func, arg: None, distinct: false, filter, distribution: None, result, label })
}

/// The value of column `column` in a group's first row, null too.
fn first(column: usize) -> AggCall {
    // A row of the value is never null, so none is eliminated
    // [ISO/IEC 9075:1992 §6.5 GR1b]; neither type is read but by binding.
    let row = DataType::row(Vec::new());
    let arg = Expr::Call(ScalarRef(Arc::new(RowOf(row.clone()))), vec![Expr::Column(column)]);
    let label = format!("{}(#{column})", super::calls::FIRST_VALUE);
    AggCall {
        func: AggregateRef(Arc::new(FirstValue)),
        arg: Some(arg),
        distinct: false,
        filter: None,
        distribution: None,
        result: row,
        label,
    }
}

/// What a place the node used the subquery reads instead, which is the
/// columns of its binding's row, from `base` on, and of a subquery made
/// a grouping by its keys, the values it has of no row.
struct Replacing<'a> {
    target: &'a Target,
    base: usize,
    defaults: Option<&'a [Expr]>,
}

impl Replacing<'_> {
    /// `c`, each place it uses the subquery replaced.
    fn cond(&self, c: Cond) -> Cond {
        match c {
            Cond::And(a, b) => Cond::And(Box::new(self.cond(*a)), Box::new(self.cond(*b))),
            Cond::Or(a, b) => Cond::Or(Box::new(self.cond(*a)), Box::new(self.cond(*b))),
            Cond::Not(a) => Cond::Not(Box::new(self.cond(*a))),
            Cond::Is(a, t, not) => Cond::Is(Box::new(self.cond(*a)), t, not),
            Cond::Exists(s) => {
                if s.id() == self.target.id {
                    return self.marked();
                }
                Cond::Exists(s).map_exprs(&mut |e| self.expr(e))
            }
            Cond::Quantified(xs, op, q, s) => {
                if s.id() == self.target.id {
                    return self.quantified(xs, op).unwrap_or(Cond::Const(Truth::Unknown));
                }
                Cond::Quantified(xs, op, q, s).map_exprs(&mut |e| self.expr(e))
            }
            other @ (Cond::Const(_)
            | Cond::Compare(..)
            | Cond::Distinct(..)
            | Cond::IsNull(..)
            | Cond::Like(..)
            | Cond::Unique(..)
            | Cond::Match { .. }
            | Cond::Value(_)
            | Cond::In(..)) => other.map_exprs(&mut |e| self.expr(e)),
        }
    }

    /// `e`, each place it uses the subquery replaced.
    fn expr(&self, e: Expr) -> Expr {
        match e {
            Expr::Subquery(s, at) => {
                if s.id() == self.target.id {
                    return self.value(at, SCALAR_ROWS);
                }
                Expr::Subquery(s.map_outer(&mut |x| self.expr(x)), at)
            }
            Expr::Case(whens, otherwise) => {
                let whens = whens.into_iter().map(|(c, r)| (self.cond(c), self.expr(r))).collect();
                Expr::Case(whens, Box::new(self.expr(*otherwise)))
            }
            Expr::Truth(c) => Expr::Truth(Box::new(self.cond(*c))),
            other @ (Expr::Column(_)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Warned(..)
            | Expr::NextValue(_)
            | Expr::Invoked(_)) => other.map_children(&mut |x| self.expr(x)),
        }
    }

    /// Of EXISTS, the marker not null; of a quantified comparison made
    /// NOT EXISTS, null.
    fn marked(&self) -> Cond {
        Cond::IsNull(Expr::Column(self.base), !self.target.negated)
    }

    /// The value of column `at` of the binding's one row, failing with
    /// `rule` where it had more; of a binding with none, null, or the
    /// value of no row if the subquery was made a grouping.
    fn value(&self, at: usize, rule: &'static str) -> Expr {
        let count = Expr::Column(self.base);
        let one = Expr::Call(
            ScalarRef(Arc::new(Single(rule))),
            vec![count.clone(), Expr::Column(self.base.saturating_add(1).saturating_add(at))],
        );
        match self.defaults.and_then(|d| d.get(at)) {
            Some(empty) => Expr::Case(vec![(Cond::IsNull(count, false), empty.clone())], Box::new(one)),
            None => one,
        }
    }

    /// A quantified comparison of `xs` by `op` that used the subquery.
    fn quantified(&self, xs: Vec<Expr>, op: CompOp) -> Option<Cond> {
        let xs: Vec<Expr> = xs.into_iter().map(|x| self.expr(x)).collect();
        match &self.target.how {
            How::Exists => Some(self.marked()),
            How::Value => {
                let ys = (0..xs.len()).map(|j| self.value(j, COMPARED_ROWS)).collect();
                compared(xs, op, ys)
            }
            How::Quantified { all, .. } => {
                let zero = Expr::Const(Value::Exact(Decimal::new(0, 0)));
                let (decided, otherwise) = if *all { (false, true) } else { (true, false) };
                let some = |column: usize| Cond::Compare(Expr::Column(column), CompOp::Gt, zero.clone());
                let whens = vec![
                    (some(self.base), Expr::Const(Value::Bool(decided))),
                    (some(self.base.saturating_add(1)), Expr::Const(Value::Null)),
                ];
                Some(Cond::Value(Expr::Case(whens, Box::new(Expr::Const(Value::Bool(otherwise))))))
            }
        }
    }
}
