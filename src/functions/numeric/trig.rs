//! The `<trigonometric function>`s of Feature T622, the `<general
//! logarithm function>` LOG of Feature T623 and the `<common logarithm>`
//! LOG10 of Feature T624 [ISO/IEC 9075-2:2023 §6.31 GR10, GR11, SR15].
//!
//! \[DESIGN\] Each is found in double precision, as LN is: of a DECFLOAT
//! operand through its decimal exponent where that keeps a value past a
//! double's range (the logarithms), kept to the 16 digits a double holds.
//! LOG of base 10, and so LOG10, is found by the base-10 logarithm, and of
//! base 2 by the base-2 one, so a power of either base is a whole number
//! (RULES.md D331).

use std::cmp::Ordering;

use super::{decimal, double, finite, out_of_range, sign};
use crate::error::Result;
use crate::value::{Decimal, Value, compare};

/// [§6.31] SIN, no key word of SQL-92.
pub const SIN: &str = "SIN";
/// [§6.31] COS, no key word of SQL-92.
pub const COS: &str = "COS";
/// [§6.31] TAN, no key word of SQL-92.
pub const TAN: &str = "TAN";
/// [§6.31] SINH, no key word of SQL-92.
pub const SINH: &str = "SINH";
/// [§6.31] COSH, no key word of SQL-92.
pub const COSH: &str = "COSH";
/// [§6.31] TANH, no key word of SQL-92.
pub const TANH: &str = "TANH";
/// [§6.31] ASIN, no key word of SQL-92.
pub const ASIN: &str = "ASIN";
/// [§6.31] ACOS, no key word of SQL-92.
pub const ACOS: &str = "ACOS";
/// [§6.31] ATAN, no key word of SQL-92.
pub const ATAN: &str = "ATAN";
/// [§6.31] LOG, no key word of SQL-92.
pub const LOG: &str = "LOG";
/// [§6.31] LOG10, no key word of SQL-92.
pub const LOG10: &str = "LOG10";

/// [§6.31 SR15] The base LOG10 is LOG of.
const TEN: i128 = 10;
/// The base whose logarithm is found by the base-2 function.
const TWO: i128 = 2;
/// [§6.31 GR10b i 1, ii 1] The least value ASIN and ACOS take; 1 the
/// greatest.
const LEAST_SINE: i128 = -1;

/// [§6.31] A `<trigonometric function name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trig {
    /// SIN.
    Sin,
    /// COS.
    Cos,
    /// TAN.
    Tan,
    /// SINH.
    Sinh,
    /// COSH.
    Cosh,
    /// TANH.
    Tanh,
    /// ASIN.
    Asin,
    /// ACOS.
    Acos,
    /// ATAN.
    Atan,
}

impl Trig {
    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Trig::Sin => SIN,
            Trig::Cos => COS,
            Trig::Tan => TAN,
            Trig::Sinh => SINH,
            Trig::Cosh => COSH,
            Trig::Tanh => TANH,
            Trig::Asin => ASIN,
            Trig::Acos => ACOS,
            Trig::Atan => ATAN,
        }
    }

    /// Its value of `x`, an angle in radians, in double precision.
    fn of(self, x: f64) -> f64 {
        match self {
            Trig::Sin => x.sin(),
            Trig::Cos => x.cos(),
            Trig::Tan => x.tan(),
            Trig::Sinh => x.sinh(),
            Trig::Cosh => x.cosh(),
            Trig::Tanh => x.tanh(),
            Trig::Asin => x.asin(),
            Trig::Acos => x.acos(),
            Trig::Atan => x.atan(),
        }
    }
}

/// The exact number `n`, of scale 0.
fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Whether the number `v` equals the whole number `n`.
fn equals(v: &Value, n: i128) -> bool {
    compare(v, &whole(n)) == Some(Ordering::Equal)
}

/// A double result `x` as the function's result: DECFLOAT of its 16
/// digits if `decimal`, else itself; 22003 by `why` if it is not finite.
fn result(x: f64, decimal: bool, why: &str) -> Result<Value> {
    let value = finite(x, why)?;
    if decimal { decimal::of_double(x).map_or_else(|| out_of_range(why), Ok) } else { Ok(value) }
}

/// [§6.31 GR10] `t` of `v`, not null: of ACOS or ASIN, 22003 if `v` is
/// below −1 or above 1 \[GR10b i 1, ii 1\].
pub(super) fn trigonometric(t: Trig, v: &Value) -> Result<Value> {
    let inverse = matches!(t, Trig::Asin | Trig::Acos);
    let outside = || compare(v, &whole(LEAST_SINE)) == Some(Ordering::Less) || compare(v, &whole(1)) == Some(Ordering::Greater);
    if inverse && outside() {
        return out_of_range("§6.31 GR10b");
    }
    result(t.of(double(v)?), matches!(v, Value::DecFloat(_)), "§6.31 GR10b")
}

/// [§6.31 GR11] LOG ( `base`, `argument` ), neither null: 22003 if the
/// argument is 0 or below \[GR11b\], or the base is 0 or below or 1
/// \[GR11c\]; else the logarithm of the argument to the base \[GR11d\],
/// DECFLOAT if either is.
pub(super) fn logarithm(base: &Value, argument: &Value) -> Result<Value> {
    if sign(argument)? != Ordering::Greater {
        return out_of_range("§6.31 GR11b");
    }
    if sign(base)? != Ordering::Greater || equals(base, 1) {
        return out_of_range("§6.31 GR11c");
    }
    let decimal = matches!(argument, Value::DecFloat(_)) || matches!(base, Value::DecFloat(_));
    let x = if equals(base, TEN) {
        decimal::log10(argument)?
    } else if equals(base, TWO) && !matches!(argument, Value::DecFloat(_)) {
        double(argument)?.log2()
    } else {
        decimal::log10(argument)? / decimal::log10(base)?
    };
    result(x, decimal, "§6.31 GR11d")
}

/// [§6.31 SR15] LOG10 ( `argument` ), LOG ( 10, `argument` ).
pub(super) fn common_logarithm(argument: &Value) -> Result<Value> {
    logarithm(&whole(TEN), argument)
}
