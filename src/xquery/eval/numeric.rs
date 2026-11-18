//! Numbers and their operators [XQueryFO §6]: `xs:integer`, `xs:decimal`,
//! `xs:float` and `xs:double`, two operands promoted to their common type
//! [XQuery Appendix B.1], the arithmetic of op:numeric-add and the rest
//! [XQueryFO §6.2], and the comparison of op:numeric-equal and the rest
//! [XQueryFO §6.3].
//!
//! \[DESIGN\] RULES.md D86, implementation-defined [XQueryFO §6.2]: an
//! `xs:integer` or `xs:decimal` is computed in 128 bits, its units and its
//! scale, so a result past them is err:FOAR0002; a quotient of decimals
//! is truncated to `DIVISION_SCALE` digits after the point. Exact numbers
//! of any digits compare exactly, by their canonical forms. Float and
//! double arithmetic is IEEE 754's, overflow an infinity.

use std::cmp::Ordering;
use std::num::FpCategory;

use super::super::error::{ErrorCode, error};
use super::super::plan::Arith;
use crate::error::Result;
use crate::value::round_to_single;
use crate::xml::{Atomic, AtomicType, Primitive, double, real_text};

/// \[DESIGN\] Implementation-defined [XQueryFO §6.2]: the digits after the
/// point a quotient of decimals has at most, more truncated.
pub const DIVISION_SCALE: u32 = 18;
/// The radix of decimals.
const TEN: i128 = 10;

/// A number [XQueryFO §6.1].
#[derive(Debug, Clone, Copy)]
pub(super) enum Num {
    /// An `xs:integer`.
    Integer(i128),
    /// An `xs:decimal`: its units and scale.
    Decimal(i128, u32),
    /// An `xs:float`, as the `f64` it widens to.
    Float(f64),
    /// An `xs:double`.
    Double(f64),
}

/// The error of an exact result past 128 bits [XQueryFO §6.2].
fn overflow() -> crate::error::SqlError {
    error(ErrorCode::FOAR0002, "the exact result is past the 128 bits turborel computes in [XQueryFO §6.2]")
}

/// The error of an exact division by zero [XQueryFO §6.2.4].
fn by_zero() -> crate::error::SqlError {
    error(ErrorCode::FOAR0001, "a decimal or integer is divided by zero [XQueryFO §6.2.4]")
}

/// 10^`n`, if it fits.
fn pow10(n: u32) -> Option<i128> {
    TEN.checked_pow(n)
}

/// The number `a` is, if it is of a numeric type.
///
/// # Errors
///
/// err:FOAR0002 of an exact number past 128 bits.
pub(super) fn of(a: &Atomic) -> Result<Option<Num>> {
    let p = a.kind.primitive();
    Ok(Some(if p == Primitive::Integer {
        Num::Integer(a.lexical.parse().map_err(|_| overflow())?)
    } else if p == Primitive::Decimal {
        let (units, scale) = decimal_parts(&a.lexical).ok_or_else(overflow)?;
        Num::Decimal(units, scale)
    } else if p == Primitive::Float {
        Num::Float(double(&a.lexical).unwrap_or(f64::NAN))
    } else if p == Primitive::Double {
        Num::Double(double(&a.lexical).unwrap_or(f64::NAN))
    } else {
        return Ok(None);
    }))
}

/// The units and scale of the canonical decimal `text`, if 128 bits hold
/// them.
fn decimal_parts(text: &str) -> Option<(i128, u32)> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = format!("{whole}{fraction}");
    Some((digits.parse().ok()?, u32::try_from(fraction.len()).ok()?))
}

/// The canonical string of the decimal of `units` and `scale` [XQueryFO
/// §17.1.2]: no zero after the point last, no point of an integer.
fn decimal_text(units: i128, scale: u32) -> String {
    let digits = units.unsigned_abs().to_string();
    let scale = usize::try_from(scale).unwrap_or(0);
    let padded = format!("{digits:0>width$}", width = scale.saturating_add(1));
    let (whole, fraction) = padded.split_at(padded.len().saturating_sub(scale));
    let fraction = fraction.trim_end_matches('0');
    let sign = if units < 0 { "-" } else { "" };
    if fraction.is_empty() { format!("{sign}{whole}") } else { format!("{sign}{whole}.{fraction}") }
}

/// `n` as an atomic value of its type [XQueryFO §17.1.2].
pub(super) fn atomic(n: Num) -> Atomic {
    let (kind, lexical) = match n {
        Num::Integer(i) => (AtomicType::Integer, i.to_string()),
        Num::Decimal(units, scale) => (AtomicType::Decimal, decimal_text(units, scale)),
        Num::Float(x) => (AtomicType::Float, real_text(x, true)),
        Num::Double(x) => (AtomicType::Double, real_text(x, false)),
    };
    Atomic { kind, lexical: lexical.into() }
}

/// Where `n`'s type is in the order of promotion [XQuery Appendix B.2].
fn rank(n: Num) -> u8 {
    match n {
        Num::Integer(_) => 0,
        Num::Decimal(..) => 1,
        Num::Float(_) => 2,
        Num::Double(_) => 3,
    }
}

/// `n` as a double.
pub(super) fn to_f64(n: Num) -> f64 {
    match n {
        Num::Integer(i) => double(&i.to_string()).unwrap_or(f64::NAN),
        Num::Decimal(units, scale) => double(&decimal_text(units, scale)).unwrap_or(f64::NAN),
        Num::Float(x) | Num::Double(x) => x,
    }
}

/// `n` promoted to the type of rank `to` [XQuery Appendix B.1].
fn promoted(n: Num, to: u8) -> Num {
    match (n, to) {
        (Num::Integer(i), 1) => Num::Decimal(i, 0),
        (Num::Integer(_) | Num::Decimal(..), 2) => Num::Float(round_to_single(to_f64(n))),
        (Num::Integer(_) | Num::Decimal(..) | Num::Float(_), 3) => Num::Double(to_f64(n)),
        (Num::Integer(_) | Num::Decimal(..) | Num::Float(_) | Num::Double(_), _) => n,
    }
}

/// The decimals of `a` and `b` at one scale, the greater.
fn aligned((a, sa): (i128, u32), (b, sb): (i128, u32)) -> Result<(i128, i128, u32)> {
    let scale = sa.max(sb);
    let up = |units: i128, s: u32| pow10(scale.saturating_sub(s)).and_then(|p| units.checked_mul(p)).ok_or_else(overflow);
    Ok((up(a, sa)?, up(b, sb)?, scale))
}

/// `a op b` [XQueryFO §6.2], the two promoted to their common type: an
/// integer quotient of `div` a decimal, `idiv` an integer always.
///
/// # Errors
///
/// err:FOAR0001 of an exact division by zero or an `idiv` by zero;
/// err:FOAR0002 of an exact overflow, or an `idiv` of NaN or an infinity.
pub(super) fn arithmetic(op: Arith, a: Num, b: Num) -> Result<Num> {
    let to = rank(a).max(rank(b)).max(u8::from(op == Arith::Divide));
    match (promoted(a, to), promoted(b, to)) {
        (Num::Integer(x), Num::Integer(y)) => integer(op, x, y),
        (Num::Decimal(x, sx), Num::Decimal(y, sy)) => decimal(op, (x, sx), (y, sy)),
        (Num::Float(x), Num::Float(y)) => real(op, x, y).map(|r| map_real(&r, |v| Num::Float(round_to_single(v)))),
        (x, y) => real(op, to_f64(x), to_f64(y)).map(|r| map_real(&r, Num::Double)),
    }
}

/// A real result, or an integer of `idiv`.
enum Real {
    /// A float or double.
    Value(f64),
    /// An integer.
    Integer(i128),
}

/// `r` as a number, a real one made by `make`.
fn map_real(r: &Real, make: impl FnOnce(f64) -> Num) -> Num {
    match r {
        Real::Value(v) => make(*v),
        Real::Integer(i) => Num::Integer(*i),
    }
}

/// `x op y` of integers [XQueryFO §6.2].
fn integer(op: Arith, x: i128, y: i128) -> Result<Num> {
    Ok(Num::Integer(match op {
        Arith::Add => x.checked_add(y).ok_or_else(overflow)?,
        Arith::Subtract => x.checked_sub(y).ok_or_else(overflow)?,
        Arith::Multiply => x.checked_mul(y).ok_or_else(overflow)?,
        Arith::IntegerDivide => {
            if y == 0 {
                return Err(by_zero());
            }
            x.checked_div(y).ok_or_else(overflow)?
        }
        // [XQueryFO §6.2.6] of the least integer and -1, 0.
        Arith::Modulo => {
            if y == 0 {
                return Err(by_zero());
            }
            x.checked_rem(y).unwrap_or(0)
        }
        // A quotient of integers is a decimal, promoted before.
        Arith::Divide => return decimal(op, (x, 0), (y, 0)),
    }))
}

/// `x op y` of decimals [XQueryFO §6.2]: a quotient truncated to
/// `DIVISION_SCALE` digits after the point, or fewer if 128 bits hold no
/// more.
fn decimal(op: Arith, x: (i128, u32), y: (i128, u32)) -> Result<Num> {
    match op {
        Arith::Add | Arith::Subtract => {
            let (a, b, scale) = aligned(x, y)?;
            let sum = if op == Arith::Add { a.checked_add(b) } else { a.checked_sub(b) };
            Ok(Num::Decimal(sum.ok_or_else(overflow)?, scale))
        }
        Arith::Multiply => Ok(Num::Decimal(x.0.checked_mul(y.0).ok_or_else(overflow)?, x.1.saturating_add(y.1))),
        Arith::Divide => quotient(x, y),
        Arith::IntegerDivide | Arith::Modulo => {
            let (a, b, scale) = aligned(x, y)?;
            if b == 0 {
                return Err(by_zero());
            }
            if op == Arith::IntegerDivide {
                Ok(Num::Integer(a.checked_div(b).ok_or_else(overflow)?))
            } else {
                Ok(Num::Decimal(a.checked_rem(b).unwrap_or(0), scale))
            }
        }
    }
}

/// `x div y` of decimals: (x / 10^sx) / (y / 10^sy) at the greatest scale
/// up to `DIVISION_SCALE` whose units fit, truncated.
fn quotient((x, sx): (i128, u32), (y, sy): (i128, u32)) -> Result<Num> {
    if y == 0 {
        return Err(by_zero());
    }
    for scale in (0..=DIVISION_SCALE).rev() {
        // x × 10^(scale + sy - sx) / y, of a power not negative.
        let up = scale.saturating_add(sy);
        let units = if up >= sx {
            pow10(up.saturating_sub(sx)).and_then(|p| x.checked_mul(p)).and_then(|n| n.checked_div(y))
        } else {
            pow10(sx.saturating_sub(up)).and_then(|p| y.checked_mul(p)).and_then(|d| x.checked_div(d))
        };
        if let Some(units) = units {
            return Ok(Num::Decimal(units, scale));
        }
    }
    Err(overflow())
}

/// `x op y` of doubles, or of floats widened [XQueryFO §6.2]: IEEE 754's.
fn real(op: Arith, x: f64, y: f64) -> Result<Real> {
    Ok(Real::Value(match op {
        Arith::Add => x + y,
        Arith::Subtract => x - y,
        Arith::Multiply => x * y,
        Arith::Divide => x / y,
        // [XQueryFO §6.2.6] truncating, of the dividend's sign.
        Arith::Modulo => x % y,
        Arith::IntegerDivide => return integer_quotient(x, y).map(Real::Integer),
    }))
}

/// `x idiv y` of doubles [XQueryFO §6.2.5]: by zero err:FOAR0001; of NaN
/// or an infinite dividend err:FOAR0002.
fn integer_quotient(x: f64, y: f64) -> Result<i128> {
    if y.classify() == FpCategory::Zero {
        return Err(error(ErrorCode::FOAR0001, "a number is divided by zero by idiv [XQueryFO §6.2.5]"));
    }
    if x.is_nan() || y.is_nan() || x.is_infinite() {
        return Err(error(ErrorCode::FOAR0002, "idiv of NaN or an infinity [XQueryFO §6.2.5]"));
    }
    whole(x / y)
}

/// The integer `x`'s whole part is, truncated toward zero.
///
/// # Errors
///
/// err:FOAR0002 if it is past 128 bits, or not finite.
pub(super) fn whole(x: f64) -> Result<i128> {
    format!("{:.0}", x.trunc()).parse().map_err(|_| overflow())
}

/// `-n` [XQueryFO §6.2.8].
pub(super) fn negated(n: Num) -> Result<Num> {
    Ok(match n {
        Num::Integer(i) => Num::Integer(i.checked_neg().ok_or_else(overflow)?),
        Num::Decimal(units, scale) => Num::Decimal(units.checked_neg().ok_or_else(overflow)?, scale),
        Num::Float(x) => Num::Float(-x),
        Num::Double(x) => Num::Double(-x),
    })
}

/// How `a` compares with `b`, numbers, promoted to their common type
/// [XQueryFO §6.3]; none if either is NaN.
pub(super) fn compare(a: &Atomic, b: &Atomic) -> Option<Ordering> {
    let exact = |t: AtomicType| matches!(t.primitive(), Primitive::Integer | Primitive::Decimal);
    if exact(a.kind) && exact(b.kind) {
        return Some(exact_order(&a.lexical, &b.lexical));
    }
    let single = a.kind.primitive() != Primitive::Double && b.kind.primitive() != Primitive::Double;
    let value = |t: &Atomic| {
        let x = double(&t.lexical).unwrap_or(f64::NAN);
        if single { round_to_single(x) } else { x }
    };
    value(a).partial_cmp(&value(b))
}

/// How the canonical exact numbers `a` and `b` compare, of any digits.
fn exact_order(a: &str, b: &str) -> Ordering {
    let (na, ma) = a.strip_prefix('-').map_or((false, a), |m| (true, m));
    let (nb, mb) = b.strip_prefix('-').map_or((false, b), |m| (true, m));
    match (na, nb) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => magnitude(ma, mb),
        (true, true) => magnitude(mb, ma),
    }
}

/// How the magnitudes `a` and `b`, canonical, compare: by the digits
/// before the point, then those after.
fn magnitude(a: &str, b: &str) -> Ordering {
    let (wa, fa) = a.split_once('.').unwrap_or((a, ""));
    let (wb, fb) = b.split_once('.').unwrap_or((b, ""));
    wa.len().cmp(&wb.len()).then_with(|| wa.cmp(wb)).then_with(|| fa.cmp(fb))
}

/// Whether the number `a` is equal to `position` [XQuery §3.2.2].
pub(super) fn is_position(a: &Atomic, position: usize) -> bool {
    let place = Atomic { kind: AtomicType::Integer, lexical: position.to_string().into() };
    compare(a, &place) == Some(Ordering::Equal)
}
