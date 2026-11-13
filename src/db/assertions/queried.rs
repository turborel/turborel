//! \[EXTENSION\] The forms of an assertion's conjuncts whose rows are a
//! query's, not one table's [ISO/IEC 9075:1992 §11.34 GR2] (RULES.md D51):
//! of no row passing, or MAX or MIN bounded, of inner joins, or of a
//! grouping of one table's rows; `delta.rs` checks them by the query run
//! over the rows changed.
//!
//! - **Joins.** Filters, projections, DISTINCT and products of scans and
//!   of constant rows, each condition and expression of the row alone, as
//!   changes.rs says. Each row it gives is of one row of each scan, so a
//!   row a statement did not change gives what it gave with others it did
//!   not change, and a row deleted is in none. So a row it gives after the
//!   statement that it did not give before is of a row inserted or
//!   updated: each scan in turn is of the rows changed of its table, the
//!   others of the whole tables, as they are now.
//! - **Groupings.** A grouping of the rows of one table that pass steps of
//!   the row alone, by their columns or expressions of them, each set
//!   function the standard one [§6.5] and HAVING of the group alone, so the
//!   same truth or error of the same rows every time. SUM and AVG of
//!   approximate numbers are not: their value depends on the order the
//!   rows are added in. A group a statement did not change gives what it
//!   gave; so the groups of the rows changed, as they are now and as they
//!   were, are made whole of the table's rows now, and only they.
//!
//! An outer join, a set operator or a grouping over a join is checked
//! whole: a row deleted may make a row of an outer join, EXCEPT or a
//! group of a join appear, as only the tables before the statement tell.

use super::forms::{Known, Step, declared, never_null, passing, rejects, row_cond, row_expr};
use crate::catalog::QualifiedName;
use crate::functions::Standard;
use crate::logical::{Cond, Expr, LogicalPlan, SetOp, Truth, and_all, or_all, split_and};
use crate::optimizer::substitute;
use crate::storage::Row;
use crate::value::{CompOp, DataType, Value};

/// The rows of a form that are a query's: its plan, below its select list,
/// and what it is.
pub(super) struct Query {
    pub(super) plan: LogicalPlan,
    pub(super) shape: Shape,
}

/// What a query is, as `delta.rs` runs it over the rows changed.
pub(super) enum Shape {
    /// Inner joins: of each scan in turn, its table's rows changed.
    Joined,
    /// A grouping of the rows of `table` that pass `steps` by `keys`, each
    /// an expression over its row. A set function's argument may be null
    /// if `nullable`, so whether the warning of a null eliminated is
    /// raised [ISO/IEC 9075:1992 §6.5 GR1b] depends on every group.
    Grouped { table: QualifiedName, steps: Vec<Step>, keys: Vec<Expr>, nullable: bool },
}

/// The query of `rows`, the rows of a form, if it is a grouping or inner
/// joins as the module says.
pub(super) fn query_of(rows: &LogicalPlan, known: &Known<'_>) -> Option<Query> {
    if let Some(shape) = grouped(rows, known) {
        return Some(Query { plan: rows.clone(), shape });
    }
    (joined(rows) && !rows.tables().is_empty()).then(|| Query { plan: rows.clone(), shape: Shape::Joined })
}

/// Whether `plan` is of inner joins as the module says.
fn joined(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Filter { cond, input } => row_cond(cond) && joined(input),
        LogicalPlan::Project { exprs, input } => exprs.iter().all(row_expr) && joined(input),
        LogicalPlan::Product { left, right } => joined(left) && joined(right),
        LogicalPlan::Distinct { input } => joined(input),
        LogicalPlan::Scan { .. } => true,
        LogicalPlan::Values { rows, .. } => rows.iter().flatten().all(row_expr),
        LogicalPlan::Aggregate { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. } => false,
    }
}

/// The grouping `plan` is, if it is as the module says: filters and
/// projections of its groups alone over a grouping of a table's rows.
fn grouped(plan: &LogicalPlan, known: &Known<'_>) -> Option<Shape> {
    let mut node = plan;
    loop {
        match node {
            LogicalPlan::Filter { cond, input } => node = row_cond(cond).then_some(&**input)?,
            LogicalPlan::Project { exprs, input } => node = exprs.iter().all(row_expr).then_some(&**input)?,
            LogicalPlan::Aggregate { groups, collations, aggs, input } => {
                if !collations.is_empty() {
                    return None;
                }
                let standard = |f| known.functions.standard(f);
                let same = aggs.iter().all(|a| standard(&a.func).is_some_and(|f| !summed_approximately(f, &a.result)));
                let rows = passing(input).filter(|_| same && !groups.is_empty())?;
                let over = |e: &Expr| rows.columns.as_deref().map_or_else(|| e.clone(), |c| substitute(e.clone(), c));
                let args: Option<Vec<Expr>> =
                    aggs.iter().filter_map(|a| a.arg.as_ref()).map(|x| Some(x).filter(|x| row_expr(x)).map(over)).collect();
                let nullable = args?.iter().any(|x| !never_null(x, (&rows.table, &rows.steps), known.catalog));
                let keys = groups.iter().map(|g| over(&Expr::Column(*g))).collect();
                return Some(Shape::Grouped { table: rows.table, steps: rows.steps, keys, nullable });
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. } => return None,
        }
    }
}

/// Whether set function `f`, of a result of type `result`, is SUM or AVG of
/// approximate numbers, whose value depends on the order its rows are
/// added in.
fn summed_approximately(f: Standard, result: &DataType) -> bool {
    matches!(f, Standard::Sum | Standard::Avg) && matches!(result, DataType::Approximate { .. })
}

/// Whether `x`, an expression over the rows of `plan`, a form's query, is
/// never null: a constant not null, arithmetic of those, or a column that
/// is, as `column_never_null` finds.
pub(super) fn never_null_in(plan: &LogicalPlan, x: &Expr, known: &Known<'_>) -> bool {
    match x {
        Expr::Const(v) => !v.is_null(),
        Expr::Neg(y) => never_null_in(plan, y, known),
        Expr::Arith(a, _, b) => never_null_in(plan, a, known) && never_null_in(plan, b, known),
        Expr::Column(at) => column_never_null(plan, *at, known),
        Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::SortKey(..)
        | Expr::Domain(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::NextValue(_) => false,
    }
}

/// Whether column `at` of `plan` is never null: a filter above it is not
/// true of it null, or it is a table's column of a NOT NULL or PRIMARY KEY
/// constraint not deferrable (`forms::declared`), or the standard COUNT's.
fn column_never_null(plan: &LogicalPlan, at: usize, known: &Known<'_>) -> bool {
    match plan {
        LogicalPlan::Filter { cond, input } => {
            let mut conjuncts = Vec::new();
            split_and(cond.clone(), &mut conjuncts);
            conjuncts.iter().any(|c| rejects(c, at)) || column_never_null(input, at, known)
        }
        LogicalPlan::Project { exprs, input } => exprs.get(at).is_some_and(|e| never_null_in(input, e, known)),
        LogicalPlan::Product { left, right } => match at.checked_sub(left.width()) {
            Some(of_right) => column_never_null(right, of_right, known),
            None => column_never_null(left, at, known),
        },
        LogicalPlan::Distinct { input } => column_never_null(input, at, known),
        LogicalPlan::Scan { table, .. } => declared(table, at, known.catalog),
        LogicalPlan::Aggregate { groups, aggs, input, .. } => match (groups.get(at), at.checked_sub(groups.len())) {
            (Some(g), _) => column_never_null(input, *g, known),
            // [ISO/IEC 9075:1992 §6.5 GR1a, GR2a] COUNT is a cardinality; the
            // others are null of no row [GR2b i].
            (None, Some(of_aggs)) => aggs.get(of_aggs).is_some_and(|a| known.functions.standard(&a.func) == Some(Standard::Count)),
            (None, None) => false,
        },
        LogicalPlan::Values { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. } => false,
    }
}

/// The tables of the scans of `plan`, in the order `with_rows` counts them.
pub(super) fn scans(plan: &LogicalPlan) -> Vec<QualifiedName> {
    let mut out = Vec::new();
    collect_scans(plan, &mut out);
    out
}

fn collect_scans(plan: &LogicalPlan, out: &mut Vec<QualifiedName>) {
    if let LogicalPlan::Scan { table, .. } = plan {
        out.push(table.clone());
    }
    for input in plan.inputs() {
        collect_scans(input, out);
    }
}

/// `plan` with its scan numbered `at`, as `scans` counts them, made of
/// `rows`, its table's rows changed, as constants of its columns' `types`,
/// so a join of them probes an index of another table as a scan's would.
pub(super) fn with_rows(plan: LogicalPlan, at: usize, (rows, types): (&[Row], &[DataType])) -> LogicalPlan {
    let mut next = 0_usize;
    replace_scan(plan, (at, &mut next), (rows, types))
}

fn replace_scan(plan: LogicalPlan, (at, next): (usize, &mut usize), (rows, types): (&[Row], &[DataType])) -> LogicalPlan {
    if matches!(plan, LogicalPlan::Scan { .. }) {
        let this = *next;
        *next = next.saturating_add(1);
        if this != at {
            return plan;
        }
        return LogicalPlan::Values { rows: constants(rows.iter()), types: types.to_vec() };
    }
    plan.map_inputs(|input| replace_scan(input, (at, &mut *next), (rows, types)))
}

/// `rows` as rows of constants.
fn constants<'r>(rows: impl Iterator<Item = &'r Row>) -> Vec<Vec<Expr>> {
    rows.map(|r| r.iter().cloned().map(Expr::Const).collect()).collect()
}

/// `plan`, a grouping's query, its grouping made only of the groups of
/// `keys`, each a list of the values of its grouping columns, of `types`
/// if each is a table's column: of the rows
/// of a key not distinct from one of them: equal, or null where it is
/// null, as grouping takes them [ISO/IEC 9075:1992 §3.1.3h, §7.7 GR1].
pub(super) fn of_groups(plan: LogicalPlan, (keys, types): (&[Row], Option<&[DataType]>)) -> LogicalPlan {
    match plan {
        LogicalPlan::Aggregate { groups, collations, aggs, input } => {
            let input = Box::new(keyed(*input, &groups, (keys, types)));
            LogicalPlan::Aggregate { groups, collations, aggs, input }
        }
        LogicalPlan::Filter { .. } | LogicalPlan::Project { .. } => plan.map_inputs(|input| of_groups(input, (keys, types))),
        LogicalPlan::Scan { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. } => plan,
    }
}

/// The rows of `input` whose columns `groups` are one of `keys`: those of
/// keys of no null joined to the keys as constant rows of `types` on
/// equalities, so a join probes an index of the table for them; those of a
/// null in it, or all of keys of no types, by a condition of each; the two
/// together, all kept.
fn keyed(input: LogicalPlan, groups: &[usize], (keys, types): (&[Row], Option<&[DataType]>)) -> LogicalPlan {
    let (whole, nulled): (Vec<&Row>, Vec<&Row>) = keys.iter().partition(|k| types.is_some() && !k.iter().any(Value::is_null));
    let joined = types.filter(|_| !whole.is_empty()).map(|types| joined_to(input.clone(), groups, (&whole, types)));
    let filtered = (!nulled.is_empty()).then(|| {
        let each = nulled.iter().filter_map(|k| and_all(groups.iter().zip(k.iter()).map(|(g, v)| key_cond(*g, v)).collect())).collect();
        let cond = or_all(each).unwrap_or(Cond::Const(Truth::False));
        LogicalPlan::Filter { cond, input: Box::new(input.clone()) }
    });
    match (joined, filtered) {
        (Some(left), Some(right)) => {
            LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: None, left: Box::new(left), right: Box::new(right) }
        }
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => LogicalPlan::Filter { cond: Cond::Const(Truth::False), input: Box::new(input) },
    }
}

/// Column `g` of a key's row is `v`: equal to it, or null if it is.
fn key_cond(g: usize, v: &Value) -> Cond {
    if v.is_null() { Cond::IsNull(Expr::Column(g), false) } else { Cond::Compare(Expr::Column(g), CompOp::Eq, Expr::Const(v.clone())) }
}

/// The rows of `input` whose columns `groups` equal one of `keys`, none
/// null: the product of the keys, as constant rows of `types`, and
/// `input`, on each column equal to its key's, projected to `input`'s
/// columns.
fn joined_to(input: LogicalPlan, groups: &[usize], (keys, types): (&[&Row], &[DataType])) -> LogicalPlan {
    let (count, width) = (groups.len(), input.width());
    let values = LogicalPlan::Values { rows: constants(keys.iter().copied()), types: types.to_vec() };
    let equal = groups
        .iter()
        .enumerate()
        .map(|(j, g)| Cond::Compare(Expr::Column(j), CompOp::Eq, Expr::Column(count.saturating_add(*g))))
        .collect();
    let product = LogicalPlan::Product { left: Box::new(values), right: Box::new(input) };
    let filtered = match and_all(equal) {
        Some(cond) => LogicalPlan::Filter { cond, input: Box::new(product) },
        None => product,
    };
    LogicalPlan::Project { exprs: (0..width).map(|i| Expr::Column(count.saturating_add(i))).collect(), input: Box::new(filtered) }
}

// Unit tests: the queries found of an assertion's bound query, and the
// plans made of them over the rows changed (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
