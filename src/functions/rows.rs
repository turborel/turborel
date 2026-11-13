//! The functions of rows [ISO/IEC 9075-2:2023 §6.16, §7.1], bound by the
//! binder as their syntax is read, not registered (RULES.md D68).

use super::ScalarFunction;
use crate::error::{Code, Result, fail};
use crate::value::{DataType, Value};

/// The names the calls of the functions here show.
const ROW: &str = "ROW";
const FIELD: &str = "FIELD";

/// [ISO/IEC 9075-2:2023 §7.1 GR3b] `<explicit row value constructor>`: a
/// row of its values.
pub struct RowOf(pub DataType);

impl ScalarFunction for RowOf {
    fn name(&self) -> &str {
        ROW
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        Ok(Value::Row(args.into()))
    }
}

/// [ISO/IEC 9075-2:2023 §6.16 GR2] `<field reference>`: the field at
/// position `.1`, from 0, of type `.0`; null of a null row.
pub struct FieldOf(pub DataType, pub usize);

impl ScalarFunction for FieldOf {
    fn name(&self) -> &str {
        FIELD
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args.first() {
            Some(Value::Row(fields)) => Ok(fields.get(self.1).cloned().unwrap_or(Value::Null)),
            None | Some(Value::Null) => Ok(Value::Null),
            Some(_) => fail(Code::Api, "[DESIGN] a field reference was given a value not a row"),
        }
    }
}
