//! Parameters [X3.135-1986 §4.10, §7.3].
//!
//! In SQL-86 an application calls a `<procedure>` of a `<module>`, passing one
//! argument per `<parameter declaration>` [X3.135-1986 §4.9, §7.3]. How a
//! program is bound to a module is left to the implementation
//! [X3.135-1986 §3.4]. \[DESIGN\] Here the host program passes a `Params` with each call:
//! it plays the role of the procedure's parameter declarations and arguments.
//!
//! An indicator parameter is an ordinary parameter of exact numeric type with
//! scale 0 [X3.135-1986 §4.10.2, §5.6 SR2], named after the parameter it
//! qualifies: `P INDICATOR PI`, or just `P PI`.

use std::collections::HashSet;

mod dynamic;

use dynamic::Dynamic;
pub use dynamic::ParameterType;

use crate::ast::ParameterSpecification;
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, lex};
use crate::value::Spelled;
use crate::value::{DataType, Decimal, INDICATOR_PRECISION, MAX_CHARACTER_LENGTH, Value, assign};

/// A `<parameter declaration>` [X3.135-1986 §7.3] with its argument.
#[derive(Debug, Clone)]
pub struct Param {
    /// The `<parameter name>`.
    pub name: String,
    /// The declared `<data type>`.
    pub data_type: DataType,
    /// The argument's value. For a parameter used only as a target, any
    /// value; it is overwritten.
    pub value: Value,
}

/// The parameters of a call, as a `<procedure>` declares them
/// [X3.135-1986 §7.3], with their arguments.
#[derive(Debug, Clone, Default)]
pub struct Params {
    list: Vec<Param>,
    /// \[EXTENSION\] The dynamic parameters of a prepared statement, if it
    /// is one (`params/dynamic.rs`).
    dynamic: Option<Dynamic>,
}

fn api(msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::Api, msg)
}

impl Params {
    /// No parameters.
    pub fn new() -> Params {
        Params::default()
    }

    /// Add a `<parameter declaration>` ::= `<parameter name>` `<data type>`, with
    /// its argument value.
    #[must_use]
    pub fn declare(mut self, name: &str, data_type: DataType, value: Value) -> Params {
        self.list.push(Param { name: name.to_string(), data_type, value });
        self
    }

    /// Add an indicator parameter: exact numeric, scale 0 [X3.135-1986 §5.6 SR2].
    #[must_use]
    pub fn indicator(self, name: &str, value: i64) -> Params {
        self.declare(
            name,
            DataType::Exact { precision: INDICATOR_PRECISION, scale: 0, spelled: Spelled::Numeric },
            Value::Exact(Decimal::new(i128::from(value), 0)),
        )
    }

    /// The parameter named `name`, if declared.
    pub fn get(&self, name: &str) -> Option<&Param> {
        self.list.iter().find(|p| p.name == name)
    }

    /// The value of a parameter (null if undeclared).
    pub fn value(&self, name: &str) -> Value {
        self.get(name).map_or(Value::Null, |p| p.value.clone())
    }

    /// The value of an indicator parameter as an integer.
    pub fn indicator_value(&self, name: &str) -> Option<i64> {
        let Value::Exact(d) = self.get(name)?.value else { return None };
        if d.scale == 0 { i64::try_from(d.units).ok() } else { None }
    }

    /// Change an argument's value between calls. Undeclared names are ignored.
    pub fn set(&mut self, name: &str, value: Value) {
        if let Some(p) = self.list.iter_mut().find(|p| p.name == name) {
            p.value = value;
        }
    }

    pub(crate) fn names(&self) -> HashSet<String> {
        self.list.iter().map(|p| p.name.clone()).collect()
    }

    /// [X3.135-1986 §7.3 SR2] names are distinct, and each is an `<identifier>`
    /// [X3.135-1986 §5.4] `<parameter name>` ::= `<identifier>`. A character
    /// string parameter is no longer than `MAX_CHARACTER_LENGTH`, as for a column:
    /// `declare` cannot report it, so it is checked here.
    pub(crate) fn validate(&self) -> Result<()> {
        for (i, p) in self.list.iter().enumerate() {
            let toks = lex(&p.name).map_err(|_| api(format!("[X3.135-1986 §5.4] parameter name {:?} is not an <identifier>", p.name)))?;
            if !matches!(&toks[..], [t, _] if matches!(t.tok, Tok::Identifier(_))) {
                return Err(api(format!("[X3.135-1986 §5.4] parameter name {:?} is not an <identifier>", p.name)));
            }
            if self.list.iter().take(i).any(|q| q.name == p.name) {
                return Err(api(format!("[X3.135-1986 §7.3 SR2] parameter {} is declared twice", p.name)));
            }
            if let DataType::Character { length } | DataType::Varying { length } = p.data_type
                && length > MAX_CHARACTER_LENGTH
            {
                return Err(api(format!(
                    "[DESIGN] parameter {} is a string of {length} characters, longer than the maximum of {MAX_CHARACTER_LENGTH}",
                    p.name
                )));
            }
        }
        Ok(())
    }

    /// [X3.135-1986 §5.6 SR2] an indicator parameter is exact numeric with scale 0.
    pub(crate) fn check_indicator(&self, name: &str) -> Result<()> {
        match self.get(name).map(|p| p.data_type) {
            Some(DataType::Exact { scale: 0, .. }) => Ok(()),
            _ => Err(SqlError::new(
                Code::SyntaxRule,
                format!("[X3.135-1986 §5.6 SR2] indicator parameter {name} must be exact numeric with scale 0"),
            )),
        }
    }

    /// [X3.135-1986 §5.6 GR1] the value specified by a <parameter
    /// specification>: null if its indicator is negative, else the argument.
    pub(crate) fn input(&self, spec: &ParameterSpecification) -> Result<(Value, DataType)> {
        let p = self
            .get(&spec.name)
            .ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[X3.135-1986 §7.3 SR3] parameter {} is not declared", spec.name)))?;
        if let Some(ind) = &spec.indicator {
            self.check_indicator(ind)?;
            if self.indicator_value(ind).unwrap_or(0) < 0 {
                return Ok((Value::Null, p.data_type));
            }
        }
        if p.value.is_null() {
            return Err(api(format!(
                "parameter {} has no value; a null argument needs a negative indicator [X3.135-1986 §5.6 GR1]",
                p.name
            )));
        }
        if !argument_fits(&p.value, p.data_type) {
            return Err(api(format!(
                "[DESIGN] the argument {} of parameter {} does not fit its declared type {}",
                p.value, p.name, p.data_type
            )));
        }
        Ok((assign(p.value.clone(), &p.data_type, "X3.135-1986 §5.6 GR1")?, p.data_type))
    }
}

/// \[DESIGN\] Whether a host program's argument `value` suits a parameter of
/// type `data_type`: a string no longer than the length, an exact number for an
/// exact parameter, any number for an approximate one.
fn argument_fits(value: &Value, data_type: DataType) -> bool {
    if let (Value::Char(s), DataType::Character { length } | DataType::Varying { length }) = (value, data_type) {
        return usize::try_from(length).is_ok_and(|l| s.chars().count() <= l);
    }
    matches!(
        (value, data_type),
        (Value::Exact(_), DataType::Exact { .. })
            | (Value::Exact(_), DataType::Approximate { .. })
            | (Value::Approx(_), DataType::Approximate { .. })
    )
}
