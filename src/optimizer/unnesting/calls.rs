//! \[DESIGN\] The functions unnesting writes into the plans it makes. A
//! statement can name none of them, as none is registered.
//!
//! * `Single` checks, where a scalar subquery was evaluated, that its
//!   binding had one row at most [ISO/IEC 9075:1992 §7.11 GR1; X3.135-1986
//!   §5.11 GR1], so the error is raised for the rows that would have raised
//!   it and no others (RULES.md D313).
//! * `FirstValue` keeps the one value a binding's rows have of a column,
//!   null too, given as a row of it so no null is eliminated.
//! * `Trailing` counts the trailing spaces of a character string, so two
//!   bindings that compare equal but are different values are kept apart
//!   (RULES.md D319).
//! * `Assigned` stores a value in a type as an SQL-invoked function's
//!   parameters and result are [ISO/IEC 9075-2:2023 §9.18 GR4a ii, GR10a
//!   iii], for an inlined function (RULES.md D316).

use std::cmp::Ordering;

use crate::error::{Code, Result, Subject, fail};
use crate::functions::{Accumulator, AggregateFunction, ScalarFunction};
use crate::value::{DataType, Decimal, Spelled, Value, assign, compare};

/// The type `Trailing` gives, an INTEGER's, of nine digits.
const COUNTED: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };

/// The name EXPLAIN shows of `Single`.
pub(super) const SINGLE: &str = "SINGLE";
/// The name EXPLAIN shows of `FirstValue`.
pub(super) const FIRST_VALUE: &str = "SINGLE_VALUE";
/// The name EXPLAIN shows of `Trailing`.
pub(super) const TRAILING: &str = "TRAILING_SPACES";
/// The name EXPLAIN shows of `Assigned`.
pub(crate) const ASSIGNED: &str = "ASSIGN";

/// [ISO/IEC 9075:1992 §7.11 GR1] The message of a scalar subquery of
/// more than one row, as `exec/eval.rs` raises it.
pub(super) const SCALAR_ROWS: &str = "[ISO/IEC 9075:1992 §7.11 GR1] a <scalar subquery> or <row subquery> gives more than one row";
/// [X3.135-1986 §5.11 GR1] The message of a comparison's subquery of more
/// than one row, as `exec/eval.rs` raises it.
pub(super) const COMPARED_ROWS: &str = "[X3.135-1986 §5.11 GR1] the <subquery> of a comparison gives more than one value";

/// `SINGLE(count, value)` gives the value, unless the count of the binding's
/// rows is more than one, which fails with SQLCODE -811.
pub(super) struct Single(pub(super) &'static str);

impl ScalarFunction for Single {
    fn name(&self) -> &str {
        SINGLE
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        args.get(1).cloned().map_or_else(|| fail(Code::Api, "[DESIGN] SINGLE takes a count and a value"), Ok)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [count, value] = args else { return fail(Code::Api, "[DESIGN] SINGLE takes a count and a value") };
        let one = Value::Exact(Decimal::new(1, 0));
        if compare(count, &one) == Some(Ordering::Greater) {
            return fail(Code::Cardinality, self.0);
        }
        Ok(value.clone())
    }
}

/// `SINGLE_VALUE(ROW(column))` gives the column's value in the first row
/// of its group, null too; null of no row.
pub(super) struct FirstValue;

impl AggregateFunction for FirstValue {
    fn name(&self) -> &str {
        FIRST_VALUE
    }
    /// The type of the one field of its argument's row type.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        match arg {
            Some(DataType::Row(row)) => {
                row.fields.first().map(|f| f.data_type.clone()).map_or_else(|| fail(Code::Api, "[DESIGN] no field"), Ok)
            }
            Some(_) | None => fail(Code::Api, "[DESIGN] SINGLE_VALUE takes a row of one field"),
        }
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Kept(None))
    }
}

/// The first row's field, once given one.
struct Kept(Option<Value>);

impl Accumulator for Kept {
    fn update(&mut self, v: &Value) -> Result<()> {
        if self.0.is_none() {
            let field = if let Value::Row(fields) = v { fields.first().cloned().unwrap_or(Value::Null) } else { v.clone() };
            self.0 = Some(field);
        }
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(self.0.clone().unwrap_or(Value::Null))
    }
}

/// `TRAILING_SPACES(value)` gives, of a character string, how many
/// spaces end it; of any other value, null.
pub(super) struct Trailing;

impl ScalarFunction for Trailing {
    fn name(&self) -> &str {
        TRAILING
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(COUNTED)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Char(s)) = args.first() else { return Ok(Value::Null) };
        let spaces = s.len().saturating_sub(s.trim_end_matches(' ').len());
        Ok(Value::Exact(Decimal::new(i128::try_from(spaces).unwrap_or(i128::MAX), 0)))
    }
    fn raise_free(&self) -> bool {
        true
    }
}

/// `ASSIGN(value)` gives the value stored in `to` by the rule `rule`, an
/// error about `about` if it cannot be.
pub(crate) struct Assigned {
    /// The type stored in.
    pub(crate) to: DataType,
    /// The rule cited of a value it cannot be stored in.
    pub(crate) rule: &'static str,
    /// What an error of it is about, the parameter or the routine.
    pub(crate) about: Subject,
}

impl ScalarFunction for Assigned {
    fn name(&self) -> &str {
        ASSIGNED
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.to.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let value = args.first().cloned().unwrap_or(Value::Null);
        assign(value, &self.to, self.rule).map_err(|e| e.or_about(self.about.clone()))
    }
}
