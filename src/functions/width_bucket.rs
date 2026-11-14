//! [ISO/IEC 9075-2:2023 §6.31] The `<width bucket function>`: which of
//! WBC buckets of equal width, between two bounds, a number falls in.
//!
//! The binder makes each call its own function (`width_bucket`), which
//! learns its result type as it is bound \[SR21\], so it can tell whether
//! WBC+1 fits it \[GR17d ii, GR17e ii\]. None is registered.
//!
//! \[DESIGN\] Of exact operands the bucket is found exactly, from the
//! operands as integers of one scale, so no quotient is rounded. Of an
//! approximate operand and none of decimal floating point, it is found in
//! approximate arithmetic, and of a decimal floating-point operand in
//! decimal floating point, as the operands' arithmetic would be
//! [§6.30 SR1]. A bucket so found that rounding puts past either end of
//! the range is the bucket at that end, since GR17 iii places every number
//! inside the range in one of 1 to WBC.

use std::cmp::Ordering;
use std::sync::{Arc, OnceLock};

use super::ScalarFunction;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::params::ParameterType;
use crate::value::{DataType, DecFloat, Decimal, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Spelled, Value, cast, compare, pow10};

/// [ISO/IEC 9075-2:2023 §6.31] WIDTH_BUCKET, no key word of SQL-92.
pub const WIDTH_BUCKET: &str = "WIDTH_BUCKET";

/// The place of `<width bucket count>` among the operands.
const COUNT: usize = 3;

/// \[EXTENSION\] The type a `?` count takes: NUMERIC(P, 0), P the greatest
/// precision, as SUBSTRING's length [ISO/IEC 9075:1992 §17.6 GR5c].
const COUNTED: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };

/// The bits of a `u128`, each of which a product is built from.
const U128_BITS: u32 = u128::BITS;

/// WIDTH_BUCKET, a new call; none if `name` is not its name.
pub fn width_bucket(name: &str) -> Option<Arc<dyn ScalarFunction>> {
    (name == WIDTH_BUCKET).then(|| -> Arc<dyn ScalarFunction> { Arc::new(WidthBucket::default()) })
}

/// [§6.31] `<width bucket function>` ::= WIDTH_BUCKET ( WBO , WBB1 , WBB2 ,
/// WBC ), called with its operands in order.
#[derive(Default)]
pub struct WidthBucket {
    /// \[SR21\] The type of its result, WBC's, known once it is bound.
    result: OnceLock<DataType>,
}

impl ScalarFunction for WidthBucket {
    fn name(&self) -> &str {
        WIDTH_BUCKET
    }

    /// \[SR21\] Each operand a number, WBC exact of scale 0; the result of
    /// WBC's type.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [operand, bound_1, bound_2, count] = args else { return fail(Code::Api, "[DESIGN] WIDTH_BUCKET takes four operands") };
        for (name, t) in [("operand", operand), ("bound 1", bound_1), ("bound 2", bound_2), ("count", count)] {
            if !t.is_numeric() {
                return fail(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §6.31] the width bucket {name} is a number, not {t}"));
            }
        }
        if !matches!(count, DataType::Exact { scale: 0, .. }) {
            return fail(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §6.31 SR21] the width bucket count is an exact number of scale 0, not {count}"),
            );
        }
        Ok(self.result.get_or_init(|| count.clone()).clone())
    }

    /// \[EXTENSION\] A `?` count NUMERIC(P, 0); a `?` operand or bound the
    /// type of the first of the other two not itself a `?`, against which
    /// it is compared; none if each is one.
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        if position == COUNT {
            return Some(ParameterType::Of(COUNTED));
        }
        args.iter().take(COUNT).enumerate().find_map(|(i, t)| t.clone().filter(|_| i != position)).map(ParameterType::Of)
    }

    /// \[GR17\] Null if an operand is; 2201G if WBC is not positive, or the
    /// bounds are equal; else the bucket.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [operand, bound_1, bound_2, count] = args else { return fail(Code::Api, "[DESIGN] WIDTH_BUCKET takes four operands") };
        // [GR17a]
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        let Value::Exact(Decimal { units: wbc, scale: 0 }) = *count else {
            return fail(Code::Api, "[DESIGN] the width bucket count is an exact number of scale 0");
        };
        // [GR17b]
        if wbc <= 0 {
            return invalid("[ISO/IEC 9075-2:2023 §6.31 GR17b] the width bucket count is not greater than 0");
        }
        let (low, high, rising) = match compare(bound_1, bound_2) {
            // [GR17c]
            Some(Ordering::Equal) => return invalid("[ISO/IEC 9075-2:2023 §6.31 GR17c] the width bucket bounds are equal"),
            Some(Ordering::Less) => (bound_1, bound_2, true),
            Some(Ordering::Greater) => (bound_2, bound_1, false),
            None => return fail(Code::Api, "[DESIGN] the width bucket bounds are numbers"),
        };
        Ok(Value::Exact(Decimal::new(self.placed(Span { operand, low, high, rising }, wbc)?, 0)))
    }
}

impl WidthBucket {
    /// \[GR17d, GR17e\] The bucket `span`'s operand falls in, of `wbc`
    /// buckets: 0 before bound 1, WBC+1 at or past bound 2, which shall fit
    /// the result's type (22003); else the bucket inside.
    fn placed(&self, span: Span<'_>, wbc: i128) -> Result<i128> {
        let (below, past) = span.ends()?;
        if below {
            return Ok(0);
        }
        if past {
            let Some(beyond) = wbc.checked_add(1) else { return out_of_range() };
            let Some(result) = self.result.get() else { return fail(Code::Api, "[DESIGN] WIDTH_BUCKET is called once bound") };
            return match cast(Value::Exact(Decimal::new(beyond, 0)), result) {
                Ok(_) => Ok(beyond),
                Err(_) => out_of_range(),
            };
        }
        span.inside(wbc)
    }
}

/// The operand and the bounds of a call, the lesser bound first, and
/// whether bound 1 is that one.
#[derive(Clone, Copy)]
struct Span<'v> {
    operand: &'v Value,
    low: &'v Value,
    high: &'v Value,
    rising: bool,
}

impl Span<'_> {
    /// Whether the operand is before bound 1, and whether at or past bound
    /// 2 [GR17d i, ii; GR17e i, ii].
    fn ends(&self) -> Result<(bool, bool)> {
        let (Some(to_low), Some(to_high)) = (compare(self.operand, self.low), compare(self.operand, self.high)) else {
            return fail(Code::Api, "[DESIGN] the width bucket operand is a number");
        };
        Ok(if self.rising {
            (to_low == Ordering::Less, to_high != Ordering::Less)
        } else {
            (to_high == Ordering::Greater, to_low != Ordering::Greater)
        })
    }

    /// \[GR17d iii, GR17e iii\] The bucket inside the range: the greatest
    /// whole number not above WBC × (WBO − WBB1) / (WBB2 − WBB1) + 1 of
    /// bound 1 the lesser, WBC × (WBB1 − WBO) / (WBB1 − WBB2) + 1 of bound
    /// 1 the greater; each WBC × the distance from bound 1 over the
    /// range's width.
    fn inside(&self, wbc: i128) -> Result<i128> {
        let start = if self.rising { self.low } else { self.high };
        let operands = [self.operand, start, self.low, self.high];
        let floor = if let [Value::Exact(o), Value::Exact(s), Value::Exact(l), Value::Exact(h)] = operands {
            exact_floor(wbc, [*o, *s, *l, *h])
        } else if operands.iter().any(|v| matches!(v, Value::DecFloat(_))) {
            decfloat_floor(wbc, operands)
        } else {
            approximate_floor(wbc, operands)
        };
        let Some(floor) = floor else { return out_of_range() };
        // Rounding keeps the bucket inside the range; see the module's doc.
        Ok(floor.min(wbc.saturating_sub(1)).max(0).saturating_add(1))
    }
}

/// Of exact operands `[WBO, S, L, H]`, ⌊WBC × |WBO − S| / (H − L)⌋, S
/// bound 1, L and H the lesser and greater bound, found exactly; none if
/// their digits at one scale are past 128 bits.
fn exact_floor(wbc: i128, operands: [Decimal; 4]) -> Option<i128> {
    let [operand, start, low, high] = operands;
    let scale = operands.iter().map(|d| d.scale).max()?;
    let at = |d: Decimal| d.units.checked_mul(pow10(scale.checked_sub(d.scale)?)?);
    let distance = at(operand)?.checked_sub(at(start)?)?.unsigned_abs();
    let width = at(high)?.checked_sub(at(low)?)?.unsigned_abs();
    i128::try_from(product_over(u128::try_from(wbc).ok()?, distance, width)?).ok()
}

/// ⌊`factor` × `part` / `whole`⌋, `part` less than `whole`, without
/// forming the product, which may be past 128 bits: the product is built
/// a bit of `factor` at a time, its quotient and remainder by `whole`
/// kept, the remainder always less than `whole`. None if `whole` is 0 or
/// past 127 bits.
fn product_over(factor: u128, part: u128, whole: u128) -> Option<u128> {
    if whole == 0 || whole > u128::MAX >> 1_u32 || part >= whole {
        return None;
    }
    let (mut quotient, mut rest) = (0_u128, 0_u128);
    for bit in (0..U128_BITS).rev() {
        quotient = quotient.checked_add(quotient)?;
        rest = rest.checked_add(rest)?;
        if rest >= whole {
            rest = rest.checked_sub(whole)?;
            quotient = quotient.checked_add(1)?;
        }
        if factor.checked_shr(bit)? & 1 == 1 {
            rest = rest.checked_add(part)?;
            if rest >= whole {
                rest = rest.checked_sub(whole)?;
                quotient = quotient.checked_add(1)?;
            }
        }
    }
    Some(quotient)
}

/// Of a decimal floating-point operand, ⌊WBC × |WBO − S| / (H − L)⌋ in
/// decimal floating point, the operands `[WBO, S, L, H]`.
fn decfloat_floor(wbc: i128, operands: [&Value; 4]) -> Option<i128> {
    let decfloat = |v: &Value| match v {
        Value::Exact(d) => DecFloat::of_decimal(d),
        Value::Approx(f) => DecFloat::of_f64(*f, MAX_DECFLOAT_PRECISION),
        Value::DecFloat(d) => Some(*d),
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Row(_)
        | Value::Extension(_) => None,
    };
    let [operand, start, low, high] = operands.map(decfloat);
    let signed = operand?.minus(start?)?;
    let distance = if signed.coefficient() < 0 { signed.negated() } else { signed };
    let quotient = DecFloat::of_decimal(&Decimal::new(wbc, 0))?.times(distance)?.over(high?.minus(low?)?)?;
    Some(quotient.to_decimal(0)?.units)
}

/// Of an approximate operand, ⌊WBC × |WBO − S| / (H − L)⌋ in approximate
/// arithmetic, the operands `[WBO, S, L, H]`.
fn approximate_floor(wbc: i128, operands: [&Value; 4]) -> Option<i128> {
    let [operand, start, low, high] = operands.map(Value::as_f64);
    let count = DecFloat::of_decimal(&Decimal::new(wbc, 0))?.to_f64();
    let quotient = (count * (operand? - start?).abs() / (high? - low?)).floor();
    if !quotient.is_finite() {
        return Some(wbc);
    }
    Some(DecFloat::of_f64(quotient, MAX_DECFLOAT_PRECISION)?.to_decimal(0)?.units)
}

/// [§6.31 GR17b, GR17c] The failure of WIDTH_BUCKET's operands, 2201G.
fn invalid<T>(message: &str) -> Result<T> {
    fail_with(Code::Data, SqlState::INVALID_ARGUMENT_FOR_WIDTH_BUCKET, message)
}

/// [§6.31 GR17d ii, GR17e ii] WBC+1 past the result's type, 22003.
fn out_of_range<T>() -> Result<T> {
    fail_with(
        Code::Data,
        SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
        "[ISO/IEC 9075-2:2023 §6.31 GR17d ii] the width bucket count plus 1 is past the type of the width bucket count",
    )
}

#[cfg(test)]
mod tests;
