//! Decimals of any number of digits, as the operators on `xs:decimal` and
//! `xs:integer` reckon with them [XQueryFO §6.2]: sums, differences,
//! products, quotients truncated to a number of digits, remainders, and
//! the roundings of [XQueryFO §6.4].
//!
//! \[DESIGN\] RULES.md D106: a value is its sign, its digits, least
//! significant first, and how many of them follow the period, none zero
//! at either end, so each value has one form. Its digits are bounded by
//! `MAX_DIGITS`, an implementation-defined limit [XQueryFO §6.2]: an
//! operation of an operand or result past it overflows, err:FOAR0002.

use std::cmp::Ordering;

/// \[DESIGN\] The most digits a decimal an operator reads or makes has:
/// past them it overflows [XQueryFO §6.2], so a product or quotient is
/// reckoned in bounded time and memory (RULES.md D18). The value is no
/// measurement: it is far past the 18 digits [Schema2 §3.2.3] asks of a
/// minimally conforming processor.
pub const MAX_DIGITS: usize = 1024;

/// \[DESIGN\] The digits after the period a quotient of decimals keeps at
/// least, more digits truncated [XQueryFO §6.2]: as many as a minimally
/// conforming processor's decimals have in all [Schema2 §3.2.3].
pub const DIVISION_SCALE: usize = 18;

/// The radix of the digits.
const RADIX: u8 = 10;
/// Half of the radix, the first digit of a remainder exactly half.
const HALF: u8 = 5;

/// A decimal of any number of digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal {
    /// Whether it is less than zero; never of zero.
    negative: bool,
    /// Its digits, least significant first, none zero at either end.
    digits: Vec<u8>,
    /// How many of its digits follow the period.
    scale: usize,
}

/// How a rounding goes [XQueryFO §6.4].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// Toward negative infinity, as `fn:floor`.
    Floor,
    /// Toward positive infinity, as `fn:ceiling`.
    Ceiling,
    /// To the nearest, a half toward positive infinity, as `fn:round`.
    HalfUp,
    /// To the nearest, a half to the even digit, as
    /// `fn:round-half-to-even`.
    HalfEven,
    /// Toward zero, as `op:numeric-integer-divide` truncates.
    Truncate,
}

/// `digits` with no zero most significant.
fn trimmed(mut digits: Vec<u8>) -> Vec<u8> {
    while digits.last() == Some(&0) {
        digits.pop();
    }
    digits
}

/// The sum of two magnitudes.
fn add_magnitudes(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len().max(b.len()).saturating_add(1));
    let mut carry = 0_u8;
    for at in 0..a.len().max(b.len()) {
        // Two digits and a carry are at most 19, within a u8.
        let sum = a.get(at).copied().unwrap_or(0).wrapping_add(b.get(at).copied().unwrap_or(0)).wrapping_add(carry);
        out.push(sum.checked_rem(RADIX).unwrap_or(0));
        carry = sum.checked_div(RADIX).unwrap_or(0);
    }
    out.push(carry);
    trimmed(out)
}

/// The difference of two magnitudes, `a` not less than `b`.
fn subtract_magnitudes(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0_u8;
    for (at, &digit) in a.iter().enumerate() {
        let take = b.get(at).copied().unwrap_or(0).wrapping_add(borrow);
        let (value, under) = if digit >= take { (digit.wrapping_sub(take), 0) } else { (digit.wrapping_add(RADIX).wrapping_sub(take), 1) };
        out.push(value);
        borrow = under;
    }
    trimmed(out)
}

/// The order of two magnitudes.
fn compare_magnitudes(a: &[u8], b: &[u8]) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.iter().rev().cmp(b.iter().rev()))
}

/// The product of two magnitudes.
fn multiply_magnitudes(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut wide = vec![0_u32; a.len().saturating_add(b.len()).saturating_add(1)];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0_u32;
        for (j, &y) in b.iter().enumerate() {
            let Some(slot) = wide.get_mut(i.saturating_add(j)) else { continue };
            // A slot, a product of two digits and a carry stay below 200.
            let total = slot.wrapping_add(u32::from(x).wrapping_mul(u32::from(y))).wrapping_add(carry);
            *slot = total.checked_rem(u32::from(RADIX)).unwrap_or(0);
            carry = total.checked_div(u32::from(RADIX)).unwrap_or(0);
        }
        if let Some(slot) = wide.get_mut(i.saturating_add(b.len())) {
            *slot = slot.wrapping_add(carry);
        }
    }
    trimmed(wide.into_iter().map(|d| u8::try_from(d).unwrap_or(0)).collect())
}

/// `digits` times ten to the `places`.
fn shifted(digits: &[u8], places: usize) -> Vec<u8> {
    if digits.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0; places];
    out.extend_from_slice(digits);
    out
}

/// The quotient of two magnitudes, truncated, `divisor` not zero.
fn divide_magnitudes(dividend: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut quotient = Vec::with_capacity(dividend.len());
    let mut remainder: Vec<u8> = Vec::new();
    for &digit in dividend.iter().rev() {
        remainder.insert(0, digit);
        remainder = trimmed(remainder);
        let mut count = 0_u8;
        while compare_magnitudes(&remainder, divisor) != Ordering::Less {
            remainder = subtract_magnitudes(&remainder, divisor);
            count = count.wrapping_add(1);
        }
        quotient.push(count);
    }
    quotient.reverse();
    trimmed(quotient)
}

impl Decimal {
    /// Zero.
    #[must_use]
    pub fn zero() -> Decimal {
        Decimal { negative: false, digits: Vec::new(), scale: 0 }
    }

    /// The decimal of sign `negative`, magnitude `digits` and `scale`,
    /// put in its one form.
    fn of(negative: bool, digits: Vec<u8>, scale: usize) -> Decimal {
        let mut digits = trimmed(digits);
        let mut scale = scale;
        let leading = digits.iter().take(scale).take_while(|&&d| d == 0).count();
        digits.drain(..leading);
        scale = scale.saturating_sub(leading);
        let negative = negative && !digits.is_empty();
        if digits.is_empty() {
            scale = 0;
        }
        Decimal { negative, digits, scale }
    }

    /// The value of `text`, a lexical form of `xs:decimal` or
    /// `xs:integer`: a sign perhaps, then digits with a period perhaps
    /// among them [Schema2 §3.2.3.1]; none if it is no such form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Decimal> {
        let (negative, body) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
        let all = whole.bytes().chain(fraction.bytes());
        if (whole.is_empty() && fraction.is_empty()) || !all.clone().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let digits = all.rev().map(|b| b.wrapping_sub(b'0')).collect();
        Some(Decimal::of(negative, digits, fraction.len()))
    }

    /// The integer `n`.
    #[must_use]
    pub fn integer(n: i128) -> Decimal {
        Decimal::parse(&n.to_string()).unwrap_or_else(Decimal::zero)
    }

    /// Whether it is past `MAX_DIGITS`.
    #[must_use]
    pub fn is_too_long(&self) -> bool {
        self.digits.len() > MAX_DIGITS || self.scale > MAX_DIGITS
    }

    /// Whether it is zero.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// Whether it is less than zero.
    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// Whether it has no fraction.
    #[must_use]
    pub fn is_integer(&self) -> bool {
        self.scale == 0
    }

    /// It negated.
    #[must_use]
    pub fn negated(&self) -> Decimal {
        Decimal::of(!self.negative, self.digits.clone(), self.scale)
    }

    /// Its magnitude.
    #[must_use]
    pub fn abs(&self) -> Decimal {
        Decimal::of(false, self.digits.clone(), self.scale)
    }

    /// Its digits brought to `scale`, not less than its own.
    fn at_scale(&self, scale: usize) -> Vec<u8> {
        shifted(&self.digits, scale.saturating_sub(self.scale))
    }

    /// The order of it and `other`, a total order [Schema2 §3.2.3].
    #[must_use]
    pub fn compare(&self, other: &Decimal) -> Ordering {
        let scale = self.scale.max(other.scale);
        let magnitude = compare_magnitudes(&self.at_scale(scale), &other.at_scale(scale));
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => magnitude,
            (true, true) => magnitude.reverse(),
        }
    }

    /// Its sum with `other`.
    #[must_use]
    pub fn add(&self, other: &Decimal) -> Decimal {
        let scale = self.scale.max(other.scale);
        let (a, b) = (self.at_scale(scale), other.at_scale(scale));
        if self.negative == other.negative {
            return Decimal::of(self.negative, add_magnitudes(&a, &b), scale);
        }
        match compare_magnitudes(&a, &b) {
            Ordering::Less => Decimal::of(other.negative, subtract_magnitudes(&b, &a), scale),
            Ordering::Equal | Ordering::Greater => Decimal::of(self.negative, subtract_magnitudes(&a, &b), scale),
        }
    }

    /// Its difference from `other`.
    #[must_use]
    pub fn subtract(&self, other: &Decimal) -> Decimal {
        self.add(&other.negated())
    }

    /// Its product with `other`.
    #[must_use]
    pub fn multiply(&self, other: &Decimal) -> Decimal {
        let digits = multiply_magnitudes(&self.digits, &other.digits);
        Decimal::of(self.negative != other.negative, digits, self.scale.saturating_add(other.scale))
    }

    /// Its quotient by `other`, truncated toward zero to `scale` digits
    /// after the period; none if `other` is zero.
    #[must_use]
    pub fn divide(&self, other: &Decimal, scale: usize) -> Option<Decimal> {
        if other.is_zero() {
            return None;
        }
        // a / b = A·10^-sa / (B·10^-sb), so its digits to `scale` places are
        // A·10^(sb + scale) / (B·10^sa), truncated.
        let numerator = shifted(&self.digits, other.scale.saturating_add(scale));
        let denominator = shifted(&other.digits, self.scale);
        let quotient = divide_magnitudes(&numerator, &denominator);
        Some(Decimal::of(self.negative != other.negative, quotient, scale))
    }

    /// Its quotient by `other` as `op:numeric-divide` makes it of decimals
    /// [XQueryFO §6.2.4]: exact if it ends within `DIVISION_SCALE` digits,
    /// or the scale of either operand, after the period; else truncated
    /// there. None if `other` is zero.
    #[must_use]
    pub fn quotient(&self, other: &Decimal) -> Option<Decimal> {
        self.divide(other, DIVISION_SCALE.max(self.scale).max(other.scale))
    }

    /// Its remainder by `other`, of its own sign [XQueryFO §6.2.6]: it
    /// less `other` times their quotient truncated to an integer; none if
    /// `other` is zero.
    #[must_use]
    pub fn remainder(&self, other: &Decimal) -> Option<Decimal> {
        let whole = self.divide(other, 0)?;
        Some(self.subtract(&whole.multiply(other)))
    }

    /// It rounded to `precision` digits after the period, or, if
    /// `precision` is negative, to a multiple of ten to the minus
    /// `precision`, as `mode` says [XQueryFO §6.4].
    #[must_use]
    pub fn round(&self, precision: i64, mode: Rounding) -> Decimal {
        let scale = i128::try_from(self.scale).unwrap_or(i128::MAX);
        let dropped = scale.saturating_sub(i128::from(precision));
        if dropped <= 0 {
            return self.clone();
        }
        let drop = usize::try_from(dropped).unwrap_or(usize::MAX);
        let kept: Vec<u8> = self.digits.iter().skip(drop).copied().collect();
        let first = drop.checked_sub(1).and_then(|at| self.digits.get(at)).copied().unwrap_or(0);
        let rest = self.digits.iter().take(drop.saturating_sub(1)).any(|&d| d != 0);
        let up = self.rounds_up(mode, (first, rest), kept.first().copied().unwrap_or(0));
        let kept = if up { add_magnitudes(&kept, &[1]) } else { kept };
        if let Ok(places) = usize::try_from(precision) {
            return Decimal::of(self.negative, kept, places);
        }
        // Past MAX_DIGITS the result is too long anyway, so it is not made
        // longer: its length alone tells the caller so.
        let places = usize::try_from(precision.unsigned_abs()).unwrap_or(usize::MAX).min(MAX_DIGITS.saturating_add(1));
        Decimal::of(self.negative, shifted(&kept, places), 0)
    }

    /// Whether a rounding by `mode` adds one to the magnitude kept, its last
    /// digit `last`, of the digits dropped `(first, rest)`: their first,
    /// and whether any after it is not zero.
    fn rounds_up(&self, mode: Rounding, (first, rest): (u8, bool), last: u8) -> bool {
        let dropped = first != 0 || rest;
        let half = first.cmp(&HALF).then(if rest { Ordering::Greater } else { Ordering::Equal });
        match mode {
            Rounding::Truncate => false,
            Rounding::Floor => self.negative && dropped,
            Rounding::Ceiling => !self.negative && dropped,
            Rounding::HalfUp => half == Ordering::Greater || (half == Ordering::Equal && !self.negative),
            Rounding::HalfEven => half == Ordering::Greater || (half == Ordering::Equal && last.checked_rem(2) == Some(1)),
        }
    }

    /// Its digits as `fn:string` writes them [XQueryFO §17.1.2]: an
    /// integer's without a period, else its canonical representation
    /// [Schema2 §3.2.3.2].
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.digits.len().saturating_add(3));
        if self.negative {
            out.push('-');
        }
        let whole: String = self.digits.iter().skip(self.scale).rev().map(|&d| char::from(b'0'.wrapping_add(d))).collect();
        out.push_str(if whole.is_empty() { "0" } else { &whole });
        if self.scale > 0 {
            out.push('.');
            out.extend((0..self.scale).rev().map(|at| char::from(b'0'.wrapping_add(self.digits.get(at).copied().unwrap_or(0)))));
        }
        out
    }

    /// It as the nearest `i128`, if it is an integer within one.
    #[must_use]
    pub fn to_i128(&self) -> Option<i128> {
        if !self.is_integer() {
            return None;
        }
        self.text().parse().ok()
    }
}

#[cfg(test)]
mod tests;
