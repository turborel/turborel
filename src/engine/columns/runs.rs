//! \[DESIGN\] Runs of one value (RULES.md D31): of a chunk of 8-byte fields,
//! each a signed integer, each run of values alike, of rows in a row, its
//! value less the least and where it ends, each bit-packed (`packed.rs`). A
//! null continues the run before it, a leading one the first, as the chunk's
//! nulls mark it apart, so nulls break no run. Of a sorted column a chunk
//! holds a few runs.
//!
//! ```text
//! RUNS:  runs u32 | least i64 | bits u8 | each run's value less the
//!        least, of bits | each run's end, of the bits the count needs
//! ```

use super::fields::Cells;
use super::packed::{bits_for, get, pack, packed_len, unpack_words};
use super::{WORD, corrupt, read_word, word};
use crate::error::Result;

/// The bytes of an integer field.
const FRAMED: usize = 8;

/// A chunk's runs, read in place.
#[derive(Debug, Clone, Copy)]
pub(super) struct Runs<'a> {
    least: i64,
    bits: u32,
    count: usize,
    /// How many runs.
    many: usize,
    values: &'a [u8],
    ends: &'a [u8],
}

/// The value each of `fields` holds, a null the one before it's, the
/// leading ones the first's.
fn held(fields: Cells<'_>) -> impl Iterator<Item = i64> + '_ {
    let first = fields.iter().flatten().find_map(integer).unwrap_or(0);
    fields.iter().scan(first, |last, f| {
        if let Some(n) = f.and_then(integer) {
            *last = n;
        }
        Some(*last)
    })
}

fn integer(field: &[u8]) -> Option<i64> {
    <[u8; FRAMED]>::try_from(field).ok().map(i64::from_le_bytes)
}

/// The bytes the runs of `count` values take, of `runs` runs of values
/// less the least in `bits`.
pub(super) fn runs_len((count, runs, bits): (usize, usize, u32)) -> Option<usize> {
    let header = WORD.checked_add(FRAMED)?.checked_add(1)?;
    header.checked_add(packed_len(runs, bits)?)?.checked_add(packed_len(runs, bits_for(u64::try_from(count).ok()?))?)
}

/// Appends `fields`, of 8 bytes each, as runs of values less `least`, each
/// in `bits`.
pub(super) fn encode(fields: Cells<'_>, (least, bits): (i64, u32), out: &mut Vec<u8>) -> Result<()> {
    let (mut values, mut ends) = (Vec::new(), Vec::new());
    let mut previous: Option<i64> = None;
    for (n, v) in held(fields).enumerate() {
        if previous != Some(v) {
            if previous.is_some() {
                ends.push(u64::try_from(n).unwrap_or(u64::MAX));
            }
            values.push(u64::try_from(i128::from(v).saturating_sub(i128::from(least))).unwrap_or(0));
            previous = Some(v);
        }
    }
    ends.push(u64::try_from(fields.len()).unwrap_or(u64::MAX));
    out.extend_from_slice(&word(values.len())?);
    out.extend_from_slice(&least.to_le_bytes());
    out.push(u8::try_from(bits).unwrap_or(u8::MAX));
    pack(values.into_iter(), bits, out);
    pack(ends.into_iter(), bits_for(u64::try_from(fields.len()).unwrap_or(u64::MAX)), out);
    Ok(())
}

impl<'a> Runs<'a> {
    /// The runs of a chunk of `count` values `rest` holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is too short, or of differences wider than 64 bits.
    pub(super) fn parse(rest: &'a [u8], count: usize) -> Result<Runs<'a>> {
        let runs = read_word(rest, 0)?;
        let least = rest.get(WORD..WORD.saturating_add(FRAMED)).and_then(integer).ok_or_else(|| corrupt("it is too short"))?;
        let bits = u32::from(*rest.get(WORD.saturating_add(FRAMED)).ok_or_else(|| corrupt("it is too short"))?);
        if bits > u64::BITS {
            return Err(corrupt("its differences are wider than 64 bits"));
        }
        let at = WORD.saturating_add(FRAMED).saturating_add(1);
        let values_len = packed_len(runs, bits).ok_or_else(|| corrupt("its runs are too many"))?;
        let ends_len =
            packed_len(runs, bits_for(u64::try_from(count).unwrap_or(u64::MAX))).ok_or_else(|| corrupt("its runs are too many"))?;
        let values = rest.get(at..at.saturating_add(values_len)).ok_or_else(|| corrupt("it is too short"))?;
        let ends = rest
            .get(at.saturating_add(values_len)..at.saturating_add(values_len).saturating_add(ends_len))
            .ok_or_else(|| corrupt("it is too short"))?;
        Ok(Runs { least, bits, count, many: runs, values, ends })
    }

    /// The end of run `r`.
    fn end(&self, r: usize) -> Option<usize> {
        usize::try_from(get(self.ends, bits_for(u64::try_from(self.count).ok()?), r)?).ok()
    }

    /// The value of run `r`.
    fn value(&self, r: usize) -> Option<i64> {
        Some(self.least.wrapping_add(i64::from_le_bytes(get(self.values, self.bits, r)?.to_le_bytes())))
    }

    /// The run of position `n`: the first whose end is past it.
    fn run_of(&self, n: usize) -> Option<usize> {
        let (mut low, mut high) = (0_usize, self.many);
        while low < high {
            let mid = low.saturating_add(high.saturating_sub(low) >> 1_u32);
            if self.end(mid)? > n {
                high = mid;
            } else {
                low = mid.saturating_add(1);
            }
        }
        (low < self.many).then_some(low)
    }

    /// The `n`th value, as its field's bytes.
    pub(super) fn field(&self, n: usize) -> Option<[u8; FRAMED]> {
        Some(self.value(self.run_of(n)?)?.to_le_bytes())
    }

    /// The values at positions `from` to `to`, pushed to `out`, each run's
    /// value repeated for its rows among them (`ints_ends`).
    pub(super) fn ints(&self, (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
        self.ints_ends((from, to), out, None)
    }

    /// \[DESIGN\] (RULES.md D1171) The values at positions `from` to `to`,
    /// pushed to `out`, each run's value repeated for its rows among them,
    /// and, if `ends` is given, where each of those runs ends among them,
    /// past its base, pushed to it, the last at `to`. The runs from the one
    /// of `from` to the one of the last position, found by two searches,
    /// have their values and ends unpacked a block at a time
    /// (`packed/blocks.rs`), so no run's value nor end is read alone.
    pub(super) fn ints_ends(&self, (from, to): (usize, usize), out: &mut Vec<i64>, ends: Option<(&mut Vec<usize>, usize)>) -> Option<()> {
        if to > self.count || from > to {
            return None;
        }
        if from == to {
            return Some(());
        }
        let (first, last) = (self.run_of(from)?, self.run_of(to.checked_sub(1)?)?.checked_add(1)?);
        let (mut values, mut run_ends) = (Vec::with_capacity(last.saturating_sub(first)), Vec::with_capacity(last.saturating_sub(first)));
        unpack_words(self.values, (self.bits, self.least), (first, last), &mut values)?;
        unpack_words(self.ends, (bits_for(u64::try_from(self.count).ok()?), 0), (first, last), &mut run_ends)?;
        out.reserve(to.checked_sub(from)?);
        let mut ends = ends.map(|(ends, base)| {
            ends.reserve(run_ends.len());
            (ends, base)
        });
        let mut at = from;
        for (&value, &end) in values.iter().zip(&run_ends) {
            let end = usize::try_from(end).ok()?.min(to);
            out.resize(out.len().checked_add(end.checked_sub(at)?)?, value);
            if let Some((ends, base)) = &mut ends {
                ends.push(base.checked_add(end.checked_sub(from)?)?);
            }
            at = end;
        }
        (at == to).then_some(())
    }
}
