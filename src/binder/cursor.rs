//! [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] A `<cursor specification>`:
//! its query bound, then the `<sort key>`s of its ORDER BY.

use super::order_by::ordered_query;
use super::query::Nesting;
use super::{Bound, Context, OutColumn, rule};
use crate::ast::{ColumnSpecification, CursorSpecification};
use crate::error::Result;

/// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] A `<cursor
/// specification>`: its query, then ORDER BY.
///
/// # Errors
///
/// Any Syntax Rule the query or its ORDER BY breaks, or a missing privilege.
pub fn bind_cursor(cx: Context<'_>, c: &CursorSpecification) -> Result<Bound> {
    // A simple table query, one after a <with clause> too, resolves ORDER
    // BY's keys against its own FROM clause [ISO/IEC 9075-2:2023 §7.17
    // SR26 d i]; its select list draws values [§6.14 SR1a ii].
    Ok(ordered_query(cx, &c.query, (&c.order_by, &Nesting::Query), true)?.sorted())
}

/// [ISO/IEC 9075:1992 §13.1 SR10a] The column of a `<query expression>`'s
/// result that a `<sort key>` names: the one column of that name.
pub(super) fn result_column(columns: &[OutColumn], cs: &ColumnSpecification) -> Result<usize> {
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
