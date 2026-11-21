//! The comparison key of a value of the JSON type [ISO/IEC 9075-2:2023
//! §4.8.2, §8.2 GR10]: octets that, compared as unsigned octets from the
//! first, order SQL/JSON values as §8.2 GR10 orders them, the same octets
//! exactly when the values are equivalent [§4.48.3]. Equality (Feature
//! T879), grouping (T880), ordering (T881) and multiset element grouping
//! (T882) of JSON values compare these keys, so each is a total order and
//! transitive.
//!
//! The implementation-defined choices are these (IA224, IA225, IA228,
//! IA229):
//!
//! - the SQL/JSON null is first, then scalars, then arrays, then objects
//!   [GR10b i to iii];
//! - of scalars, numbers before character strings, those before truth
//!   values, those before datetimes \[GR10b iv\];
//! - numbers by their exact values, an approximate number's being the
//!   decimal its binary fraction is exactly, as SQL compares an exact
//!   number with an approximate one \[GR10b v\];
//! - character strings by their code points, no character padded or
//!   ignored, as UCS_BASIC orders them \[GR10b vi\];
//! - of datetimes, times before dates and timestamps, a date compared as
//!   the timestamp of its midnight, each by its instant \[GR10b viii\];
//! - arrays element by element, a shorter array that is a prefix of a
//!   longer one first \[GR10b ix\];
//! - an object as the list of its members sorted by key and then by
//!   value, compared member by member as an array is \[GR10b x\], so two
//!   objects are equivalent exactly when their sorted members are, a
//!   bijection of equivalent members [§4.48.3] that duplicate keys do not
//!   confuse.
//!
//! ```text
//! null:      1
//! number:    2 | sign (1 negative, 2 zero, 3 positive) | exponent u32
//!            big-endian, biased | digits as ASCII | 0, all but the sign
//!            inverted if negative; -∞ sign 0, +∞ 4, NaN 5
//! string:    3 | octets, each 0 written 0 255 | 0 0
//! boolean:   4 | 0 or 1
//! datetime:  5 | 0 time, 1 date or timestamp | instant i64 big-endian,
//!            its sign bit flipped
//! array:     6 | each element: 1 | its key | then 0
//! object:    7 | each member, sorted: 1 | key as a string's | value's key
//!            | then 0
//! ```
//!
//! A number's digits have no leading or trailing zeros, and its exponent
//! is that of its first digit, so equal numbers have one key.

use std::cmp::Ordering;

use super::{Node, NodeKind, root, within};
use crate::error::{Code, Result, SqlError};
use crate::json::text::JSON_EXACT_DIGITS;
use crate::json::{Item, Member, Scalar};
use crate::value::datetime::Moment;
use crate::value::{Datetime, DecFloat, Decimal, MAX_DECFLOAT_PRECISION, Value};

// The tags of a key's nodes, in the order §8.2 GR10 and the choices above
// give them.
/// The SQL/JSON null.
const NULL_KEY: u8 = 1;
/// A number.
const NUMBER_KEY: u8 = 2;
/// A character string.
const STRING_KEY: u8 = 3;
/// A truth value.
const BOOLEAN_KEY: u8 = 4;
/// A datetime.
const DATETIME_KEY: u8 = 5;
/// An array.
const ARRAY_KEY: u8 = 6;
/// An object.
const OBJECT_KEY: u8 = 7;
/// Before each element or member of a key.
const MORE: u8 = 1;
/// After the last element or member, and a number's digits.
const END: u8 = 0;
/// After a string's 0 octet, so the string goes on; a 0 then `END` ends it.
const ESCAPED_ZERO: u8 = 255;

// A number's sign octets.
/// Negative infinity.
const MINUS_INFINITY: u8 = 0;
/// A negative number.
const NEGATIVE: u8 = 1;
/// Zero.
const ZERO: u8 = 2;
/// A positive number.
const POSITIVE: u8 = 3;
/// Positive infinity.
const PLUS_INFINITY: u8 = 4;
/// Not a number.
const NOT_A_NUMBER: u8 = 5;
/// What a number's exponent is biased by, so its octets order as it does.
const EXPONENT_BIAS: i64 = 1 << 31;

// A datetime's class octets.
/// A time, of a time zone or not.
const TIME_CLASS: u8 = 0;
/// A date or a timestamp.
const INSTANT_CLASS: u8 = 1;
/// The octets of an instant.
const INSTANT_BYTES: usize = 8;
/// The sign bit of an instant, flipped so the octets order as it does.
const SIGN_BIT: u64 = 1 << 63;

/// The bits of an f64's biased exponent.
const F64_EXPONENT_MASK: u64 = 0x7FF;
/// Where an f64's exponent begins, after its fraction.
const F64_FRACTION_BITS: u32 = 52;
/// The biased exponent of an f64 below which it has fraction digits: its
/// bias and its fraction's bits. One of exponent `e` is a multiple of
/// 2^(e − this), so has this − `e` binary, and as many decimal, digits after
/// its point.
const F64_POINT: u64 = 1075;

/// The comparison key of the document `bytes`.
///
/// # Errors
///
/// SQLCODE -916 if `bytes` is no document of the format.
pub fn document_key(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    node_key(root(bytes)?, &mut out, 0)?;
    Ok(out)
}

/// [§8.2 GR10] How the document `a` compares with the document `b`, by
/// their keys.
///
/// # Errors
///
/// As `document_key`.
pub fn compare_documents(a: &[u8], b: &[u8]) -> Result<Ordering> {
    if a == b {
        return Ok(Ordering::Equal);
    }
    Ok(document_key(a)?.cmp(&document_key(b)?))
}

/// Appends the key of `node`, `depth` arrays and objects in, to `out`.
fn node_key(node: Node<'_>, out: &mut Vec<u8>, depth: usize) -> Result<()> {
    within(depth)?;
    match node.kind()? {
        NodeKind::Null => out.push(NULL_KEY),
        NodeKind::Array => {
            out.push(ARRAY_KEY);
            for element in node.elements()?.unwrap_or_default() {
                out.push(MORE);
                node_key(element, out, depth.saturating_add(1))?;
            }
            out.push(END);
        }
        NodeKind::Object => {
            out.push(OBJECT_KEY);
            let mut members = Vec::new();
            for (key, value) in node.members()?.unwrap_or_default() {
                let mut member = Vec::new();
                put_string(key, &mut member);
                node_key(value, &mut member, depth.saturating_add(1))?;
                members.push(member);
            }
            members.sort();
            for member in members {
                out.push(MORE);
                out.extend_from_slice(&member);
            }
            out.push(END);
        }
        NodeKind::Boolean | NodeKind::Number | NodeKind::String | NodeKind::Datetime => {
            let value = node.scalar()?.ok_or_else(|| malformed("a scalar has no value"))?;
            scalar_key(&value, out)?;
        }
    }
    Ok(())
}

/// Appends the key of the SQL/JSON scalar `value` to `out`.
fn scalar_key(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Bool(truth) => {
            out.push(BOOLEAN_KEY);
            out.push(u8::from(*truth));
        }
        Value::Char(s) => {
            out.push(STRING_KEY);
            put_string(s, out);
        }
        Value::Exact(d) => number_key(&d.to_string(), out),
        Value::DecFloat(d) => number_key(&d.numeral(), out),
        Value::Approx(x) => approximate_key(*x, out),
        Value::Datetime(d) => {
            out.push(DATETIME_KEY);
            out.push(if d.moment == Moment::Time { TIME_CLASS } else { INSTANT_CLASS });
            out.extend_from_slice(&(d.utc.cast_unsigned() ^ SIGN_BIT).to_be_bytes());
        }
        Value::Null
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => return Err(malformed("a scalar is of no scalar's type")),
    }
    Ok(())
}

/// Appends a string's octets, each 0 escaped, then its end.
fn put_string(s: &str, out: &mut Vec<u8>) {
    for &b in s.as_bytes() {
        out.push(b);
        if b == END {
            out.push(ESCAPED_ZERO);
        }
    }
    out.push(END);
    out.push(END);
}

/// Appends the key of the approximate number `x`: of its exact decimal
/// value if finite.
fn approximate_key(x: f64, out: &mut Vec<u8>) {
    if !x.is_finite() {
        out.push(NUMBER_KEY);
        out.push(if x.is_nan() {
            NOT_A_NUMBER
        } else if x > 0.0_f64 {
            PLUS_INFINITY
        } else {
            MINUS_INFINITY
        });
        return;
    }
    let exponent = (x.to_bits() >> F64_FRACTION_BITS) & F64_EXPONENT_MASK;
    let digits = usize::try_from(F64_POINT.saturating_sub(exponent.max(1))).unwrap_or(0);
    number_key(&format!("{x:.digits$}"), out);
}

/// Appends the key of the number whose numeral is `numeral`,
/// `[-]digits[.digits]`.
fn number_key(numeral: &str, out: &mut Vec<u8>) {
    out.push(NUMBER_KEY);
    let (negative, magnitude) = numeral.strip_prefix('-').map_or((false, numeral), |rest| (true, rest));
    let (whole, fraction) = magnitude.split_once('.').unwrap_or((magnitude, ""));
    let whole = whole.trim_start_matches('0');
    let (digits, exponent) = if whole.is_empty() {
        let significant = fraction.trim_start_matches('0');
        let zeros = fraction.len().saturating_sub(significant.len());
        (significant.to_owned(), i64::try_from(zeros).map_or(i64::MIN, i64::saturating_neg))
    } else {
        (format!("{whole}{fraction}"), i64::try_from(whole.len()).unwrap_or(i64::MAX))
    };
    let digits = digits.trim_end_matches('0');
    if digits.is_empty() {
        out.push(ZERO);
        return;
    }
    out.push(if negative { NEGATIVE } else { POSITIVE });
    let biased = u32::try_from(exponent.saturating_add(EXPONENT_BIAS).max(0)).unwrap_or(u32::MAX);
    let start = out.len();
    out.extend_from_slice(&biased.to_be_bytes());
    out.extend_from_slice(digits.as_bytes());
    out.push(END);
    if negative {
        for b in out.iter_mut().skip(start) {
            *b = !*b;
        }
    }
}

/// The error of a key that is no key of a JSON value, `why`.
fn malformed(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a JSON value's key is malformed: {why}"))
}

/// The first octet of `rest`, which then holds those after it.
fn next(rest: &mut &[u8]) -> Result<u8> {
    let (&first, after) = rest.split_first().ok_or_else(|| malformed("it ends early"))?;
    *rest = after;
    Ok(first)
}

/// An SQL/JSON item whose key begins `rest`, which then holds the octets
/// after it: one equivalent to the item the key was made of, its object's
/// members in the key's order, a number of the key's digits exact if an
/// exact number holds them, else decimal floating-point, else
/// approximate, and a datetime a timestamp of no time zone.
///
/// # Errors
///
/// SQLCODE -916 if `rest` begins with no key.
pub fn item_of_key(rest: &mut &[u8]) -> Result<Item> {
    item_within(rest, 0)
}

/// `item_of_key`, `depth` arrays and objects in.
fn item_within(rest: &mut &[u8], depth: usize) -> Result<Item> {
    within(depth)?;
    match next(rest)? {
        NULL_KEY => Ok(Item::Null),
        NUMBER_KEY => scalar(number_of_key(rest)?),
        STRING_KEY => scalar(Value::Char(string_of_key(rest)?)),
        BOOLEAN_KEY => match next(rest)? {
            0 => scalar(Value::Bool(false)),
            1 => scalar(Value::Bool(true)),
            _ => Err(malformed("a truth value is neither 0 nor 1")),
        },
        DATETIME_KEY => {
            let class = next(rest)?;
            let (instant, after) = rest.split_first_chunk::<INSTANT_BYTES>().ok_or_else(|| malformed("an instant ends early"))?;
            let instant = *instant;
            *rest = after;
            let utc = (u64::from_be_bytes(instant) ^ SIGN_BIT).cast_signed();
            let moment = if class == TIME_CLASS { Moment::Time } else { Moment::Timestamp };
            scalar(Value::Datetime(Datetime { moment, utc, zone: 0, zoned: false, precision: INSTANT_PRECISION }))
        }
        ARRAY_KEY => {
            let mut elements = Vec::new();
            while next(rest)? == MORE {
                elements.push(item_within(rest, depth.saturating_add(1))?);
            }
            Ok(Item::Array(elements))
        }
        OBJECT_KEY => {
            let mut members = Vec::new();
            while next(rest)? == MORE {
                let key = string_of_key(rest)?;
                members.push(Member { key, value: item_within(rest, depth.saturating_add(1))? });
            }
            Ok(Item::Object(members))
        }
        _ => Err(malformed("its tag is no node's")),
    }
}

/// The digits of a second's fraction of a datetime read from a key: its
/// instant's microseconds.
const INSTANT_PRECISION: u8 = 6;

/// The scalar item of `value`.
fn scalar(value: Value) -> Result<Item> {
    Scalar::new(value).map(Item::Scalar).ok_or_else(|| malformed("a scalar is of no scalar's type"))
}

/// The string of a key, after its tag.
fn string_of_key(rest: &mut &[u8]) -> Result<String> {
    let mut octets = Vec::new();
    loop {
        let b = next(rest)?;
        if b != END {
            octets.push(b);
            continue;
        }
        match next(rest)? {
            END => break,
            ESCAPED_ZERO => octets.push(END),
            _ => return Err(malformed("a string's 0 is not escaped")),
        }
    }
    String::from_utf8(octets).map_err(|_| malformed("a string is not UTF-8"))
}

/// The number of a key, after its tag.
fn number_of_key(rest: &mut &[u8]) -> Result<Value> {
    let sign = next(rest)?;
    let negative = match sign {
        ZERO => return Ok(Value::Exact(Decimal::new(0, 0))),
        MINUS_INFINITY => return Ok(Value::Approx(f64::NEG_INFINITY)),
        PLUS_INFINITY => return Ok(Value::Approx(f64::INFINITY)),
        NOT_A_NUMBER => return Ok(Value::Approx(f64::NAN)),
        NEGATIVE => true,
        POSITIVE => false,
        _ => return Err(malformed("a number's sign is no sign")),
    };
    let flip = |b: u8| if negative { !b } else { b };
    let mut exponent = [0_u8; 4];
    for slot in &mut exponent {
        *slot = flip(next(rest)?);
    }
    let exponent = i64::from(u32::from_be_bytes(exponent)).saturating_sub(EXPONENT_BIAS);
    let mut digits = String::new();
    loop {
        let b = flip(next(rest)?);
        if b == END {
            break;
        }
        digits.push(char::from(b));
    }
    number_of_digits(negative, &digits, exponent)
}

/// The number `0.digits` × 10^`exponent`, negated if `negative`: exact if
/// an exact number holds it, else decimal floating-point if one holds its
/// digits, else approximate, the nearest.
fn number_of_digits(negative: bool, digits: &str, exponent: i64) -> Result<Value> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed("a number's digits are not digits"));
    }
    let count = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let scale = count.saturating_sub(exponent);
    let precision = u32::try_from(count.max(exponent)).unwrap_or(u32::MAX);
    if let (Ok(scale), true) = (u32::try_from(scale), precision <= JSON_EXACT_DIGITS) {
        let units: i128 = digits.parse().map_err(|_| malformed("a number's digits are not digits"))?;
        return Ok(Value::Exact(Decimal::new(if negative { units.saturating_neg() } else { units }, scale)));
    }
    if let Some(d) = DecFloat::of_digits(negative, digits, scale.saturating_neg(), MAX_DECFLOAT_PRECISION)
        .filter(|_| u32::try_from(digits.len()).is_ok_and(|n| n <= MAX_DECFLOAT_PRECISION))
    {
        return Ok(Value::DecFloat(d));
    }
    let sign = if negative { "-" } else { "" };
    let x: f64 = format!("{sign}0.{digits}e{exponent}").parse().map_err(|_| malformed("a number's digits are not digits"))?;
    Ok(Value::Approx(x))
}
