//! \[EXTENSION\] [ISO/IEC 9075:1992 §6.2, §17.6, §17.9] The `<dynamic
//! parameter specification>`s of a prepared statement: each `?`'s type,
//! deduced from where it stands as the statement is bound [§17.6 GR5], and,
//! as it is executed, its value, cast to that type [§17.9 GR5].
//!
//! A `?` in a string context takes CHARACTER VARYING of the longest length,
//! `MAX_CHARACTER_LENGTH` [§17.6 GR5b-d], and its value, cast to it, is kept
//! as given, not padded.
//!
//! A statement bound to be kept between executions (RULES.md D45) binds
//! each `?` its context types as a slot (`Expr::Dynamic`), its value
//! filled as each execution begins (`logical/dynamic.rs`); a binding that
//! reads a value otherwise, a `?`'s or the statement's instant, holds it,
//! and is not kept. Split from params.rs (RULES.md R-SIZE-4).

use std::cell::{Cell, RefCell};

use super::Params;
use crate::ast::TableName;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::parser::Parser;
use crate::value::cast;
use crate::value::{DataType, MAX_CHARACTER_LENGTH, Value};

/// [ISO/IEC 9075:1992 §17.6 GR5] The type a `?` takes from its context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParameterType {
    /// The data type of another operand, a column, or a CAST's target.
    Of(DataType),
    /// CHARACTER VARYING of the longest length: of a string context.
    Varying,
}

impl ParameterType {
    /// The data type it is described as.
    pub fn data_type(self) -> DataType {
        match self {
            ParameterType::Of(t) => t,
            ParameterType::Varying => DataType::Varying { length: MAX_CHARACTER_LENGTH },
        }
    }
}

/// The dynamic parameters of a statement being bound.
#[derive(Debug, Clone, Default)]
pub(crate) struct Dynamic {
    /// Their values, as the statement is executed; none as it is prepared.
    values: Option<Vec<Value>>,
    /// Each one's type, once its context gave it one.
    types: RefCell<Vec<Option<ParameterType>>>,
    /// Whether each typed `?` is bound as a slot, to be kept.
    slots: bool,
    /// Whether the binding holds a value it read: a `?`'s other than as a
    /// slot, or the statement's instant (`Clocked`).
    held: Cell<bool>,
    /// [§17.19 SR1, §17.20 SR1] Of a positioned DELETE or UPDATE that
    /// names no table, the table of its cursor.
    implicit: Option<TableName>,
}

/// [ISO/IEC 9075:1992 §17.6 GR4] A `?` where its context gives it no type
/// (SQLSTATE 37000).
fn untyped() -> SqlError {
    SqlError::new(
        Code::SyntaxRule,
        "[ISO/IEC 9075:1992 §17.6 GR4] a <dynamic parameter specification> here takes no type from where it stands",
    )
    .with_state(SqlState::DYNAMIC_SYNTAX_ERROR)
}

/// [ISO/IEC 9075:1992 §17.9 GR5f, g] `value`, a dynamic parameter's, cast
/// to `to`'s data type, and that type.
fn conformed(value: Value, to: ParameterType) -> Result<(Value, DataType)> {
    let t = to.data_type();
    Ok((cast(value, &t)?, t))
}

impl Params {
    /// The parameters of a statement being prepared, with `count` dynamic
    /// parameters, their types still to be deduced.
    pub(crate) fn preparing(count: usize) -> Params {
        let types = RefCell::new(vec![None; count]);
        Params { dynamic: Some(Dynamic { values: None, types, slots: false, held: Cell::new(false), implicit: None }), ..Params::default() }
    }

    /// The parameters of a prepared statement being executed with `values`,
    /// one for each of its dynamic parameters.
    pub(crate) fn executing(values: Vec<Value>) -> Params {
        let types = RefCell::new(vec![None; values.len()]);
        Params {
            dynamic: Some(Dynamic { values: Some(values), types, slots: false, held: Cell::new(false), implicit: None }),
            ..Params::default()
        }
    }

    /// These parameters, of a prepared statement being executed, for
    /// binding it to be kept: each typed `?` a slot.
    pub(crate) fn keeping(&self) -> Params {
        let values = self.dynamic_values().to_vec();
        let types = RefCell::new(vec![None; values.len()]);
        let implicit = self.dynamic.as_ref().and_then(|d| d.implicit.clone());
        Params {
            dynamic: Some(Dynamic { values: Some(values), types, slots: true, held: Cell::new(false), implicit }),
            ..Params::default()
        }
    }

    /// The values of its dynamic parameters, as given; none as a
    /// statement is prepared.
    pub(crate) fn dynamic_values(&self) -> &[Value] {
        self.dynamic.as_ref().and_then(|d| d.values.as_deref()).unwrap_or_default()
    }

    /// Whether each typed `?` binds as a slot (`keeping`).
    pub(crate) fn slots(&self) -> bool {
        self.dynamic.as_ref().is_some_and(|d| d.slots)
    }

    /// Where a binding to be kept notes a value it holds (`Clocked`).
    pub(crate) fn noted(&self) -> Option<&Cell<bool>> {
        self.dynamic.as_ref().filter(|d| d.slots).map(|d| &d.held)
    }

    /// Whether the binding holds a value it read, so is not kept.
    pub(crate) fn held(&self) -> bool {
        self.dynamic.as_ref().is_some_and(|d| d.held.get())
    }

    /// These parameters, of a positioned DELETE or UPDATE prepared that
    /// names no table, whose cursor's table is `implicit` [§17.19 SR1,
    /// §17.20 SR1]: the parser of its text takes it.
    #[must_use]
    pub(crate) fn implying(mut self, implicit: Option<TableName>) -> Params {
        if let Some(dynamic) = self.dynamic.as_mut() {
            dynamic.implicit = implicit;
        }
        self
    }

    /// A parser of `sql`, a statement with these parameters: one being
    /// prepared or executed takes `<dynamic parameter specification>`s
    /// [§6.2 SR2], any other its procedure's `<parameter name>`s; its
    /// names in `catalog` (`Parser::in_catalog`).
    ///
    /// # Errors
    ///
    /// Those of `Parser::new`.
    pub(crate) fn parser(&self, sql: &str, extensions: bool, catalog: &str) -> Result<Parser> {
        let parser = if let Some(dynamic) = &self.dynamic {
            Parser::for_prepare(sql, extensions)?.implying(dynamic.implicit.clone())
        } else {
            Parser::for_procedure(sql, extensions, self.names())?
        };
        Ok(parser.in_catalog(catalog))
    }

    /// The type each dynamic parameter took, in order; none for one not
    /// bound.
    pub(crate) fn dynamic_types(&self) -> Vec<Option<ParameterType>> {
        self.dynamic.as_ref().map(|d| d.types.borrow().clone()).unwrap_or_default()
    }

    /// The value of the `index`-th dynamic parameter, of the type `taken`
    /// that its context gives it: null of it as the statement is prepared,
    /// its value cast to it as it is executed.
    ///
    /// # Errors
    ///
    /// SQLSTATE 37000 if its context gives it no type [§17.6 GR4], or there
    /// is no such parameter; CAST's [§17.9 GR5f].
    pub(crate) fn dynamic_value(&self, index: usize, taken: Option<ParameterType>) -> Result<(Value, DataType)> {
        if let Some(read) = self.noted() {
            read.set(true);
        }
        self.dynamic_typed(index, taken)
    }

    /// `dynamic_value`, of a `?` its context types, bound as a slot if
    /// they are `slots`: its value is not held.
    ///
    /// # Errors
    ///
    /// As `dynamic_value`.
    pub(crate) fn dynamic_typed(&self, index: usize, taken: Option<ParameterType>) -> Result<(Value, DataType)> {
        let (Some(dynamic), Some(to)) = (&self.dynamic, taken) else { return Err(untyped()) };
        if let Some(slot) = dynamic.types.borrow_mut().get_mut(index) {
            *slot = Some(to);
        }
        let Some(values) = &dynamic.values else { return Ok((Value::Null, to.data_type())) };
        let value = values.get(index).cloned().ok_or_else(untyped)?;
        conformed(value, to)
    }
}
