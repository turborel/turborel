//! [ISO/IEC 9075-2:2023 §4.26] A grouping by fewer columns (RULES.md
//! D1201): a grouping column functionally dependent on the others is not
//! grouped by, and its value is carried, the group's first row's, as a
//! column the query references without grouping by it is [§6.7 GR1e].
//! Two rows not distinct in the columns kept are not distinct in the one
//! left out [§4.26.1], and rows are of one group as they are not distinct
//! in every grouping column [§9.12], so the groups are the same ones, each
//! of the same rows, and every set function is given what it was. Of
//! `GROUP BY C.KEY, C.NAME, C.ADDRESS` of a table whose primary key is
//! KEY, the groups are told apart by KEY alone.
//!
//! The dependencies are the strict ones of the grouping's input
//! (`binder/dependencies/strict.rs`), found by the standard's rules for a
//! base table's keys, a condition's equalities and an outer join, so a key
//! that may be null, a row an outer join extends with nulls and an
//! equality that compares by another relation each give what those rules
//! give and no more. A column takes part, kept to determine or left out as
//! determined, only if the grouping groups by it as its own values are
//! told apart, by its base column's collation or by none of none. One
//! grouped by another collation stays a grouping column and determines
//! nothing. A column left out is compared by nothing.
//!
//! A grouping that makes groups before any row, of an empty grouping set
//! [§7.13 GR2a], is left, and one column at least is kept, as a grouping by
//! none is one group even of no rows.
//!
//! A HAVING condition over the grouping is moved below the projection
//! that puts the columns where they were, so it stays next to the
//! grouping.
//!
//! The rule is a late one (`Optimizer::add_late`): the rules that move a
//! HAVING condition of grouping columns into WHERE, or group below a join,
//! read the grouping as written first.

use super::{Rule, RuleContext};
use crate::binder::{Known, group_value};
use crate::logical::{Cond, Expr, LogicalPlan, map_cond_columns, plan_column_type};
use crate::value::DataType;
use crate::value::collation::CollationRef;

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const REDUCE_GROUPING_KEYS: &str = "reduce_grouping_keys";

/// The fewest grouping columns one of which may depend on the others.
const TWO_COLUMNS: usize = 2;

/// A grouping by fewer columns, as the module says.
pub struct ReduceGroupingKeys;

impl Rule for ReduceGroupingKeys {
    fn name(&self) -> &'static str {
        REDUCE_GROUPING_KEYS
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        if let LogicalPlan::Filter { cond, input } = plan {
            return having_below(cond, input);
        }
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = plan else { return None };
        if groups.len() < TWO_COLUMNS || !seeds.is_empty() {
            return None;
        }
        let known = Known::strict(ctx.db, input);
        let types: Vec<Option<DataType>> = groups.iter().map(|&g| plan_column_type(input, g, &*ctx.db.catalog)).collect();
        let kept = kept_of(&known, groups, &|i| by_own(&known, (groups, collations), i), &|i| types.get(i).is_some_and(Option::is_some));
        if kept.iter().all(|&k| k) {
            return None;
        }
        let names = input.names();
        let (mut fewer, mut by, mut carried, mut exprs) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let first = kept.iter().filter(|&&k| k).count().saturating_add(aggs.len());
        for (i, (&g, &keep)) in groups.iter().zip(&kept).enumerate() {
            if keep {
                exprs.push(Expr::Column(fewer.len()));
                fewer.push(g);
                by.push(collations.get(i).cloned().flatten());
            } else {
                let t = types.get(i).cloned().flatten()?;
                exprs.push(Expr::Column(first.saturating_add(carried.len())));
                carried.push(group_value(g, &t, names.get(g).cloned().unwrap_or_default()));
            }
        }
        exprs.extend((0..aggs.len()).map(|a| Expr::Column(fewer.len().saturating_add(a))));
        let kept_by = if by.iter().all(Option::is_none) { Vec::new() } else { by };
        let computed = aggs.iter().cloned().chain(carried).collect();
        let grouped =
            LogicalPlan::Aggregate { groups: fewer, collations: kept_by, aggs: computed, seeds: Vec::new(), input: input.clone() };
        Some(LogicalPlan::Project { exprs, input: Box::new(grouped) })
    }
}

/// A HAVING condition `cond` over the projection that puts a grouping's
/// columns where they were, moved below it, onto the grouping, so it is
/// still judged with the grouping where that is run in parallel (RULES.md
/// D1073). It is judged of the same groups, each column read where the
/// projection took it from.
fn having_below(cond: &Cond, input: &LogicalPlan) -> Option<LogicalPlan> {
    let LogicalPlan::Project { exprs, input: grouped } = input else { return None };
    if !matches!(&**grouped, LogicalPlan::Aggregate { .. }) {
        return None;
    }
    let from = exprs.iter().map(|e| if let Expr::Column(c) = e { Some(*c) } else { None }).collect::<Option<Vec<usize>>>()?;
    let moved = map_cond_columns(cond.clone(), &|c| from.get(c).copied().unwrap_or(c));
    let kept = LogicalPlan::Filter { cond: moved, input: grouped.clone() };
    Some(LogicalPlan::Project { exprs: exprs.clone(), input: Box::new(kept) })
}

/// Whether the `i`th of `groups` is grouped by its own values: by the
/// collation of its base column, or by none of none.
pub(super) fn by_own(known: &Known, (groups, collations): (&[usize], &[Option<CollationRef>]), i: usize) -> bool {
    groups.get(i).is_some_and(|&g| collations.get(i).and_then(Option::as_ref) == known.collation_of(g))
}

/// Of each of `groups`, whether it is still grouped by: not if the others
/// kept, of those `own` says are grouped by their own values, determine
/// it in the rows `known` is of, and `typed` says its type is known, as a
/// column carried needs. The later columns are left out first, so of two
/// that determine each other the first written is kept.
fn kept_of(known: &Known, groups: &[usize], own: &dyn Fn(usize) -> bool, typed: &dyn Fn(usize) -> bool) -> Vec<bool> {
    let mut kept = vec![true; groups.len()];
    for at in (0..groups.len()).rev() {
        let taking = |i: usize| i != at && own(i) && kept.get(i) == Some(&true);
        let others: Vec<usize> = groups.iter().enumerate().filter(|(i, _)| taking(*i)).map(|(_, &g)| g).collect();
        let dependent = own(at) && typed(at) && !others.is_empty() && groups.get(at).is_some_and(|&g| known.determines(&others, g));
        if let Some(slot) = kept.get_mut(at).filter(|_| dependent) {
            *slot = false;
        }
    }
    kept
}

// Unit tests: a grouping of groups made before any row, which no query
// gives the rule with columns that depend (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// Verifies: [ISO/IEC 9075-2:2023 §7.13 GR2a]; [DESIGN] RULES.md
    /// D1201: a grouping by a key and a column it determines is grouped by
    /// the key, and the same grouping with a group made before any row is
    /// left, as that group's row has a value of each column written.
    #[test]
    fn a_grouping_of_a_group_made_before_any_row_is_left() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION O CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY, N VARCHAR(10))").unwrap();
        let s = db.session("O");
        let (st, bound) = s.bound_plan("SELECT K, N, COUNT(*) FROM P GROUP BY K, N").unwrap();
        let ctx = RuleContext { db: &st, seen: &crate::physical::Seen::default() };
        let LogicalPlan::Project { input, .. } = bound else { panic!("{bound:?}") };
        let LogicalPlan::Aggregate { groups, collations, aggs, input: rows, .. } = *input else { panic!("no grouping") };
        let grouping = |seeds| LogicalPlan::Aggregate {
            groups: groups.clone(),
            collations: collations.clone(),
            aggs: aggs.clone(),
            seeds,
            input: rows.clone(),
        };
        let Some(LogicalPlan::Project { input: fewer, .. }) = ReduceGroupingKeys.apply(&grouping(Vec::new()), &ctx) else { panic!("left") };
        let LogicalPlan::Aggregate { groups: kept, aggs: carried, .. } = *fewer else { panic!("no grouping") };
        assert_eq!((kept, carried.len()), (vec![0], 2));
        let seeded = grouping(vec![vec![crate::value::Value::Null, crate::value::Value::Null]]);
        assert!(ReduceGroupingKeys.apply(&seeded, &ctx).is_none());
    }
}
