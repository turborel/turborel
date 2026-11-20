//! \[DESIGN\] (RULES.md D490) What a groupjoin's recognition reads of a
//! physical plan's columns: how many its rows have, the declared type of
//! one, the sets of them no two rows agree on, and its estimated rows.
//! Each is known through the operators that keep a column as it is, a
//! filter, a projection of columns, a join's operands side by side, a
//! grouping's columns and DISTINCT's rows, down to a table's, whose
//! catalog entry declares its types, its unique keys and its NOT NULL
//! columns; of any other, nothing is known, and no groupjoin is made of
//! it.

use crate::catalog::{ConstraintKind, TableDesc};
use crate::db::State;
use crate::logical::{Expr, MAX_JUDGED_DEPTH, OuterJoin};
use crate::physical::{JoinKey, Plan};
use crate::value::DataType;

/// A set of columns no two rows of a plan agree on: of rows with no null
/// among them, or, `null_safe`, of any rows, a null equal to a null, as a
/// grouping and DISTINCT take nulls [X3.135-1986 §5.22 GR2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Unique {
    pub(super) columns: Vec<usize>,
    pub(super) null_safe: bool,
}

/// The catalog's description of the table a scan reads.
fn table<'a>(db: &'a State, plan: &Plan) -> Option<&'a TableDesc> {
    let (Plan::SeqScan { table, .. } | Plan::IndexScan { table, .. }) = plan else { return None };
    db.catalog.table(table)
}

/// The input of a node that keeps each column of its rows as it is.
fn kept_input(plan: &Plan) -> Option<&Plan> {
    if let Plan::Parallel(p) = plan {
        return Some(&p.input);
    }
    let (Plan::Filter { input, .. } | Plan::Sort { input, .. } | Plan::Limit { input, .. }) = plan else { return None };
    Some(input)
}

/// The two operands of a join whose rows are a left row's columns then a
/// right row's.
fn operands(plan: &Plan) -> Option<(&Plan, &Plan)> {
    if let Plan::MergeJoin(m) = plan {
        return Some((&m.left, &m.right));
    }
    let (Plan::Product { left, right, .. } | Plan::HashJoin { left, right, .. } | Plan::OuterJoin { left, right, .. }) = plan else {
        return None;
    };
    Some((left, right))
}

/// How many columns the rows of `plan` have, if known.
pub(super) fn width(plan: &Plan, db: &State) -> Option<usize> {
    width_at(plan, db, 0)
}

/// `width` at nesting `depth`.
fn width_at(plan: &Plan, db: &State, depth: usize) -> Option<usize> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    if let Some(input) = kept_input(plan) {
        return width_at(input, db, deeper);
    }
    if let Some((left, right)) = operands(plan) {
        return width_at(left, db, deeper)?.checked_add(width_at(right, db, deeper)?);
    }
    if let Plan::SeqScan { located, .. } | Plan::IndexScan { located, .. } = plan {
        return table(db, plan)?.columns.len().checked_add(usize::from(*located));
    }
    if let Plan::Project { exprs, .. } = plan {
        return Some(exprs.len());
    }
    if let Plan::Distinct { input, .. } = plan {
        return width_at(input, db, deeper);
    }
    if let Plan::Aggregate { groups, aggs, .. } = plan {
        return groups.len().checked_add(aggs.len());
    }
    if let Plan::IndexJoin { left, width, .. } = plan {
        return width_at(left, db, deeper)?.checked_add(*width);
    }
    let Plan::GroupJoin(g) = plan else { return None };
    g.columns.len().checked_add(g.aggs.len())
}

/// The declared type of column `column` of `plan`'s rows, if known.
pub(super) fn column_type(plan: &Plan, column: usize, db: &State) -> Option<DataType> {
    type_at(plan, column, db, 0)
}

/// `column_type` at nesting `depth`.
fn type_at(plan: &Plan, column: usize, db: &State, depth: usize) -> Option<DataType> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    if let Some(input) = kept_input(plan) {
        return type_at(input, column, db, deeper);
    }
    if let Some((left, right)) = operands(plan) {
        let lw = width_at(left, db, deeper)?;
        return match column.checked_sub(lw) {
            Some(at) => type_at(right, at, db, deeper),
            None => type_at(left, column, db, deeper),
        };
    }
    if let Plan::Project { exprs, input, .. } = plan {
        return expr_type(exprs.get(column)?, input, db, deeper);
    }
    if let Plan::Distinct { input, .. } = plan {
        return type_at(input, column, db, deeper);
    }
    if let Plan::Aggregate { groups, input, .. } = plan {
        return type_at(input, *groups.get(column)?, db, deeper);
    }
    if let Plan::GroupJoin(g) = plan {
        return type_at(&g.build, *g.columns.get(column)?, db, deeper);
    }
    table(db, plan)?.columns.get(column).map(|c| c.data_type.clone())
}

/// The declared type of `e` over the rows of `input`, if known: of a
/// column, or of a call of a function on arguments of known types.
fn expr_type(e: &Expr, input: &Plan, db: &State, depth: usize) -> Option<DataType> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    if let Expr::Column(c) = e {
        return type_at(input, *c, db, depth.saturating_add(1));
    }
    let Expr::Call(f, args) = e else { return None };
    let types = args.iter().map(|a| expr_type(a, input, db, depth.saturating_add(1))).collect::<Option<Vec<_>>>()?;
    f.0.return_type(&types).ok()
}

/// Whether column `at` of `desc` holds no null whenever a statement reads
/// it: NOT NULL, or of the PRIMARY KEY, by a constraint not deferrable.
pub(super) fn never_null(desc: &TableDesc, at: usize) -> bool {
    desc.constraints.iter().any(|c| {
        let constrained = match &c.kind {
            ConstraintKind::NotNull(column) => *column == at,
            ConstraintKind::Unique { columns, primary, .. } => *primary && columns.contains(&at),
            ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } | ConstraintKind::Connection(_) => false,
        };
        constrained && !c.attributes.deferrable
    })
}

/// The unique keys of the table `desc`, each of a constraint not
/// deferrable and of no period: null-safe if each column is never null, or
/// nulls are not distinct in it.
fn table_keys(desc: &TableDesc) -> Vec<Unique> {
    desc.constraints
        .iter()
        .filter_map(|c| {
            let ConstraintKind::Unique { columns, period: None, nulls_distinct, .. } = &c.kind else { return None };
            let null_safe = !nulls_distinct || columns.iter().all(|&k| never_null(desc, k));
            (!c.attributes.deferrable).then(|| Unique { columns: columns.clone(), null_safe })
        })
        .collect()
}

/// The sets of columns of `plan` no two of its rows agree on, as known.
pub(super) fn unique(plan: &Plan, db: &State) -> Vec<Unique> {
    unique_at(plan, db, 0)
}

/// `unique` at nesting `depth`.
fn unique_at(plan: &Plan, db: &State, depth: usize) -> Vec<Unique> {
    if depth > MAX_JUDGED_DEPTH {
        return Vec::new();
    }
    let deeper = depth.saturating_add(1);
    if let Some(input) = kept_input(plan) {
        return unique_at(input, db, deeper);
    }
    if let Plan::Distinct { input, .. } = plan {
        return width_at(input, db, deeper).map_or_else(Vec::new, |w| vec![Unique { columns: (0..w).collect(), null_safe: true }]);
    }
    if let Plan::Aggregate { groups, .. } = plan {
        return vec![Unique { columns: (0..groups.len()).collect(), null_safe: true }];
    }
    if let Plan::Project { exprs, input, .. } = plan {
        return unique_at(input, db, deeper).into_iter().filter_map(|u| projected(&u, exprs)).collect();
    }
    if let Plan::HashJoin { left, right, keys, .. } = plan {
        return joined_keys((left, right), keys, true, (db, deeper));
    }
    if let Plan::OuterJoin { kind: OuterJoin::Left, left, right, keys, .. } = plan {
        return joined_keys((left, right), keys, false, (db, deeper));
    }
    table(db, plan).map(table_keys).unwrap_or_default()
}

/// `u`, a unique set of a projection's input, as its columns are projected
/// by `exprs`, if each is.
fn projected(u: &Unique, exprs: &[Expr]) -> Option<Unique> {
    let columns = u.columns.iter().map(|&c| exprs.iter().position(|e| matches!(e, Expr::Column(k) if *k == c))).collect::<Option<_>>()?;
    Some(Unique { columns, null_safe: u.null_safe })
}

/// Whether the columns `keyed` of rows whose unique sets are `sets` find
/// one row at most for any values matched by `keys`, each key's column
/// of those rows as `side` picks it: a set among them, null-safe or each
/// of its columns matched by a key on which a null matches nothing.
pub(super) fn found_once(sets: &[Unique], keys: &[JoinKey], side: fn(&JoinKey) -> usize) -> bool {
    sets.iter().any(|u| u.columns.iter().all(|&c| keys.iter().any(|k| side(k) == c && (u.null_safe || !k.nulls))))
}

/// The unique sets of a join of `left` and `right` on `keys`, of an inner
/// join if `inner`, else of a LEFT one: the left's, if each left row
/// matches one right row at most; and of an inner join, the right's, its
/// columns after the left's, if each right row matches one left row at
/// most.
fn joined_keys((left, right): (&Plan, &Plan), keys: &[JoinKey], inner: bool, (db, depth): (&State, usize)) -> Vec<Unique> {
    let (lefts, rights) = (unique_at(left, db, depth), unique_at(right, db, depth));
    let mut out = Vec::new();
    if found_once(&rights, keys, |k| k.right) {
        out.extend(lefts.iter().cloned());
    }
    if inner && found_once(&lefts, keys, |k| k.left) {
        let Some(lw) = width_at(left, db, depth) else { return out };
        out.extend(rights.into_iter().map(|u| Unique { columns: u.columns.iter().map(|c| c.saturating_add(lw)).collect(), ..u }));
    }
    out
}

/// The rows `plan` is estimated to give.
pub(super) fn rows(plan: &Plan) -> f64 {
    rows_at(plan, 0)
}

/// `rows` at nesting `depth`.
fn rows_at(plan: &Plan, depth: usize) -> f64 {
    if depth > MAX_JUDGED_DEPTH {
        return 1.0_f64;
    }
    if let Plan::SeqScan { rows, .. } = plan {
        return crate::storage::cost_of(*rows);
    }
    if let Plan::IndexScan { est, .. }
    | Plan::Filter { est, .. }
    | Plan::Product { est, .. }
    | Plan::HashJoin { est, .. }
    | Plan::OuterJoin { est, .. }
    | Plan::Aggregate { est, .. }
    | Plan::Distinct { est, .. }
    | Plan::Limit { est, .. } = plan
    {
        return *est;
    }
    if let Plan::MergeJoin(m) = plan {
        return m.est;
    }
    if let Plan::GroupJoin(g) = plan {
        return g.est;
    }
    if let Plan::IndexJoin { left, est, .. } = plan {
        return rows_at(left, depth.saturating_add(1)) * est.max(1.0_f64);
    }
    let projected = if let Plan::Project { input, .. } = plan { Some(&**input) } else { None };
    kept_input(plan).or(projected).map_or(1.0_f64, |input| rows_at(input, depth.saturating_add(1)))
}

/// The table, and its column, that column `column` of `plan`'s rows is a
/// value of, as it is or null-extended by no outer join, if known.
pub(super) fn traced<'a>(plan: &Plan, column: usize, db: &'a State) -> Option<(&'a TableDesc, usize)> {
    traced_at(plan, column, db, 0)
}

/// `traced` at nesting `depth`.
fn traced_at<'a>(plan: &Plan, column: usize, db: &'a State, depth: usize) -> Option<(&'a TableDesc, usize)> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    if let Some(input) = kept_input(plan) {
        return traced_at(input, column, db, deeper);
    }
    if let Plan::Project { exprs, input, .. } = plan {
        let Some(Expr::Column(c)) = exprs.get(column) else { return None };
        return traced_at(input, *c, db, deeper);
    }
    if let Plan::Aggregate { groups, input, .. } = plan {
        return traced_at(input, *groups.get(column)?, db, deeper);
    }
    if let Plan::Distinct { input, .. } = plan {
        return traced_at(input, column, db, deeper);
    }
    if let Some((left, right)) = operands(plan) {
        let lw = width_at(left, db, deeper)?;
        // An outer join's columns of an operand it extends with nulls are
        // no table's values.
        let (left_kept, right_kept) = kept_sides(plan);
        return match column.checked_sub(lw) {
            Some(at) => right_kept.then(|| traced_at(right, at, db, deeper)).flatten(),
            None => left_kept.then(|| traced_at(left, column, db, deeper)).flatten(),
        };
    }
    table(db, plan).filter(|desc| column < desc.columns.len()).map(|desc| (desc, column))
}

/// Of a join, whether its left operand's columns are that operand's values
/// in every row, and whether its right's are: not of an operand an outer
/// join extends with nulls.
fn kept_sides(plan: &Plan) -> (bool, bool) {
    if let Plan::OuterJoin { kind, .. } = plan {
        return (matches!(kind, OuterJoin::Left), matches!(kind, OuterJoin::Right));
    }
    if let Plan::MergeJoin(m) = plan {
        return (true, !m.outer);
    }
    (true, true)
}
