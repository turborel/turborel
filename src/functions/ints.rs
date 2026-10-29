//! \[DESIGN\] RULES.md D31: the built-in set functions of exact numbers
//! given as integers (`storage::Ints`), a vector at a time, no value made
//! of each: how many are not null, their sum, and the greatest or least,
//! each of the rows given or all, nulls left out [X3.135-1986 §5.8 GR1].
//! Split from functions.rs (RULES.md R-SIZE-2).

use crate::storage::Ints;
use crate::value::{Decimal, MAX_EXACT_PRECISION, Value, pow10};

/// The units of `ints` of the rows `rows`, or all, not null.
fn units<'i>(ints: &'i Ints, rows: Option<&'i [usize]>) -> Box<dyn Iterator<Item = i64> + 'i> {
    let held = move |n: usize| (!ints.is_null(n)).then(|| ints.units.get(n).copied()).flatten();
    match rows {
        Some(rows) => Box::new(rows.iter().filter_map(move |&n| held(n))),
        None if ints.nulls.is_none() => Box::new(ints.units.iter().copied()),
        None => Box::new((0..ints.units.len()).filter_map(held)),
    }
}

/// How many of the rows `rows` of `ints`, or all, are not null.
pub(super) fn count(ints: &Ints, rows: Option<&[usize]>) -> usize {
    match (rows, &ints.nulls) {
        (Some(rows), None) => rows.len(),
        (None, None) => ints.units.len(),
        (Some(rows), Some(_)) => rows.iter().filter(|&&n| !ints.is_null(n)).count(),
        (None, Some(nulls)) => nulls.iter().filter(|&&null| !null).count(),
    }
}

/// `start` plus each of the rows `rows` of `ints`, or all, not null, in
/// turn, and whether one was: none if a running sum goes past the most
/// units an exact number holds [X3.135-1986 §5.8 GR4e], as adding them
/// one by one as decimals would raise of it (`Decimal::checked_add`).
pub(super) fn sum_from(start: i128, ints: &Ints, rows: Option<&[usize]>) -> Option<(i128, bool)> {
    let bound = pow10(MAX_EXACT_PRECISION)?;
    let (mut total, mut any) = (start, false);
    for u in units(ints, rows) {
        total = total.checked_add(i128::from(u)).filter(|t| t.checked_abs().is_some_and(|a| a < bound))?;
        any = true;
    }
    Some((total, any))
}

/// The greatest, if `max`, or least of the rows `rows` of `ints`, or all,
/// not null, as an exact number of their scale: none if every one is null.
pub(super) fn extreme(ints: &Ints, rows: Option<&[usize]>, max: bool) -> Option<Value> {
    let best = if max { units(ints, rows).max() } else { units(ints, rows).min() };
    best.map(|u| Value::Exact(Decimal::new(i128::from(u), ints.scale)))
}
