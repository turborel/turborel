//! Comparison of values [X3.135-1986 §5.11]: character strings with padding,
//! and numbers by algebraic value, exact and approximate alike; datetimes and
//! intervals as [ISO/IEC 9075:1992 §8.2] says, binary strings as [ISO/IEC
//! 9075-2:2023 §8.2] does; a value beyond the standard as its kind says.
//!
//! A large object is compared for equality alone [ISO/IEC 9075-2:2023
//! §9.14 SR3], and reading it may fail: `compare` compares those held
//! inline, and `compare_checked` any, reading them first (`proxies`).

use std::cmp::Ordering;

use super::lob::Lob;
use super::types::composite::{bucket_parts, compare_parts};
use super::types::{BINARY_VARYING, MAX_TYPE_NESTING, SqlType};
use super::{DecFloat, Decimal, Value};
use crate::error::{Code, Result, SqlError};

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

/// `a` and `b`, one a large object: as its kind compares them if held
/// inline; else none, as `compare_checked` reads them.
fn compare_lob(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Lob(l), _) => super::types::lob::kind_of(l.kind()).compare(a, b),
        (_, Value::Lob(l)) => super::types::lob::kind_of(l.kind()).compare(b, a).map(Ordering::reverse),
        (
            Value::Null
            | Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Extension(_)
            | Value::Xml(_),
            _,
        ) => None,
    }
}

/// [X3.135-1986 §5.11] compare two values. `None` means the comparison is unknown
/// because one side is null ([X3.135-1986 §5.11 GR2]), or the values are not comparable.
pub fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Structured(_) | Value::Reference(_) | Value::Array(_) | Value::Multiset(_) | Value::Row(_), _)
        | (_, Value::Structured(_) | Value::Reference(_) | Value::Array(_) | Value::Multiset(_) | Value::Row(_)) => {
            compare_composites(a, b)
        }
        (Value::Lob(_), _) | (_, Value::Lob(_)) => compare_lob(a, b),
        // A null makes the comparison unknown [X3.135-1986 §5.11 GR2]; no XML
        // value is comparable [ISO/IEC 9075-14:2006 draft §4.2.4]; a character
        // string and a number are not comparable [X3.135-1986 §5.11 SR1], nor a
        // binary string with either: binary strings are with each other
        // [ISO/IEC 9075-2:2023 §4.4.1]; nor a datetime or an interval with
        // another kind.
        (Value::Null | Value::Xml(_), _)
        | (_, Value::Null | Value::Xml(_))
        | (Value::Char(_), Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_))
        | (Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_), Value::Char(_))
        | (Value::Binary(_), Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Datetime(_))
        | (Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Datetime(_), Value::Binary(_))
        | (Value::Datetime(_), Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_))
        | (Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_), Value::Datetime(_))
        | (
            Value::Interval(_),
            Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Binary(_) | Value::Datetime(_),
        )
        | (
            Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Binary(_) | Value::Datetime(_),
            Value::Interval(_),
        ) => None,
        (Value::Char(x), Value::Char(y)) => Some(compare_strings(x, y)),
        // [ISO/IEC 9075-2:2023 §4.5.1] any two numbers are comparable.
        (Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_), Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_)) => {
            compare_numbers(a, b)
        }
        // [ISO/IEC 9075-2:2023 §8.2 GR5] octet by octet.
        (Value::Binary(x), Value::Binary(y)) => Some(x.cmp(y)),
        // [ISO/IEC 9075-2:2023 §4.6.2] True is greater than False.
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        // [ISO/IEC 9075:1992 §8.2 GR5] by their instants.
        (Value::Datetime(x), Value::Datetime(y)) => x.compare(y),
        // [ISO/IEC 9075:1992 §8.2 GR6] of one class, by their units.
        (Value::Interval(x), Value::Interval(y)) => (x.qualifier.year_month() == y.qualifier.year_month()).then(|| x.units.cmp(&y.units)),
        // A value of a kind beyond the standard compares as its kind says.
        (Value::Extension(e), _) => e.kind.compare(a, b),
        (_, Value::Extension(e)) => e.kind.compare(b, a).map(Ordering::reverse),
        // [ISO/IEC 9075-2:2023 §4.6.2] a truth value with truth values alone.
        (Value::Bool(_), _) | (_, Value::Bool(_)) => None,
    }
}

/// [ISO/IEC 9075-2:2023 §8.2] `compare`, of values that may be or hold
/// large objects, each read as it must be (`proxies`).
///
/// # Errors
///
/// As reading a large object fails; SQLCODE -910 if a value nests deeper
/// than a type may.
pub fn compare_checked(a: &Value, b: &Value) -> Result<Option<Ordering>> {
    Ok(match proxies(a, b)? {
        Some((x, y)) => compare(&x, &y),
        None => compare(a, b),
    })
}

/// [ISO/IEC 9075-2:2023 §8.2 GR3, GR5] `a` and `b` with each large object
/// of one and a string of its family at the same place in the other, in
/// arrays and rows alike, compared, read a chunk at a time, and both made
/// truth values, TRUE and whether they are equal: so `compare` and the
/// rules of rows and arrays give "=" and "<>", the operations a large
/// object takes [§9.14 SR3], as of the values. None if neither holds a
/// large object, as most do not: then the values themselves serve.
///
/// # Errors
///
/// As `compare_checked`.
pub fn proxies(a: &Value, b: &Value) -> Result<Option<(Value, Value)>> {
    if !holds_lob(a, 0)? && !holds_lob(b, 0)? {
        return Ok(None);
    }
    proxied(a, b, 0, Some(&binary_equal)).map(Some)
}

/// \[DESIGN\] Two large objects of one kind equal as `Lob::compare_to`
/// says: of the default collation, or binary.
fn binary_equal(a: &Lob, b: &Lob) -> Result<bool> {
    Ok(a.compare_to(b)?.is_eq())
}

/// How two large objects are told equal: under a collation, or not.
pub type LobEquality<'e> = &'e dyn Fn(&Lob, &Lob) -> Result<bool>;

/// `proxies` of `a` and `b`, two large objects at one place told equal by
/// `equal`; if none, not read, as of the first's proxy, which is TRUE
/// whether they are equal or not.
///
/// # Errors
///
/// As `compare_checked`.
pub fn lob_proxies(a: &Value, b: &Value, equal: Option<LobEquality<'_>>) -> Result<(Value, Value)> {
    proxied(a, b, 0, equal)
}

/// `proxies` of each pair of `xs` and `ys`, the fields of two rows: none
/// if no field holds a large object.
///
/// # Errors
///
/// As `compare_checked`.
pub fn row_proxies(xs: &[Value], ys: &[Value]) -> Result<Option<(Vec<Value>, Vec<Value>)>> {
    if !any_lob(xs)? && !any_lob(ys)? {
        return Ok(None);
    }
    let (mut ps, mut qs) = (Vec::with_capacity(xs.len()), Vec::with_capacity(ys.len()));
    for (x, y) in xs.iter().zip(ys) {
        let (p, q) = proxied(x, y, 0, Some(&binary_equal))?;
        ps.push(p);
        qs.push(q);
    }
    // Fields past the shorter, of rows not of one degree, which no rule
    // pairs.
    ps.extend(xs.iter().skip(ys.len()).cloned());
    qs.extend(ys.iter().skip(xs.len()).cloned());
    Ok(Some((ps, qs)))
}

/// Whether any of `values` is or holds a large object.
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
pub fn any_lob(values: &[Value]) -> Result<bool> {
    for v in values {
        if holds_lob(v, 0)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `v` is or holds a large object, `depth` levels into arrays and
/// rows.
fn holds_lob(v: &Value, depth: usize) -> Result<bool> {
    if depth > MAX_TYPE_NESTING {
        return Err(too_deep());
    }
    match v {
        Value::Lob(_) => Ok(true),
        Value::Array(parts) | Value::Multiset(parts) | Value::Row(parts) => any_held(parts, depth),
        Value::Structured(s) => any_held(&s.attributes, depth),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Reference(_)
        | Value::Extension(_)
        | Value::Xml(_) => Ok(false),
    }
}

/// Whether any of `parts`, `depth` levels in, holds a large object.
fn any_held(parts: &[Value], depth: usize) -> Result<bool> {
    for p in parts {
        if holds_lob(p, depth.saturating_add(1))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `proxies`, `depth` levels into arrays and rows, two large objects told
/// equal by `equal`, or not read if none.
fn proxied(a: &Value, b: &Value, depth: usize, equal: Option<LobEquality<'_>>) -> Result<(Value, Value)> {
    if depth > MAX_TYPE_NESTING {
        return Err(too_deep());
    }
    if let (Value::Array(x), Value::Array(y)) | (Value::Row(x), Value::Row(y)) = (a, b)
        && x.len() == y.len()
    {
        let (mut xs, mut ys) = (Vec::with_capacity(x.len()), Vec::with_capacity(y.len()));
        for (p, q) in x.iter().zip(y.iter()) {
            let (p, q) = proxied(p, q, depth.saturating_add(1), equal)?;
            xs.push(p);
            ys.push(q);
        }
        let wrap = |parts: Vec<Value>| if matches!(a, Value::Array(_)) { Value::Array(parts.into()) } else { Value::Row(parts.into()) };
        return Ok((wrap(xs), wrap(ys)));
    }
    // A large object and the null value: TRUE for the large object, so
    // the proxies are as null as the values.
    if let (Value::Lob(_), Value::Null) = (a, b) {
        return Ok((Value::Bool(true), Value::Null));
    }
    if let (Value::Null, Value::Lob(_)) = (a, b) {
        return Ok((Value::Null, Value::Bool(true)));
    }
    if let (Value::Lob(l), other) | (other, Value::Lob(l)) = (a, b)
        && let Some(them) = super::types::lob::as_lob(other, l.kind())
    {
        let same = match equal {
            Some(equal) => equal(l, &them)?,
            None => true,
        };
        return Ok((Value::Bool(true), Value::Bool(same)));
    }
    Ok((a.clone(), b.clone()))
}

/// A value nested deeper than any type nests.
fn too_deep() -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] a value nested deeper than the {MAX_TYPE_NESTING} levels a type may"))
}

/// [ISO/IEC 9075-2:2023 §8.2 GR1b] Two arrays pair by pair, their
/// cardinalities first \[GR1b ii\]; two rows of one degree likewise
/// \[GR1b i\]; two multisets equal or not as their elements pair
/// \[GR1b iii\], unequal as less; none if either is null, or of another
/// kind [§4.10, §4.12.4].
fn compare_composites(a: &Value, b: &Value) -> Option<Ordering> {
    // [ISO/IEC 9075-2:2023 §4.9.5, §8.2 GR9] As their keys, or their
    // identities (`types::structured`); references by their octets.
    if let (Value::Structured(x), Value::Structured(y)) = (a, b) {
        return super::types::structured::compare_structured(x, y);
    }
    if let (Value::Reference(x), Value::Reference(y)) = (a, b) {
        return Some(x.bytes().cmp(y.bytes()));
    }
    if let (Value::Multiset(x), Value::Multiset(y)) = (a, b) {
        return super::multisets::equal(x, y).map(|same| if same { Ordering::Equal } else { Ordering::Less });
    }
    if let (Value::Array(x), Value::Array(y)) | (Value::Row(x), Value::Row(y)) = (a, b)
        && (matches!(a, Value::Array(_)) || x.len() == y.len())
    {
        return compare_parts(x, y);
    }
    None
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
        (Value::DecFloat(x), Value::DecFloat(y)) => Some(x.cmp(y)),
        (Value::DecFloat(x), Value::Exact(y)) => Some(x.cmp(&DecFloat::of_decimal(y)?)),
        (Value::Exact(x), Value::DecFloat(y)) => Some(DecFloat::of_decimal(x)?.cmp(y)),
        (Value::DecFloat(x), Value::Approx(y)) => compare_decfloat_approximate(x, *y),
        (Value::Approx(x), Value::DecFloat(y)) => compare_decfloat_approximate(y, *x).map(Ordering::reverse),
        (
            Value::Null
            | Value::Char(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Extension(_)
            | Value::Xml(_),
            _,
        )
        | (
            _,
            Value::Null
            | Value::Char(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Extension(_)
            | Value::Xml(_),
        ) => None,
    }
}

/// A hashable form of a value, shared by every two values that compare
/// equal [X3.135-1986 §5.11 GR4, GR5]: a string without trailing spaces, or
/// a number's nearest `f64`, as an exact and an approximate number compare
/// equal only if the exact one's nearest `f64` is the approximate one
/// (`compare_exact_approximate`); of the other kinds, as each variant says. Values that are not equal may share it
/// (two exact numbers nearest the same `f64`), so a hash join compares the
/// values it pairs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// A character string, without trailing spaces.
    Char(String),
    /// The bits of a number's nearest `f64`, with -0 as +0.
    Number(u64),
    /// A binary string, as its key: boxed, so a bucket of a number or a
    /// character string stays small.
    Binary(Box<[u8]>),
    /// A truth value.
    Bool(bool),
    /// A datetime's instant, its microseconds in UTC.
    Instant(i64),
    /// An interval's months or microseconds.
    Span(i128),
    /// A value of a type beyond the standard, as its kind buckets it: boxed,
    /// so a bucket of the standard's types stays small.
    Extension(Box<[u8]>),
    /// An array's or a row's parts, each's bucket.
    Composite(Box<[Bucket]>),
    /// A multiset's elements' buckets, their hashes summed, the same in
    /// any order.
    Bag(u64),
    /// [ISO/IEC 9075-2:2023 §8.15] The null value, of a join key on which
    /// a null matches a null (`key_bucket`); `bucket` gives none.
    Null,
}

/// The bucket of `v`, or none for the null value, which equals nothing
/// [X3.135-1986 §5.11 GR2].
pub fn bucket(v: &Value) -> Option<Bucket> {
    match v {
        // None for a large object too: none is grouped or hashed
        // [ISO/IEC 9075-2:2023 §9.12 SR3].
        // None for an XML value: none is grouped [ISO/IEC 9075-14:2006 draft
        // §10.9 SR1].
        Value::Null | Value::Lob(_) | Value::Xml(_) => None,
        Value::Char(s) => Some(Bucket::Char(s.trim_end_matches(' ').to_owned())),
        Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) => number_bucket(v),
        Value::Binary(_) => BINARY_VARYING.bucket(v),
        Value::Bool(b) => Some(Bucket::Bool(*b)),
        Value::Datetime(d) => Some(Bucket::Instant(d.utc)),
        Value::Interval(i) => Some(Bucket::Span(i.units)),
        Value::Array(p) | Value::Row(p) => bucket_parts(p),
        Value::Multiset(p) => super::multisets::bag_bucket(p),
        Value::Structured(s) => super::types::structured::bucket_structured(s),
        Value::Reference(r) => Some(Bucket::Binary(r.bytes().into())),
        Value::Extension(e) => e.kind.bucket(v),
    }
}

/// The bucket of a number: its nearest `f64`'s bits, -0 as +0; none if it
/// is no number.
pub(crate) fn number_bucket(v: &Value) -> Option<Bucket> {
    let f = match v {
        Value::Exact(d) => d.nearest(),
        Value::Approx(f) => *f,
        Value::DecFloat(d) => d.to_f64(),
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Extension(_)
        | Value::Xml(_) => {
            return None;
        }
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

/// [ISO/IEC 9075-2:2023 §8.2 GR3] Compares a DECFLOAT and an approximate
/// number by value, as `compare_exact_approximate` does: by their `f64`s,
/// and if those are one, by their digits.
fn compare_decfloat_approximate(d: &DecFloat, approx: f64) -> Option<Ordering> {
    if approx.is_infinite() {
        return Some(if approx > 0.0_f64 { Ordering::Less } else { Ordering::Greater });
    }
    match d.to_f64().partial_cmp(&approx)? {
        Ordering::Equal => Some(compare_numerals(&d.numeral(), &format!("{approx:.F64_EXACT_FRACTION_DIGITS$}"))),
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
