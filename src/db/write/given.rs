//! \[EXTENSION\] The rows given an INSERT of one row of dynamic parameters
//! (RULES.md D530, `Session::execute_prepared_rows`): each the values of
//! its parameters, inserted as an execution of its own would insert it,
//! all by one execution, so they go in together and the statement is
//! bound once, not once a row.

use super::Target;
use crate::ast::{InsertSource, InsertStatement, QueryExpression, RowElement, ValueExpression, ValueSpecification};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::exec::{HeldRows, Subqueries};
use crate::storage::Row;

/// Whether `stmt` inserts one row whose values are its dynamic parameters,
/// each once, in order: an INSERT rows may be given.
fn of_parameters(stmt: &InsertStatement) -> bool {
    let InsertSource::Query(query) = &stmt.source else { return false };
    let QueryExpression::Values(rows) = query.as_ref() else { return false };
    let [row] = rows.as_slice() else { return false };
    row.elements.iter().enumerate().all(|(i, element)| {
        if let RowElement::Value(ValueExpression::Value(ValueSpecification::Dynamic { index, .. }), _) = element {
            *index == i
        } else {
            false
        }
    })
}

/// The candidate rows of `given`, each the values of `stmt`'s parameters,
/// for `target`, as each execution's VALUES would give it.
///
/// # Errors
///
/// SQLCODE -910 if `stmt` is not an INSERT rows may be given; as
/// `Target::candidate`.
pub(super) fn given_rows(
    st: &State,
    (stmt, target): (&InsertStatement, &Target),
    given: &[Row],
    subqueries: &dyn Subqueries,
) -> Result<HeldRows<Row>> {
    if !of_parameters(stmt) {
        return Err(not_given());
    }
    let mut rows = HeldRows::new(st.list_space());
    for values in given {
        rows.push(target.candidate(st, values.clone(), subqueries)?)?;
    }
    Ok(rows)
}

/// The failure of rows given a statement that takes none.
pub(in crate::db) fn not_given() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] rows are given only to an INSERT of one row of dynamic parameters into a table or an updatable view")
}
