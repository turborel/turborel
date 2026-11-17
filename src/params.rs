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
mod routine;
mod transition;

pub(crate) use routine::{RoutineScope, ScopedVariable, local_key};
pub(crate) use transition::Transitions;

use dynamic::Dynamic;
pub use dynamic::ParameterType;

use crate::ast::{ParameterSpecification, TargetKind};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, lex};
use crate::value::Spelled;
use crate::value::lob::LobKind;
use crate::value::types::MAX_TYPE_NESTING;
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
    /// [ISO/IEC 9075-2:2023 §4.46.2] Of a triggered action, its transition
    /// variables and tables (`params/transition.rs`).
    transitions: Option<std::sync::Arc<Transitions>>,
    /// [ISO/IEC 9075-2:2023 §4.35.1] Of the body of an SQL-invoked
    /// procedure, its SQL parameters (`params/routine.rs`).
    routine: Option<std::sync::Arc<RoutineScope>>,
    /// [ISO/IEC 9075-4:2006 draft §14.1 SR15] Of a program's frame, its
    /// execution: what tells its local cursors from those of another
    /// running in the session (`params/routine.rs`).
    execution: usize,
    /// [ISO/IEC 9075-4:2006 draft §9.21 SR6] Of the body of a function of
    /// an SQL-server module bound inline, the module's SQL-path, which
    /// `routine` gives of any other routine's (`params/routine.rs`).
    path: Vec<String>,
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
            if let DataType::Character { length, .. } | DataType::Varying { length, .. } = p.data_type
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
        match self.get(name).map(|p| p.data_type.clone()) {
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
        // [ISO/IEC 9075-2:2023 §6.4, §6.7] an SQL parameter or variable of
        // the routine whose body this is: its value, of its declared type.
        if spec.kind == TargetKind::Routine {
            let qualifier = spec.qualifier.as_deref();
            let found = self.routine_type(qualifier, &spec.name).zip(self.routine_value(qualifier, &spec.name));
            let msg = || format!("[ISO/IEC 9075-2:2023 §6.4 SR7] {} is no SQL parameter or variable of the routine", spec.name);
            return found.map(|(t, v)| (v, t)).ok_or_else(|| SqlError::new(Code::SyntaxRule, msg()));
        }
        let p = self
            .get(&spec.name)
            .ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[X3.135-1986 §7.3 SR3] parameter {} is not declared", spec.name)))?;
        if let Some(ind) = &spec.indicator {
            self.check_indicator(ind)?;
            if self.indicator_value(ind).unwrap_or(0) < 0 {
                return Ok((Value::Null, p.data_type.clone()));
            }
        }
        if p.value.is_null() {
            return Err(api(format!(
                "parameter {} has no value; a null argument needs a negative indicator [X3.135-1986 §5.6 GR1]",
                p.name
            )));
        }
        if !argument_fits(&p.value, &p.data_type, 0) {
            return Err(api(format!(
                "[DESIGN] the argument {} of parameter {} does not fit its declared type {}",
                p.value, p.name, p.data_type
            )));
        }
        Ok((assign(p.value.clone(), &p.data_type, "X3.135-1986 §5.6 GR1")?, p.data_type.clone()))
    }
}

/// \[DESIGN\] Whether a host program's argument `value` suits a parameter of
/// type `data_type`, `depth` levels inside a composite argument. A string
/// suits one no longer than its length, an exact number an exact
/// parameter, any number an approximate or decimal floating-point one. A
/// binary string, a truth value, a datetime, an interval, a large object,
/// a structured value, a reference or a plug-in type's value suits a
/// parameter of its kind, a character or binary string a large object of
/// its kind, and a value of a distinct type's source type a parameter of
/// the distinct type, its length, fields and precision then as its
/// assignment checks them. An array suits an array type whose maximum
/// cardinality it is within, each element null or suiting the element
/// type, and a multiset a multiset type so. A row suits a row type of its degree, each field null or suiting
/// the field's type. A type is nested at most `MAX_TYPE_NESTING` deep.
fn argument_fits(value: &Value, data_type: &DataType, depth: usize) -> bool {
    if let (Value::Char(s), DataType::Character { length, units } | DataType::Varying { length, units }) = (value, data_type) {
        return usize::try_from(*length).is_ok_and(|l| units.count(s) <= l);
    }
    if let (Value::Array(elements), DataType::Array(a)) = (value, data_type) {
        return depth < MAX_TYPE_NESTING
            && usize::try_from(a.max).is_ok_and(|max| elements.len() <= max)
            && elements.iter().all(|e| e.is_null() || argument_fits(e, &a.element, depth.saturating_add(1)));
    }
    if let (Value::Multiset(elements), DataType::Multiset(m)) = (value, data_type) {
        return depth < MAX_TYPE_NESTING
            && u32::try_from(elements.len()).is_ok_and(|n| n <= crate::value::types::MAX_MULTISET_CARDINALITY)
            && elements.iter().all(|e| e.is_null() || argument_fits(e, &m.element, depth.saturating_add(1)));
    }
    if let (Value::Row(fields), DataType::Row(r)) = (value, data_type) {
        return depth < MAX_TYPE_NESTING
            && fields.len() == r.fields.len()
            && fields.iter().zip(&r.fields).all(|(v, f)| v.is_null() || argument_fits(v, &f.data_type, depth.saturating_add(1)));
    }
    // [ISO/IEC 9075-2:2023 §4.7.5] a distinct type's host value is of its
    // source type.
    if let DataType::Distinct(d) = data_type {
        return depth < MAX_TYPE_NESTING && argument_fits(value, &d.source, depth.saturating_add(1));
    }
    matches!(
        (value, data_type),
        (Value::Exact(_), DataType::Exact { .. })
            | (Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_), DataType::Approximate { .. } | DataType::DecFloat { .. })
            | (Value::Binary(_), DataType::Binary { .. } | DataType::VarBinary { .. })
            | (Value::Char(_), DataType::Lob { kind: LobKind::Character, .. })
            | (Value::Binary(_), DataType::Lob { kind: LobKind::Binary, .. })
            | (Value::Lob(_), DataType::Lob { .. })
            | (Value::Structured(_), DataType::Structured(_))
            | (Value::Reference(_), DataType::Reference(_))
            | (Value::Bool(_), DataType::Boolean)
            | (Value::Datetime(_), DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. })
            | (Value::Interval(_), DataType::Interval(_))
            | (Value::Extension(_), DataType::Extension(_))
    )
}
