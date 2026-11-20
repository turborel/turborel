//! \[DESIGN\] (RULES.md D490) The eager form of a groupjoin, as Fent and
//! Neumann, "A Practical Approach to Groupjoin and Nested Aggregates"
//! (PVLDB 2021), make it: the probe operand grouped by its keys first,
//! then joined with the group operand, whose key finds one row at most,
//! so no group's states are taken twice. Of an inner join alone, and of
//! set functions the early grouping cannot make raise or warn where the
//! join would not: it takes every probe row, those no group operand row
//! joins too, so each set function is COUNT, SUM, AVG, MAX or MIN, of an
//! argument that cannot raise (RULES.md D15) and is never null, as the
//! warning that a null was eliminated would otherwise be raised of rows
//! the join leaves out [ISO/IEC 9075:1992 §6.5 GR1b]; and a SUM or AVG is
//! of a column of exact numbers of at most `SAFE_PRECISION` digits, whose
//! total fits 128 bits however many rows there are, so none of its
//! groups' totals raises.
//!
//! Its groups come as the grouping's did: the group operand probing the
//! early groups if it was the join's left operand, its rows in their
//! order; else the early groups probing it, in the order of their first
//! rows, each a key's first probe row, which is its first row joined.

use super::columns::{column_type, never_null, rows, traced};
use super::recognized::Found;
use crate::db::State;
use crate::logical::{AggCall, Expr, can_raise_typed};
use crate::physical::{JoinKey, Plan};
use crate::value::DataType;

/// \[DESIGN\] The most digits of an exact column a SUM or AVG of the
/// eager form takes: a value of fewer than 10^18 units, times fewer than
/// 2^64 rows, is less than 2^127.
const SAFE_PRECISION: u32 = 18;

/// The set functions the eager form computes, by name.
const EAGER_FUNCTIONS: [&str; 5] = ["COUNT", "SUM", "AVG", "MAX", "MIN"];

/// The summing set functions, whose totals the precision bounds.
const SUMMING: [&str; 2] = ["SUM", "AVG"];

/// The eager form of `found`, standing for the grouping `plan`, and the
/// groups its early grouping is estimated to make, if it may be made (as
/// the module says).
pub(super) fn eager(found: &Found<'_>, plan: &Plan, db: &State) -> Option<(Plan, f64)> {
    if found.outer || !found.aggs.iter().all(|a| eager_safe(a, found.probe, db)) {
        return None;
    }
    let groups = groups_of(found, db);
    let Plan::Aggregate { detail, est, .. } = plan else { return None };
    let shown = !detail.is_empty();
    let early = Plan::Aggregate {
        groups: found.keys.iter().map(|k| k.right).collect(),
        collations: Vec::new(),
        aggs: found.aggs.clone(),
        seeds: Vec::new(),
        detail: if shown { format!("early, by the keys of {} for {detail}", found.joined) } else { String::new() },
        input: Box::new(found.probe.clone()),
        est: groups,
    };
    let (keyed, count) = (found.keys.len(), found.aggs.len());
    let joined = if shown { found.joined.to_owned() } else { String::new() };
    let (join, columns, results) = if found.by_first {
        let keys = found.keys.iter().zip(0..).map(|(k, at)| JoinKey { left: at, right: k.left, nulls: k.nulls }).collect();
        let join = Plan::HashJoin { left: Box::new(early), right: Box::new(found.build.clone()), keys, detail: joined, est: *est };
        (join, keyed.saturating_add(count), keyed)
    } else {
        let keys = found.keys.iter().zip(0..).map(|(k, at)| JoinKey { left: k.left, right: at, nulls: k.nulls }).collect();
        let join = Plan::HashJoin { left: Box::new(found.build.clone()), right: Box::new(early), keys, detail: joined, est: *est };
        (join, 0, found.widths.0.saturating_add(keyed))
    };
    let exprs = found
        .columns
        .iter()
        .map(|&c| Expr::Column(columns.saturating_add(c)))
        .chain((0..count).map(|a| Expr::Column(results.saturating_add(a))))
        .collect();
    let detail = if shown { format!("groups of {detail}") } else { String::new() };
    Some((Plan::Project { exprs, detail, input: Box::new(join) }, groups))
}

/// Whether the set function `a` over the rows of `probe` may be computed
/// of every probe row (as the module says).
fn eager_safe(a: &AggCall, probe: &Plan, db: &State) -> bool {
    let name = a.func.0.name();
    if !EAGER_FUNCTIONS.contains(&name) {
        return false;
    }
    let Some(arg) = &a.arg else { return true };
    let types = |c: usize| column_type(probe, c, db);
    if can_raise_typed(arg, &types) || !never_null_expr(arg, probe, db) {
        return false;
    }
    if !SUMMING.contains(&name) {
        return true;
    }
    let Expr::Column(c) = arg else { return false };
    let Some(DataType::Exact { precision, .. }) = column_type(probe, *c, db) else { return false };
    precision <= SAFE_PRECISION
}

/// Whether `e` over the rows of `probe` is never null: a column of a
/// table's that is NOT NULL, or a constant not null.
fn never_null_expr(e: &Expr, probe: &Plan, db: &State) -> bool {
    if let Expr::Const(v) = e {
        return !v.is_null();
    }
    let Expr::Column(c) = e else { return false };
    traced(probe, *c, db).is_some_and(|(desc, at)| never_null(desc, at))
}

/// The groups the probe operand's keys make, as its tables' statistics
/// count their columns' values, no more than its rows.
fn groups_of(found: &Found<'_>, db: &State) -> f64 {
    let probed = rows(found.probe);
    let mut groups = 1.0_f64;
    for k in &found.keys {
        let distinct = traced(found.probe, k.right, db).and_then(|(desc, at)| db.statistics(&desc.name)?.distinct(at));
        groups *= distinct.unwrap_or(probed);
    }
    groups.min(probed).max(1.0_f64)
}
