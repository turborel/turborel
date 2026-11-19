//! [ISO/IEC 9075-2:2023 §4.35.1, §9.18 GR6g] The SQL parameters of an
//! SQL-invoked procedure, as the statement of its body sees them; and
//! [ISO/IEC 9075-4:2006 draft §6.2, §14.1] the SQL variables and cursors of
//! the compound statements a statement is in.
//!
//! The values of the procedure's parameters are its dynamic values, in
//! order: an `<SQL parameter reference>` of the body is bound as a slot
//! (`Expr::Dynamic`), as a transition variable is (`transition.rs`), and
//! a `<target specification>` that is one stores its value there by store
//! assignment [§9.2], so the procedure's caller reads each OUT and INOUT
//! parameter's value from there once the body has run [§9.18 GR10b].
//!
//! An SQL variable is a dynamic value too, after the parameters, or after
//! a triggered action's transition variables: the frame of the routine or
//! action (`logical/program.rs`). A reference `V`, or `L.V` of a variable
//! of the compound statement of the label `L`, is of the innermost
//! variable of the name in scope, which hides a parameter of it [Part 4
//! §6.2 SR3 to SR7].

use std::sync::Arc;

use super::Params;
use crate::ast::LexicallyScopedOptions;
use crate::catalog::{Horizon, ParameterMode, QualifiedName, SqlRoutineDesc};
use crate::error::{Code, Result, SqlError};
use crate::value::{DataType, Value, assign};

/// The SQL parameters of the procedure whose body is run, and the
/// variables and cursors in scope where a statement of it is.
#[derive(Debug, Clone)]
pub(crate) struct RoutineScope {
    /// The procedure's `<schema qualified routine name>`, whose
    /// `<qualified identifier>` qualifies a reference to a parameter
    /// [§6.6 SR8a ii]; of a triggered action or a statement executed
    /// directly, no name.
    pub(crate) name: QualifiedName,
    /// Each parameter's name, declared type and mode, in order.
    pub(crate) parameters: Vec<(String, DataType, ParameterMode)>,
    /// The columns its body sees of each base table it reads, as they
    /// were when the procedure was defined.
    pub(crate) horizon: Horizon,
    /// [ISO/IEC 9075-4:2006 draft §14.1 SR13] The SQL variables in scope,
    /// the innermost last.
    pub(crate) variables: Vec<ScopedVariable>,
    /// [ISO/IEC 9075-4:2006 draft §14.1 SR15] The cursors in scope, the
    /// innermost last: each `<cursor name>` and the session's key of the
    /// cursor it names in this execution of the routine.
    pub(crate) cursors: Vec<(String, String)>,
    /// [ISO/IEC 9075-4:2006 draft §9.21 SR6] Of a routine of an SQL-server
    /// module, the module's SQL-path, the schemas a name of a routine its
    /// body invokes is of, if written without one; else none, and the name
    /// is of the routine's schema.
    pub(crate) path: Vec<String>,
    /// [ISO/IEC 9075-14:2006 draft §15.1 SR1, SR2] The `<XML lexically
    /// scoped options>` of the compound statements it is in, the innermost
    /// last.
    pub(crate) xml: Vec<LexicallyScopedOptions>,
}

/// [ISO/IEC 9075-4:2006 draft §14.4] An SQL variable in scope.
#[derive(Debug, Clone)]
pub(crate) struct ScopedVariable {
    /// The `<beginning label>` of the compound statement that declares it,
    /// explicit or implicit, which qualifies a reference to it.
    pub(crate) label: Option<String>,
    /// Its `<SQL variable name>`.
    pub(crate) name: String,
    /// Its declared type.
    pub(crate) data_type: DataType,
    /// Its place among the dynamic values.
    pub(crate) slot: usize,
}

impl RoutineScope {
    /// The scope of a statement of no routine, a triggered action's or one
    /// executed directly, which sees the columns of the base tables
    /// `horizon` gives.
    pub(crate) fn unnamed(horizon: Horizon) -> RoutineScope {
        RoutineScope {
            name: QualifiedName { auth: String::new(), ident: String::new() },
            parameters: Vec::new(),
            horizon,
            variables: Vec::new(),
            cursors: Vec::new(),
            path: Vec::new(),
            xml: Vec::new(),
        }
    }

    /// The scope of the body of the routine `desc`, of the module SQL-path
    /// `path` if of a module: its parameters, and none of its variables yet.
    pub(crate) fn of(desc: &SqlRoutineDesc, path: Vec<String>) -> RoutineScope {
        RoutineScope {
            name: desc.name.clone(),
            parameters: desc.parameters.iter().map(|p| (p.name.clone(), p.data_type.clone(), p.mode)).collect(),
            horizon: desc.horizon.clone(),
            variables: Vec::new(),
            cursors: Vec::new(),
            path,
            xml: Vec::new(),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.6 SR8; ISO/IEC 9075-4:2006 draft §6.2] The
    /// place among the dynamic values of the variable or parameter `name`
    /// qualified by `qualifier`, if one is written, and its type: of the
    /// innermost variable of the name, of the compound statement labelled
    /// `qualifier`; else of the parameter, if `qualifier` is the routine's
    /// name.
    pub(crate) fn parameter(&self, qualifier: Option<&str>, name: &str) -> Option<(usize, DataType)> {
        let labelled = |v: &&ScopedVariable| qualifier.is_none_or(|q| v.label.as_deref() == Some(q));
        if let Some(v) = self.variables.iter().rev().filter(labelled).find(|v| v.name == name) {
            return Some((v.slot, v.data_type.clone()));
        }
        if qualifier.is_some_and(|q| q != self.name.ident || self.variables.iter().any(|v| v.label.as_deref() == Some(q))) {
            return None;
        }
        self.parameters.iter().enumerate().find(|(_, (n, _, _))| n == name).map(|(k, (_, t, _))| (k, t.clone()))
    }

    /// [ISO/IEC 9075-4:2006 draft §14.1 SR15] The program's key of the
    /// cursor `name` in scope, if one is.
    pub(crate) fn cursor(&self, name: &str) -> Option<&str> {
        self.cursors.iter().rev().find(|(n, _)| n == name).map(|(_, key)| key.as_str())
    }
}

/// [ISO/IEC 9075-4:2006 draft §14.1 SR15] The name of the local cursor of
/// the program's key `key` in its execution `execution`: no `<cursor
/// name>`, as none holds a NUL (`binder/psm.rs`, `cursor_key`), and none
/// of another execution's cursors, so a program running within itself, a
/// function a trigger it activates invokes, declares cursors of its own.
pub(crate) fn local_key(key: &str, execution: usize) -> String {
    format!("{key}.{execution}")
}

/// A target that is no parameter of the procedure.
fn no_parameter(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.8 SR1] the procedure has no SQL parameter or variable {name}"))
}

impl Params {
    /// The parameters of the body of the procedure whose SQL parameters
    /// `scope` names, invoked with `values`, one for each of them.
    pub(crate) fn of_routine(values: Vec<Value>, scope: Arc<RoutineScope>) -> Params {
        let mut params = Params::executing(values);
        params.routine = Some(scope);
        params
    }

    /// Makes `execution` the execution of the program these are the frame
    /// of (`local_key`).
    pub(crate) fn set_execution(&mut self, execution: usize) {
        self.execution = execution;
    }

    /// The execution of the program these are the frame of.
    pub(crate) fn execution(&self) -> usize {
        self.execution
    }

    /// [ISO/IEC 9075-4:2006 draft §14.1 SR15] The name of the local cursor
    /// `name` in scope of the statement these are the parameters of, if
    /// one is (`local_key`).
    pub(crate) fn local_cursor(&self, name: &str) -> Option<String> {
        self.routine.as_ref()?.cursor(name).map(|key| local_key(key, self.execution))
    }

    /// Makes `scope` the variables and cursors of the statement these are
    /// the parameters of: of the statement of a program about to run.
    pub(crate) fn set_routine(&mut self, scope: Arc<RoutineScope>) {
        self.routine = Some(scope);
    }

    /// [ISO/IEC 9075-4:2006 draft §14.4 GR1] Makes `value` the value of the
    /// slot `slot` of a program's frame, its variable's as it is created,
    /// or a FOR statement's as it fetches a row.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such slot.
    pub(crate) fn set_slot(&mut self, slot: usize, value: Value) -> Result<()> {
        self.set_dynamic(slot, value)
    }

    /// The SQL parameters of the procedure whose body these are the
    /// parameters of, if they are a procedure's.
    pub(crate) fn routine_scope(&self) -> Option<&RoutineScope> {
        self.routine.as_deref()
    }

    /// [ISO/IEC 9075-4:2006 draft §9.21 SR6] The schemas a routine named
    /// without one is of here: the module SQL-path of the routine whose
    /// body these are the parameters of, if it is of a module; else none.
    pub(crate) fn routine_path(&self) -> &[String] {
        self.routine.as_ref().map_or(self.path.as_slice(), |r| r.path.as_slice())
    }

    /// These parameters, of the body of a function of a module of SQL-path
    /// `path` bound inline, with no parameters of their own.
    #[must_use]
    pub(crate) fn with_path(mut self, path: Vec<String>) -> Params {
        self.path = path;
        self
    }

    /// The declared type of the SQL parameter or variable `name`, qualified
    /// by `qualifier`, if there is one.
    pub(crate) fn routine_type(&self, qualifier: Option<&str>, name: &str) -> Option<DataType> {
        self.routine.as_ref()?.parameter(qualifier, name).map(|(_, t)| t)
    }

    /// The value the SQL parameter or variable `name`, qualified by
    /// `qualifier`, holds, if there is one.
    pub(crate) fn routine_value(&self, qualifier: Option<&str>, name: &str) -> Option<Value> {
        let (at, _) = self.routine.as_ref()?.parameter(qualifier, name)?;
        self.dynamic_values().get(at).cloned()
    }

    /// [ISO/IEC 9075-2:2023 §9.2] Stores `value` in the SQL parameter or
    /// variable `name`, qualified by `qualifier`, as `rule`, cited in full,
    /// assigns it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such parameter or variable; store
    /// assignment's, of a value its type takes not.
    pub(crate) fn assign_routine(&mut self, (qualifier, name): (Option<&str>, &str), value: Value, rule: &str) -> Result<()> {
        let (at, t) = self.routine.as_ref().and_then(|r| r.parameter(qualifier, name)).ok_or_else(|| no_parameter(name))?;
        let stored = assign(value, &t, rule)?;
        self.set_dynamic(at, stored)
    }
}
