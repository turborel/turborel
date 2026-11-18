//! The statistical set functions of a DECFLOAT operand, in DECFLOAT
//! arithmetic [ISO/IEC 9075-2:2023 §10.9 GR7d ix, GR8]: the sums N, SX,
//! SY, SXS, SYS and SXY, and the General Rules' formulas of them, each
//! operation rounded to 34 digits [§6.30 GR7].
//!
//! \[DESIGN\] RULES.md D15: each sum on the way is of any exponent, and
//! judged once finished, so whether it is out of range, 22003 [GR3], does
//! not depend on the order of the values; a square out of range is, of a
//! value alone. Rounding may leave N × SXS − SX × SX below 0 where the
//! values are alike: it is then taken as 0, the variance's least. It
//! writes no state, as SUM of DECFLOAT writes none: a sum on the way may
//! be out of range.

use super::{Statistic, operands};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::functions::Accumulator;
use crate::functions::numeric::square_root;
use crate::value::{DecFloat, Decimal, MAX_DECFLOAT_PRECISION, Value};

/// The sums of the values so far.
pub(super) struct Sums {
    kind: Statistic,
    count: i128,
    sx: DecFloat,
    sy: DecFloat,
    sxs: DecFloat,
    sys: DecFloat,
    sxy: DecFloat,
}

/// SQLSTATE 22003 [§10.9 GR3e].
fn out_of_range<T>() -> Result<T> {
    fail_with(
        Code::Data,
        SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
        "[ISO/IEC 9075-2:2023 §10.9 GR3e] a result of the set function, or a sum on the way, is past the range of DECFLOAT",
    )
}

/// The number `v` as DECFLOAT: an exact one rounded to 34 digits, an
/// approximate one from its binary value.
fn decimal(v: &Value) -> Result<DecFloat> {
    let d = if let Value::DecFloat(d) = v {
        Some(*d)
    } else if let Value::Exact(d) = v {
        DecFloat::of_decimal(d)
    } else {
        let Some(f) = v.as_f64() else { return fail(Code::Api, format!("[DESIGN] {v} is not a number")) };
        DecFloat::of_f64(f, MAX_DECFLOAT_PRECISION)
    };
    d.map_or_else(out_of_range, Ok)
}

/// `a` + `b`, of any exponent.
fn plus(a: DecFloat, b: DecFloat) -> Result<DecFloat> {
    a.plus_unbounded(b).map_or_else(out_of_range, Ok)
}

/// `d`, or 0 if below it.
fn at_least_zero(d: DecFloat) -> DecFloat {
    if d.coefficient() < 0 { DecFloat::ZERO } else { d }
}

/// Whether `d` is above 0.
fn positive(d: DecFloat) -> bool {
    d.coefficient() > 0
}

impl Sums {
    /// Of no values yet, of the set function `kind`.
    pub(super) fn new(kind: Statistic) -> Sums {
        let zero = DecFloat::ZERO;
        Sums { kind, count: 0, sx: zero, sy: zero, sxs: zero, sys: zero, sxy: zero }
    }

    /// N, SX, SY, SXS, SYS and SXY, judged; none if one is out of range.
    fn judged(&self) -> Option<[DecFloat; 6]> {
        let n = DecFloat::new(self.count, 0, MAX_DECFLOAT_PRECISION)?;
        Some([n, self.sx.within()?, self.sy.within()?, self.sxs.within()?, self.sys.within()?, self.sxy.within()?])
    }

    /// The result, none if null: CORR, REGR_R2, REGR_SLOPE and
    /// REGR_INTERCEPT, of which one out of range is null too.
    fn fitted(&self, [n, sx, sy, sxs, sys, sxy]: [DecFloat; 6]) -> Option<DecFloat> {
        let nxx = n.times(sxs)?.minus(sx.times(sx)?)?;
        // [GR8c x 1, xi 1, xii 1, xiii 1] N × SXS = SX × SX: null.
        if !positive(nxx) {
            return None;
        }
        let nyy = n.times(sys)?.minus(sy.times(sy)?)?;
        let nxy = n.times(sxy)?.minus(sx.times(sy)?)?;
        match self.kind {
            Statistic::Corr => square_root(nxy.times(nxy)?.over(nxx.times(nyy)?)?).filter(|_| positive(nyy)),
            // [GR8c xi 2] 1 of Y all alike.
            Statistic::R2 => {
                if positive(nyy) {
                    nxy.times(nxy)?.over(nxx.times(nyy)?)
                } else {
                    DecFloat::new(1, 0, MAX_DECFLOAT_PRECISION)
                }
            }
            Statistic::Slope => nxy.over(nxx),
            Statistic::Intercept => sy.times(sxs)?.minus(sx.times(sxy)?)?.over(nxx),
            Statistic::VarPop
            | Statistic::VarSamp
            | Statistic::StddevPop
            | Statistic::StddevSamp
            | Statistic::CovarPop
            | Statistic::CovarSamp
            | Statistic::Count
            | Statistic::AvgX
            | Statistic::AvgY
            | Statistic::Sxx
            | Statistic::Syy
            | Statistic::Sxy => None,
        }
    }

    /// The result of the others: 22003 if out of range.
    fn plain(&self, [n, sx, sy, sxs, sys, sxy]: [DecFloat; 6]) -> Result<Value> {
        let one = DecFloat::new(1, 0, MAX_DECFLOAT_PRECISION);
        let sample = || one.and_then(|one| n.minus(one));
        // [GR7d ix 2 A, GR8c ix 1] of one value, a sample's is null.
        if self.count == 1 && matches!(self.kind, Statistic::VarSamp | Statistic::StddevSamp | Statistic::CovarSamp) {
            return Ok(Value::Null);
        }
        let deviations = |s: DecFloat, a: DecFloat, b: DecFloat| s.minus(a.times(b)?.over(n)?);
        let variance = |over: Option<DecFloat>| deviations(sxs, sx, sx).map(at_least_zero).and_then(|d| d.over(over?));
        let result = match self.kind {
            // [GR7d ix] (SXS − SX × SX / N) / N, or / (N − 1) of more
            // than one value.
            Statistic::VarPop => variance(Some(n)),
            Statistic::VarSamp => variance(sample()),
            // [SR7i, SR7j] the square root of the variance.
            Statistic::StddevPop => variance(Some(n)).and_then(square_root),
            Statistic::StddevSamp => variance(sample()).and_then(square_root),
            Statistic::Sxx => deviations(sxs, sx, sx),
            Statistic::Syy => deviations(sys, sy, sy),
            Statistic::Sxy => deviations(sxy, sx, sy),
            Statistic::AvgX => sx.over(n),
            Statistic::AvgY => sy.over(n),
            Statistic::CovarPop => deviations(sxy, sx, sy).and_then(|d| d.over(n)),
            Statistic::CovarSamp => deviations(sxy, sx, sy).and_then(|d| d.over(sample()?)),
            Statistic::Corr | Statistic::Slope | Statistic::Intercept | Statistic::Count | Statistic::R2 => return Ok(Value::Null),
        };
        result.map(Value::DecFloat).map_or_else(out_of_range, Ok)
    }
}

impl Accumulator for Sums {
    fn update(&mut self, v: &Value) -> Result<()> {
        let (y, x) = operands(self.kind, v)?;
        let x = decimal(x)?;
        self.count = self.count.saturating_add(1);
        self.sx = plus(self.sx, x)?;
        self.sxs = plus(self.sxs, x.times(x).map_or_else(out_of_range, Ok)?)?;
        if self.kind.binary() {
            let y = decimal(y)?;
            self.sy = plus(self.sy, y)?;
            self.sys = plus(self.sys, y.times(y).map_or_else(out_of_range, Ok)?)?;
            self.sxy = plus(self.sxy, x.times(y).map_or_else(out_of_range, Ok)?)?;
        }
        Ok(())
    }

    /// [§10.9 GR7d, GR8c] REGR_COUNT N; null of no values; 22003 if a sum
    /// is out of range [GR3]; of CORR, REGR_R2, REGR_SLOPE and
    /// REGR_INTERCEPT null if out of range, of the others 22003.
    fn finish(&self) -> Result<Value> {
        if self.kind == Statistic::Count {
            return Ok(Value::Exact(Decimal::new(self.count, 0)));
        }
        if self.count == 0 {
            return Ok(Value::Null);
        }
        let Some(sums) = self.judged() else { return out_of_range() };
        if self.kind.nulled_out_of_range() {
            return Ok(self.fitted(sums).map_or(Value::Null, Value::DecFloat));
        }
        self.plain(sums)
    }
}
