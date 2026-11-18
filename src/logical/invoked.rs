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

use super::{Expr, Program, Subquery};
use crate::catalog::QualifiedName;
use crate::value::DataType;
use crate::value::types::UdtName;

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
    /// [ISO/IEC 9075-4:2006 draft §11.2] Of a body that is an SQL control
    /// statement, its program, over the parameters' values.
    pub(crate) program: Option<Arc<Program>>,
    /// Whether the function possibly modifies SQL-data: run where an
    /// SQL-session runs its statements (`Subqueries::invoke_hosted`).
    pub modifies: bool,
    /// The result type [§9.18 SR10e].
    pub result: DataType,
    /// Whether it is a null-call function [§9.18 GR8a].
    pub null_call: bool,
    /// Whether it is deterministic [§4.35.1, §9.16 GR1].
    pub deterministic: bool,
    /// Of an instance SQL-invoked method, some: its value is null if its
    /// first argument, SELF, is [ISO/IEC 9075-2:2023 §9.18 GR8a]; and,
    /// invoked with dynamic dispatch, the method that overrides it for each
    /// type of its subject's family whose values another method takes, by
    /// that type's name, each of no arguments, as it takes these [GR3b v]
    /// (RULES.md D81). None of any other routine.
    pub overrides: Option<Vec<(UdtName, Arc<Invocation>)>>,
    /// \[EXTENSION\] Of a procedure written in Rust invoked as a
    /// function (`Procedure::returns`), it: run where an SQL-session runs
    /// its statements, as it possibly modifies SQL-data. None of an
    /// SQL-invoked function.
    pub rust: Option<Arc<dyn crate::functions::builtin::Procedure>>,
}

impl Invocation {
    /// [ISO/IEC 9075-2:2023 §9.18 GR3b v] The method invoked of a subject
    /// `self_value`: the one overriding it for the subject's most specific
    /// type, if one does; else it.
    pub fn dispatched(&self, self_value: Option<&crate::value::Value>) -> &Invocation {
        let Some(crate::value::Value::Structured(s)) = self_value else { return self };
        self.overrides.iter().flatten().find(|(t, _)| *t == *s.of).map_or(self, |(_, i)| i)
    }

    /// It, its arguments replaced by `f` of each.
    pub fn map_args(&self, f: &mut dyn FnMut(Expr) -> Expr) -> Arc<Invocation> {
        Arc::new(Invocation { args: self.args.iter().cloned().map(f).collect(), ..self.clone() })
    }
}

impl Expr {
    /// Whether it invokes, but in a subquery, a function that possibly
    /// modifies SQL-data: a worklist, not recursion, so no depth bounds it.
    pub fn invokes_modifying(&self) -> bool {
        let mut work = vec![self];
        while let Some(e) = work.pop() {
            if matches!(e, Expr::Invoked(i) if i.modifies) {
                return true;
            }
            e.for_each_child(&mut |child| work.push(child));
        }
        false
    }
}
