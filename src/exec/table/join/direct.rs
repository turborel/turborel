//! \[DESIGN\] (RULES.md D601) A join's table of keys dense in their span:
//! of one key column of whole numbers whose span, the greatest less the
//! least and one, is at most `DENSE_SPAN` times the rows built, the rows
//! of each key are found by their key's place in the span, no hash made,
//! no tag read, no slot walked, as Pedreira et al. (VLDB 2022, §4.5.2)
//! index an array by a key of a small range, and Boncz, Zukowski and Nes
//! (CIDR 2005, §4.1.2) join a key to a dense row number by its place.
//! The rows are put by a counting sort of their keys, which keeps the
//! rows of each key in the order they were built.
//!
//! A key is given as its number's nearest `f64`, as a join's keys are
//! compared by it (`keys.rs`), the bits of it with -0 as 0. A whole
//! number below 2^51 in magnitude is its `f64` exactly, so the place of a
//! key is exact, and a probe's number of a fraction, or out of the span,
//! finds no row. The rows a probe finds are those of its key's number,
//! their keys compared then, as a hashed table's candidates are.
//!
//! On a key whose nulls match, as a join a subquery is unnested into
//! matches its outer references by IS NOT DISTINCT FROM, a null is given
//! as `NULL_NUMBER`, a not-a-number no key's place is, and its rows are
//! kept apart, in the order built.

use super::placed::summed;

/// \[DESIGN\] (RULES.md D601) The widest span of the keys of a direct
/// table, a multiple of its rows: its places of each key in the span, 4
/// bytes each, and its rows, 4 bytes each, take no more than the 32 bytes
/// a row of a hashed table costs (`TABLE_ROW_BYTES`).
pub(in crate::exec) const DENSE_SPAN: usize = 4;

/// \[DESIGN\] 1.5 × 2^52, to which a whole number below 2^51 is added to
/// read it from a double's bits.
const MAGIC: f64 = 6_755_399_441_055_744.0_f64;

/// \[DESIGN\] 2^51, below which a whole number is its `f64` exactly.
const EXACT: f64 = 2_251_799_813_685_248.0_f64;

/// \[DESIGN\] What a null is given as to a direct table, on a key whose
/// nulls match: a quiet not-a-number, of no place in a span. A probe's
/// not-a-number of these bits finds the null rows, and its key, compared
/// then, matches none of them.
pub(super) const NULL_NUMBER: u64 = 0x7FF8_0000_0000_0E11;

/// The rows of a join's built operand by their keys' places in their
/// span.
#[derive(Debug, Default)]
pub(super) struct Dense {
    /// The least key.
    least: i64,
    /// Of each place in the span and one past it, where its key's rows
    /// begin among `rows`.
    starts: Vec<u32>,
    /// The built rows, those of each key after the key before's, each in
    /// the order built.
    rows: Vec<u32>,
    /// The built rows of a null key whose nulls match, in the order built.
    nulls: Vec<u32>,
}

impl Dense {
    /// The table of the rows whose keys' `numbers` are given, the bits of
    /// each one's nearest `f64`, each row's number its place among them,
    /// but those `misses` marks, and those `NULL_NUMBER` gives kept apart;
    /// none unless each other is a whole number below 2^51 in magnitude
    /// and their span is at most `DENSE_SPAN` times their count, of rows a
    /// `u32` numbers.
    pub(super) fn of(numbers: &[u64], misses: &[bool]) -> Option<Dense> {
        let mut keyed: Vec<(u32, i64)> = Vec::with_capacity(numbers.len());
        let mut nulls = Vec::new();
        let (mut least, mut most) = (i64::MAX, i64::MIN);
        for (r, (&bits, _)) in numbers.iter().zip(misses).enumerate().filter(|(_, (_, m))| !**m) {
            if bits == NULL_NUMBER {
                nulls.push(u32::try_from(r).ok()?);
                continue;
            }
            let key = whole(bits)?;
            (least, most) = (least.min(key), most.max(key));
            keyed.push((u32::try_from(r).ok()?, key));
        }
        let width = usize::try_from(most.checked_sub(least)?).ok()?.checked_add(1)?;
        if keyed.is_empty() || width > keyed.len().saturating_mul(DENSE_SPAN) {
            return None;
        }
        let place = |key: i64| usize::try_from(key.wrapping_sub(least)).unwrap_or(usize::MAX);
        let mut ends = vec![0_usize; width.saturating_add(1)];
        for &(_, key) in &keyed {
            if let Some(end) = ends.get_mut(place(key).saturating_add(1)) {
                *end = end.saturating_add(1);
            }
        }
        summed(&mut ends);
        let mut next = ends.clone();
        let mut rows = vec![0_u32; keyed.len()];
        for &(r, key) in &keyed {
            let Some(at) = next.get_mut(place(key)) else { continue };
            if let Some(row) = rows.get_mut(*at) {
                *row = r;
            }
            *at = at.saturating_add(1);
        }
        let starts = ends.into_iter().map(|e| u32::try_from(e).unwrap_or(u32::MAX)).collect();
        Some(Dense { least, starts, rows, nulls })
    }

    /// The rows of the key whose number's bits are `bits`, in the order
    /// they were built; none of a number not whole or out of the span;
    /// those of a null key, of `NULL_NUMBER`.
    pub(super) fn candidates(&self, bits: u64) -> &[u32] {
        if bits == NULL_NUMBER {
            return &self.nulls;
        }
        let Some(at) = whole(bits).and_then(|key| key.checked_sub(self.least)).and_then(|p| usize::try_from(p).ok()) else {
            return &[];
        };
        let (Some(&from), Some(&to)) = (self.starts.get(at), self.starts.get(at.saturating_add(1))) else { return &[] };
        let span = usize::try_from(from).unwrap_or(usize::MAX)..usize::try_from(to).unwrap_or(0);
        self.rows.get(span).unwrap_or_default()
    }
}

/// The whole number whose `f64`'s bits are `bits`, if it is one below
/// 2^51 in magnitude: read from the bits of it added to `MAGIC`.
fn whole(bits: u64) -> Option<i64> {
    let f = f64::from_bits(bits);
    if f.fract() != 0.0_f64 || f.abs() >= EXACT {
        return None;
    }
    Some((f + MAGIC).to_bits().wrapping_sub(MAGIC.to_bits()).cast_signed())
}

// Unit tests: spans at their bound, negative and fractional keys, build
// order within a key (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
