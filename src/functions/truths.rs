//! EVERY, ANY and SOME, the set functions of truth values [ISO/IEC
//! 9075-2:2023 §10.9]: whether each value of a group is True, or whether
//! one is.

use super::{Accumulator, AggregateFunction};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};
use crate::lexer::kw;
use crate::value::codec::{put_value, value};
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §10.9] EVERY's name, no key word of SQL-92.
pub const EVERY: &str = "EVERY";

/// EVERY, or ANY and SOME, which are one function under two names.
pub(super) struct Truths {
    /// Whether it is EVERY.
    pub(super) every: bool,
    /// The name it is called by.
    pub(super) name: &'static str,
}

impl Truths {
    /// EVERY.
    pub(super) const EVERY: Truths = Truths { every: true, name: EVERY };
    /// ANY.
    pub(super) const ANY: Truths = Truths { every: false, name: kw::ANY };
    /// SOME, ANY by another name.
    pub(super) const SOME: Truths = Truths { every: false, name: kw::SOME };
}

impl AggregateFunction for Truths {
    fn name(&self) -> &str {
        self.name
    }

    /// [ISO/IEC 9075-2:2023 §10.9 SR7e] Its argument is a BOOLEAN, and so
    /// is its result.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        match arg {
            Some(DataType::Boolean) => Ok(DataType::Boolean),
            Some(_) | None => Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §10.9 SR7e] the argument of {} shall be of type BOOLEAN", self.name),
            )),
        }
    }

    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        Box::new(TruthsAcc { every: self.every, so_far: None })
    }
}

/// The result of the values so far, none before the first.
struct TruthsAcc {
    every: bool,
    so_far: Option<bool>,
}

impl Accumulator for TruthsAcc {
    /// [ISO/IEC 9075-2:2023 §10.9 GR7d vii, viii] Of EVERY, False once one
    /// value is False; of ANY, True once one is True.
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Bool(b) = *v else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {v} is not a truth value")));
        };
        let was = self.so_far.unwrap_or(self.every);
        self.so_far = Some(if self.every { was && b } else { was || b });
        Ok(())
    }

    /// The result so far, null if none, as `codec::put_value` writes it.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        put_value(out, &self.so_far.map_or(Value::Null, Value::Bool));
        true
    }

    /// The result of the state taken in as a value.
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let mut reader = Reader::new(state, "a state of EVERY or ANY");
        let so_far = value(&mut reader)?;
        reader.finish()?;
        if so_far.is_null() { Ok(()) } else { self.update(&so_far) }
    }

    /// [ISO/IEC 9075-2:2023 §10.9 GR7d ii] Null of no values.
    fn finish(&self) -> Result<Value> {
        Ok(self.so_far.map_or(Value::Null, Value::Bool))
    }
}

// Unit tests: each value given, states merged (RULES.md R-TEST-2).
#[cfg(test)]
mod tests;
