//! [ISO/IEC 9075-2:2023 §6.37] JSON_SERIALIZE, `<JSON serialize>`: the
//! JSON text of a value of the JSON type, as a string (Feature T801).

use super::serialize_as;
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::params::ParameterType;
use crate::value::{DataType, Value};

/// The name of `<JSON serialize>`.
pub const JSON_SERIALIZE: &str = "JSON_SERIALIZE";

/// A call of `<JSON serialize>`, of the type of its `<JSON output
/// clause>`, and of one argument, its `<JSON value expression>`.
pub struct JsonSerialize {
    /// What the call shows as its name.
    label: String,
    /// The `<data type>` of its explicit or implicit output clause.
    returning: DataType,
}

impl JsonSerialize {
    /// JSON_SERIALIZE RETURNING `returning`, a character string type.
    pub fn new(returning: DataType) -> JsonSerialize {
        JsonSerialize { label: format!("{JSON_SERIALIZE}[? RETURNING {returning}]"), returning }
    }
}

impl ScalarFunction for JsonSerialize {
    fn name(&self) -> &str {
        &self.label
    }

    /// \[SR4\] The type of its output clause.
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.returning.clone())
    }

    /// \[EXTENSION\] [§20.7 GR5a] A `?` is of the JSON type [§6.38 SR1].
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Of(DataType::Json))
    }

    /// \[GR2, GR3\] The null value of the null value; else the JSON text
    /// of the item [§9.43].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22032 if the item is or holds a datetime, of which no JSON
    /// text is [§9.43 GR2a ii]; 22001 if the text is longer than the type
    /// [§9.43 GR2a iii]; SQLCODE -916 if the value is damaged.
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args {
            [Value::Null] => Ok(Value::Null),
            [Value::Json(j)] => serialize_as(&j.item()?, &self.returning),
            _ => Err(SqlError::new(Code::Api, "[DESIGN] JSON_SERIALIZE is called with other arguments than a value of the JSON type")),
        }
    }
}
