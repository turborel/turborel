//! \[DESIGN\] Grouping columns computed after the grouping (RULES.md
//! D1305). A grouping column that is an expression of other grouping
//! columns alone, as `A - 1` beside `A` of a derived table's
//! `SELECT A, A - 1`, or of none, a constant, is one value of every row
//! of a group the others make. So the others alone tell the groups apart
//! [ISO/IEC 9075-2:2023 §9.12, §4.26.1], and the rule
//! `compute_keys_after_grouping` leaves such a column out of the
//! grouping and computes it of each group, in a projection over the
//! grouping that puts every column where it was. The projection below
//! computes it of the rows no more.
//!
//! An expression is taken only if it gives the same value of the same
//! columns each time (`expr_pure`) and cannot raise or warn, judged by
//! its columns' declared types (`can_raise_typed`). Its rows are then
//! evaluated for no longer, and its groups once each, which changes no
//! error (RULES.md D15). One that may raise stays a grouping column,
//! computed of every row as it was.
//!
//! Each column it reads must be grouped by as its own values are told
//! apart: a base table's column of a type whose values not distinct are
//! one value to every expression (`Known::one_value`,
//! `binder/dependencies/strict.rs`), which no collation groups. Two
//! character strings equal but for trailing spaces are of one group and
//! differ in length, so an expression of a string stays a grouping
//! column. A null is one value too, of which the expression gives what
//! it gave of each row.
//!
//! The expression is found through the nodes that hand each row of the
//! projection on whole: filters, products, semi joins and the preserved
//! operand of an outer join. Of the operand an outer join extends with
//! nulls the column is null where the expression of nulls may not be, so
//! it is left.
//!
//! A grouping that makes groups before any row, of an empty grouping
//! set [§7.13 GR2a], is left, and one column at least is kept, as a
//! grouping by none is one group even of no rows.
//!
//! A HAVING condition over the projection is moved below it, onto the
//! grouping, and a select list over it is merged with it, so the plan is
//! the one of the query that groups by the columns kept and writes the
//! expressions in its select list. The rule is a late one
//! (`Optimizer::add_late`), before `reduce_grouping_keys`, which then
//! reads the grouping columns kept.

use std::ptr;

use crate::binder::Known;
use crate::catalog::Catalog;
use crate::logical::{
    AggCall, Cond, Distribution, Expr, LogicalPlan, MAX_JUDGED_DEPTH, OuterJoin, can_raise_typed, expr_columns, map_cond_columns,
    map_cond_exprs, map_expr_columns, plan_column_type,
};
use crate::optimizer::unnesting::expr_pure;
use crate::optimizer::{Rule, RuleContext, substitute};
use crate::value::collation::CollationRef;

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const COMPUTE_KEYS_AFTER_GROUPING: &str = "compute_keys_after_grouping";

/// The fewest grouping columns one of which may be computed of the
/// others, as one at least is kept.
const TWO_COLUMNS: usize = 2;

/// Grouping columns computed after the grouping, as the module says.
pub struct ComputeKeysAfterGrouping;

impl Rule for ComputeKeysAfterGrouping {
    fn name(&self) -> &'static str {
        COMPUTE_KEYS_AFTER_GROUPING
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let catalog = &*ctx.db.catalog;
        if let LogicalPlan::Filter { cond, input } = plan {
            return having_below(cond, input, catalog);
        }
        if let LogicalPlan::Project { exprs, input } = plan {
            return merged(exprs, input, catalog);
        }
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = plan else { return None };
        if groups.len() < TWO_COLUMNS || !seeds.is_empty() {
            return None;
        }
        let later = computed_of(groups, input, ctx);
        if later.iter().all(Option::is_none) {
            return None;
        }
        let is_kept = |at: &usize| later.get(*at).is_some_and(Option::is_none);
        let place = |at: usize| (0..at).filter(is_kept).count();
        let kept: Vec<usize> = (0..groups.len()).filter(is_kept).collect();
        let of_group = |(at, l): (usize, &Option<Expr>)| l.clone().map_or(Expr::Column(place(at)), |e| map_expr_columns(e, &place));
        let mut exprs: Vec<Expr> = later.iter().enumerate().map(of_group).collect();
        exprs.extend((0..aggs.len()).map(|a| Expr::Column(kept.len().saturating_add(a))));
        let by: Vec<Option<CollationRef>> = kept.iter().map(|&at| collations.get(at).cloned().flatten()).collect();
        let kept_by = if by.iter().all(Option::is_none) { Vec::new() } else { by };
        let column = |at: usize| groups.get(at).copied();
        let left_out: Vec<usize> = (0..groups.len()).filter(|at| !is_kept(at)).filter_map(column).collect();
        let fewer: Vec<usize> = kept.iter().copied().filter_map(column).collect();
        let (fewer, computed, rows) = narrowed((fewer, aggs.clone()), input, &left_out);
        let grouped =
            LogicalPlan::Aggregate { groups: fewer, collations: kept_by, aggs: computed, seeds: Vec::new(), input: Box::new(rows) };
        Some(LogicalPlan::Project { exprs, input: Box::new(grouped) })
    }
}

/// Of each of the grouping columns `groups` of `input`, the expression
/// of the others it is computed as after the grouping, each column of it
/// the number of the grouping column it reads; none of a column still
/// grouped by.
fn computed_of(groups: &[usize], input: &LogicalPlan, ctx: &RuleContext<'_>) -> Vec<Option<Expr>> {
    let made: Vec<Vec<Made<'_>>> = groups.iter().map(|&g| made_of(input, g)).collect();
    // Most groupings are of column references alone, and ask no more.
    if made.iter().flatten().all(|m| matches!(m.expr, Expr::Column(_))) {
        return vec![None; groups.len()];
    }
    let known = Known::strict(ctx.db, input);
    let own = |j: usize| groups.get(j).is_some_and(|&g| known.one_value(g));
    let keys = Keys { made: &made, own: &own, catalog: &*ctx.db.catalog };
    let mut later: Vec<Option<Expr>> = (0..groups.len()).map(|at| keys.computed(at)).collect();
    // Constants alone are computed of nothing, and one column is kept.
    if later.iter().all(Option::is_some)
        && let Some(first) = later.first_mut()
    {
        *first = None;
    }
    later
}

/// How a column of a grouping's input is made: the node whose rows it is
/// computed of, each row of the input holding one of them whole, and the
/// expression of that node's columns it is.
#[derive(Clone, Copy)]
struct Made<'p> {
    of: &'p LogicalPlan,
    expr: &'p Expr,
}

impl Made<'_> {
    /// Whether it is the column `c` of the node `of`.
    fn is_column(&self, of: &LogicalPlan, c: usize) -> bool {
        ptr::eq(self.of, of) && if let Expr::Column(own) = self.expr { *own == c } else { false }
    }
}

/// Each way the column `column` of `plan` is made, the uppermost first:
/// of each projection it comes through, the expression it is there, down
/// to the first that is no column reference.
fn made_of(plan: &LogicalPlan, column: usize) -> Vec<Made<'_>> {
    let (mut at, mut place, mut out) = (plan, column, Vec::new());
    // RULES.md R-CF-6: no deeper than a column's type is judged.
    for _ in 0..MAX_JUDGED_DEPTH {
        let Some((next, below)) = handed_on(at, place, &mut out) else { break };
        (at, place) = (next, below);
    }
    out
}

/// One step of `made_of`: the node the column `place` of `at` comes from
/// whole, and its place there, a projection's expression added to `out`.
/// None of a node that makes its rows anew, of the operand an outer join
/// extends with nulls, or of an expression that is no column reference.
fn handed_on<'p>(at: &'p LogicalPlan, place: usize, out: &mut Vec<Made<'p>>) -> Option<(&'p LogicalPlan, usize)> {
    let side = |left: &'p LogicalPlan, right: &'p LogicalPlan| match place.checked_sub(left.width()) {
        Some(of_right) => (right, of_right, false),
        None => (left, place, true),
    };
    match at {
        LogicalPlan::Project { exprs, input } => {
            let expr = exprs.get(place)?;
            out.push(Made { of: input, expr });
            if let Expr::Column(c) = expr { Some((&**input, *c)) } else { None }
        }
        LogicalPlan::Filter { input, .. } | LogicalPlan::SemiJoin { left: input, .. } => Some((&**input, place)),
        LogicalPlan::Product { left, right } => {
            let (of, below, _) = side(left, right);
            Some((of, below))
        }
        LogicalPlan::OuterJoin { kind, left, right, .. } => {
            let (of, below, first) = side(left, right);
            let preserved = match kind {
                OuterJoin::Left => first,
                OuterJoin::Right => !first,
                OuterJoin::Full => false,
            };
            preserved.then_some((of, below))
        }
        LogicalPlan::Scan { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::MarkJoin(_)
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

/// Whether `e`, over the rows of `over`, gives the same value of the same
/// columns each time and can neither raise nor warn, so it may be
/// evaluated of other rows than it was (RULES.md D15).
pub(in crate::optimizer) fn quiet(e: &Expr, over: &LogicalPlan, catalog: &dyn Catalog) -> bool {
    expr_pure(e) && !can_raise_typed(e, &|c| plan_column_type(over, c, catalog))
}

/// A grouping's columns as they are made, and which of them are grouped
/// by as their own values are told apart.
struct Keys<'a, 'p> {
    made: &'a [Vec<Made<'p>>],
    own: &'a dyn Fn(usize) -> bool,
    catalog: &'a dyn Catalog,
}

impl Keys<'_, '_> {
    /// The grouping column `at` as an expression of the others, each
    /// column of it the number of the grouping column it reads, if it is
    /// made as one. A column that gives is a base table's, which no
    /// expression makes, so none is itself computed.
    fn computed(&self, at: usize) -> Option<Expr> {
        self.made.get(at)?.iter().find_map(|m| {
            if matches!(m.expr, Expr::Column(_)) || !quiet(m.expr, m.of, self.catalog) {
                return None;
            }
            let mut read = Vec::new();
            expr_columns(m.expr, &mut read);
            let given: Vec<(usize, usize)> = read.iter().map(|&c| Some((c, self.giving(m.of, c)?))).collect::<Option<_>>()?;
            let from = |c: usize| given.iter().find(|(of, _)| *of == c).map_or(c, |(_, key)| *key);
            Some(map_expr_columns(m.expr.clone(), &from))
        })
    }

    /// The grouping column that is the column `c` of the node `of`,
    /// grouped by as its own values are told apart.
    fn giving(&self, of: &LogicalPlan, c: usize) -> Option<usize> {
        let gives = |j: &usize| (self.own)(*j) && self.made.get(*j).is_some_and(|ways| ways.iter().any(|m| m.is_column(of, c)));
        (0..self.made.len()).find(gives)
    }
}

/// The grouping columns `groups` and set functions `aggs` over `input`
/// without its columns `left_out`, which the grouping reads no more: of a
/// projection, without those of them nothing else of the grouping reads,
/// and of one of column references alone then, over its input in its
/// place, narrowed in turn. The expressions left out are quiet, as each
/// was computed of the grouping's columns.
fn narrowed(
    (mut groups, mut aggs): (Vec<usize>, Vec<AggCall>),
    input: &LogicalPlan,
    left_out: &[usize],
) -> (Vec<usize>, Vec<AggCall>, LogicalPlan) {
    let (mut rows, mut out) = (input, left_out.to_vec());
    // RULES.md R-CF-6: no deeper than the columns were traced.
    for _ in 0..MAX_JUDGED_DEPTH {
        let LogicalPlan::Project { exprs, input: below } = rows else { break };
        let mut read = groups.clone();
        aggs.iter().flat_map(AggCall::exprs).for_each(|e| expr_columns(e, &mut read));
        let staying: Vec<usize> = (0..exprs.len()).filter(|c| read.contains(c) || !out.contains(c)).collect();
        let reference = |c: usize| if let Some(Expr::Column(of)) = exprs.get(c) { Some(*of) } else { None };
        let Some(columns) = staying.iter().map(|&c| reference(c)).collect::<Option<Vec<usize>>>() else {
            // Each column is read where it is among those that stay.
            let to = |c: usize| staying.iter().position(|&s| s == c).unwrap_or(c);
            let fewer =
                LogicalPlan::Project { exprs: staying.iter().filter_map(|&c| exprs.get(c).cloned()).collect(), input: below.clone() };
            return (groups.into_iter().map(to).collect(), aggs.into_iter().map(|a| reading(a, &to)).collect(), fewer);
        };
        // Of column references alone, each is read where it comes from.
        let to = |c: usize| staying.iter().position(|&s| s == c).and_then(|at| columns.get(at).copied()).unwrap_or(c);
        out = out.iter().filter(|c| !read.contains(c)).filter_map(|&c| reference(c)).collect::<Vec<usize>>();
        groups = groups.into_iter().map(to).collect();
        aggs = aggs.into_iter().map(|a| reading(a, &to)).collect();
        rows = below;
    }
    (groups, aggs, rows.clone())
}

/// The set function `a`, each column it reads the one `to` gives.
fn reading(a: AggCall, to: &impl Fn(usize) -> usize) -> AggCall {
    AggCall {
        arg: a.arg.map(|e| map_expr_columns(e, to)),
        filter: a.filter.map(|c| map_cond_columns(c, to)),
        distribution: a.distribution.map(|d| Box::new(Distribution { fraction: map_expr_columns(d.fraction.clone(), to), ..*d })),
        ..a
    }
}

/// Whether the select list `exprs` over the grouping `grouped` is one
/// this rule made or might have: of column references and of at least
/// one quiet expression of the grouping columns alone.
fn of_keys(exprs: &[Expr], grouped: &LogicalPlan, catalog: &dyn Catalog) -> bool {
    let LogicalPlan::Aggregate { groups, .. } = grouped else { return false };
    let of_groups = |e: &Expr| {
        let mut read = Vec::new();
        expr_columns(e, &mut read);
        read.iter().all(|&c| c < groups.len())
    };
    let computed: Vec<&Expr> = exprs.iter().filter(|e| !matches!(e, Expr::Column(_))).collect();
    !computed.is_empty() && computed.iter().all(|e| of_groups(e) && quiet(e, grouped, catalog))
}

/// A HAVING condition `cond` over a projection that computes grouping
/// columns of a grouping, moved below it, onto the grouping, so it is
/// still judged with the grouping where that is run in parallel (RULES.md
/// D1073). It is judged of the same groups, each column it reads the
/// expression the projection has for it, which is quiet.
fn having_below(cond: &Cond, input: &LogicalPlan, catalog: &dyn Catalog) -> Option<LogicalPlan> {
    let LogicalPlan::Project { exprs, input: grouped } = input else { return None };
    if !of_keys(exprs, grouped, catalog) {
        return None;
    }
    let moved = map_cond_exprs(cond.clone(), &|e| substitute(e, exprs));
    let kept = LogicalPlan::Filter { cond: moved, input: grouped.clone() };
    Some(LogicalPlan::Project { exprs: exprs.clone(), input: Box::new(kept) })
}

/// The select list `exprs` over `input`, a projection that computes
/// grouping columns of a grouping, or of its groups a HAVING condition
/// kept, as one projection: each column `exprs` reads the expression the
/// lower one has for it, which is quiet, so evaluating it where it is
/// read, of the same groups, changes nothing.
fn merged(exprs: &[Expr], input: &LogicalPlan, catalog: &dyn Catalog) -> Option<LogicalPlan> {
    let LogicalPlan::Project { exprs: lower, input: rows } = input else { return None };
    let grouped = if let LogicalPlan::Filter { input: grouped, .. } = &**rows { grouped } else { rows };
    if !of_keys(lower, grouped, catalog) {
        return None;
    }
    let exprs = exprs.iter().map(|e| substitute(e.clone(), lower)).collect();
    Some(LogicalPlan::Project { exprs, input: rows.clone() })
}

// Unit tests: an expression not deterministic that raises nothing, and a
// grouping of a group made before any row, neither of which a query gives
// the rule apart from what else it refuses (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::Database;
    use crate::functions::{ScalarFunction, ScalarRef};
    use crate::value::{DataType, Value};

    /// A function that raises nothing, deterministic if `.0`.
    struct Quiet(bool);
    impl ScalarFunction for Quiet {
        fn name(&self) -> &'static str {
            "QUIET"
        }
        fn return_type(&self, args: &[DataType]) -> crate::error::Result<DataType> {
            Ok(args.first().cloned().unwrap_or(DataType::Boolean))
        }
        fn call(&self, args: &[Value]) -> crate::error::Result<Value> {
            Ok(args.first().cloned().unwrap_or(Value::Null))
        }
        fn raise_free(&self) -> bool {
            true
        }
        fn deterministic(&self) -> bool {
            self.0
        }
    }

    /// The rule applied to a grouping by the column K of a table and by
    /// `second` of it, both of a projection, with the groups `seeds` made
    /// before any row.
    fn applied(second: Expr, seeds: Vec<crate::storage::Row>) -> Option<LogicalPlan> {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION O CREATE TABLE P (K INTEGER NOT NULL, N VARCHAR(10))").unwrap();
        let s = db.session("O");
        let (st, bound) = s.bound_plan("SELECT K, COUNT(*) FROM P GROUP BY K").unwrap();
        let ctx = RuleContext { db: &st, seen: &crate::physical::Seen::default() };
        let LogicalPlan::Project { input, .. } = bound else { panic!("{bound:?}") };
        let LogicalPlan::Aggregate { aggs, input: scan, .. } = *input else { panic!("no grouping") };
        let rows = LogicalPlan::Project { exprs: vec![Expr::Column(0), second], input: scan };
        let grouping = LogicalPlan::Aggregate { groups: vec![0, 1], collations: Vec::new(), aggs, seeds, input: Box::new(rows) };
        ComputeKeysAfterGrouping.apply(&grouping, &ctx)
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §4.35.1]; [DESIGN] RULES.md D1305: a
    /// grouping column that is a call of the other which raises nothing
    /// is computed after the grouping if the function is deterministic,
    /// the grouping then by the other alone over the table's rows, and
    /// stays a grouping column if it is not, as it may give another value
    /// of each row of a group.
    #[test]
    fn a_call_not_deterministic_stays_a_grouping_column() {
        let quiet = |deterministic: bool| Expr::Call(ScalarRef(Arc::new(Quiet(deterministic))), vec![Expr::Column(0)]);
        let Some(LogicalPlan::Project { exprs, input }) = applied(quiet(true), Vec::new()) else { panic!("left") };
        let LogicalPlan::Aggregate { groups, input: rows, .. } = *input else { panic!("no grouping") };
        assert!(matches!(exprs.get(1), Some(Expr::Call(..))), "{exprs:?}");
        assert_eq!(groups, vec![0]);
        assert!(matches!(*rows, LogicalPlan::Scan { .. }), "{rows:?}");
        assert!(applied(quiet(false), Vec::new()).is_none());
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §7.13 GR2a]; [DESIGN] RULES.md
    /// D1305: a grouping with a group made before any row is left, as
    /// that group's row has a value of each column written, which the
    /// expression of the others need not give.
    #[test]
    fn a_grouping_of_a_group_made_before_any_row_is_left() {
        let twice = Expr::Arith(Box::new(Expr::Column(0)), crate::value::ArithOp::Add, Box::new(Expr::Column(0)));
        assert!(applied(twice.clone(), Vec::new()).is_some());
        assert!(applied(twice, vec![vec![Value::Null, Value::Null]]).is_none());
    }
}
