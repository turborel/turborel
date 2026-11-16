//! [ISO/IEC 9075-2:2023 §16.1, §9.18] A CALL in the body of a function, or
//! of a procedure one invokes, bound with the program: its procedure
//! resolved and its arguments bound as CALL binds them, and the
//! procedure's body bound as a program of its own, as the owner of its
//! schema binds it, as the body of a function it invokes is bound with
//! the invocation (RULES.md D73). So the function runs where no
//! SQL-session may run a statement, and the tables the procedure reads are
//! read by the statement that invokes the function.

use std::sync::Arc;

use super::super::Context;
use super::super::invocation::bind_procedure_program;
use super::ProgramBinder;
use crate::ast::CallStatement;
use crate::db::State;
use crate::error::Result;
use crate::logical::{Called, Leaf, LogicalPlan, Step, Subquery};

impl ProgramBinder<'_> {
    /// [ISO/IEC 9075-2:2023 §16.1, §9.18 SR9] `call`, bound with the program.
    ///
    /// # Errors
    ///
    /// As CALL's binding; the procedure's body's, bound; SQLCODE -103 if
    /// the procedure invokes itself through the bodies it invokes.
    pub(super) fn inline_call(&mut self, call: &CallStatement) -> Result<Step> {
        let params = self.params();
        let bound = State::bind_call(Context { params: &params, ..self.cx }, call)?;
        let program = bind_procedure_program(self.cx, &bound.desc)?;
        let at = call.name.at;
        let plan = LogicalPlan::Values { rows: vec![bound.inputs], types: bound.desc.parameter_types() };
        let inputs = Leaf { query: Arc::new(Subquery { plan, outer: Vec::new() }), at };
        let desc = bound.desc;
        let parameters = desc.parameters.iter().map(|p| (p.name.clone(), p.mode, p.data_type.clone())).collect();
        let called = Called {
            name: desc.name,
            specific: desc.specific,
            parameters,
            inputs,
            program,
            targets: bound.targets,
            scope: Arc::clone(&self.scope),
        };
        Ok(Step::Call(Box::new(called)))
    }
}
