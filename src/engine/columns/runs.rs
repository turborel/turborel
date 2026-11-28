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
use super::packed::{bits_for, dense, get, pack, packed_len, unpack_words};
use super::{WORD, corrupt, read_word, word};
use crate::error::Result;

/// The bytes of an integer field.
const FRAMED: usize = 8;

/// \[DESIGN\] (RULES.md D1315) The integers a run's value is appended as
/// at once, of a run no longer than that, and the room a vector of rows
/// is given past its last for it. It is the eight of a cache line of 64
/// bytes. The rows of a line are written together whichever of them a
/// run holds, as the runs after it write the line's rest anyway, and a
/// longer write would touch a line more for every run. A run longer than
/// a line is filled as it is, so the same length says which runs are
/// short.
pub(super) const LINE: usize = 8;

/// The values unpacked a run where a span's runs are read whole, its
/// value and its end (RULES.md D1340).
const PAIR: usize = 2;

/// \[DESIGN\] (RULES.md D1315) The values and the ends of the runs a
/// reading unpacks, the vectors kept by the scan from one reading to the
/// next (`prepared.rs`), so that a reading of a vector of rows makes none.
#[derive(Debug, Default)]
pub(in crate::engine) struct Unpacked {
    values: Vec<i64>,
    ends: Vec<i64>,
}

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
    /// value repeated for its rows among them (`ints_ends`), its runs
    /// unpacked into vectors of its own.
    pub(super) fn ints(&self, (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
        self.ints_ends((from, to), (out, None), &mut Unpacked::default())
    }

    /// \[DESIGN\] (RULES.md D1171, D1315) The values at positions `from`
    /// to `to`, pushed to `out`, each run's value repeated for its rows
    /// among them, and, if `ends` is given, where each of those runs ends
    /// among them, past its base, pushed to it, the last at `to`. The runs
    /// from the one of `from` to the one of the last position, found by
    /// two searches, have their values and ends unpacked a block at a time
    /// (`packed/blocks.rs`) into `unpacked`, so no run's value nor end is
    /// read alone and a reading that is given the vectors of the one
    /// before makes none. The values are then appended a run at a time,
    /// a short run as a whole line (`spread`). None, with `out` as it
    /// was, if the runs are not those of the positions.
    pub(super) fn ints_ends(
        &self,
        (from, to): (usize, usize),
        (out, ends): (&mut Vec<i64>, Option<(&mut Vec<usize>, usize)>),
        unpacked: &mut Unpacked,
    ) -> Option<()> {
        if to > self.count || from > to {
            return None;
        }
        if from == to {
            return Some(());
        }
        let (first, last) = (self.run_of(from)?, self.run_of(to.checked_sub(1)?)?.checked_add(1)?);
        let Unpacked { values, ends: run_ends } = unpacked;
        values.clear();
        run_ends.clear();
        unpack_words(self.values, (self.bits, self.least), (first, last), values)?;
        unpack_words(self.ends, (bits_for(u64::try_from(self.count).ok()?), 0), (first, last), run_ends)?;
        // Room for the rows and for the line of a last run shorter than
        // one, so that no run's write grows the vector.
        let start = out.len();
        out.reserve(to.checked_sub(from)?.checked_add(LINE)?);
        let spread = Self::spread((values.as_slice(), run_ends.as_slice()), (from, to), out, ends);
        if spread.is_none() {
            out.truncate(start);
        }
        spread
    }

    /// \[DESIGN\] (RULES.md D1315) Appends to `out` the value of each run
    /// of `values` ending at `run_ends`, cut at `to`, for its rows from
    /// `from` on, and pushes each end to `ends` past its base. A run of a
    /// line or less has its value appended as a whole line, whatever its
    /// length, and the vector is then cut back to the run's rows. So a
    /// run of a few rows, which is most of a column kept as runs whose
    /// values change often, takes no loop whose count the processor must
    /// guess. A run longer than a line is one fill of its rows and
    /// nothing else, as a line written first would only be written
    /// again. None if an end lies before the one before it, or the runs
    /// end short of `to`.
    fn spread(
        (values, run_ends): (&[i64], &[i64]),
        (from, to): (usize, usize),
        out: &mut Vec<i64>,
        ends: Option<(&mut Vec<usize>, usize)>,
    ) -> Option<()> {
        let mut ends = ends.map(|(ends, base)| {
            ends.reserve(run_ends.len());
            (ends, base)
        });
        let (start, mut at) = (out.len(), from);
        for (&value, &end) in values.iter().zip(run_ends) {
            let end = usize::try_from(end).ok()?.min(to);
            let rows = start.checked_add(end.checked_sub(from)?)?;
            if end.checked_sub(at)? > LINE {
                out.resize(rows, value);
            } else {
                out.extend_from_slice(&[value; LINE]);
                out.truncate(rows);
            }
            if let Some((ends, base)) = &mut ends {
                ends.push(base.checked_add(end.checked_sub(from)?)?);
            }
            at = end;
        }
        (at == to).then_some(())
    }

    /// \[DESIGN\] (RULES.md D1340, D360) The values at `positions`, which
    /// ascend from `low` to `high`, pushed to `out`. A position's run is
    /// found alone by a search of as many of the runs' ends as their count
    /// has bits, and its value then read, each read alone costing what
    /// `packed::DENSE_SPAN` values unpacked in a block cost. So where the
    /// positions are that many reads apiece worth more than the two
    /// values a run of their span is unpacked for (`walked`), the span's
    /// runs are unpacked once into `unpacked` and the positions walked
    /// along them, no row written. Where they are fewer each is found by
    /// its own search, the first's and the last's being those that told
    /// the span. None, with `out` as it was, if they are not there.
    pub(super) fn ints_at(
        &self,
        positions: impl ExactSizeIterator<Item = usize>,
        (low, high): (usize, usize),
        out: &mut Vec<i64>,
        unpacked: &mut Unpacked,
    ) -> Option<()> {
        if high >= self.count || low > high {
            return None;
        }
        let (first, last) = (self.run_of(low)?, self.run_of(high)?);
        let (start, count) = (out.len(), positions.len());
        let read = if self.walked(count, last.saturating_sub(first).saturating_add(1)) {
            self.walk(positions, (first, last), out, unpacked)
        } else {
            let run = |(n, p): (usize, usize)| {
                if n == 0 {
                    Some(first)
                } else if n.saturating_add(1) == count {
                    Some(last)
                } else {
                    self.run_of(p)
                }
            };
            positions.enumerate().try_for_each(|at| run(at).and_then(|r| self.value(r)).map(|value| out.push(value)))
        };
        if read.is_none() {
            out.truncate(start);
        }
        read
    }

    /// \[DESIGN\] (RULES.md D1340) Whether `count` positions that span
    /// `runs` of its runs are read by unpacking those runs whole and
    /// walking them: when the reads their searches would take alone,
    /// each of the bits of the runs' count and one for the value, are
    /// dense among the two values unpacked a run (`packed::dense`).
    fn walked(&self, count: usize, runs: usize) -> bool {
        let steps = usize::try_from(bits_for(u64::try_from(self.many).unwrap_or(u64::MAX))).unwrap_or(usize::MAX);
        dense(count.saturating_mul(steps.saturating_add(1)), runs.saturating_mul(PAIR))
    }

    /// The values at `positions`, ascending, all within the runs from
    /// `first` to `last`, pushed to `out`: those runs' values and ends
    /// unpacked into `unpacked`, and each position's run the first of
    /// them whose end is past it, looked for from the run of the position
    /// before. None if the runs end before a position.
    fn walk(
        &self,
        mut positions: impl Iterator<Item = usize>,
        (first, last): (usize, usize),
        out: &mut Vec<i64>,
        unpacked: &mut Unpacked,
    ) -> Option<()> {
        let Unpacked { values, ends } = unpacked;
        values.clear();
        ends.clear();
        let to = last.checked_add(1)?;
        unpack_words(self.values, (self.bits, self.least), (first, to), values)?;
        unpack_words(self.ends, (bits_for(u64::try_from(self.count).ok()?), 0), (first, to), ends)?;
        let mut r = 0_usize;
        positions.try_for_each(|p| {
            while usize::try_from(*ends.get(r)?).ok()? <= p {
                r = r.saturating_add(1);
            }
            out.push(*values.get(r)?);
            Some(())
        })
    }

    /// Timing only, never merged: the expansion as it was before D1315.
    pub(super) fn ints_old(&self, (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
        if to > self.count || from >= to {
            return (from == to).then_some(());
        }
        let (first, last) = (self.run_of(from)?, self.run_of(to.checked_sub(1)?)?.checked_add(1)?);
        let (mut values, mut run_ends) = (Vec::with_capacity(last.saturating_sub(first)), Vec::with_capacity(last.saturating_sub(first)));
        unpack_words(self.values, (self.bits, self.least), (first, last), &mut values)?;
        unpack_words(self.ends, (bits_for(u64::try_from(self.count).ok()?), 0), (first, last), &mut run_ends)?;
        out.reserve(to.checked_sub(from)?);
        let mut at = from;
        for (&value, &end) in values.iter().zip(&run_ends) {
            let end = usize::try_from(end).ok()?.min(to);
            out.resize(out.len().checked_add(end.checked_sub(at)?)?, value);
            at = end;
        }
        (at == to).then_some(())
    }

    /// Timing only, never merged: the expansion of 426af8698, rows zeroed first and a line written a run.
    pub(super) fn ints_slots(
        &self,
        (from, to): (usize, usize),
        (out, ends): (&mut Vec<i64>, Option<(&mut Vec<usize>, usize)>),
        unpacked: &mut Unpacked,
    ) -> Option<()> {
        if to > self.count || from > to {
            return None;
        }
        if from == to {
            return Some(());
        }
        let (first, last) = (self.run_of(from)?, self.run_of(to.checked_sub(1)?)?.checked_add(1)?);
        let Unpacked { values, ends: run_ends } = unpacked;
        values.clear();
        run_ends.clear();
        unpack_words(self.values, (self.bits, self.least), (first, last), values)?;
        unpack_words(self.ends, (bits_for(u64::try_from(self.count).ok()?), 0), (first, last), run_ends)?;
        let (start, count) = (out.len(), to.checked_sub(from)?);
        // The last run's line may pass the last position, by less than a line.
        out.resize(start.checked_add(count)?.checked_add(LINE)?, 0);
        let spread = Self::spread_slots((values.as_slice(), run_ends.as_slice()), (from, to), out.get_mut(start..)?, ends);
        out.truncate(start.saturating_add(if spread.is_some() { count } else { 0 }));
        spread
    }

    /// \[DESIGN\] (RULES.md D1315) Writes to `slots`, the rows from `from`
    /// to `to` and a line more, the value of each run of `values` ending
    /// at `run_ends`, cut at `to`, for its rows among them, and pushes
    /// each end to `ends` past its base. A run's value is written to a
    /// whole line where the run begins, whatever its length, and to the
    /// rest of its rows only if they are more than a line. So a run of a
    /// few rows, which is most of a column kept as runs whose values
    /// change often, takes no loop whose count the processor must guess,
    /// and the rows after the run in its line are written again by the
    /// runs after it. None if an end lies before the one before it, or
    /// the runs end short of `to`.
    fn spread_slots(
        (values, run_ends): (&[i64], &[i64]),
        (from, to): (usize, usize),
        slots: &mut [i64],
        ends: Option<(&mut Vec<usize>, usize)>,
    ) -> Option<()> {
        let mut ends = ends.map(|(ends, base)| {
            ends.reserve(run_ends.len());
            (ends, base)
        });
        let mut at = from;
        for (&value, &end) in values.iter().zip(run_ends) {
            let end = usize::try_from(end).ok()?.min(to);
            let run = slots.get_mut(at.checked_sub(from)?..)?;
            *run.first_chunk_mut::<LINE>()? = [value; LINE];
            if let Some(rest) = run.get_mut(LINE..end.checked_sub(at)?) {
                rest.fill(value);
            }
            if let Some((ends, base)) = &mut ends {
                ends.push(base.checked_add(end.checked_sub(from)?)?);
            }
            at = end;
        }
        (at == to).then_some(())
    }
}

// Unit tests: runs written a line at a time are their rows' values, by
// vectors kept from one reading to the next (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
