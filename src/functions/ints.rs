//! \[DESIGN\] RULES.md D31: the built-in set functions of exact numbers
//! given as integers (`storage::Ints`), a vector at a time, no value made
//! of each: how many are not null, their sum, and the greatest or least,
//! each of the rows given or all, nulls left out [X3.135-1986 §5.8 GR1].

use crate::storage::Ints;
use crate::value::{Decimal, Value};

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
/// turn, and whether one was: none if 128 bits cannot hold the sum, as
/// adding them one by one as decimals would find (`Decimal::wide_add`).
/// No sum so far is judged against `MAX_EXACT_PRECISION`: the total
/// alone is [ISO/IEC 9075:1992 §6.5 GR2b iv].
pub(super) fn sum_from(start: i128, ints: &Ints, rows: Option<&[usize]>) -> Option<(i128, bool)> {
    if let Some(runs) = run_counts(ints, rows) {
        return runs.into_iter().try_fold((start, false), |(total, any), (u, n)| {
            let n = i128::try_from(n).ok()?;
            Some((total.checked_add(i128::from(u).checked_mul(n)?)?, any || n > 0))
        });
    }
    // Each case its own loop, no call a value.
    let add = |total: i128, u: &i64| total.checked_add(i128::from(*u));
    let held = |n: &usize| (!ints.is_null(*n)).then(|| ints.units.get(*n)).flatten();
    match (rows, &ints.nulls) {
        (None, None) => Some((ints.units.iter().try_fold(start, add)?, !ints.units.is_empty())),
        (Some(rows), None) => {
            let mut any = false;
            let total = rows.iter().filter_map(|&n| ints.units.get(n)).inspect(|_| any = true).try_fold(start, add)?;
            Some((total, any))
        }
        (Some(rows), Some(_)) => {
            let mut any = false;
            let total = rows.iter().filter_map(held).inspect(|_| any = true).try_fold(start, add)?;
            Some((total, any))
        }
        (None, Some(_)) => {
            let mut any = false;
            let total = (0..ints.units.len()).filter_map(|n| held(&n)).inspect(|_| any = true).try_fold(start, add)?;
            Some((total, any))
        }
    }
}

/// \[DESIGN\] (RULES.md D31) Of `ints` known as runs (`Ints::runs_of`),
/// each run's unit and how many of the rows `rows`, or all, are in it and
/// not null, so
/// a run's are added once: of runs of one unit added one by one, each
/// total between the run's first and last, so 128 bits hold them all if
/// they hold the last.
fn run_counts(ints: &Ints, rows: Option<&[usize]>) -> Option<Vec<(i64, usize)>> {
    match rows {
        Some(rows) => Some(ints.runs_of(rows)?.map(|(u, these)| (u, ints.held(these))).collect()),
        None => Some(ints.run_lengths()?.collect()),
    }
}

/// The greatest, if `max`, or least of the rows `rows` of `ints`, or all,
/// not null, as an exact number of their scale: none if every one is null.
pub(super) fn extreme(ints: &Ints, rows: Option<&[usize]>, max: bool) -> Option<Value> {
    let best = match run_counts(ints, rows) {
        Some(runs) => {
            let held = runs.into_iter().filter(|&(_, n)| n > 0).map(|(u, _)| u);
            if max { held.max() } else { held.min() }
        }
        None if max => units(ints, rows).max(),
        None => units(ints, rows).min(),
    };
    best.map(|u| Value::Exact(Decimal::new(i128::from(u), ints.scale)))
}
