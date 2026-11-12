//! [ISO/IEC 9075-2:2023 §4.5.2] A decimal floating-point value: a
//! coefficient of decimal digits times a power of ten. A value of
//! DECFLOAT(p) has at most p digits, and a result of arithmetic at most
//! `MAX_DECFLOAT_PRECISION`, the precision of IV134.
//!
//! Each value has one form, its coefficient with no trailing zeros (0 of
//! exponent 0), so two values are equal exactly when their fields are, and
//! 1.50 is 1.5. A result is rounded half to even, the value nearest it
//! [§6.30 GR7] and of two the even one (IA192), and is out of range if its
//! adjusted exponent, that of its leading digit, is outside
//! `MIN_EXPONENT`..=`MAX_EXPONENT` (IL054), as decimal128's is.
//!
//! Its key orders as the values compare, and no key begins another:
//!
//! ```text
//! key: 1 (zero) | 2 adjusted exponent biased (2 bytes) coefficient padded (16)
//!               | 0 the same, every bit inverted (negative)
//! ```

use std::cmp::Ordering;
use std::fmt;

use super::{Decimal, MAX_DECFLOAT_PRECISION};

mod wide;
use wide::{Wide, digits};

/// [ISO/IEC 9075-2:2023 §6.1 SR36] The greatest adjusted exponent of a
/// DECFLOAT value (IL054), decimal128's.
pub const MAX_EXPONENT: i64 = 6144;
/// The least adjusted exponent of a DECFLOAT value (IL054), decimal128's.
pub const MIN_EXPONENT: i64 = -6143;
/// The decimal radix.
const RADIX: u64 = 10;
/// The digit at which a dropped digit rounds up, or to even.
const HALF: u64 = 5;
/// The most digits a coefficient's `u128` holds whatever they are.
const U128_DIGITS: u32 = 38;
/// Exponents apart beyond which the smaller operand of a sum lies below
/// the rounding digit (`DecFloat::plus`).
const ALIGN_LIMIT: i64 = 40;
/// Digits below the larger operand of a sum at which the smaller, below
/// the rounding digit, stands for its sign alone.
const STICKY_PLACES: u32 = 3;
/// A key's first byte: of a negative value, zero, a positive value.
const NEGATIVE: u8 = 0;
/// A key's first byte of zero.
const ZERO_KEY: u8 = 1;
/// A key's first byte of a positive value.
const POSITIVE: u8 = 2;
/// The adjusted exponents in plain notation, without an E (IV110).
const PLAIN_LEAST: i64 = -6;

/// A decimal floating-point value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DecFloat {
    /// Its digits, signed, with no trailing zeros.
    coefficient: i128,
    /// The power of ten they are multiplied by.
    exponent: i32,
}

/// 10^`n` as a `u128`, if it holds it.
fn pow10(n: u32) -> Option<u128> {
    u128::from(RADIX).checked_pow(n)
}

/// A value not yet rounded: `magnitude` × 10^`exponent`, of the sign
/// `negative` says, `sticky` if digits below it, all lost, were not all 0.
#[derive(Debug, Clone, Copy)]
struct Unrounded {
    negative: bool,
    magnitude: Wide,
    exponent: i64,
    sticky: bool,
}

impl Unrounded {
    /// `magnitude` × 10^`exponent`, exactly.
    fn exact(negative: bool, magnitude: Wide, exponent: i64) -> Unrounded {
        Unrounded { negative, magnitude, exponent, sticky: false }
    }

    /// The value of `precision` digits nearest it, half to even; none if
    /// its adjusted exponent is out of range.
    fn rounded(self, precision: u32) -> Option<DecFloat> {
        self.unbounded(precision)?.within()
    }

    /// The value of `precision` digits nearest it, half to even, whatever
    /// its adjusted exponent; none if 32 bits cannot hold its exponent.
    fn unbounded(self, precision: u32) -> Option<DecFloat> {
        let Unrounded { negative, mut magnitude, mut exponent, mut sticky } = self;
        let mut dropped = 0_u64;
        for _ in 0..magnitude.digits().saturating_sub(precision) {
            sticky |= dropped != 0;
            (magnitude, dropped) = magnitude.over(RADIX);
            exponent = exponent.saturating_add(1);
        }
        let mut c = magnitude.narrow()?;
        let odd = c.checked_rem(2) == Some(1);
        if dropped > HALF || (dropped == HALF && (sticky || odd)) {
            c = c.checked_add(1)?;
        }
        DecFloat::canonical(negative, c, exponent)
    }
}

impl DecFloat {
    /// Zero.
    pub const ZERO: DecFloat = DecFloat { coefficient: 0, exponent: 0 };

    /// Its digits, signed, with no trailing zeros.
    pub fn coefficient(&self) -> i128 {
        self.coefficient
    }

    /// The power of ten its coefficient is multiplied by.
    pub fn exponent(&self) -> i32 {
        self.exponent
    }

    /// Whether it is zero.
    pub fn is_zero(&self) -> bool {
        self.coefficient == 0
    }

    /// The value `c` × 10^`e`, of the sign `negative` says, its trailing
    /// zeros taken into the exponent; none if its adjusted exponent is out
    /// of range, or its coefficient past 38 digits.
    fn finite(negative: bool, c: u128, e: i64) -> Option<DecFloat> {
        DecFloat::canonical(negative, c, e)?.within()
    }

    /// It, if its adjusted exponent is in range (IL054); 0 always is.
    pub(crate) fn within(self) -> Option<DecFloat> {
        (self.is_zero() || (MIN_EXPONENT..=MAX_EXPONENT).contains(&self.adjusted())).then_some(self)
    }

    /// `c` × 10^`e` as `finite` says, whatever its adjusted exponent.
    fn canonical(negative: bool, mut c: u128, mut e: i64) -> Option<DecFloat> {
        if c == 0 {
            return Some(DecFloat::ZERO);
        }
        while c.checked_rem(u128::from(RADIX)) == Some(0) {
            c = c.checked_div(u128::from(RADIX))?;
            e = e.checked_add(1)?;
        }
        let magnitude = i128::try_from(c).ok()?;
        Some(DecFloat { coefficient: if negative { magnitude.checked_neg()? } else { magnitude }, exponent: i32::try_from(e).ok()? })
    }

    /// `coefficient` × 10^`exponent`, rounded to `precision` digits; none
    /// if out of range.
    pub fn new(coefficient: i128, exponent: i64, precision: u32) -> Option<DecFloat> {
        Unrounded::exact(coefficient < 0, Wide::of(coefficient.unsigned_abs()), exponent).rounded(precision)
    }

    /// It rounded to `precision` digits [§9.2 GR3e]; none if out of range.
    pub fn rounded_to(self, precision: u32) -> Option<DecFloat> {
        DecFloat::new(self.coefficient, i64::from(self.exponent), precision)
    }

    /// The exact number `d`, rounded to `MAX_DECFLOAT_PRECISION` digits,
    /// which hold every exact value's.
    pub fn of_decimal(d: &Decimal) -> Option<DecFloat> {
        DecFloat::new(d.units, 0_i64.checked_sub(i64::from(d.scale))?, MAX_DECFLOAT_PRECISION)
    }

    /// The approximate number `x`, rounded to `precision` digits from its
    /// exact binary value; none if it is no finite number or is out of
    /// range.
    pub fn of_f64(x: f64, precision: u32) -> Option<DecFloat> {
        if !x.is_finite() {
            return None;
        }
        let places = usize::try_from(precision.checked_sub(1)?).ok()?;
        // Rust writes the exact value's digits, correctly rounded.
        let text = format!("{x:.places$e}");
        let (mantissa, e) = text.split_once('e')?;
        let negative = mantissa.starts_with('-');
        let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
        let c: u128 = digits.parse().ok()?;
        let e = e.parse::<i64>().ok()?.checked_sub(i64::try_from(places).ok()?)?;
        Unrounded::exact(negative, Wide::of(c), e).rounded(precision)
    }

    /// The value of a `<signed numeric literal>` of the sign `negative`
    /// says, its digits `digits`, its exponent `exponent`: rounded to
    /// `precision` digits; none if out of range.
    pub fn of_digits(negative: bool, digits: &str, exponent: i64, precision: u32) -> Option<DecFloat> {
        let digits = digits.trim_start_matches('0');
        let held = digits.len().min(usize::try_from(U128_DIGITS).ok()?);
        let (kept, lost) = digits.split_at(held);
        let c: u128 = if kept.is_empty() { 0 } else { kept.parse().ok()? };
        let e = exponent.saturating_add(i64::try_from(lost.len()).ok()?);
        let sticky = lost.bytes().any(|b| b != b'0');
        Unrounded { negative, magnitude: Wide::of(c), exponent: e, sticky }.rounded(precision)
    }

    /// The nearest `f64`, correctly rounded; infinite past its range.
    pub fn to_f64(&self) -> f64 {
        format!("{}e{}", self.coefficient, self.exponent).parse().unwrap_or(f64::NAN)
    }

    /// It at `scale` digits after the point, its other digits truncated
    /// toward zero (IA002, IA195); none if 128 bits cannot hold it.
    pub fn to_decimal(&self, scale: u32) -> Option<Decimal> {
        let up = i64::from(self.exponent).checked_add(i64::from(scale))?;
        let units = if up >= 0 {
            self.coefficient.checked_mul(i128::try_from(pow10(u32::try_from(up).ok()?)?).ok()?)?
        } else {
            let down = u32::try_from(up.checked_neg()?).ok()?;
            pow10(down).and_then(|p| i128::try_from(p).ok()).map_or(Some(0), |p| self.coefficient.checked_div(p))?
        };
        Some(Decimal::new(units, scale))
    }

    /// Its sign reversed [§6.30 GR3].
    #[must_use]
    pub fn negated(self) -> DecFloat {
        DecFloat { coefficient: self.coefficient.saturating_neg(), ..self }
    }

    /// Its magnitude, unsigned.
    fn magnitude(&self) -> u128 {
        self.coefficient.unsigned_abs()
    }

    /// The exponent of its leading digit.
    fn adjusted(&self) -> i64 {
        i64::from(self.exponent).saturating_add(i64::from(digits(self.magnitude()))).saturating_sub(1)
    }

    /// Its magnitude padded to `MAX_DECFLOAT_PRECISION` digits, and the
    /// exponent so lowered; zero's is 0.
    fn padded(&self) -> (u128, i64) {
        let pad = MAX_DECFLOAT_PRECISION.saturating_sub(digits(self.magnitude()));
        let m = pow10(pad).and_then(|p| self.magnitude().checked_mul(p)).unwrap_or_default();
        (m, i64::from(self.exponent).saturating_sub(i64::from(pad)))
    }

    /// It + `o`, rounded to `MAX_DECFLOAT_PRECISION` digits [§6.30 GR4,
    /// GR7]; none if out of range.
    pub fn plus(self, o: DecFloat) -> Option<DecFloat> {
        self.plus_unbounded(o)?.within()
    }

    /// It + `o`, as `plus` says, whatever its adjusted exponent, for a sum
    /// on the way judged once it is finished (RULES.md D15). Of two
    /// operands far apart, the smaller, below the rounding digit, counts
    /// for its sign alone.
    pub(crate) fn plus_unbounded(self, o: DecFloat) -> Option<DecFloat> {
        if self.is_zero() || o.is_zero() {
            return Some(if self.is_zero() { o } else { self });
        }
        let ((ma, ea), (mb, eb)) = (self.padded(), o.padded());
        let (big, small, (mbig, ebig), (msmall, esmall)) =
            if ea >= eb { (self, o, (ma, ea), (mb, eb)) } else { (o, self, (mb, eb), (ma, ea)) };
        let gap = ebig.checked_sub(esmall)?;
        let (a, b, e) = if gap <= ALIGN_LIMIT {
            (Wide::of(mbig).shifted(u32::try_from(gap).ok()?)?, Wide::of(msmall), esmall)
        } else {
            (Wide::of(mbig).shifted(STICKY_PLACES)?, Wide::of(1), ebig.checked_sub(i64::from(STICKY_PLACES))?)
        };
        let (negative, magnitude) = if (big.coefficient < 0) == (small.coefficient < 0) {
            (big.coefficient < 0, a.plus(b)?)
        } else if a >= b {
            (big.coefficient < 0, a.minus(b)?)
        } else {
            (small.coefficient < 0, b.minus(a)?)
        };
        Unrounded::exact(negative, magnitude, e).unbounded(MAX_DECFLOAT_PRECISION)
    }

    /// It − `o`, as `plus` says.
    pub fn minus(self, o: DecFloat) -> Option<DecFloat> {
        self.plus(o.negated())
    }

    /// It × `o`, rounded to `MAX_DECFLOAT_PRECISION` digits; none if out of
    /// range.
    pub fn times(self, o: DecFloat) -> Option<DecFloat> {
        let e = i64::from(self.exponent).checked_add(i64::from(o.exponent))?;
        let negative = (self.coefficient < 0) != (o.coefficient < 0);
        Unrounded::exact(negative, Wide::product(self.magnitude(), o.magnitude()), e).rounded(MAX_DECFLOAT_PRECISION)
    }

    /// It ÷ `o`, not 0, rounded to `MAX_DECFLOAT_PRECISION` digits; none
    /// if out of range or `o` is 0.
    pub fn over(self, o: DecFloat) -> Option<DecFloat> {
        self.over_unbounded(o)?.within()
    }

    /// It ÷ `o`, as `over` says, whatever its adjusted exponent, for a
    /// mean of a sum on the way. Its digits are found one at a time, past
    /// the rounding digit, and what remains decides a tie.
    pub(crate) fn over_unbounded(self, o: DecFloat) -> Option<DecFloat> {
        let divisor = o.magnitude();
        let mut quotient = self.magnitude().checked_div(divisor)?;
        let mut rest = self.magnitude().checked_rem(divisor)?;
        let mut exponent = i64::from(self.exponent).checked_sub(i64::from(o.exponent))?;
        let enough = pow10(MAX_DECFLOAT_PRECISION.checked_add(1)?)?;
        while rest != 0 && quotient < enough {
            let shifted = rest.checked_mul(u128::from(RADIX))?;
            quotient = quotient.checked_mul(u128::from(RADIX))?.checked_add(shifted.checked_div(divisor)?)?;
            rest = shifted.checked_rem(divisor)?;
            exponent = exponent.checked_sub(1)?;
        }
        let negative = (self.coefficient < 0) != (o.coefficient < 0);
        Unrounded { negative, magnitude: Wide::of(quotient), exponent, sticky: rest != 0 }.unbounded(MAX_DECFLOAT_PRECISION)
    }

    /// Appends its key to `out`, as the module's documentation says.
    pub fn put_key(&self, out: &mut Vec<u8>) {
        if self.is_zero() {
            out.push(ZERO_KEY);
            return;
        }
        let biased = u16::try_from(self.adjusted().saturating_sub(MIN_EXPONENT)).unwrap_or(u16::MAX);
        let mut body = [biased.to_be_bytes().as_slice(), self.padded().0.to_be_bytes().as_slice()].concat();
        if self.coefficient < 0 {
            for b in &mut body {
                *b = !*b;
            }
        }
        out.push(if self.coefficient < 0 { NEGATIVE } else { POSITIVE });
        out.extend_from_slice(&body);
    }

    /// The value whose key begins `rest`, which then holds the bytes after
    /// it; none if it begins with no key.
    pub fn take_key(rest: &mut &[u8]) -> Option<DecFloat> {
        let (&first, after) = rest.split_first()?;
        if first == ZERO_KEY {
            *rest = after;
            return Some(DecFloat::ZERO);
        }
        let (body, past) = after.split_at_checked(KEY_BODY)?;
        let negative = match first {
            NEGATIVE => true,
            POSITIVE => false,
            ZERO_KEY | 3..=u8::MAX => return None,
        };
        let body: Vec<u8> = body.iter().map(|&b| if negative { !b } else { b }).collect();
        let (biased, padded) = body.split_at(EXPONENT_BYTES);
        let adjusted = i64::from(u16::from_be_bytes(biased.try_into().ok()?)).checked_add(MIN_EXPONENT)?;
        let m = u128::from_be_bytes(padded.try_into().ok()?);
        let e = adjusted.checked_sub(i64::from(MAX_DECFLOAT_PRECISION.checked_sub(1)?))?;
        *rest = past;
        DecFloat::finite(negative, m, e)
    }

    /// It as a numeral `[-]digits[.digits]`, every digit written: of a
    /// value an `f64` holds, at most some hundreds.
    pub(crate) fn numeral(&self) -> String {
        let sign = if self.coefficient < 0 { "-" } else { "" };
        let digits = self.magnitude().to_string();
        let e = self.exponent;
        if e >= 0_i32 {
            return format!("{sign}{digits}{}", "0".repeat(usize::try_from(e).unwrap_or_default()));
        }
        let places = usize::try_from(e.unsigned_abs()).unwrap_or(usize::MAX);
        let padded = format!("{digits:0>width$}", width = places.saturating_add(1));
        let (whole, fraction) = padded.split_at(padded.len().saturating_sub(places));
        format!("{sign}{whole}.{fraction}")
    }
}

/// Bytes of a key after its first: the biased exponent, then the padded
/// coefficient.
const KEY_BODY: usize = EXPONENT_BYTES + COEFFICIENT_BYTES;
/// Bytes of a key's biased exponent.
const EXPONENT_BYTES: usize = 2;
/// Bytes of a key's padded coefficient.
const COEFFICIENT_BYTES: usize = 16;

impl PartialOrd for DecFloat {
    fn partial_cmp(&self, o: &DecFloat) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for DecFloat {
    /// By value [§8.2 GR3]: by sign, then of one sign by the adjusted
    /// exponent, then by the digits.
    fn cmp(&self, o: &DecFloat) -> Ordering {
        let by_sign = self.coefficient.signum().cmp(&o.coefficient.signum());
        if by_sign != Ordering::Equal || self.is_zero() {
            return by_sign;
        }
        let magnitude = self.adjusted().cmp(&o.adjusted()).then_with(|| self.padded().0.cmp(&o.padded().0));
        if self.coefficient < 0 { magnitude.reverse() } else { magnitude }
    }
}

/// [§6.13 GR11c, GR12c] A `<signed numeric literal>` whose value is it
/// (IV110): plain, as an exact numeric literal, if its leading digit's
/// exponent is from -6 to 33, else as an approximate numeric literal, one
/// digit before the point and E and the exponent.
impl fmt::Display for DecFloat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let adjusted = self.adjusted();
        if self.is_zero() || (PLAIN_LEAST..i64::from(MAX_DECFLOAT_PRECISION)).contains(&adjusted) {
            return f.write_str(&self.numeral());
        }
        let sign = if self.coefficient < 0 { "-" } else { "" };
        let digits = self.magnitude().to_string();
        let (lead, rest) = digits.split_at(1);
        let point = if rest.is_empty() { "" } else { "." };
        write!(f, "{sign}{lead}{point}{rest}E{adjusted}")
    }
}

#[cfg(test)]
mod tests;
