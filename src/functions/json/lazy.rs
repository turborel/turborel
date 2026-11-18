//! [ISO/IEC 9075-2:2023 §9.48 GR4c iii, GR5d] The functions that let a
//! DEFAULT of JSON_VALUE be evaluated only when its behavior is taken.
//!
//! §9.48 evaluates `CAST (VE AS DT)` of a DEFAULT's `<value expression>`
//! VE only if the sequence is empty, of ON EMPTY, or an exception
//! condition was raised, of ON ERROR. A call's arguments are all
//! evaluated before it is called, so a DEFAULT that may raise an
//! exception is not an argument: the binder makes the operator a CASE
//! (`binder/json/lazy.rs`) of which behavior is taken (`JsonValueTaken`),
//! whose branches evaluate VE. A DEFAULT that cannot raise is still an
//! argument, evaluated every row, since no row could tell; so is one that
//! draws a sequence's value, which §6.14 draws of every row.

use super::{JsonCall, ValueSlot, arity};
use crate::error::Result;
use crate::functions::ScalarFunction;
use crate::json::{CommonSyntax, Taken, ValueBehavior, is_exception, json_value_taken};
use crate::params::ParameterType;
use crate::value::{DataType, Decimal, SMALLINT_PRECISION, Spelled, Value, cast_warned};

// [DESIGN] What `JsonValueTaken` gives of each behavior taken.
/// The value the operator's rules give, a NULL behavior's among them.
pub const TAKEN_VALUE: i128 = 0;
/// DEFAULT ON EMPTY.
pub const TAKEN_EMPTY: i128 = 1;
/// DEFAULT ON ERROR.
pub const TAKEN_ERROR: i128 = 2;

/// The SMALLINT `JsonValueTaken` returns.
const SMALLINT: DataType = DataType::Exact { precision: SMALLINT_PRECISION, scale: 0, spelled: Spelled::Smallint };

/// [§6.28 GR, §9.48] Which behavior a JSON_VALUE takes: `TAKEN_VALUE`,
/// `TAKEN_EMPTY` or `TAKEN_ERROR`, of its context item and PASSING
/// arguments, its arguments; a DEFAULT's value is none of them.
pub struct JsonValueTaken {
    /// What it shows as its name: the operator's, and that it is this.
    label: String,
    /// The operator: its path, arguments and clauses, ON EMPTY's and ON
    /// ERROR's each `ValueSlot::Default` if written so.
    call: JsonCall,
}

impl JsonValueTaken {
    /// Which behavior `call`, a JSON_VALUE, takes.
    pub fn new(call: JsonCall) -> JsonValueTaken {
        JsonValueTaken { label: format!("{} TAKEN", call.label), call }
    }
}

/// The behavior of `slot` without its DEFAULT's value, which
/// `json_value_taken` does not read.
fn unvalued(slot: ValueSlot) -> ValueBehavior {
    match slot {
        ValueSlot::Error => ValueBehavior::Error,
        ValueSlot::Null => ValueBehavior::Null,
        ValueSlot::Default => ValueBehavior::Default(Value::Null),
    }
}

impl ScalarFunction for JsonValueTaken {
    fn name(&self) -> &str {
        &self.label
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(SMALLINT)
    }
    /// As the operator types them, of no DEFAULT.
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        let arguments = self.call.passing.len();
        if position > arguments {
            return None;
        }
        self.call.parameter_type(position, args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (context, values) = args.split_first().ok_or_else(arity)?;
        let passing = self.call.passing_of(values)?;
        let common = CommonSyntax { context, path: &self.call.path, passing: &passing };
        let super::JsonOperation::Value { returning, on_empty, on_error } = &self.call.operation else { return Err(arity()) };
        let taken = json_value_taken(&common, returning, &unvalued(*on_empty), &unvalued(*on_error))?;
        let code = match taken {
            Taken::Value => TAKEN_VALUE,
            Taken::EmptyDefault => TAKEN_EMPTY,
            Taken::ErrorDefault => TAKEN_ERROR,
        };
        Ok(Value::Exact(Decimal::new(code, 0)))
    }
}

/// [§9.48 GR4c iii] Whether `CAST (VE AS DT)` of a DEFAULT ON EMPTY's
/// value raises an exception condition, which ON ERROR then replaces: of
/// its one argument, VE's value, BOOLEAN.
pub struct JsonCastFails {
    /// What it shows as its name.
    label: String,
    /// DT, the returned type.
    to: DataType,
}

impl JsonCastFails {
    /// Whether a value's cast to `to` raises an exception condition.
    pub fn new(to: DataType) -> JsonCastFails {
        JsonCastFails { label: format!("JSON_VALUE DEFAULT CAST FAILS {to}"), to }
    }
}

impl ScalarFunction for JsonCastFails {
    fn name(&self) -> &str {
        &self.label
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    /// # Errors
    ///
    /// The cast's error if it is no exception condition.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [value] = args else { return Err(arity()) };
        match cast_warned(value.clone(), &self.to) {
            Ok(_) => Ok(Value::Bool(false)),
            Err(error) => {
                if is_exception(&error) {
                    Ok(Value::Bool(true))
                } else {
                    Err(error)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
