//! \[DESIGN\] Three-valued logic over truth vectors (RULES.md D362): each
//! a bit of truth and a bit of Unknown a row, a word of 64 rows at a
//! time, by the truth tables of AND, OR and NOT [X3.135-1986 §5.18 GR2],
//! IS [ISO/IEC 9075:1992 §8.12 GR2, Table 15] and IS NULL [X3.135-1986
//! §5.15 GR2-GR4], the last two never unknown. A row true has its bit set
//! and is not unknown; one unknown has its bit unset.

use super::vector::{Vector, nulls as nulls_of, some};
use crate::logical::Truth;
use crate::storage::{Bools, Column, Mask};

/// The truth values of each of `count` rows of `v`: of truth values as
/// they are, the bit of a null cleared, of a constant once, of other values as a row's evaluation
/// reads a BOOLEAN value's truth, a null Unknown [ISO/IEC 9075-2:2023
/// §4.6.1] (`Truth::of`).
pub(super) fn truths(v: Vector<'_>, count: usize) -> Bools {
    match v {
        Vector::Owned(Column::Bools(b)) => made(&b.bits, b.nulls),
        Vector::Column(Column::Bools(b)) => made(&b.bits, b.nulls.clone()),
        Vector::Const(c) => constant(Truth::of(&c), count),
        Vector::Column(_) | Vector::Owned(_) => {
            let seq = v.seq();
            let (mut bits, mut unknown) = (Mask::none(count), Mask::none(count));
            for n in 0..count {
                match Truth::of(seq.get(n)) {
                    Truth::True => bits.set(n),
                    Truth::Unknown => unknown.set(n),
                    Truth::False => {}
                }
            }
            Bools::new(bits, some(unknown))
        }
    }
}

/// `count` rows of the truth value `t`.
fn constant(t: Truth, count: usize) -> Bools {
    let all = Mask::none(count).not();
    match t {
        Truth::True => Bools::new(all, None),
        Truth::False => Bools::new(Mask::none(count), None),
        Truth::Unknown => Bools::new(Mask::none(count), some(all)),
    }
}

/// The rows of `t` that are false: neither true nor unknown.
fn falses(t: &Bools) -> Mask {
    let not_true = t.bits.not();
    t.nulls.as_ref().map_or_else(|| not_true.clone(), |u| not_true.and_not(u))
}

/// Truth values of these bits, unknown where `unknown` says, a bit of a
/// row unknown cleared.
fn made(bits: &Mask, unknown: Option<Mask>) -> Bools {
    let unknown = unknown.and_then(some);
    let bits = unknown.as_ref().map_or_else(|| bits.clone(), |u| bits.and_not(u));
    Bools::new(bits, unknown)
}

/// The unknowns of either.
fn either(a: &Bools, b: &Bools) -> Option<Mask> {
    super::vector::either(a.nulls.as_ref(), b.nulls.as_ref())
}

/// `a AND b`: false if either is false, true if both are true, else
/// unknown.
pub(super) fn and(a: &Bools, b: &Bools) -> Bools {
    let false_of = falses(a).or(&falses(b));
    made(&a.bits.and(&b.bits), either(a, b).map(|u| u.and_not(&false_of)))
}

/// `a OR b`: true if either is true, false if both are false, else
/// unknown.
pub(super) fn or(a: &Bools, b: &Bools) -> Bools {
    let true_of = a.bits.or(&b.bits);
    let unknown = either(a, b).map(|u| u.and_not(&true_of));
    made(&true_of, unknown)
}

/// `NOT a`: unknown of unknown.
pub(super) fn not(a: &Bools) -> Bools {
    made(&a.bits.not(), a.nulls.clone())
}

/// `a IS [NOT] value`: whether each is `value`, its negation if `not`.
pub(super) fn is(a: &Bools, value: Truth, not: bool) -> Bools {
    let of = match value {
        Truth::True => a.bits.clone(),
        Truth::False => falses(a),
        Truth::Unknown => a.nulls.clone().unwrap_or_else(|| Mask::none(a.bits.len())),
    };
    Bools::new(if not { of.not() } else { of }, None)
}

/// `x IS [NOT] NULL` of each of `count` rows of `x`.
pub(super) fn is_null(x: &Vector<'_>, not: bool, count: usize) -> Bools {
    if let Vector::Const(v) = x {
        return constant(if v.is_null() == not { Truth::False } else { Truth::True }, count);
    }
    let nulls = nulls_of(x, count);
    Bools::new(if not { nulls.not() } else { nulls }, None)
}
