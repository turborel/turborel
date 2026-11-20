//! \[DESIGN\] Exact arithmetic over integers of a scale (RULES.md D362):
//! sums, differences and products of vectors of exact numbers kept as
//! integers (`storage::Ints`) and exact constants, at the scale decimals
//! give each [X3.135-1986 §5.9 SR4b, SR4c] (`Decimal::checked_add`,
//! `checked_mul`), in `i64` with `overflowing_*` and a flag of the batch:
//! set by an overflow, or by a result of `MAX_EXACT_PRECISION` digits or
//! more, which `Decimal::checked` refuses [GR6a]. A flag set of a row not
//! null gives none, and the batch is evaluated a row at a time, which
//! raises the first row's error. A null is null of a null [GR1], its
//! slot 0. A quotient's scale is each value's own (`Decimal::checked_div`),
//! which integers of one scale cannot hold, so it is not computed here.

use super::vector::{Vector, either};
use crate::storage::{Column, Ints, Mask};
use crate::value::{ArithOp, MAX_EXACT_PRECISION, Value, pow10};

/// Exact numbers of a vector as integers: a column's, or a constant.
pub(super) enum Exact<'v> {
    Column(&'v Ints),
    Const(i64, u32),
}

impl Exact<'_> {
    /// Its scale.
    fn scale(&self) -> u32 {
        match self {
            Exact::Column(ints) => ints.scale,
            Exact::Const(_, scale) => *scale,
        }
    }

    /// Its nulls, if any is.
    fn nulls(&self) -> Option<&Mask> {
        match self {
            Exact::Column(ints) => ints.nulls.as_ref(),
            Exact::Const(..) => None,
        }
    }

    /// Where each of its runs ends, of `count` rows, if it is known as
    /// runs and none is null: a constant one run.
    fn runs(&self, count: usize) -> Option<Vec<usize>> {
        match self {
            Exact::Column(ints) => ints.runs().map(<[usize]>::to_vec),
            Exact::Const(..) => Some(if count == 0 { Vec::new() } else { vec![count] }),
        }
    }

    /// Its unit of row `n`.
    fn unit(&self, n: usize) -> i64 {
        match self {
            Exact::Column(ints) => ints.units.get(n).copied().unwrap_or(0),
            Exact::Const(u, _) => *u,
        }
    }
}

/// The exact numbers of `v` as integers, if it is a vector of them or an
/// exact constant whose units fit.
pub(super) fn exact<'v>(v: &'v Vector<'_>) -> Option<Exact<'v>> {
    match v {
        Vector::Const(Value::Exact(d)) => Some(Exact::Const(i64::try_from(d.units).ok()?, d.scale)),
        Vector::Column(Column::Ints(ints)) | Vector::Owned(Column::Ints(ints)) => Some(Exact::Column(ints)),
        Vector::Const(_) | Vector::Column(_) | Vector::Owned(_) => None,
    }
}

/// The units every exact result is below in magnitude, 10^18.
fn bound() -> u64 {
    pow10(MAX_EXACT_PRECISION).and_then(|b| u64::try_from(b).ok()).unwrap_or(u64::MAX)
}

/// `op` of `x` and `y`, `count` rows of each, a sum, difference or
/// product, at the scale decimals give it: none if a row not null of it
/// overflows or reaches the digits no decimal holds.
pub(super) fn arith(op: ArithOp, (x, y): (&Exact<'_>, &Exact<'_>), count: usize) -> Option<Ints> {
    let limit = bound();
    match op {
        // [X3.135-1986 §5.9 SR4b] at the greater scale.
        ArithOp::Add | ArithOp::Sub => {
            let scale = x.scale().max(y.scale());
            let fx = i64::try_from(pow10(scale.checked_sub(x.scale())?)?).ok()?;
            let fy = i64::try_from(pow10(scale.checked_sub(y.scale())?)?).ok()?;
            let subtract = op == ArithOp::Sub;
            let sum = move |first: i64, second: i64| {
                let (scaled, over_first) = first.overflowing_mul(fx);
                let (other, over_second) = second.overflowing_mul(fy);
                let (made, over) = if subtract { scaled.overflowing_sub(other) } else { scaled.overflowing_add(other) };
                (made, over_first | over_second | over | (made.unsigned_abs() >= limit))
            };
            computed((x, y), (count, scale), sum)
        }
        // [X3.135-1986 §5.9 SR4c] at the sum of the scales.
        ArithOp::Mul => {
            let scale = x.scale().checked_add(y.scale()).filter(|s| *s <= MAX_EXACT_PRECISION)?;
            let product = move |first: i64, second: i64| {
                let (made, over) = first.overflowing_mul(second);
                (made, over | (made.unsigned_abs() >= limit))
            };
            computed((x, y), (count, scale), product)
        }
        ArithOp::Div => None,
    }
}

/// The integers of scale `scale` that `f` gives of each of `count` rows
/// of `x` and `y`, with whether it overflowed: none if it did of a row
/// not null. Null of a null, its slot 0; known as runs if both are.
fn computed(
    (left, right): (&Exact<'_>, &Exact<'_>),
    (count, scale): (usize, u32),
    apply: impl Fn(i64, i64) -> (i64, bool),
) -> Option<Ints> {
    let mut flag = false;
    let mut step = |(first, second): (i64, i64)| {
        let (made, over) = apply(first, second);
        flag |= over;
        made
    };
    let mut units: Vec<i64> = match (left, right) {
        (Exact::Column(xs), Exact::Column(ys)) => xs.units.iter().copied().zip(ys.units.iter().copied()).map(&mut step).collect(),
        (Exact::Column(xs), Exact::Const(unit, _)) => xs.units.iter().map(|&first| step((first, *unit))).collect(),
        (Exact::Const(unit, _), Exact::Column(ys)) => ys.units.iter().map(|&second| step((*unit, second))).collect(),
        (Exact::Const(first, _), Exact::Const(second, _)) => vec![step((*first, *second)); count],
    };
    let nulls = either(left.nulls(), right.nulls());
    if let Some(nulls) = &nulls {
        if flag {
            flag = (0..count).any(|n| !nulls.get(n) && apply(left.unit(n), right.unit(n)).1);
        }
        zeroed(&mut units, nulls);
    }
    if flag {
        return None;
    }
    let made = Ints::new(units, scale, nulls);
    Some(match (left.runs(count), right.runs(count)) {
        (Some(mut first), Some(second)) => {
            first.extend(second);
            first.sort_unstable();
            first.dedup();
            made.with_runs(first)
        }
        (None, _) | (_, None) => made,
    })
}

/// Writes 0 to the slot of each null `nulls` marks among `units`.
pub(super) fn zeroed(units: &mut [i64], nulls: &Mask) {
    for (word, chunk) in nulls.words().iter().zip(units.chunks_mut(crate::storage::mask::WORD_BITS)) {
        let mut bits = *word;
        while bits != 0 {
            if let Some(slot) = usize::try_from(bits.trailing_zeros()).ok().and_then(|at| chunk.get_mut(at)) {
                *slot = 0;
            }
            bits &= bits.wrapping_sub(1);
        }
    }
}

/// [X3.135-1986 §5.9 GR4] Each of `ints` negated, of its scale: none if
/// one's negation does not fit, which a row's makes past 64 bits.
pub(super) fn negated(ints: &Ints) -> Option<Ints> {
    let units = ints.units.iter().map(|u| u.checked_neg()).collect::<Option<Vec<i64>>>()?;
    let made = Ints::new(units, ints.scale, ints.nulls.clone());
    Some(match ints.runs() {
        Some(ends) => made.with_runs(ends.to_vec()),
        None => made,
    })
}

// Unit tests: the flag of the batch, its scales, its nulls and runs
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
