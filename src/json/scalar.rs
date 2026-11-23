//! [ISO/IEC 9075-2:2023 §6.41] JSON_SCALAR, `<JSON scalar>`: the value of
//! the JSON type that is an SQL/JSON scalar of an SQL value (Feature
//! T801).

use super::{Item, Scalar};
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::functions::lobs::like::held_text;
use crate::value::json::Json;
use crate::value::lob::LobKind;
use crate::value::{DataType, Value};

/// The name of `<JSON scalar>`.
pub const JSON_SCALAR: &str = "JSON_SCALAR";

/// What a character large object read whole as a scalar is, for the error
/// of one too long.
const SCALAR_TEXT: &str = "the operand of JSON_SCALAR";

/// A call of `<JSON scalar>`, of one argument.
pub struct JsonScalar;

/// [§6.41 SR2] Whether JSON_SCALAR takes an operand of type `t`: a
/// character string, a number, a truth value or a datetime.
pub fn scalar_operand(t: &DataType) -> bool {
    t.is_character() || t.is_numeric() || t.is_datetime() || matches!(t, DataType::Boolean)
}

/// The error of an operand of no type JSON_SCALAR takes.
fn unscalar(v: &Value) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] JSON_SCALAR is called with {v}, of no type its binder takes"))
}

impl ScalarFunction for JsonScalar {
    fn name(&self) -> &str {
        JSON_SCALAR
    }

    /// \[SR1\] JSON. The binder checks the operand's type \[SR2\].
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Json)
    }

    /// \[GR2a\] The null value of the null value, no SQL/JSON null
    /// \[NOTE 200\]; \[GR2b\] else the scalar of the value, a character
    /// large object's read whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of a character large object longer than one chunk.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [value] = args else {
            return Err(SqlError::new(Code::Api, "[DESIGN] JSON_SCALAR is called with other arguments than one"));
        };
        let value = match value {
            Value::Null => return Ok(Value::Null),
            Value::Lob(lob) => match lob.kind() {
                LobKind::Character => Value::Char(held_text(lob, SCALAR_TEXT)?.into()),
                LobKind::Binary => return Err(unscalar(value)),
            },
            Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Binary(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Xml(_)
            | Value::Extension(_) => value.clone(),
        };
        let scalar = Scalar::new(value.clone()).ok_or_else(|| unscalar(&value))?;
        Json::of_item(&Item::Scalar(scalar)).map(Value::Json)
    }
}
