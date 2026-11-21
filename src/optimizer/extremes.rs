//! \[DESIGN\] MIN and MAX read from the first row in order (RULES.md
//! D413). [ISO/IEC 9075-2:2023 §10.9 GR7d iv] MAX and MIN give the
//! greatest or least value of their argument. Of an aggregation without
//! grouping columns whose one set function is MIN or MAX of a column
//! known not null, that value is the column's in the first row of the
//! rows sorted by it, ascending for MIN and descending for MAX. So the
//! aggregation's input becomes its rows, so sorted, under FETCH FIRST 1
//! ROW ONLY, and the aggregation then gives the one value of that row, or the
//! null value of no row, as before. A column that may hold a null is left,
//! as eliminating its nulls raises a warning [GR7a] that the rows sorted
//! would not.
//!
//! The sort keeps the first of rows alike (`exec/sort/top.rs`), as MIN and
//! MAX keep the first of values alike (`functions/extremes.rs`). Only an
//! exact numeric column is taken, whose values alike are one value, so no
//! collation or zone tells them apart. An index whose first columns the
//! filter fixes and whose next is the column then gives the rows in the
//! sort's order, and FETCH FIRST stops at the first (`physical/ordered.rs`),
//! where the aggregation read every row. A read stopped so evaluates the
//! filter of fewer rows, so it is taken only over a table's rows filtered
//! by a condition that cannot raise.

use super::{Rule, RuleContext};
use crate::catalog::QualifiedName;
use crate::lexer::kw;
use crate::logical::{AggCall, Expr, Fetch, Limit, LogicalPlan, Search, SortKey};
use crate::value::{DataType, Decimal, SortOrder, Value};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const EXTREMES_BY_ORDER: &str = "extremes_by_order";

/// MIN and MAX read from the first row in order, as the module says.
pub struct ExtremesByOrder;

impl Rule for ExtremesByOrder {
    fn name(&self) -> &'static str {
        EXTREMES_BY_ORDER
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = plan else { return None };
        let ([agg], true) = (aggs.as_slice(), groups.is_empty()) else { return None };
        let (column, descending) = extreme_of(agg)?;
        let table = table_rows(input)?;
        if !ctx.db.catalog.table(table)?.columns.get(column)?.not_null {
            return None;
        }
        let key = SortKey::new(column, SortOrder::of(descending, None));
        let sorted = LogicalPlan::Sort { keys: vec![key], input: input.clone() };
        let limited = LogicalPlan::Limit { limit: first_row(), input: Box::new(sorted) };
        Some(LogicalPlan::Aggregate {
            groups: Vec::new(),
            collations: collations.clone(),
            aggs: aggs.clone(),
            seeds: seeds.clone(),
            input: Box::new(limited),
        })
    }
}

/// Of `agg`, MIN or MAX of an exact numeric column with neither a filter
/// nor an inverse distribution: the column, and whether it is MAX.
fn extreme_of(agg: &AggCall) -> Option<(usize, bool)> {
    let name = agg.func.0.name();
    let descending = if name == kw::MAX {
        true
    } else if name == kw::MIN {
        false
    } else {
        return None;
    };
    let (Some(Expr::Column(column)), None, None) = (&agg.arg, &agg.filter, &agg.distribution) else { return None };
    matches!(agg.result, DataType::Exact { .. }).then_some((*column, descending))
}

/// The table whose rows `plan` is, if it is them, or them filtered by a
/// condition that cannot raise: none of an input already limited, which
/// the rule made.
fn table_rows(plan: &LogicalPlan) -> Option<&QualifiedName> {
    let (cond, rows) = if let LogicalPlan::Filter { cond, input } = plan { (Some(cond), &**input) } else { (None, plan) };
    let LogicalPlan::Scan { table, located: false, .. } = rows else { return None };
    cond.is_none_or(|c| !c.can_raise()).then_some(table)
}

/// FETCH FIRST 1 ROW ONLY.
fn first_row() -> Limit {
    let count = Expr::Const(Value::Exact(Decimal::new(1, 0)));
    let fetch = Fetch { count, percent: false, ties: None, search: Search::Exact, hints: Box::default() };
    Limit { offset: None, fetch: Some(fetch) }
}
