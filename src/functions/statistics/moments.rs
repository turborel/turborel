//! \[DESIGN\] The statistical set functions of numbers not DECFLOAT, in
//! double precision [ISO/IEC 9075-2:2023 §10.9 GR7d ix, GR8]: the count
//! N, the means of X and Y, and the sums of the squared deviations from
//! them, SXX = SXS − SX × SX / N and SYY, and of their products, SXY =
//! SXY − SX × SY / N, kept by Welford's updates, a value at a time, so no
//! digits are lost to the subtraction the General Rules write; two states
//! merged by Chan's formulas. Each result is the General Rules' in these:
//! N × SXS = SX × SX exactly where SXX is 0.
//!
//! SXX and SYY only grow as values come, so whether they pass the range of
//! a double, which raises 22003 [GR3], does not depend on the values'
//! order (RULES.md D15). Of CORR and REGR_R2, a value past 1 by rounding
//! is 1, which bounds them.

use super::{Statistic, operands};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::functions::Accumulator;
use crate::value::{Decimal, Value};

/// The doubles of its state, after its count.
const DOUBLES: usize = 6;
/// The bytes of a double.
const DOUBLE_BYTES: usize = 8;

/// The moments of the values so far.
#[derive(Debug, Clone, Copy)]
pub(super) struct Moments {
    kind: Statistic,
    /// N, exactly, for REGR_COUNT.
    count: i128,
    /// N, as a double.
    n: f64,
    /// The mean of X, or of the one argument.
    mean_x: f64,
    /// The mean of Y.
    mean_y: f64,
    /// The sum of the squared deviations of X from its mean.
    sxx: f64,
    /// Of Y.
    syy: f64,
    /// The sum of the products of the deviations of X and Y.
    sxy: f64,
}

impl Moments {
    /// Of no values yet, of the set function `kind`.
    pub(super) fn new(kind: Statistic) -> Moments {
        Moments { kind, count: 0, n: 0.0, mean_x: 0.0, mean_y: 0.0, sxx: 0.0, syy: 0.0, sxy: 0.0 }
    }

    /// Welford's update by Y `y` and X `x`.
    fn add(&mut self, y: f64, x: f64) {
        self.count = self.count.saturating_add(1);
        self.n += 1.0_f64;
        let dx = x - self.mean_x;
        let dy = y - self.mean_y;
        self.mean_x += dx / self.n;
        self.mean_y += dy / self.n;
        self.sxx += dx * (x - self.mean_x);
        self.syy += dy * (y - self.mean_y);
        self.sxy += dx * (y - self.mean_y);
    }

    /// Chan's merge of `other`, as if its values had been added.
    fn join(&mut self, other: &Moments) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = Moments { kind: self.kind, ..*other };
            return;
        }
        let n = self.n + other.n;
        let (dx, dy) = (other.mean_x - self.mean_x, other.mean_y - self.mean_y);
        let weight = self.n * other.n / n;
        self.mean_x += dx * other.n / n;
        self.mean_y += dy * other.n / n;
        self.sxx += other.sxx + dx * dx * weight;
        self.syy += other.syy + dy * dy * weight;
        self.sxy += other.sxy + dx * dy * weight;
        self.count = self.count.saturating_add(other.count);
        self.n = n;
    }

    /// Its doubles, in the order of its state.
    fn doubles(&self) -> [f64; DOUBLES] {
        [self.n, self.mean_x, self.mean_y, self.sxx, self.syy, self.sxy]
    }

    /// [GR7d ix 2, GR8c ix] The sum `s` of N values over N − 1: none of
    /// one value.
    fn sample(&self, s: f64) -> Option<f64> {
        (self.count > 1).then(|| s / (self.n - 1.0_f64))
    }

    /// [GR8c xii, xiii] SXX not 0: the fit line is not vertical.
    fn spread_x(&self) -> bool {
        self.sxx > 0.0
    }

    /// The result, none if null: CORR, REGR_R2, REGR_SLOPE, REGR_INTERCEPT.
    fn fitted(&self) -> Option<f64> {
        let Moments { mean_x, mean_y, sxx, syy, sxy, .. } = *self;
        if !self.spread_x() {
            return None;
        }
        match self.kind {
            // [GR8c x] √((N SXY − SX SY)² / ((N SXS − SX²)(N SYS − SY²))).
            Statistic::Corr => (syy > 0.0).then(|| (sxy / sxx * (sxy / syy)).min(1.0).sqrt()),
            // [GR8c xi 2] 1 of Y all alike.
            Statistic::R2 => Some(if syy > 0.0 { (sxy / sxx * (sxy / syy)).min(1.0) } else { 1.0 }),
            Statistic::Slope => Some(sxy / sxx),
            // (SY SXS − SX SXY) / (N SXS − SX²), the mean of Y less the
            // slope times the mean of X.
            Statistic::Intercept => Some(mean_y - sxy / sxx * mean_x),
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

    /// The result of the others, none if null.
    fn plain(&self) -> Option<f64> {
        let Moments { n, mean_x, mean_y, sxx, syy, sxy, .. } = *self;
        match self.kind {
            Statistic::VarPop => Some(sxx / n),
            Statistic::VarSamp => self.sample(sxx),
            // [SR7i, SR7j] the square root of the variance.
            Statistic::StddevPop => Some((sxx / n).sqrt()),
            Statistic::StddevSamp => self.sample(sxx).map(f64::sqrt),
            Statistic::CovarPop => Some(sxy / n),
            Statistic::CovarSamp => self.sample(sxy),
            Statistic::AvgX => Some(mean_x),
            Statistic::AvgY => Some(mean_y),
            Statistic::Sxx => Some(sxx),
            Statistic::Syy => Some(syy),
            Statistic::Sxy => Some(sxy),
            Statistic::Corr | Statistic::Slope | Statistic::Intercept | Statistic::Count | Statistic::R2 => None,
        }
    }
}

/// The number `v` as a double.
fn double(v: &Value) -> Result<f64> {
    v.as_f64().map_or_else(|| fail(Code::Api, format!("[DESIGN] {v} is not a number")), Ok)
}

/// SQLSTATE 22003 [§10.9 GR3]: a result, or a sum on the way to it, past
/// the range of a double.
fn out_of_range<T>() -> Result<T> {
    fail_with(
        Code::Data,
        SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
        "[ISO/IEC 9075-2:2023 §10.9 GR3e] a result of the set function is past the range of DOUBLE PRECISION",
    )
}

impl Accumulator for Moments {
    fn update(&mut self, v: &Value) -> Result<()> {
        let (y, x) = operands(self.kind, v)?;
        let x = double(x)?;
        let y = if self.kind.binary() { double(y)? } else { x };
        self.add(y, x);
        Ok(())
    }

    /// \[DESIGN\] RULES.md D53: its count, 16 bytes, then its doubles, 8
    /// bytes each; written so a window's frames merge states
    /// (`exec/window/aggregate.rs`).
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(&self.count.to_le_bytes());
        for d in self.doubles() {
            out.extend_from_slice(&d.to_le_bytes());
        }
        true
    }

    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let mut reader = Reader::new(state, "a state of a statistical set function");
        let count = reader.i128()?;
        let mut doubles = [0.0_f64; DOUBLES];
        for d in &mut doubles {
            let bytes = <[u8; DOUBLE_BYTES]>::try_from(reader.take(DOUBLE_BYTES)?)
                .map_err(|_| reader.corrupt("a double of a statistical set function's state is short"))?;
            *d = f64::from_le_bytes(bytes);
        }
        reader.finish()?;
        let [n, mean_x, mean_y, sxx, syy, sxy] = doubles;
        self.join(&Moments { kind: self.kind, count, n, mean_x, mean_y, sxx, syy, sxy });
        Ok(())
    }

    /// [§10.9 GR7d, GR8c] REGR_COUNT N; null of no values; 22003 if a sum
    /// passed the range [GR3]; of CORR, REGR_R2, REGR_SLOPE and
    /// REGR_INTERCEPT null if not finite, of the others 22003.
    fn finish(&self) -> Result<Value> {
        if self.kind == Statistic::Count {
            return Ok(Value::Exact(Decimal::new(self.count, 0)));
        }
        if self.count == 0 {
            return Ok(Value::Null);
        }
        if !self.doubles().iter().all(|d| d.is_finite()) {
            return out_of_range();
        }
        let nulled = self.kind.nulled_out_of_range();
        let Some(r) = (if nulled { self.fitted() } else { self.plain() }) else { return Ok(Value::Null) };
        if r.is_finite() {
            Ok(Value::Approx(r))
        } else if nulled {
            Ok(Value::Null)
        } else {
            out_of_range()
        }
    }
}
