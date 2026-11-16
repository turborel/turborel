//! [ISO/IEC 9075-2:2023 §9.18, §10.4] A bound `<routine invocation>` of an
//! SQL-invoked function of an SQL routine body (Feature T321).
//!
//! \[DESIGN\] Its body is bound as a subquery of the row of its parameters
//! (RULES.md D73): its `<return value>` the one column of a plan of no
//! table, each SQL parameter reference an outer reference, so the body is
//! run as a correlated subquery is, for each invocation, with the values
//! of the arguments assigned to the parameters as its outer references'.
//! Its outer expressions are over that row, not the invoking one's: the
//! body is never one of the subqueries of the plan it is in, and an
//! expression's walk reaches its arguments alone.

use std::sync::Arc;

use super::{Expr, Subquery};
use crate::catalog::QualifiedName;
use crate::value::DataType;

/// \[DESIGN\] A bound invocation of an SQL-invoked function.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The function's `<schema qualified routine name>`.
    pub name: QualifiedName,
    /// Its `<specific name>`, which a condition its invocation raises
    /// carries [§23.1 GR4l].
    pub specific: QualifiedName,
    /// The arguments, over the invoking row, in order.
    pub args: Vec<Expr>,
    /// The name and declared type of each parameter, to whose type its
    /// argument's value is assigned [§9.18 GR4a].
    pub params: Vec<(String, DataType)>,
    /// The body: a subquery of one row and one column, the value of its
    /// `<return statement>`, over the row of the parameters' values. None
    /// of a body that is no `<return statement>`, which executes none
    /// [§9.18 GR8b ix].
    pub body: Option<Arc<Subquery>>,
    /// The result type [§9.18 SR10e].
    pub result: DataType,
    /// Whether it is a null-call function [§9.18 GR8a].
    pub null_call: bool,
    /// Whether it is deterministic [§4.35.1, §9.16 GR1].
    pub deterministic: bool,
}

impl Invocation {
    /// It, its arguments replaced by `f` of each.
    pub fn map_args(&self, f: &mut dyn FnMut(Expr) -> Expr) -> Arc<Invocation> {
        Arc::new(Invocation { args: self.args.iter().cloned().map(f).collect(), ..self.clone() })
    }
}
