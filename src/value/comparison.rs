//! Comparison of values [X3.135-1986 §5.11]: character strings with padding,
//! and numbers by algebraic value, exact and approximate alike. Split from
//! value.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;

use super::types::{BIT_VARYING, SqlType};
use super::{Decimal, Value};

/// [X3.135-1986 §5.11 GR5] compare character strings after padding the shorter with spaces.
/// The collating sequence is implementor-defined [X3.135-1986 §5.11 GR6]: Unicode scalar order.
pub fn compare_strings(left: &str, right: &str) -> Ordering {
    let length = left.chars().count().max(right.chars().count());
    padded(left, length).cmp(padded(right, length))
}

/// The characters of `s` followed by spaces, `length` in all.
fn padded(s: &str, length: usize) -> impl Iterator<Item = char> + '_ {
    s.chars().chain(std::iter::repeat(' ')).take(length)
}

/// [X3.135-1986 §5.11] compare two values. `None` means the comparison is unknown
/// because one side is null ([X3.135-1986 §5.11 GR2]).
pub fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        // A null makes the comparison unknown [X3.135-1986 §5.11 GR2]; a character
        // string and a number are not comparable [X3.135-1986 §5.11 SR1], nor a
        // bit string with either: bit strings are with each other [ISO/IEC
        // 9075:1992 §4.3.1].
        (Value::Null, _)
        | (_, Value::Null)
        | (Value::Char(_), Value::Exact(_))
        | (Value::Char(_), Value::Approx(_))
        | (Value::Exact(_), Value::Char(_))
        | (Value::Approx(_), Value::Char(_))
        | (Value::Bits(_), Value::Char(_) | Value::Exact(_) | Value::Approx(_))
        | (Value::Char(_) | Value::Exact(_) | Value::Approx(_), Value::Bits(_)) => None,
        (Value::Char(x), Value::Char(y)) => Some(compare_strings(x, y)),
        (Value::Exact(_) | Value::Approx(_), Value::Exact(_) | Value::Approx(_)) => compare_numbers(a, b),
        // [ISO/IEC 9075:1992 §8.2 GR4] bit by bit.
        (Value::Bits(x), Value::Bits(y)) => Some(x.cmp(y)),
        // A value of a kind beyond the standard compares as its kind says.
        (Value::Extension(e), _) => e.kind.compare(a, b),
        (_, Value::Extension(e)) => e.kind.compare(b, a).map(Ordering::reverse),
    }
}

/// [X3.135-1986 §5.11 GR4] Two numbers compared by algebraic value, also
/// when one is exact and the other approximate; none if either is no
/// number.
pub(crate) fn compare_numbers(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Exact(x), Value::Exact(y)) => Some(x.cmp_value(y)),
        (Value::Exact(x), Value::Approx(y)) => compare_exact_approximate(x, *y),
        (Value::Approx(x), Value::Exact(y)) => compare_exact_approximate(y, *x).map(Ordering::reverse),
        (Value::Approx(x), Value::Approx(y)) => x.partial_cmp(y),
        (Value::Null | Value::Char(_) | Value::Bits(_) | Value::Extension(_), _)
        | (_, Value::Null | Value::Char(_) | Value::Bits(_) | Value::Extension(_)) => None,
    }
}

/// A hashable form of a value, shared by every two values that compare
/// equal [X3.135-1986 §5.11 GR4, GR5]: a string without trailing spaces, or
/// a number's nearest `f64`, as an exact and an approximate number compare
/// equal only if the exact one's nearest `f64` is the approximate one
/// (`compare_exact_approximate`). Values that are not equal may share it
/// (two exact numbers nearest the same `f64`), so a hash join compares the
/// values it pairs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// A character string, without trailing spaces.
    Char(String),
    /// The bits of a number's nearest `f64`, with -0 as +0.
    Number(u64),
    /// A bit string, as its key: boxed, so a bucket of a number or a
    /// character string stays small.
    Bits(Box<[u8]>),
    /// A value of a type beyond the standard, as its kind buckets it: boxed,
    /// so a bucket of the standard's types stays small.
    Extension(Box<[u8]>),
}

/// The bucket of `v`, or none for the null value, which equals nothing
/// [X3.135-1986 §5.11 GR2].
pub fn bucket(v: &Value) -> Option<Bucket> {
    match v {
        Value::Null => None,
        Value::Char(s) => Some(Bucket::Char(s.trim_end_matches(' ').to_owned())),
        Value::Exact(_) | Value::Approx(_) => number_bucket(v),
        Value::Bits(_) => BIT_VARYING.bucket(v),
        Value::Extension(e) => e.kind.bucket(v),
    }
}

/// The bucket of a number: its nearest `f64`'s bits, -0 as +0; none if it
/// is no number.
pub(crate) fn number_bucket(v: &Value) -> Option<Bucket> {
    let f = match v {
        Value::Exact(d) => d.nearest(),
        Value::Approx(f) => *f,
        Value::Null | Value::Char(_) | Value::Bits(_) | Value::Extension(_) => return None,
    };
    Some(Bucket::Number(if f == 0.0_f64 { 0.0_f64.to_bits() } else { f.to_bits() }))
}

/// Fraction digits that write any finite `f64` exactly: every `f64` is a
/// multiple of 2^-1074, and 2^-1074 = 5^1074 / 10^1074 has 1074 of them.
const F64_EXACT_FRACTION_DIGITS: usize = 1074;

/// [X3.135-1986 §5.11 GR4] Compares an exact and an approximate number by
/// algebraic value. The exact value is converted to the nearest `f64` (Rust's
/// parsing rounds correctly, so the conversion is monotonic and leaves every
/// `f64` unchanged): if that differs from `approx`, the `f64` comparison gives
/// the answer. Only when both round to the same `f64` are their exact decimal
/// digits compared. Comparing through `f64` alone lost digits above 2^53.
fn compare_exact_approximate(exact: &Decimal, approx: f64) -> Option<Ordering> {
    match exact.nearest().partial_cmp(&approx)? {
        Ordering::Equal => Some(compare_numerals(&exact.to_string(), &format!("{approx:.F64_EXACT_FRACTION_DIGITS$}"))),
        Ordering::Less => Some(Ordering::Less),
        Ordering::Greater => Some(Ordering::Greater),
    }
}

/// [X3.135-1986 §5.11 GR4] Compares two decimal numerals of the form
/// `[-]digits[.digits]` by algebraic value.
fn compare_numerals(a: &str, b: &str) -> Ordering {
    let (a_negative, a_digits) = a.strip_prefix('-').map_or((false, a), |rest| (true, rest));
    let (b_negative, b_digits) = b.strip_prefix('-').map_or((false, b), |rest| (true, rest));
    let a_sign = if is_zero_numeral(a_digits) {
        Ordering::Equal
    } else if a_negative {
        Ordering::Less
    } else {
        Ordering::Greater
    };
    let b_sign = if is_zero_numeral(b_digits) {
        Ordering::Equal
    } else if b_negative {
        Ordering::Less
    } else {
        Ordering::Greater
    };
    match (a_sign, b_sign) {
        (Ordering::Greater, Ordering::Greater) => compare_magnitudes(a_digits, b_digits),
        (Ordering::Less, Ordering::Less) => compare_magnitudes(b_digits, a_digits),
        // Different signs, or a zero: the signs alone decide.
        _ => a_sign.cmp(&b_sign),
    }
}

fn is_zero_numeral(digits: &str) -> bool {
    digits.chars().all(|c| c == '0' || c == '.')
}

/// [X3.135-1986 §5.11 GR4] Compares two unsigned numerals `digits[.digits]` by value: integer parts by
/// length then digits, after leading zeros; fractions digit by digit, after trailing zeros.
fn compare_magnitudes(a: &str, b: &str) -> Ordering {
    let (a_int, a_frac) = a.split_once('.').unwrap_or((a, ""));
    let (b_int, b_frac) = b.split_once('.').unwrap_or((b, ""));
    let (a_int, b_int) = (a_int.trim_start_matches('0'), b_int.trim_start_matches('0'));
    a_int
        .len()
        .cmp(&b_int.len())
        .then_with(|| a_int.cmp(b_int))
        .then_with(|| a_frac.trim_end_matches('0').cmp(b_frac.trim_end_matches('0')))
}

// Unit tests: comparison decisions (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// 2^53 + 1: the smallest positive integer an `f64` cannot hold.
    const TWO_53_PLUS_1: i128 = 9_007_199_254_740_993;

    /// Verifies: [X3.135-1986 §5.11 GR4] an exact and an approximate number compare
    /// by algebraic value, in both argument orders, even where they round to the
    /// same `f64`. Regression test: 2^53 + 1 used to compare equal to 2^53E0.
    #[test]
    fn exact_and_approximate_compare_by_algebraic_value() {
        let exact = |units, scale| Value::Exact(Decimal::new(units, scale));
        let two_53 = 9_007_199_254_740_992.0_f64;
        assert_eq!(compare(&exact(TWO_53_PLUS_1, 0), &Value::Approx(two_53)), Some(Ordering::Greater));
        assert_eq!(compare(&Value::Approx(two_53), &exact(TWO_53_PLUS_1, 0)), Some(Ordering::Less));
        assert_eq!(compare(&exact(-TWO_53_PLUS_1, 0), &Value::Approx(-two_53)), Some(Ordering::Less));
        assert_eq!(compare(&exact(1, 0), &Value::Approx(1.0)), Some(Ordering::Equal));
        assert_eq!(compare(&exact(0, 2), &Value::Approx(-0.0)), Some(Ordering::Equal));
        // The f64 nearest 0.1 is 0.1000000000000000055511151231257827..., not 0.1.
        assert_eq!(compare(&exact(10, 2), &Value::Approx(0.1)), Some(Ordering::Less));
        assert_eq!(compare(&exact(1, 0), &Value::Char("1".into())), None);
    }

    /// Verifies: [DESIGN] `compare_numerals` orders by sign first, then by
    /// magnitude, reversed for two negatives; zero has no sign.
    #[test]
    fn numerals_compare_by_sign_then_magnitude() {
        assert_eq!(compare_numerals("012.50", "12.5"), Ordering::Equal); // + +
        assert_eq!(compare_numerals("9.99", "10"), Ordering::Less); // + +
        assert_eq!(compare_numerals("0.05", "0.5"), Ordering::Less); // + +
        assert_eq!(compare_numerals("-2", "-10"), Ordering::Greater); // - -
        assert_eq!(compare_numerals("0.00", "-0"), Ordering::Equal); // 0 0
        assert_eq!(compare_numerals("-1", "0"), Ordering::Less); // - 0
        assert_eq!(compare_numerals("0", "1"), Ordering::Less); // 0 +
        assert_eq!(compare_numerals("1", "-1"), Ordering::Greater); // + -
    }
}
