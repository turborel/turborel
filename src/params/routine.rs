//! [ISO/IEC 9075-2:2023 §4.35.1, §9.18 GR6g] The SQL parameters of an
//! SQL-invoked procedure, as the statement of its body sees them.
//!
//! The values of the procedure's parameters are its dynamic values, in
//! order: an `<SQL parameter reference>` of the body is bound as a slot
//! (`Expr::Dynamic`), as a transition variable is (`transition.rs`), and
//! a `<target specification>` that is one stores its value there by store
//! assignment [§9.2], so the procedure's caller reads each OUT and INOUT
//! parameter's value from there once the body has run [§9.18 GR10b].

use std::sync::Arc;

use super::Params;
use crate::catalog::{Horizon, ParameterMode, QualifiedName};
use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Value, assign};

/// The SQL parameters of the procedure whose body is run.
#[derive(Debug)]
pub(crate) struct RoutineScope {
    /// The procedure's `<schema qualified routine name>`, whose
    /// `<qualified identifier>` qualifies a reference to a parameter
    /// [§6.6 SR8a ii].
    pub(crate) name: QualifiedName,
    /// Each parameter's name, declared type and mode, in order.
    pub(crate) parameters: Vec<(String, DataType, ParameterMode)>,
    /// The columns its body sees of each base table it reads, as they
    /// were when the procedure was defined.
    pub(crate) horizon: Horizon,
}

impl RoutineScope {
    /// [ISO/IEC 9075-2:2023 §6.6 SR8] The place among the dynamic values
    /// of the parameter `name`, and its type, if `qualifier`, if any, is
    /// the procedure's name and it has such a parameter.
    pub(crate) fn parameter(&self, qualifier: Option<&str>, name: &str) -> Option<(usize, DataType)> {
        if qualifier.is_some_and(|q| q != self.name.ident) {
            return None;
        }
        self.parameters.iter().enumerate().find(|(_, (n, _, _))| n == name).map(|(k, (_, t, _))| (k, t.clone()))
    }
}

/// A target that is no parameter of the procedure.
fn no_parameter(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.8 SR1] the procedure has no SQL parameter {name}"))
}

impl Params {
    /// The parameters of the body of the procedure whose SQL parameters
    /// `scope` names, invoked with `values`, one for each of them.
    pub(crate) fn of_routine(values: Vec<Value>, scope: Arc<RoutineScope>) -> Params {
        let mut params = Params::executing(values);
        params.routine = Some(scope);
        params
    }

    /// The SQL parameters of the procedure whose body these are the
    /// parameters of, if they are a procedure's.
    pub(crate) fn routine_scope(&self) -> Option<&RoutineScope> {
        self.routine.as_deref()
    }

    /// The declared type of the SQL parameter `name` of the procedure,
    /// if it has one.
    pub(crate) fn routine_type(&self, name: &str) -> Option<DataType> {
        self.routine.as_ref()?.parameter(None, name).map(|(_, t)| t)
    }

    /// [ISO/IEC 9075-2:2023 §9.2] Stores `value` in the SQL parameter
    /// `name`, as `rule`, cited in full, assigns it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the procedure has no such parameter; store
    /// assignment's, of a value its type takes not.
    pub(crate) fn assign_routine(&mut self, name: &str, value: Value, rule: &str) -> Result<()> {
        let (at, t) = self.routine.as_ref().and_then(|r| r.parameter(None, name)).ok_or_else(|| no_parameter(name))?;
        let stored = assign(value, &t, rule)?;
        self.set_dynamic(at, stored)
    }
}
