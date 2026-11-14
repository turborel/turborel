//! What an updatable cursor's plan says of the rows of its table: the
//! condition each must still satisfy, and the columns its order reads.

use crate::error::{Code, Result, SqlError};
use crate::logical::{Cond, LogicalPlan, SortKey, and_all, expr_columns};

/// The condition the rows of an updatable cursor's table satisfy, over the
/// rows of the table it reads: the conditions of the filters of `plan`
/// below its select list, its WHERE clause's and its view's. A row must
/// still satisfy it when the cursor reaches it (`db/scroll.rs`).
pub(super) fn row_condition(plan: &LogicalPlan) -> Option<Cond> {
    let mut conds = Vec::new();
    let mut node = plan;
    loop {
        match node {
            LogicalPlan::Project { input, .. } | LogicalPlan::Sort { input, .. } => node = input,
            LogicalPlan::Filter { cond, input } => {
                conds.push(cond.clone());
                node = input;
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::WorkTable { .. } => break,
        }
    }
    and_all(conds)
}

/// [ISO/IEC 9075:1992 §13.9 SR4] The columns of its table an updatable
/// cursor's ORDER BY reads: those the keys of the Sort in its `plan` are
/// made of, through the projections and filters between it and the table.
///
/// # Errors
///
/// SQLCODE -900 if a key is of a window function, whose value is not of
/// its row alone.
pub(super) fn ordered_columns(plan: &LogicalPlan) -> Result<Vec<usize>> {
    let Some((keys, input)) = sort_of(plan)? else { return Ok(Vec::new()) };
    table_columns(keys.iter().map(|k| k.column).collect(), input)
}

/// The keys of the Sort under the projections atop `plan`, and what it
/// sorts; none if it has no Sort.
///
/// # Errors
///
/// As `ordered_columns`'s.
fn sort_of(plan: &LogicalPlan) -> Result<Option<(&[SortKey], &LogicalPlan)>> {
    let mut node = plan;
    loop {
        match node {
            LogicalPlan::Project { input, .. } => node = input,
            LogicalPlan::Sort { keys, input } => return Ok(Some((keys, input))),
            LogicalPlan::Filter { .. } | LogicalPlan::Scan { .. } => return Ok(None),
            LogicalPlan::Product { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::WorkTable { .. } => return Err(unordered()),
        }
    }
}

/// The columns of the table under `plan` that its `columns` are made of,
/// through its projections and filters.
///
/// # Errors
///
/// As `ordered_columns`'s.
fn table_columns(mut columns: Vec<usize>, plan: &LogicalPlan) -> Result<Vec<usize>> {
    let mut node = plan;
    loop {
        match node {
            LogicalPlan::Project { exprs, input } => {
                let mut read = Vec::new();
                for e in columns.iter().filter_map(|&c| exprs.get(c)) {
                    expr_columns(e, &mut read);
                }
                columns = read;
                node = input;
            }
            LogicalPlan::Filter { input, .. } => node = input,
            LogicalPlan::Scan { .. } => break,
            LogicalPlan::Sort { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::WorkTable { .. } => return Err(unordered()),
        }
    }
    columns.sort_unstable();
    columns.dedup();
    Ok(columns)
}

/// [ISO/IEC 9075-2:2023 §14.3, Feature F833] The refusal of an updatable
/// cursor whose order is not of each row's own values.
fn unordered() -> SqlError {
    SqlError::new(
        Code::Unsupported,
        "[ISO/IEC 9075-2:2023 §14.3, Feature F833] turborel orders an updatable cursor by each row's own values alone, and not by a window function",
    )
}
