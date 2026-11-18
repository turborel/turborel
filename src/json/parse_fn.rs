//! [ISO/IEC 9075-2:2023 §6.40] JSON ( ), `<JSON parse>`: the value of the
//! JSON type a string's JSON text is (Features T801, T802).

use super::Keys;
use super::query::json_input;
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::params::ParameterType;
use crate::value::json::Json;
use crate::value::{DataType, Value};

/// The name of `<JSON parse>`, JSON, which is also the JSON type's.
pub const JSON_PARSE: &str = "JSON";

/// The Syntax Rule an operand of another type than a string's breaks.
const PARSE_RULE: &str = "[ISO/IEC 9075-2:2023 §6.40 F] the operand of JSON ( ) is a <string value expression>";

/// A call of `<JSON parse>`, of its key uniqueness constraint, explicit
/// or implicit, and of one argument, its string.
pub struct JsonParse {
    /// What the call shows as its name.
    label: String,
    /// The `<JSON key uniqueness constraint>` \[SR4\].
    keys: Keys,
}

impl JsonParse {
    /// JSON ( ) of the uniqueness `keys`.
    pub fn new(keys: Keys) -> JsonParse {
        let words = match keys {
            Keys::Any => "WITHOUT UNIQUE KEYS",
            Keys::Unique => "WITH UNIQUE KEYS",
        };
        JsonParse { label: format!("{JSON_PARSE}[? {words}]"), keys }
    }
}

impl ScalarFunction for JsonParse {
    fn name(&self) -> &str {
        &self.label
    }

    /// \[SR1\] JSON. The binder checks the operand's type.
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Json)
    }

    /// \[EXTENSION\] [§20.7 GR5a] No rule types a `?` here: JSON text, a
    /// CHARACTER VARYING.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Varying)
    }

    /// \[GR2, GR3\] The null value of the null value; else the item the
    /// text parses to [§9.42], a value of JSON given back as it is.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22032 if the text is no JSON text, 22037 if two members
    /// of an object have one key WITH UNIQUE KEYS [§9.42 GR3a iii];
    /// SQLCODE -103 of an operand of no string type; -900 of a binary
    /// string, or a text beyond turborel's limits.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [input] = args else {
            return Err(SqlError::new(Code::Api, "[DESIGN] JSON ( ) is called with other arguments than one"));
        };
        if let Value::Char(text) = input {
            return Json::parse(text, self.keys).map(Value::Json);
        }
        let item = json_input(input, PARSE_RULE, self.keys)?;
        item.map_or(Ok(Value::Null), |item| Json::of_item(&item).map(Value::Json))
    }
}
