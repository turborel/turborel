//! \[DESIGN\] RULES.md D31: the built-in set functions of exact numbers
//! given as integers (`storage::Ints`), a vector at a time, no value made
//! of each: how many are not null, their sum, and the greatest or least,
//! each of the rows given or all, nulls left out [X3.135-1986 §5.8 GR1];
//! sums of all and extremes on the vector kernels (RULES.md D570).

use crate::kernels::Kernels;
use crate::storage::Ints;
use crate::storage::mask::index;
use crate::value::{Decimal, Value};

/// How many of the rows `rows` of `ints`, or all, are not null.
pub(super) fn count(ints: &Ints, rows: Option<&[u32]>) -> usize {
    match (rows, &ints.nulls) {
        (Some(rows), None) => rows.len(),
        (None, None) => ints.units.len(),
        (Some(_), Some(_)) | (None, Some(_)) => ints.held(rows),
    }
}

/// `start` plus each of the rows `rows` of `ints`, or all, not null, in
/// turn, and whether one was: none if 128 bits cannot hold the sum, as
/// adding them one by one as decimals would find (`Decimal::wide_add`).
/// No sum so far is judged against `MAX_EXACT_PRECISION`: the total
/// alone is [ISO/IEC 9075:1992 §6.5 GR2b iv].
pub(super) fn sum_from(start: i128, ints: &Ints, rows: Option<&[u32]>, kernels: &dyn Kernels) -> Option<(i128, bool)> {
    let runs = run_counts(ints, rows);
    if let (None, None) = (rows, &runs)
        && let Some(summed) = summed(start, ints, kernels)
    {
        return Some(summed);
    }
    if let Some(runs) = runs {
        return runs.into_iter().try_fold((start, false), |(total, any), (u, n)| {
            let n = i128::try_from(n).ok()?;
            Some((total.checked_add(i128::from(u).checked_mul(n)?)?, any || n > 0))
        });
    }
    // Each case its own loop, no call a value.
    let add = |total: i128, u: &i64| total.checked_add(i128::from(*u));
    let held = |n: usize| (!ints.is_null(n)).then(|| ints.units.get(n)).flatten();
    match (rows, &ints.nulls) {
        (None, None) => Some((ints.units.iter().try_fold(start, add)?, !ints.units.is_empty())),
        (Some(rows), None) => {
            let mut any = false;
            let total = rows.iter().filter_map(|&n| ints.units.get(index(n))).inspect(|_| any = true).try_fold(start, add)?;
            Some((total, any))
        }
        (Some(rows), Some(_)) => {
            let mut any = false;
            let total = rows.iter().filter_map(|&n| held(index(n))).inspect(|_| any = true).try_fold(start, add)?;
            Some((total, any))
        }
        (None, Some(_)) => {
            let mut any = false;
            let total = (0..ints.units.len()).filter_map(held).inspect(|_| any = true).try_fold(start, add)?;
            Some((total, any))
        }
    }
}

/// \[DESIGN\] Bits of a bound on a sum's magnitude below which none
/// passes 128 bits: of `start` below 2^126 and fewer than 2^62 values,
/// each below 2^63, every partial sum is below 2^127 (RULES.md D570).
const SAFE_BITS: u32 = 126;

/// Bits of a bound on the values summed at once, so that their sum and
/// a start below 2^`SAFE_BITS` stay within 128 bits.
const FEW_BITS: u32 = 62;

/// `start` plus every unit of `ints` not null, and whether one was, summed
/// by `kernels`: of all the units, less those of the nulls, which `Ints`
/// does not promise are 0. None where a partial sum could pass 128 bits,
/// so the total is what adding the values in turn makes, or its overflow
/// raised by that.
fn summed(start: i128, ints: &Ints, kernels: &dyn Kernels) -> Option<(i128, bool)> {
    let few = u64::try_from(ints.units.len()).is_ok_and(|n| n >> FEW_BITS == 0);
    if start.unsigned_abs() >> SAFE_BITS != 0 || !few {
        return None;
    }
    let all = kernels.sum_i64(&ints.units, None);
    let nulled = match &ints.nulls {
        Some(nulls) => nulled_sum(&ints.units, nulls.words()),
        None => 0,
    };
    Some((start.checked_add(all)?.checked_sub(nulled)?, ints.held(None) > 0))
}

/// The sum of the units at the bits set of `words`, each a null's.
fn nulled_sum(units: &[i64], words: &[u64]) -> i128 {
    let mut sum = 0_i128;
    for (w, &word) in words.iter().enumerate() {
        let mut bits = word;
        while bits != 0 {
            let at = w.checked_mul(64).and_then(|base| base.checked_add(usize::try_from(bits.trailing_zeros()).ok()?));
            sum = sum.saturating_add(i128::from(at.and_then(|n| units.get(n)).copied().unwrap_or(0)));
            bits &= bits.wrapping_sub(1);
        }
    }
    sum
}

/// \[DESIGN\] (RULES.md D31) Of `ints` known as runs (`Ints::runs_of`),
/// each run's unit and how many of the rows `rows`, or all, are in it and
/// not null, so
/// a run's are added once: of runs of one unit added one by one, each
/// total between the run's first and last, so 128 bits hold them all if
/// they hold the last.
fn run_counts(ints: &Ints, rows: Option<&[u32]>) -> Option<Vec<(i64, usize)>> {
    match rows {
        Some(rows) => Some(ints.runs_of(rows)?.map(|(u, these)| (u, ints.held(Some(these)))).collect()),
        None => Some(ints.run_lengths()?.collect()),
    }
}

/// The greatest, if `max`, or least of the rows `rows` of `ints`, or all,
/// not null, as an exact number of their scale: none if every one is null.
/// Of no runs, found by `kernels`.
pub(super) fn extreme(ints: &Ints, rows: Option<&[u32]>, max: bool, kernels: &dyn Kernels) -> Option<Value> {
    let best = match run_counts(ints, rows) {
        Some(runs) => {
            let held = runs.into_iter().filter(|&(_, n)| n > 0).map(|(u, _)| u);
            if max { held.max() } else { held.min() }
        }
        None => kernels.min_max_i64(&ints.units, (ints.nulls.as_ref(), rows)).map(|(least, most)| if max { most } else { least }),
    };
    best.map(|u| Value::Exact(Decimal::new(i128::from(u), ints.scale)))
}
