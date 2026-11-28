//! \[DESIGN\] (RULES.md D1267) The octet lengths of a chunk's texts, read
//! of the chunk with no text made. A VARCHAR value is written as the
//! octets of its text (`value/types/varying.rs`), so its length is how
//! many octets the chunk holds for it. A varying chunk has it between
//! two ends, a constant chunk in its one field, a dictionary in the ends
//! of the value's entry, and texts coded by symbols in the lengths of
//! their codes' symbols (`fsst/lengths.rs`). An end outside the octets
//! the chunk holds gives none, and the column is then decoded, which
//! raises what is malformed.

use super::super::Chunk;
use super::super::packed::{dense, get, unpack_words};
use super::{Body, WORD, between};
use crate::storage::Mask;

/// `n` octets as a count's units.
fn whole(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The octets between `ends`, a field's, if it lies within `held` octets.
fn within((from, to): (usize, usize), held: usize) -> Option<i64> {
    if to > held {
        return None;
    }
    to.checked_sub(from).map(whole)
}

impl Body<'_> {
    /// The octet length of the value at each of `places`, ascending,
    /// pushed to `out`, a null's as its place holds it. None if the
    /// encoding does not keep texts or a value is not within the chunk.
    fn octets(&self, places: &[usize], out: &mut Vec<i64>) -> Option<()> {
        match self {
            Body::Varying { offsets, values } => {
                for &n in places {
                    out.push(within(between(offsets, n)?, values.len())?);
                }
            }
            Body::Constant { field } => out.extend(std::iter::repeat_n(whole(field.len()), places.len())),
            Body::Dictionary { .. } => self.entry_octets(places, out)?,
            Body::Fsst(coded) => {
                for &n in places {
                    out.push(whole(coded.octets(n)?));
                }
            }
            Body::Fixed { .. } | Body::Frame { .. } | Body::Runs(_) | Body::Alp(_) => return None,
        }
        Some(())
    }

    /// As `octets`, of a dictionary. Each entry's length is read once
    /// and a row's taken by its code, the codes of the span unpacked at
    /// once, unless the places are few of their span (`packed::dense`),
    /// each row's entry then read alone.
    fn entry_octets(&self, places: &[usize], out: &mut Vec<i64>) -> Option<()> {
        let Body::Dictionary { bits, offsets, entries, codes, .. } = self else { return None };
        let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
        let length = |e: usize| within(between(offsets, e)?, entries.len());
        let (Some(&low), Some(&high)) = (places.first(), places.last()) else { return Some(()) };
        if !dense(places.len(), high.checked_sub(low)?.saturating_add(1)) {
            for &n in places {
                out.push(length(usize::try_from(get(codes, *bits, n)?).ok()?)?);
            }
            return Some(());
        }
        let lengths = (0..count).map(length).collect::<Option<Vec<i64>>>()?;
        let mut held = Vec::new();
        unpack_words(codes, (*bits, 0), (low, high.checked_add(1)?), &mut held)?;
        for &n in places {
            let entry = usize::try_from(*held.get(n.checked_sub(low)?)?).ok()?;
            out.push(*lengths.get(entry)?);
        }
        Some(())
    }
}

impl Chunk<'_> {
    /// \[DESIGN\] (RULES.md D1267) The octet length of its value at each
    /// of `places`, ascending, texts of a VARCHAR column, pushed to
    /// `units`, a null's 0, and whether each is null to `nulls`. None if
    /// a place is past its end, its encoding keeps no texts or a value
    /// is not within it. What was pushed before is then of no use.
    pub(in crate::engine::columns) fn octets(&self, places: &[usize], (units, nulls): (&mut Vec<i64>, &mut Mask)) -> Option<()> {
        if places.iter().any(|&n| n >= self.count) {
            return None;
        }
        let before = units.len();
        self.body.octets(places, units)?;
        for (unit, &n) in units.iter_mut().skip(before).zip(places) {
            let null = self.is_null(n);
            if null {
                *unit = 0;
            }
            nulls.push(null);
        }
        Some(())
    }
}

// Unit tests: each encoding's lengths against its values' (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
