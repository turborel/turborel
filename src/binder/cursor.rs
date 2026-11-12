//! [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] A `<cursor specification>`:
//! its query bound, then the `<sort key>`s of its ORDER BY.

use super::collation::{sort_collations, with_sort};
use super::query::{Nesting, bind_query_specification};
use super::set_operations::bind_query_expression;
use super::{Bound, Context, OutColumn, rule};
use crate::ast::{ColumnSpecification, CursorSpecification, SortColumn, SortSpecification};
use crate::error::{At, Result};
use crate::logical::SortKey;

/// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] A `<cursor
/// specification>`: its query, then ORDER BY.
///
/// # Errors
///
/// Any Syntax Rule the query or its ORDER BY breaks, or a missing privilege.
pub fn bind_cursor(cx: Context<'_>, c: &CursorSpecification) -> Result<Bound> {
    // A single <query specification> can resolve ORDER BY column
    // specifications against its own FROM clause.
    if let Some(spec) = c.query.spec() {
        return bind_query_specification(Context { draws: true, ..cx }, spec, &c.order_by);
    }
    let (db, auth, session) = (cx.db, cx.auth, cx.collation);
    let bound = bind_query_expression(cx, &c.query, &Nesting::Query)?;
    let keys = sort_keys(&c.order_by, &bound.columns, |cs| result_column(&bound.columns, cs).or_at(cs.at))?;
    let collations = sort_collations((db, auth, session), &c.order_by, &keys, &bound.columns)?;
    Ok(with_sort(bound, (keys, collations)))
}

/// [ISO/IEC 9075:1992 §13.1 SR10a] The column of a `<query expression>`'s
/// result that a `<sort key>` names: the one column of that name.
fn result_column(columns: &[OutColumn], cs: &ColumnSpecification) -> Result<usize> {
    if cs.qualifier.is_some() {
        return Err(rule(format!("[ISO/IEC 9075:1992 §13.1 SR10a] ORDER BY {} names a column of the result, unqualified", cs.column)));
    }
    let matches: Vec<usize> = columns.iter().enumerate().filter(|(_, c)| c.name.as_deref() == Some(&cs.column)).map(|(i, _)| i).collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(rule(format!("[ISO/IEC 9075:1992 §13.1 SR10a] the result has no column {}", cs.column))),
        [_, _, ..] => Err(rule(format!("[ISO/IEC 9075:1992 §13.1 SR10a] the result has more than one column {}", cs.column))),
    }
}

/// [X3.135-1986 §8.3 SR6] and GR3b viii: each `<sort specification>` names a column of T.
pub(super) fn sort_keys(
    order_by: &[SortSpecification],
    columns: &[OutColumn],
    by_column: impl Fn(&ColumnSpecification) -> Result<usize>,
) -> Result<Vec<SortKey>> {
    order_by
        .iter()
        .map(|s| {
            let column = match &s.key {
                SortColumn::Position(n) => usize::try_from(*n)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .filter(|&i| i < columns.len())
                    .ok_or_else(|| rule(format!("[X3.135-1986 §8.3 SR6] ORDER BY {n} must be between 1 and {}", columns.len())))?,
                SortColumn::Column(cs) => by_column(cs)?,
            };
            Ok(SortKey { column, descending: s.descending })
        })
        .collect()
}
