//! [ISO/IEC 9075-2:2023 §4.46.2, §15.21 GR5] The transition variables and
//! transition tables of a triggered action, as its statements see them.
//!
//! The values of the old and new row of the transition the action is
//! invoked for are its dynamic values: each column of the subject table's
//! old row, then each of its new row. A statement of the action bound to
//! be kept reads each as a slot (`Expr::Dynamic`), filled at each
//! invocation, as a prepared statement's `?` is (RULES.md D45). Its
//! transition tables are read through handles its plans hold, given the
//! rows of each state change it is invoked for (`exec/transition.rs`).
//!
//! Of a BEFORE row trigger, a `<target specification>` may be a new
//! transition variable column reference [§6.4 SR7, §6.6 SR13]: what is
//! assigned to it is stored in the new row by store assignment [§9.2], so
//! the action's later statements read it, and the row is written so
//! [§15.21 GR5b iii].

use std::sync::Arc;

use super::Params;
use crate::catalog::{ColumnDesc, Horizon, QualifiedName, WHOLE};
use crate::error::{Code, Result, SqlError};
use crate::exec::TransitionRows;
use crate::value::{DataType, Value, assign};

/// The names a trigger gives its transition variables and tables, and
/// what they read.
#[derive(Debug)]
pub(crate) struct Transitions {
    /// The subject table's name.
    pub(crate) subject: QualifiedName,
    /// The columns of the subject table: of the old row and of the new.
    pub(crate) columns: Vec<ColumnDesc>,
    /// The old transition variable name, if given.
    pub(crate) old_row: Option<String>,
    /// The new transition variable name, if given.
    pub(crate) new_row: Option<String>,
    /// The old transition table name, if given, and its rows.
    pub(crate) old_table: Option<(String, TransitionRows)>,
    /// The new transition table name, if given, and its rows.
    pub(crate) new_table: Option<(String, TransitionRows)>,
    /// Whether a target may assign a column of the new row: of a BEFORE
    /// row trigger.
    pub(crate) assigns: bool,
    /// The columns its action sees of each base table it reads, as they
    /// were when its trigger was defined [ISO/IEC 9075-2:2023 §11.21 GR2,
    /// Note 561].
    pub(crate) horizon: Horizon,
    /// Of the rows of a view read as a table by a statement of a triggered
    /// action, a change of the view an INSTEAD OF trigger takes: the
    /// action's own, which name what these do not.
    pub(crate) beside: Option<Arc<Transitions>>,
}

impl Transitions {
    /// Whether `name` is the old or new transition variable name.
    pub(crate) fn names_variable(&self, name: &str) -> bool {
        self.own_variable(name) || self.beside.as_ref().is_some_and(|b| b.names_variable(name))
    }

    /// Whether `name` is its own old or new transition variable name, not
    /// one of those beside it.
    fn own_variable(&self, name: &str) -> bool {
        self.old_row.as_deref() == Some(name) || self.new_row.as_deref() == Some(name)
    }

    /// [ISO/IEC 9075-2:2023 §6.6 SR9] The place among the dynamic values
    /// of column `column` of the transition variable `variable`, and its
    /// type, if `variable` is the old or new transition variable name and
    /// the subject table has such a column.
    pub(crate) fn variable(&self, variable: &str, column: &str) -> Option<(usize, DataType)> {
        if !self.own_variable(variable) {
            return self.beside.as_ref()?.variable(variable, column);
        }
        let new = self.new_row.as_deref() == Some(variable);
        let (k, c) = self.columns.iter().enumerate().find(|(_, c)| c.name == column)?;
        let at = if new { self.columns.len().saturating_add(k) } else { k };
        Some((at, c.data_type.clone()))
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR14c] Whether column `column` of the
    /// transition variable `variable` is one a BEFORE trigger's action may
    /// not reference: a generated column of the new row.
    pub(crate) fn withheld(&self, variable: &str, column: &str) -> bool {
        if !self.own_variable(variable) {
            return self.beside.as_ref().is_some_and(|b| b.withheld(variable, column));
        }
        let generated = || self.columns.iter().any(|c| c.name == column && c.generation().is_some());
        self.assigns && self.new_row.as_deref() == Some(variable) && generated()
    }

    /// The transition table named `name`, if it is the old or new
    /// transition table name, and its columns: its own, else one beside
    /// it.
    pub(crate) fn table(&self, name: &str) -> Option<(&TransitionRows, &[ColumnDesc])> {
        match [&self.old_table, &self.new_table].into_iter().flatten().find(|(n, _)| n == name) {
            Some((_, rows)) => Some((rows, &self.columns)),
            None => self.beside.as_ref()?.table(name),
        }
    }
}

/// A target of a column the new row has none of, or not of a BEFORE row
/// trigger.
fn no_column(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.6 SR13] the new transition variable has no column {name}"))
}

/// A target of a generated column of the new row.
fn generated_column(name: &str) -> SqlError {
    SqlError::new(
        Code::SyntaxRule,
        format!("[ISO/IEC 9075-2:2023 §11.49 SR14c] a BEFORE trigger's action references generated column {name} of the new row"),
    )
}

impl Params {
    /// The parameters of a triggered action whose transition variables
    /// and tables `transitions` names, invoked with `values`: those of the
    /// old row's columns and then the new row's.
    pub(crate) fn transitional(values: Vec<Value>, transitions: Arc<Transitions>) -> Params {
        let mut params = Params::executing(values);
        params.transitions = Some(transitions);
        params
    }

    /// These parameters, with the transition tables of `transitions` read
    /// by their names.
    #[must_use]
    pub(crate) fn with_transitions(mut self, transitions: Arc<Transitions>) -> Params {
        self.transitions = Some(transitions);
        self
    }

    /// The transition variables and tables of the triggered action whose
    /// parameters these are, if they are a triggered action's.
    pub(crate) fn transitions(&self) -> Option<&Transitions> {
        self.transitions.as_deref()
    }

    /// Those, shared, if they are a triggered action's.
    pub(crate) fn transitions_shared(&self) -> Option<Arc<Transitions>> {
        self.transitions.clone()
    }

    /// The place among the dynamic values of column `name` of the new row
    /// a target may assign, and its type, if it has one.
    fn assignable(&self, name: &str) -> Option<(usize, DataType)> {
        let transitions = self.transitions.as_ref().filter(|t| t.assigns)?;
        let (k, c) = transitions.columns.iter().enumerate().find(|(_, c)| c.name == name)?;
        Some((transitions.columns.len().saturating_add(k), c.data_type.clone()))
    }

    /// The columns a triggered action sees of each base table it reads,
    /// as its trigger was defined, and those the body of an SQL-invoked
    /// procedure sees, as it was defined; of other parameters, every
    /// column.
    pub(crate) fn horizon(&self) -> &Horizon {
        match (&self.transitions, self.routine_scope()) {
            (Some(t), _) => &t.horizon,
            (None, Some(r)) => &r.horizon,
            (None, None) => &WHOLE,
        }
    }

    /// Whether these see every column of every table, as neither a
    /// triggered action's nor a procedure body's do (`horizon`).
    pub(crate) fn sees_whole(&self) -> bool {
        std::ptr::eq(self.horizon(), &raw const WHOLE)
    }

    /// Whether these are a triggered action's or a procedure body's, which
    /// see the columns of `horizon`, not those of the statement's context.
    pub(crate) fn horizoned(&self) -> bool {
        self.transitions.is_some() || self.routine_scope().is_some()
    }

    /// The type of column `name` of the new row a target may assign, if
    /// it has one.
    pub(crate) fn transition_type(&self, name: &str) -> Option<DataType> {
        self.assignable(name).map(|(_, t)| t)
    }

    /// The value column `name` of the new row a target may assign holds,
    /// if it has one.
    pub(crate) fn transition_value(&self, name: &str) -> Option<Value> {
        let (at, _) = self.assignable(name)?;
        self.dynamic_values().get(at).cloned()
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR14c] Checks that column `name` of the
    /// new row is one a target may assign.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the new row has no such column a target may assign,
    /// or it is a generated column.
    pub(crate) fn check_transition(&self, name: &str) -> Result<()> {
        let Some(transitions) = self.transitions.as_ref().filter(|t| t.assigns) else { return Err(no_column(name)) };
        match transitions.columns.iter().find(|c| c.name == name) {
            None => Err(no_column(name)),
            Some(c) if c.generation().is_some() => Err(generated_column(name)),
            Some(_) => Ok(()),
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.2] Stores `value` in column `name` of the
    /// new row, as `rule`, cited in full, assigns it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the new row has no such column a target may assign;
    /// store assignment's, of a value its type takes not.
    pub(crate) fn assign_transition(&mut self, name: &str, value: Value, rule: &str) -> Result<()> {
        self.check_transition(name)?;
        let (at, t) = self.assignable(name).ok_or_else(|| no_column(name))?;
        let stored = assign(value, &t, rule)?;
        self.set_dynamic(at, stored)
    }

    /// The new row as the action's statements left it, if its targets may
    /// assign it.
    pub(crate) fn new_row(&self) -> Option<Vec<Value>> {
        let transitions = self.transitions.as_ref().filter(|t| t.assigns)?;
        let width = transitions.columns.len();
        self.dynamic_values().get(width..width.saturating_mul(2)).map(<[Value]>::to_vec)
    }
}
