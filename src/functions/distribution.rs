//! The inverse distribution functions [ISO/IEC 9075-2:2023 §10.9]:
//! PERCENTILE_CONT and PERCENTILE_DISC, of a group's values not null,
//! sorted, the value at a fraction of the way through them \[GR10\].
//!
//! Their values are sorted by their grouping (`exec/group/distribution.rs`)
//! or window (`exec/window/distribution.rs`), which may spill them, so no
//! accumulator holds them. This module finds which of the sorted values a
//! fraction picks, and makes the result of them.

use std::cmp::Ordering;

use super::{Accumulator, AggregateFunction};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::{ArithOp, DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value, arith, cast, compare, pow10};

#[cfg(test)]
mod tests;

/// [ISO/IEC 9075-2:2023 §10.9] PERCENTILE_CONT, no key word of SQL-92.
pub const PERCENTILE_CONT: &str = "PERCENTILE_CONT";
/// [ISO/IEC 9075-2:2023 §10.9] PERCENTILE_DISC, no key word of SQL-92.
pub const PERCENTILE_DISC: &str = "PERCENTILE_DISC";

/// \[DESIGN\] The decimal places a DECFLOAT fraction is taken to, as rows
/// are counted by it: those of an exact number of one digit before them.
const DECFLOAT_FRACTION_SCALE: u32 = MAX_EXACT_PRECISION - 1;

/// The exact type a DECFLOAT fraction is cast to, of
/// `DECFLOAT_FRACTION_SCALE` places, and \[EXTENSION\] that of a `?`
/// fraction.
pub const FRACTION: DataType =
    DataType::Exact { precision: MAX_EXACT_PRECISION, scale: DECFLOAT_FRACTION_SCALE, spelled: Spelled::Numeric };

/// [ISO/IEC 9075-2:2023 §10.9 SR12, GR10] A call of an inverse
/// distribution function: which, and of what result type \[SR12e\].
pub struct InverseDistribution {
    /// PERCENTILE_CONT rather than PERCENTILE_DISC.
    pub continuous: bool,
    /// The type of its result.
    pub result: DataType,
}

impl AggregateFunction for InverseDistribution {
    fn name(&self) -> &str {
        if self.continuous { PERCENTILE_CONT } else { PERCENTILE_DISC }
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Unfed)
    }
}

/// The accumulator of an inverse distribution function, which is never
/// fed, as its grouping or window sorts its values and picks of them.
struct Unfed;

impl Accumulator for Unfed {
    fn update(&mut self, _v: &Value) -> Result<()> {
        Err(unfed())
    }
    fn finish(&self) -> Result<Value> {
        Err(unfed())
    }
}

fn unfed() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] an inverse distribution function's values are sorted by its grouping, not fed to it")
}

/// Which of a group's sorted values a fraction picks, each counted from 0:
/// of PERCENTILE_CONT, ROW0 and ROW1 and FACTOR, how far from the first
/// to the second; of PERCENTILE_DISC, one value twice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picked {
    /// ROW0, or PERCENTILE_DISC's row.
    pub low: u64,
    /// ROW1, or PERCENTILE_DISC's row.
    pub high: u64,
    /// FACTOR, NVE·(N−1) − ROW0; 0 of PERCENTILE_DISC.
    pub factor: f64,
}

/// [ISO/IEC 9075-2:2023 §10.9 GR10] Which of `n` sorted values the
/// fraction NVE `fraction` picks: none of a null fraction or of no values,
/// whose result is null. Of PERCENTILE_CONT, ROW0 = ⌊NVE·(N−1)⌋ and ROW1
/// = ⌈NVE·(N−1)⌉. Of PERCENTILE_DISC, the first value whose CUME_DIST,
/// the share of the values not after it, is at least NVE: the ⌈NVE·N⌉th,
/// the first if NVE is 0, as its peers after it share its CUME_DIST.
/// \[DESIGN\] Of an exact fraction the rows are found exactly, so a share
/// equal to NVE is at least it; of an approximate one in approximate
/// arithmetic, as NVE·(N−1) is computed.
///
/// # Errors
///
/// SQLSTATE 22003 if the fraction is below 0 or above 1 \[GR10c\], even
/// of no values.
pub fn picked(continuous: bool, fraction: &Value, n: u64) -> Result<Option<Picked>> {
    if fraction.is_null() {
        return Ok(None);
    }
    let zero = Value::Exact(Decimal::new(0, 0));
    let one = Value::Exact(Decimal::new(1, 0));
    let within = compare(fraction, &zero).is_some_and(Ordering::is_ge) && compare(fraction, &one).is_some_and(Ordering::is_le);
    if !within {
        return fail_with(
            Code::Data,
            SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
            format!("[ISO/IEC 9075-2:2023 §10.9 GR10c] the fraction of an inverse distribution function is from 0 to 1, not {fraction}"),
        );
    }
    let Some(last) = n.checked_sub(1) else { return Ok(None) };
    let (whole, part) = product(fraction, if continuous { last } else { n })?;
    if continuous {
        let high = if part > 0.0_f64 { whole.saturating_add(1) } else { whole };
        return Ok(Some(Picked { low: whole.min(last), high: high.min(last), factor: part }));
    }
    let ceiling = if part > 0.0_f64 { whole.saturating_add(1) } else { whole };
    let row = ceiling.saturating_sub(1).min(last);
    Ok(Some(Picked { low: row, high: row, factor: 0.0_f64 }))
}

/// The fraction `fraction`, from 0 to 1, times `count`: its whole part,
/// and what is left, as an approximate number. Exactly of an exact or
/// DECFLOAT fraction, the latter taken to `DECFLOAT_FRACTION_SCALE`
/// places; else in approximate arithmetic.
fn product(fraction: &Value, count: u64) -> Result<(u64, f64)> {
    let exact = match fraction {
        Value::Exact(d) => *d,
        Value::DecFloat(_) => {
            let Value::Exact(d) = cast(fraction.clone(), &FRACTION)? else { return Err(not_a_fraction(fraction)) };
            d
        }
        Value::Approx(x) => {
            let times = x * Decimal::new(i128::from(count), 0).to_f64();
            let floor = times.floor();
            let whole = format!("{floor:.0}").parse::<u64>().map_err(|_| not_a_fraction(fraction))?;
            return Ok((whole, times - floor));
        }
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Row(_)
        | Value::Extension(_) => return Err(not_a_fraction(fraction)),
    };
    let (Ok(units), Some(unit)) = (u128::try_from(exact.units), pow10(exact.scale).and_then(|p| u128::try_from(p).ok())) else {
        return Err(not_a_fraction(fraction));
    };
    let times = units.checked_mul(u128::from(count)).ok_or_else(|| not_a_fraction(fraction))?;
    let (whole, left) = (times.checked_div(unit).unwrap_or_default(), times.checked_rem(unit).unwrap_or_default());
    let part = Decimal::new(i128::try_from(left).unwrap_or_default(), exact.scale).to_f64();
    Ok((u64::try_from(whole).map_err(|_| not_a_fraction(fraction))?, part))
}

fn not_a_fraction(v: &Value) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} was taken as the fraction of an inverse distribution function"))
}

/// [ISO/IEC 9075-2:2023 §10.9 GR10h] The result, of type `result`, of the
/// values `low` and `high` at the rows `picked` gives: of PERCENTILE_DISC,
/// `low`; of PERCENTILE_CONT, CAST ( T0.Y + FACTOR * ( T1.Y − T0.Y ) AS
/// DT ), FACTOR an approximate number, in the arithmetic of SQL, so of
/// intervals an interval.
///
/// # Errors
///
/// As that arithmetic or cast raises.
pub fn picked_value(continuous: bool, (low, high): (Value, Value), factor: f64, result: &DataType) -> Result<Value> {
    if !continuous {
        return cast(low, result);
    }
    let apart = arith(ArithOp::Sub, &high, &low)?;
    let moved = arith(ArithOp::Mul, &Value::Approx(factor), &apart)?;
    cast(arith(ArithOp::Add, &low, &moved)?, result)
}
