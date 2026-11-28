//! \[DESIGN\] (RULES.md D1295) The constants of an IN list of exact
//! numbers, folded once to the scale of a vector of integers
//! (`storage::Ints`), so each of its rows is looked for among them as an
//! integer, no value made of it: what a filter over batches judges
//! (`exec/batch/judge/ints.rs`) and what a column store's scan judges
//! before it decodes the other columns (`engine/columns/judged.rs`), so
//! both judge alike. A constant folds as an equality does
//! (`rescaled_bound`): one of fewer fractional digits is scaled up, one
//! of more is divided if the scale divides it, and one the scale cannot
//! hold, or past 64 bits, equals no row and is left out. The units kept
//! are in order and each once. Where the greatest lies fewer than 64
//! above the least they are the bits of one word, a bit a unit from the
//! least on, and a row's unit is found by its bit; else it is found among
//! them by halving.

use super::bound::{Bound, rescaled_bound};
use crate::value::{CompOp, Value};

/// \[DESIGN\] The span of units one word of bits holds, a bit each: the
/// bits of a `u64`.
const WORD: u64 = 64;

/// \[DESIGN\] (RULES.md D1295) An IN list of exact constants, or NOT IN
/// one, as a vector of integers of one scale is judged by it.
#[derive(Debug, Clone)]
pub struct Listed {
    /// The units equal to a constant, ascending, each once.
    units: Vec<i64>,
    /// The least of them and a word with the bit of each set, counted
    /// from the least, if they span fewer than `WORD`.
    bits: Option<(i64, u64)>,
    /// NOT IN.
    not: bool,
    /// Whether a constant is null.
    null: bool,
}

impl Listed {
    /// The list of `values` for integers of `scale`, NOT IN if `not`:
    /// none if a constant not null is not an exact number or is too
    /// large to scale (`rescaled_bound`).
    pub fn of(scale: u32, values: &[Value], not: bool) -> Option<Listed> {
        let mut units = Vec::with_capacity(values.len());
        for v in values.iter().filter(|v| !v.is_null()) {
            match rescaled_bound(scale, CompOp::Eq, v)? {
                Bound::Of(_, c) => units.push(c),
                // No integer of the scale is equal to it.
                Bound::All | Bound::None => {}
            }
        }
        units.sort_unstable();
        units.dedup();
        let bits = units.first().zip(units.last()).and_then(|(&least, &most)| {
            most.checked_sub(least).and_then(|s| u64::try_from(s).ok()).filter(|&s| s < WORD)?;
            Some((least, units.iter().fold(0_u64, |word, &u| word | bit_of(u, least).unwrap_or(0))))
        });
        Some(Listed { units, bits, not, null: values.iter().any(Value::is_null) })
    }

    /// [X3.135-1986 §5.13 GR2, §5.18] Whether the predicate is true of
    /// the unit `u`, not null: IN, if it equals a constant; NOT IN, if it
    /// equals none and no constant is null, as it is unknown with one.
    pub fn holds(&self, u: i64) -> bool {
        let found = match self.bits {
            Some((least, word)) => bit_of(u, least).is_some_and(|bit| word & bit != 0),
            None => self.units.binary_search(&u).is_ok(),
        };
        if self.not { !self.null && !found } else { found }
    }
}

/// The bit of unit `u` in a word counted from `least`: none if it lies
/// below `least`, or a word's bits or more above it.
fn bit_of(u: i64, least: i64) -> Option<u64> {
    let place = u.checked_sub(least).and_then(|p| u32::try_from(p).ok())?;
    1_u64.checked_shl(place)
}

// Unit tests: a list folded to a scale finds what comparing each value
// with each constant finds (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
