//! \[DESIGN\] RULES.md D31: the built-in set functions of approximate
//! numbers given as `f64`s (`storage::Floats`), a vector at a time, no
//! value made of each: how many are not null, their sum, and the greatest
//! or least, each of the rows given or all, nulls left out [X3.135-1986
//! §5.8 GR1], in the order a row at a time takes them, so each total is
//! the same; and how a sum is kept (`Wide`). Split from functions.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;

use crate::storage::Floats;
use crate::value::Value;

/// The numbers of `floats` of the rows `rows`, or all, not null.
fn numbers<'f>(floats: &'f Floats, rows: Option<&'f [usize]>) -> Box<dyn Iterator<Item = f64> + 'f> {
    match rows {
        Some(rows) => Box::new(rows.iter().filter_map(move |&n| floats.get(n))),
        None if floats.nulls.is_none() => Box::new(floats.numbers.iter().copied()),
        None => Box::new((0..floats.numbers.len()).filter_map(move |n| floats.get(n))),
    }
}

/// How many of the rows `rows` of `floats`, or all, are not null.
pub(super) fn count(floats: &Floats, rows: Option<&[usize]>) -> usize {
    numbers(floats, rows).count()
}

/// Adds to `sum` each of the rows `rows` of `floats`, or all, not null, in
/// order, as `Wide::with` adds a value; whether there was one.
pub(super) fn sum_into(sum: &mut Wide, floats: &Floats, rows: Option<&[usize]>) -> bool {
    let mut any = false;
    for f in numbers(floats, rows) {
        *sum = sum.with(f);
        any = true;
    }
    any
}

/// 2^64, by which `Wide` scales back up the values it keeps in `big`.
const SHIFT: f64 = f64::from_bits(1087_u64 << 52);
/// 2^-64, by which `Wide` scales them down: multiplied, exactly.
const UNSHIFT: f64 = f64::from_bits(959_u64 << 52);
/// 2^-900: the least magnitude `Wide` scales down, so that none scaled is
/// subnormal and loses a bit.
const TINY: f64 = f64::from_bits(123_u64 << 52);

/// \[DESIGN\] RULES.md D15: an approximate sum that leaves the range of
/// `f64` only if its total does, whatever the order its values came in
/// [ISO/IEC 9075:1992 §6.5 GR2b iv]. A value of magnitude 2^-900 or more
/// is added to `big` scaled by 2^-64, exactly, so `big` takes the same
/// roundings as the plain sum but cannot overflow before 2^63 values; the
/// rest are added to `small`, which cannot either. Of values all such,
/// the total is the plain sum's, in the same order.
#[derive(Debug, Clone, Copy)]
pub(super) struct Wide {
    big: f64,
    small: f64,
}

impl Wide {
    /// No value yet: each part -0, which adds to any value as nothing,
    /// so a sum of -0s is -0.
    pub(super) const EMPTY: Wide = Wide { big: -0.0_f64, small: -0.0_f64 };

    /// The sum with `f` added.
    #[must_use]
    pub(super) fn with(self, f: f64) -> Wide {
        if f.abs() >= TINY { Wide { big: self.big + f * UNSHIFT, ..self } } else { Wide { small: self.small + f, ..self } }
    }

    /// The sum: not finite if out of the range of `f64`.
    pub(super) fn total(self) -> f64 {
        self.big * SHIFT + self.small
    }

    /// The sum divided by `n`, the mean of `n` values, which is in range
    /// even where the sum is not.
    pub(super) fn over(self, n: f64) -> f64 {
        let total = self.total();
        if total.is_finite() { total / n } else { self.big / n * SHIFT + self.small / n }
    }
}

/// The greatest, if `max`, or least of the rows `rows` of `floats`, or
/// all, not null: the first of those equal [X3.135-1986 §5.8 GR4d].
pub(super) fn extreme(floats: &Floats, rows: Option<&[usize]>, max: bool) -> Option<Value> {
    let wanted = if max { Ordering::Greater } else { Ordering::Less };
    numbers(floats, rows).reduce(|best, f| if f.partial_cmp(&best) == Some(wanted) { f } else { best }).map(Value::Approx)
}
