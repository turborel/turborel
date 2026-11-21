//! \[DESIGN\] (RULES.md D662) A set function of values each standing for
//! a number of rows alike, as eager aggregation leaves them
//! (`physical/eager.rs`): a row of a grouping made below a join stands
//! for every row of its group, so a value the join pairs with it is a
//! value of that many rows of the join. COUNT, and SUM and AVG of exact
//! numbers, take a value `v` of `n` rows as the state of `n` values `v`
//! (RULES.md D53's states), merged: COUNT's count `n`, SUM's total `v`
//! times `n`, AVG's count `n` and that total. The total is a 128-bit
//! multiply, past which it raises SQLCODE -900 as a sum on the way does
//! (`functions/sum.rs`); the result is judged once, as the function's own.
//!
//! The value and its count come as a row of the two (`Paired`), null if
//! the value is, so a null is taken out of the function, and its warning
//! raised, as it would be of every row it stands for.

use super::super::{Accumulator, AggregateFunction, AggregateRef, ColumnarKind, ScalarFunction};
use crate::error::{Code, Result, fail};
use crate::value::codec::put_value;
use crate::value::{DataType, Decimal, Value};

/// The name a scaled function shows as, before the function's own.
pub(crate) const SCALED: &str = "SCALED";

/// The name of the function pairing a value with its count.
const PAIRED: &str = "PAIRED";

/// The set function `func`, COUNT, SUM or AVG as `kind` says, of result
/// type `result`, of values each of a number of rows (as the module
/// says).
pub(crate) struct Scaled {
    /// The set function.
    pub(crate) func: AggregateRef,
    /// Which built-in it is.
    pub(crate) kind: ColumnarKind,
    /// Its result type.
    pub(crate) result: DataType,
}

impl AggregateFunction for Scaled {
    fn name(&self) -> &str {
        SCALED
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(ScaledAcc { acc: self.func.0.accumulator(self.result.clone()), kind: self.kind })
    }
}

/// The function's accumulator, and which built-in it is.
struct ScaledAcc {
    acc: Box<dyn Accumulator>,
    kind: ColumnarKind,
}

impl Accumulator for ScaledAcc {
    /// The state of `n` values `v`, of the row of the two, merged.
    fn update(&mut self, v: &Value) -> Result<()> {
        let (value, rows) = pair_of(v)?;
        let mut state = Vec::new();
        match self.kind {
            ColumnarKind::Count => state.extend_from_slice(&rows.to_le_bytes()),
            ColumnarKind::Sum => put_value(&mut state, &times(value, rows)?),
            ColumnarKind::Avg => {
                state.extend_from_slice(&rows.to_le_bytes());
                put_value(&mut state, &times(value, rows)?);
            }
            ColumnarKind::Min | ColumnarKind::Max => return fail(Code::Corrupt, "[DESIGN] an extreme is not scaled"),
        }
        self.acc.merge(&state)
    }
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        self.acc.write_state(out)
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        self.acc.merge(state)
    }
    fn finish(&self) -> Result<Value> {
        self.acc.finish()
    }
}

/// The value and the count of rows of `v`, a row `Paired` makes.
fn pair_of(v: &Value) -> Result<(&Value, i128)> {
    if let Value::Row(fields) = v
        && let [value, Value::Exact(rows)] = &**fields
        && rows.scale == 0
    {
        return Ok((value, rows.units));
    }
    fail(Code::Corrupt, "[DESIGN] a scaled set function was given no value and count")
}

/// `v`, an exact number, times `rows`: SQLCODE -900 past 128 bits.
fn times(v: &Value, rows: i128) -> Result<Value> {
    let Value::Exact(d) = v else { return fail(Code::Corrupt, "[DESIGN] a scaled sum of a value not an exact number") };
    match d.units.checked_mul(rows) {
        Some(units) => Ok(Value::Exact(Decimal::new(units, d.scale))),
        None => fail(Code::Unsupported, "[DESIGN] a sum on the way needs more than the 128 bits this implementation sums in"),
    }
}

/// A value and the count of rows it stands for, as a row of the two; null
/// if the value is.
pub(crate) struct Paired(pub(crate) DataType);

impl ScalarFunction for Paired {
    fn name(&self) -> &str {
        PAIRED
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args {
            [Value::Null, _] => Ok(Value::Null),
            [value, rows] => Ok(Value::Row(vec![value.clone(), rows.clone()].into())),
            _ => fail(Code::Api, "[DESIGN] a value paired with its count takes two arguments"),
        }
    }
}

// Unit tests: a value of n rows is n values (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
