//! The accumulators of SUM and AVG [ISO/IEC 9075:1992 §6.5 GR2b ii, iv].
//! \[DESIGN\] RULES.md D15: whether one raises does not depend on the
//! order its values come in, which is the plan's. GR2b iv judges "the sum
//! of the values in TXA", so the sum so far is kept wider than its result
//! (`Total`) and judged once, as it is finished: exact numbers in the 128
//! bits of `Decimal::units`, not `MAX_EXACT_PRECISION` digits; approximate
//! ones scaled (`floats::Wide`); intervals in 128 bits of units, not their
//! leading field's digits. GR2b ii sets no range for AVG's sum, so AVG
//! judges its mean alone. Split from functions.rs (RULES.md R-SIZE-2).

use super::floats::{self, Wide};
use super::{Accumulator, ints};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::storage::{Floats, Ints};
use crate::value::interval::{Interval, MAX_LEADING_PRECISION};
use crate::value::{ArithOp, DataType, Decimal, Value, arith};

/// A sum so far.
#[derive(Debug, Clone)]
enum Total {
    /// Of no value yet.
    Empty,
    /// Of exact numbers, of any number of digits 128 bits hold.
    Exact(Decimal),
    /// Of approximate numbers, exact ones among them as `f64`s.
    Approx(Wide),
    /// Of intervals, of any units 128 bits hold, in the fields of all.
    Interval(Interval),
    /// Of values of another type, as `arith` adds them, which raises of
    /// what it cannot add.
    Other(Value),
}

/// SQLCODE -900 (RULES.md D18): 128 bits cannot hold a sum on the way,
/// a limit of this implementation (CONFORMANCE.md, Limits). A sum of
/// exact numbers reaches it only past some 10^20 values of 18 digits, of
/// intervals past some 10^18 of 9 digits of days.
fn beyond<T>() -> Result<T> {
    fail(Code::Unsupported, "[DESIGN] a sum on the way needs more than the 128 bits this implementation sums in")
}

impl Total {
    /// The sum with `v` added; a null adds nothing.
    fn plus(&self, v: &Value) -> Result<Total> {
        Ok(match (self, v) {
            (Total::Empty | Total::Exact(_) | Total::Approx(_) | Total::Interval(_) | Total::Other(_), Value::Null) => self.clone(),
            (Total::Empty, Value::Exact(d)) => Total::Exact(*d),
            (Total::Exact(s), Value::Exact(d)) => Total::Exact(s.wide_add(*d).map_or_else(beyond, Ok)?),
            (Total::Empty, Value::Approx(f)) => Total::Approx(Wide::EMPTY.with(*f)),
            (Total::Exact(s), Value::Approx(f)) => Total::Approx(Wide::EMPTY.with(s.to_f64()).with(*f)),
            (Total::Approx(w), Value::Approx(f)) => Total::Approx(w.with(*f)),
            (Total::Approx(w), Value::Exact(d)) => Total::Approx(w.with(d.to_f64())),
            (Total::Empty, Value::Interval(i)) => Total::Interval(*i),
            (Total::Interval(s), Value::Interval(i)) => {
                let units = s.units.checked_add(i.units).map_or_else(beyond, Ok)?;
                Total::Interval(Interval { units, qualifier: s.qualifier.joined(&i.qualifier, MAX_LEADING_PRECISION) })
            }
            (Total::Empty, Value::Char(_) | Value::Bits(_) | Value::Datetime(_) | Value::Extension(_)) => Total::Other(v.clone()),
            (Total::Exact(_) | Total::Approx(_) | Total::Interval(_) | Total::Other(_), _) => {
                Total::Other(arith(ArithOp::Add, &self.value(), v)?)
            }
        })
    }

    /// The sum as a value, unjudged: null of none.
    fn value(&self) -> Value {
        match self {
            Total::Empty => Value::Null,
            Total::Exact(d) => Value::Exact(*d),
            Total::Approx(w) => Value::Approx(w.total()),
            Total::Interval(i) => Value::Interval(*i),
            Total::Other(v) => v.clone(),
        }
    }
}

/// SUM's accumulator: the sum so far, and the type of the result.
pub(super) struct SumAcc {
    total: Total,
    result: DataType,
}

impl SumAcc {
    /// No value yet, of a result of type `result`.
    pub(super) fn new(result: DataType) -> SumAcc {
        SumAcc { total: Total::Empty, result }
    }
}

impl Accumulator for SumAcc {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.total = self.total.plus(v)?;
        Ok(())
    }
    /// Exact numbers summed as decimals in place, nothing cloned; others
    /// by `update`.
    fn update_many(&mut self, values: &[Value], rows: Option<&[usize]>) -> Result<()> {
        let mut add = |v: &Value| match (&mut self.total, v) {
            (Total::Exact(sum), Value::Exact(d)) => {
                *sum = sum.wide_add(*d).map_or_else(beyond, Ok)?;
                Ok(())
            }
            (_, _) => self.update(v),
        };
        match rows {
            Some(rows) => rows.iter().filter_map(|&r| values.get(r)).try_for_each(&mut add),
            None => values.iter().try_for_each(&mut add),
        }
    }
    /// The integers added to the sum as integers, of one scale; else, or
    /// if 128 bits cannot hold the sum, as decimals, which raise of it.
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[usize]>) -> Result<()> {
        let start = match &self.total {
            Total::Empty => Some(0),
            Total::Exact(d) => (d.scale == ints.scale).then_some(d.units),
            Total::Approx(_) | Total::Interval(_) | Total::Other(_) => None,
        };
        match start.and_then(|s| ints::sum_from(s, ints, rows)) {
            Some((total, true)) => self.total = Total::Exact(Decimal::new(total, ints.scale)),
            Some((_, false)) => {}
            None => self.update_many(ints.values(), rows)?,
        }
        Ok(())
    }
    /// The numbers added as `f64`s, in order, as `update` adds them.
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[usize]>) -> Result<()> {
        let mut sum = match &self.total {
            Total::Empty => Wide::EMPTY,
            Total::Approx(w) => *w,
            Total::Exact(_) | Total::Interval(_) | Total::Other(_) => return self.update_many(floats.values(), rows),
        };
        if floats::sum_into(&mut sum, floats, rows) {
            self.total = Total::Approx(sum);
        }
        Ok(())
    }
    /// [ISO/IEC 9075:1992 §6.5 GR2b iv] The sum, judged: of exact numbers,
    /// within `MAX_EXACT_PRECISION` digits; of approximate ones, finite;
    /// of intervals, in the argument's fields [SR9e]. Out of range, of
    /// any type, it is "data exception-numeric value out of range", 22003.
    fn finish(&self) -> Result<Value> {
        match &self.total {
            Total::Exact(d) => Ok(Value::Exact(d.checked()?)),
            Total::Approx(w) => finite(w.total()),
            Total::Interval(i) => in_fields(Value::Interval(*i), self.result).or_else(|_| {
                fail_with(
                    Code::Data,
                    SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                    format!("[ISO/IEC 9075:1992 §6.5 GR2b iv] the sum {i} is not within the range of {}", self.result),
                )
            }),
            Total::Empty | Total::Other(_) => in_fields(self.total.value(), self.result),
        }
    }
}

/// The approximate sum or mean `f`.
///
/// # Errors
///
/// SQLSTATE 22003 if it is out of the range of `f64` [ISO/IEC 9075:1992
/// §6.5 GR2b iv], as `arith` raises of a sum.
fn finite(f: f64) -> Result<Value> {
    if f.is_finite() {
        Ok(Value::Approx(f))
    } else {
        fail_with(
            Code::Data,
            SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
            "[ISO/IEC 9075:1992 §6.5 GR2b iv] the approximate sum is outside the range of this implementation's approximate numbers",
        )
    }
}

/// `v`, if an interval and `t` an interval type, in `t`'s fields.
///
/// # Errors
///
/// SQLSTATE 22015 if they cannot hold it.
fn in_fields(v: Value, t: DataType) -> Result<Value> {
    match (&v, t) {
        (Value::Interval(i), DataType::Interval(q)) => crate::value::types::interval::fitted(i, q),
        _ => Ok(v),
    }
}

/// AVG's accumulator: the sum so far, how many values, and the type of
/// the result.
pub(super) struct AvgAcc {
    sum: SumAcc,
    count: i128,
    result: DataType,
}

impl AvgAcc {
    /// No value yet, of a result of type `result`.
    pub(super) fn new(result: DataType) -> AvgAcc {
        AvgAcc { sum: SumAcc::new(result), count: 0, result }
    }
}

impl Accumulator for AvgAcc {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.count = self.count.saturating_add(1);
        self.sum.update(v)
    }
    fn update_many(&mut self, values: &[Value], rows: Option<&[usize]>) -> Result<()> {
        let counted = match rows {
            Some(rows) => rows.iter().filter_map(|&r| values.get(r)).filter(|v| !v.is_null()).count(),
            None => values.iter().filter(|v| !v.is_null()).count(),
        };
        self.count = self.count.saturating_add(i128::try_from(counted).unwrap_or(i128::MAX));
        self.sum.update_many(values, rows)
    }
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[usize]>) -> Result<()> {
        self.count = self.count.saturating_add(i128::try_from(ints::count(ints, rows)).unwrap_or(i128::MAX));
        self.sum.update_ints(ints, rows)
    }
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[usize]>) -> Result<()> {
        self.count = self.count.saturating_add(i128::try_from(floats::count(floats, rows)).unwrap_or(i128::MAX));
        self.sum.update_floats(floats, rows)
    }
    /// [ISO/IEC 9075:1992 §6.5 GR2b ii] The mean, judged alone: the sum
    /// divided by the count, of an exact result at its scale.
    fn finish(&self) -> Result<Value> {
        let count = Value::Exact(Decimal::new(self.count, 0));
        match (&self.sum.total, self.result) {
            (Total::Empty, _) => Ok(Value::Null),
            (Total::Exact(d), DataType::Exact { scale, .. }) => Ok(Value::Exact(d.quotient_at(Decimal::new(self.count, 0), scale)?)),
            (Total::Approx(w), _) => finite(w.over(Decimal::new(self.count, 0).to_f64())),
            // An interval's mean is an interval in its fields.
            (sum, _) => in_fields(arith(ArithOp::Div, &sum.value(), &count)?, self.result),
        }
    }
}

// Unit tests: totals judged once, whatever the order (RULES.md R-TEST-2
// robustness).
#[cfg(test)]
mod tests;
