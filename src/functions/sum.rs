//! The accumulators of SUM and AVG [ISO/IEC 9075:1992 §6.5 GR2b ii, iv].
//! \[DESIGN\] RULES.md D15: whether one raises does not depend on the
//! order its values come in, which is the plan's. GR2b iv judges "the sum
//! of the values in TXA", so the sum so far is kept wider than its result
//! (`Total`) and judged once, as it is finished: exact numbers in the 128
//! bits of `Decimal::units`, not `MAX_EXACT_PRECISION` digits; approximate
//! ones scaled (`floats::Wide`); intervals in 128 bits of units, not their
//! leading field's digits; DECFLOAT ones each sum rounded to 34 digits
//! but of any exponent. GR2b ii sets no range for AVG's sum, so AVG
//! judges its mean alone.

use super::floats::{self, Wide};
use super::{Accumulator, ints};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::storage::{Floats, Ints};
use crate::value::codec::{put_value, value};
use crate::value::interval::{Interval, MAX_LEADING_PRECISION};
use crate::value::{ArithOp, DataType, DecFloat, Decimal, Value, arith};

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
    /// [ISO/IEC 9075-2:2023 §6.30 GR7] Of DECFLOAT numbers, each sum of
    /// 34 digits, of any adjusted exponent.
    DecFloat(DecFloat),
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
            (
                Total::Empty | Total::Exact(_) | Total::Approx(_) | Total::Interval(_) | Total::DecFloat(_) | Total::Other(_),
                Value::Null,
            ) => self.clone(),
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
            (Total::Empty, Value::DecFloat(d)) => Total::DecFloat(*d),
            (Total::DecFloat(s), Value::DecFloat(d)) => Total::DecFloat(s.plus_unbounded(*d).map_or_else(beyond, Ok)?),
            (
                Total::Empty,
                Value::Char(_)
                | Value::Binary(_)
                | Value::Bool(_)
                | Value::Datetime(_)
                | Value::Array(_)
                | Value::Row(_)
                | Value::Extension(_),
            ) => Total::Other(v.clone()),
            (Total::Exact(_) | Total::Approx(_) | Total::Interval(_) | Total::DecFloat(_) | Total::Other(_), _) => {
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
            Total::DecFloat(d) => Value::DecFloat(*d),
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
            Total::Approx(_) | Total::Interval(_) | Total::DecFloat(_) | Total::Other(_) => None,
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
            Total::Exact(_) | Total::Interval(_) | Total::DecFloat(_) | Total::Other(_) => return self.update_many(floats.values(), rows),
        };
        if floats::sum_into(&mut sum, floats, rows) {
            self.total = Total::Approx(sum);
        }
        Ok(())
    }
    /// \[DESIGN\] RULES.md D53: of an exact or interval result, the total
    /// so far, unjudged, as the value codec writes it, null of none; none
    /// of an approximate or a DECFLOAT one, whose total depends on the
    /// order it is added in, and a DECFLOAT's may be out of range.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        if matches!(self.result, DataType::Approximate { .. } | DataType::DecFloat { .. }) {
            return false;
        }
        put_value(out, &self.total.value());
        true
    }
    /// The total of `state` added as `update` adds a value: unjudged.
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let mut reader = Reader::new(state, "a state of SUM");
        let total = value(&mut reader)?;
        reader.finish()?;
        self.update(&total)
    }
    /// [ISO/IEC 9075:1992 §6.5 GR2b iv] The sum, judged: of exact numbers,
    /// within `MAX_EXACT_PRECISION` digits; of approximate ones, finite;
    /// of intervals, in the argument's fields [SR9e]. Out of range, of
    /// any type, it is "data exception-numeric value out of range", 22003.
    fn finish(&self) -> Result<Value> {
        match &self.total {
            Total::Exact(d) => Ok(Value::Exact(d.checked()?)),
            Total::Approx(w) => finite(w.total()),
            Total::DecFloat(d) => in_range(d.within()),
            Total::Interval(i) => in_fields(Value::Interval(*i), self.result.clone()).or_else(|_| {
                fail_with(
                    Code::Data,
                    SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                    format!("[ISO/IEC 9075:1992 §6.5 GR2b iv] the sum {i} is not within the range of {}", self.result),
                )
            }),
            Total::Empty | Total::Other(_) => in_fields(self.total.value(), self.result.clone()),
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

/// The DECFLOAT sum or mean `d`, if it is in range.
///
/// # Errors
///
/// SQLSTATE 22003 if it is not [ISO/IEC 9075:1992 §6.5 GR2b iv].
fn in_range(d: Option<DecFloat>) -> Result<Value> {
    d.map(Value::DecFloat).map_or_else(
        || {
            fail_with(
                Code::Data,
                SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                "[ISO/IEC 9075:1992 §6.5 GR2b iv] the DECFLOAT sum is outside the range of DECFLOAT",
            )
        },
        Ok,
    )
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
        AvgAcc { sum: SumAcc::new(result.clone()), count: 0, result }
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
    /// \[DESIGN\] RULES.md D53: its count, 16 bytes, then its sum's state;
    /// none if its sum has none.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        let start = out.len();
        out.extend_from_slice(&self.count.to_le_bytes());
        let written = self.sum.write_state(out);
        if !written {
            out.truncate(start);
        }
        written
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let Some((count, sum)) = state.split_first_chunk::<16>() else {
            return fail(Code::Corrupt, "[DESIGN] a state of AVG is short");
        };
        self.count = self.count.saturating_add(i128::from_le_bytes(*count));
        self.sum.merge(sum)
    }
    /// [ISO/IEC 9075:1992 §6.5 GR2b ii] The mean, judged alone: the sum
    /// divided by the count, of an exact result at its scale.
    fn finish(&self) -> Result<Value> {
        let count = Value::Exact(Decimal::new(self.count, 0));
        match (&self.sum.total, self.result.clone()) {
            (Total::Empty, _) => Ok(Value::Null),
            (Total::Exact(d), DataType::Exact { scale, .. }) => Ok(Value::Exact(d.quotient_at(Decimal::new(self.count, 0), scale)?)),
            (Total::Approx(w), _) => finite(w.over(Decimal::new(self.count, 0).to_f64())),
            (Total::DecFloat(d), _) => {
                in_range(DecFloat::new(self.count, 0, crate::value::MAX_DECFLOAT_PRECISION).and_then(|n| d.over_unbounded(n)?.within()))
            }
            // An interval's mean is an interval in its fields.
            (sum, _) => in_fields(arith(ArithOp::Div, &sum.value(), &count)?, self.result.clone()),
        }
    }
}

// Unit tests: totals judged once, whatever the order (RULES.md R-TEST-2
// robustness).
#[cfg(test)]
mod tests;
