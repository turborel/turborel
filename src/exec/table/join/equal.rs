//! \[DESIGN\] (RULES.md D368) Whether a probing row's key and a built
//! row's match, compared in their vectors where both are of a kind the
//! join compares in place: integers by their units, rescaled to the
//! larger scale, a units' overflow meaning no match, as its value is
//! larger than any the other vector holds; texts less their trailing
//! spaces [X3.135-1986 §5.11 GR5]; approximate numbers by their values,
//! on a key whose nulls match nothing; datetimes of one type by their
//! instants. Any other pair, or a null on either side, is compared as
//! values (`value::key_equal`), as the join compared every key before.

use crate::error::Result;
use crate::storage::{Batch, Column, Ints};
use crate::value::key_equal;

/// \[DESIGN\] The radix a scale counts digits of.
const RADIX: i64 = 10;

/// Whether the key of row `r` of `probe`, of its columns at `left`, and
/// that of row `p` of `built`, of its columns at `right`, match: equal, or
/// on a key whose `nulls` says so, not distinct [ISO/IEC 9075-2:2023
/// §8.15].
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
pub(in crate::exec) fn keys_equal(
    (probe, left, r): (&Batch, &[usize], usize),
    (built, right, p): (&Batch, &[usize], usize),
    nulls: &[bool],
) -> Result<bool> {
    for ((&l, &b), &matches_null) in left.iter().zip(right).zip(nulls) {
        let (Some(a), Some(z)) = (probe.columns.get(l), built.columns.get(b)) else { return Ok(false) };
        let equal = match cells_equal((a, r), (z, p), matches_null) {
            Some(equal) => equal,
            None => key_equal(&probe.cell(l, r).to_value(), &built.cell(b, p).to_value(), matches_null)?,
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether row `row` of `probe` and row `at` of `built` match, compared
/// in place; none if they are not of kinds compared so, or either is null.
fn cells_equal((probe, row): (&Column, usize), (built, at): (&Column, usize), nulls: bool) -> Option<bool> {
    match probe {
        Column::Ints(ints) => {
            if let Column::Ints(other) = built {
                units_equal((ints, row), (other, at))
            } else {
                None
            }
        }
        Column::Texts(texts) => {
            let Column::Texts(other) = built else { return None };
            Some(texts.get(row)?.trim_end_matches(' ') == other.get(at)?.trim_end_matches(' '))
        }
        Column::Floats(floats) => {
            let Column::Floats(other) = built else { return None };
            (!nulls).then_some(()).and_then(|()| Some(floats.get(row)?.partial_cmp(&other.get(at)?) == Some(std::cmp::Ordering::Equal)))
        }
        Column::Datetimes(times) => {
            let Column::Datetimes(other) = built else { return None };
            let (mine, theirs) = (times.get(row)?, other.get(at)?);
            (times.moment == other.moment && times.zoned == other.zoned).then_some(mine.utc == theirs.utc)
        }
        Column::Values(_) | Column::Bools(_) | Column::Nulls(_) | Column::Late(_) => None,
    }
}

/// Whether row `row` of `probe` and row `at` of `built` are equal
/// numbers; none if either is null.
fn units_equal((probe, row): (&Ints, usize), (built, at): (&Ints, usize)) -> Option<bool> {
    let mine = *probe.units.get(row).filter(|_| !probe.is_null(row))?;
    let theirs = *built.units.get(at).filter(|_| !built.is_null(at))?;
    if probe.scale == built.scale {
        return Some(mine == theirs);
    }
    let (low, high, by) = if probe.scale < built.scale {
        (mine, theirs, built.scale.saturating_sub(probe.scale))
    } else {
        (theirs, mine, probe.scale.saturating_sub(built.scale))
    };
    Some(RADIX.checked_pow(by).and_then(|f| low.checked_mul(f)) == Some(high))
}
