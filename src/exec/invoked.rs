//! [ISO/IEC 9075-2:2023 §9.18 GR3 to GR10] An SQL-invoked function of an
//! SQL routine body invoked, as its `<routine invocation>` is evaluated
//! (`logical/invoked.rs`).
//!
//! Each argument is evaluated, in order, and its value assigned to its
//! parameter by store assignment [GR3, GR4a]. A null-call function given
//! a null returns null, its body not executed [GR8a]. Else the body, a
//! subquery of the row of the parameters' values, gives the value of its
//! `<return statement>` [GR8b ix 2, §16.2 GR1], which is assigned to the
//! result type [GR10a iii]. A body that is an SQL control statement is a
//! program run over a frame of the parameters' values (`exec/program.rs`):
//! one ending with no RETURN executed raises SQLSTATE 2F005. Another body
//! that is no `<return statement>` would execute none: SQLSTATE 2F005
//! [GR8b ix 1], raised without executing it, since the exception undoes
//! all it would have done.

use super::{Subqueries, broken, eval};
use crate::db::run_unhosted;
use crate::error::{Code, Result, SqlError, SqlState, Subject};
use crate::logical::{Expr, Invocation, LogicalPlan, Program, Subquery, bound_expr};
use crate::storage::Row;
use crate::value::{Value, assign};

/// [ISO/IEC 9075-2:2023 §9.18] The value of the invocation `i` for `row`.
///
/// # Errors
///
/// An argument's, or its store assignment's, as the parameter's type takes
/// its value not [§9.2]; SQLSTATE 2F005 of a body that is no `<return
/// statement>`; the body's own; the result's store assignment's.
pub(super) fn invoke(invoked: &Invocation, row: &Row, subqueries: &dyn Subqueries) -> Result<Value> {
    let args = invoked.args.iter().map(|a| eval(a, row, subqueries)).collect::<Result<Vec<Value>>>()?;
    // [ISO/IEC 9075-2:2023 §9.18 GR8a] of an instance method of a null
    // SELF, the null value; [GR3b v] else the method for its type.
    if invoked.overrides.is_some() && args.first().is_none_or(Value::is_null) {
        return Ok(Value::Null);
    }
    let i = invoked.dispatched(args.first());
    let mut values: Row = Vec::with_capacity(args.len());
    for (k, (arg, (name, t))) in args.into_iter().zip(&i.params).enumerate() {
        let assigned = assign(arg, t, "ISO/IEC 9075-2:2023 §9.18 GR4a ii");
        // [§23.1 GR4q] of an assignment to the parameter.
        let about = || Subject::parameter(i.name.parts(), i.specific.parts(), (k.saturating_add(1), "IN", name.clone()));
        values.push(assigned.map_err(|e| e.or_about(about()))?);
    }
    if i.null_call && values.iter().any(Value::is_null) {
        return Ok(Value::Null);
    }
    if i.rust.is_some() {
        return rust_hosted(i, values, subqueries);
    }
    if let Some(program) = &i.program {
        return programmed(i, program, values, subqueries);
    }
    let Some(body) = &i.body else { return Err(no_return(i)) };
    let outer = body.outer.iter().map(|e| eval(e, &values, subqueries)).collect::<Result<Vec<Value>>>()?;
    let returned = match single_value(body) {
        Some(e) => Some(eval(&bound_expr(e.clone(), &outer), &Vec::new(), subqueries)?),
        None => subqueries.rows(body, &outer, Some(1))?.first(1)?.into_iter().next().and_then(|r| r.into_iter().next()),
    };
    assign(returned.unwrap_or(Value::Null), &i.result, "ISO/IEC 9075-2:2023 §9.18 GR10a iii")
}

/// [ISO/IEC 9075-4:2006 draft §11.2; ISO/IEC 9075-2:2023 §9.18 GR8b ix] The
/// value the function `i`, whose body is `program`, returns of the
/// parameters' values `values`: run where a query evaluates it, or, of one
/// that possibly modifies SQL-data, where an SQL-session runs its
/// statements; SQLSTATE 2F005 if it returns none.
fn programmed(i: &Invocation, program: &Program, values: Row, subqueries: &dyn Subqueries) -> Result<Value> {
    let returned = match (i.modifies, subqueries.invoke_hosted(i, values.clone())) {
        (_, Some(hosted)) => hosted?,
        (false, None) => run_unhosted(program, values, subqueries)?,
        (true, None) => {
            let msg = format!("function {} possibly modifies SQL-data, and no SQL-session runs it here", i.name);
            return Err(broken(&msg).about(Subject::routine(i.name.parts(), i.specific.parts())));
        }
    };
    let returned = returned.ok_or_else(|| no_return(i))?;
    assign(returned, &i.result, "ISO/IEC 9075-2:2023 §9.18 GR10a iii")
}

/// \[EXTENSION\] The value of `i`, a procedure written in Rust invoked as
/// a function, of the parameters' values `values`: run where an
/// SQL-session runs its statements.
fn rust_hosted(i: &Invocation, values: Row, subqueries: &dyn Subqueries) -> Result<Value> {
    let Some(hosted) = subqueries.invoke_hosted(i, values) else {
        let msg = format!("function {} possibly modifies SQL-data, and no SQL-session runs it here", i.name);
        return Err(broken(&msg).about(Subject::routine(i.name.parts(), i.specific.parts())));
    };
    assign(hosted?.unwrap_or(Value::Null), &i.result, "ISO/IEC 9075-2:2023 §9.18 GR10a iii")
}

/// \[DESIGN\] The one expression of a body that is a VALUES of one row of
/// one column, as a `<return statement>` is bound: evaluated where it is,
/// its outer references its parameters' values, not run as a plan, so a
/// body that reads no table is evaluated where no subquery may run, as
/// in a CHECK.
fn single_value(body: &Subquery) -> Option<&Expr> {
    let LogicalPlan::Values { rows, .. } = &body.plan else { return None };
    let [row] = rows.as_slice() else { return None };
    let [e] = row.as_slice() else { return None };
    Some(e)
}

/// [ISO/IEC 9075-2:2023 §9.18 GR8b ix 1, §23.1 GR4l] The exception of the
/// invocation `i` of a function whose body is no `<return statement>`.
fn no_return(i: &Invocation) -> SqlError {
    let msg = format!("[ISO/IEC 9075-2:2023 §9.18 GR8b ix 1] function {} executed no <return statement>", i.name);
    SqlError::new(Code::Data, msg).with_state(SqlState::NO_RETURN_STATEMENT).about(Subject::routine(i.name.parts(), i.specific.parts()))
}
