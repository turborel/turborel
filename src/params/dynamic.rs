//! \[EXTENSION\] [ISO/IEC 9075:1992 §6.2, §17.6, §17.9] The `<dynamic
//! parameter specification>`s of a prepared statement: each `?`'s type,
//! deduced from where it stands as the statement is bound [§17.6 GR5], and,
//! as it is executed, its value, cast to that type [§17.9 GR5].
//!
//! CHARACTER VARYING, which GR5 gives a `?` in a string context, is not
//! implemented: such a `?` is described as CHARACTER of the longest length,
//! and its value taken as given, not padded. Split from params.rs (RULES.md
//! R-SIZE-4).

use std::cell::RefCell;

use super::Params;
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
    /// The data type it is described as: CHARACTER VARYING as CHARACTER of
    /// the longest length, as it is not implemented.
    pub fn data_type(self) -> DataType {
        match self {
            ParameterType::Of(t) => t,
            ParameterType::Varying => DataType::Character { length: MAX_CHARACTER_LENGTH },
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
/// to `to`: CHARACTER VARYING, not implemented, as the string it is or a
/// number's literal, not padded.
fn conformed(value: Value, to: ParameterType) -> Result<(Value, DataType)> {
    match to {
        ParameterType::Of(t) => Ok((cast(value, &t)?, t)),
        ParameterType::Varying => {
            let text = match value {
                Value::Null => return Ok((Value::Null, to.data_type())),
                Value::Char(s) => s,
                number @ (Value::Exact(_) | Value::Approx(_) | Value::Extension(_)) => match cast(number, &to.data_type())? {
                    Value::Char(s) => s.trim_end_matches(' ').to_owned(),
                    other @ (Value::Null | Value::Exact(_) | Value::Approx(_) | Value::Extension(_)) => other.to_string(),
                },
            };
            let length = u32::try_from(text.chars().count()).ok().filter(|l| *l <= MAX_CHARACTER_LENGTH).ok_or_else(|| {
                SqlError::new(Code::Data, format!("[DESIGN] a string of more than {MAX_CHARACTER_LENGTH} characters"))
                    .with_state(SqlState::STRING_DATA_RIGHT_TRUNCATION)
            })?;
            Ok((Value::Char(text), DataType::Character { length: length.max(1) }))
        }
    }
}

impl Params {
    /// The parameters of a statement being prepared, with `count` dynamic
    /// parameters, their types still to be deduced.
    pub(crate) fn preparing(count: usize) -> Params {
        Params { dynamic: Some(Dynamic { values: None, types: RefCell::new(vec![None; count]) }), ..Params::default() }
    }

    /// The parameters of a prepared statement being executed with `values`,
    /// one for each of its dynamic parameters.
    pub(crate) fn executing(values: Vec<Value>) -> Params {
        let types = RefCell::new(vec![None; values.len()]);
        Params { dynamic: Some(Dynamic { values: Some(values), types }), ..Params::default() }
    }

    /// A parser of `sql`, a statement with these parameters: one being
    /// prepared or executed takes `<dynamic parameter specification>`s
    /// [§6.2 SR2], any other its procedure's `<parameter name>`s.
    ///
    /// # Errors
    ///
    /// Those of `Parser::new`.
    pub(crate) fn parser(&self, sql: &str, extensions: bool) -> Result<Parser> {
        if self.dynamic.is_some() {
            return Parser::for_prepare(sql, extensions);
        }
        Parser::for_procedure(sql, extensions, self.names())
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
        let (Some(dynamic), Some(to)) = (&self.dynamic, taken) else { return Err(untyped()) };
        if let Some(slot) = dynamic.types.borrow_mut().get_mut(index) {
            *slot = Some(to);
        }
        let Some(values) = &dynamic.values else { return Ok((Value::Null, to.data_type())) };
        let value = values.get(index).cloned().ok_or_else(untyped)?;
        conformed(value, to)
    }
}
