//! [ISO/IEC 9075-2:2023 §4.35, §11.60, §11.62, §16.1, §16.2] SQL-invoked
//! routines of an SQL routine body (Feature T321): defined and dropped
//! (`define.rs`), invoked by CALL (`call.rs`), and dropped with what their
//! bodies read (`dependents.rs`). A function is invoked as a value
//! expression is evaluated (`binder/invocation.rs`, `exec/invoked.rs`).

mod body;
mod call;
mod define;
mod dependents;
mod privileges;
mod program;

pub(super) use define::of_schema_routine;
pub(super) use program::atomic;
pub(crate) use program::run_unhosted;
pub(super) use program::{Hosted, hosted_rows};

use super::Completion;
use crate::ast::ReturnStatement;
use crate::error::{Code, Result, SqlError};

/// [ISO/IEC 9075-2:2023 §16.2 SR1] A `<return statement>` not in the body
/// of an SQL-invoked function.
pub(super) fn return_outside(r: &ReturnStatement) -> SqlError {
    SqlError::at(
        Code::SyntaxRule,
        r.at,
        "[ISO/IEC 9075-2:2023 §16.2 SR1] a <return statement> is only in the body of an SQL-invoked function",
    )
}

/// [ISO/IEC 9075-2:2023 §16.2] A `<return statement>` run as a
/// statement of its own, in no function's body.
///
/// # Errors
///
/// SQLCODE -103 always [SR1].
pub(super) fn return_statement(r: &ReturnStatement) -> Result<Completion> {
    Err(return_outside(r))
}
