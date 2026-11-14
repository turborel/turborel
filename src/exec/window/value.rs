//! [ISO/IEC 9075-2:2023 §6.10 GR1] NTILE, LEAD, LAG, FIRST_VALUE,
//! LAST_VALUE and NTH_VALUE over a partition, row after row. The rows such
//! a function counts are a sequence: every row of the partition, or under
//! IGNORE NULLS those whose argument is not null [GR1b iv 1 B, GR1c ii 2,
//! GR1d ii 3 B II]. Of each row is held how many of the sequence are
//! before it, and under IGNORE NULLS the sequence's values are held apart,
//! in order, spilled past their part of the work area (RULES.md D38). So
//! the row LEAD or LAG takes, and the nth of a frame's at most three
//! ranges, are found by counting, with no walk over the rows between.

use super::frame::{EMPTY, Partition, Ranges, whole};
use super::reader::{Placed, Reader};
use crate::error::{Code, Result, SqlState, fail_with};
use crate::exec::{HeldRows, Runner, broken, eval};
use crate::logical::{Pick, WindowKind};
use crate::value::{Decimal, Value};

/// How many readers of the places of a frame's ranges there are: one for
/// each end of each of its at most three ranges.
pub(super) const RANGE_ENDS: usize = 6;

/// The number of tiles of NTILE, or the `<nth row>` of NTH_VALUE,
/// evaluated once [§6.10 GR1a i, GR1d i].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::exec) enum Count {
    /// The function takes none.
    Unused,
    /// It is null, so each row's value is [GR1a ii 1, GR1d ii 1].
    Null,
    /// It is this many, at least one.
    Of(usize),
}

impl Count {
    /// The count of the window function `kind`, evaluated by `runner`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22014 if NTILE's is not positive [GR1a ii 2], 22016 if
    /// NTH_VALUE's is not [GR1d ii 2]; its evaluation's.
    pub(in crate::exec) fn of(kind: &WindowKind, runner: &Runner) -> Result<Count> {
        let (e, state, rule) = match kind {
            WindowKind::Ntile(tiles) => (tiles, SqlState::INVALID_ARGUMENT_FOR_NTILE, "§6.10 GR1a ii 2] the number of tiles of NTILE"),
            WindowKind::Value(v) => match &v.pick {
                Pick::Nth { row, .. } => (row, SqlState::INVALID_ARGUMENT_FOR_NTH_VALUE, "§6.10 GR1d ii 2] the <nth row> of NTH_VALUE"),
                Pick::First | Pick::Last => return Ok(Count::Unused),
            },
            WindowKind::RowNumber
            | WindowKind::Rank
            | WindowKind::DenseRank
            | WindowKind::PercentRank
            | WindowKind::CumeDist
            | WindowKind::Aggregate(_)
            | WindowKind::Offset(_) => return Ok(Count::Unused),
        };
        let v = eval(e, &Vec::new(), runner)?;
        if v.is_null() {
            return Ok(Count::Null);
        }
        let n = whole(&v)?;
        if n <= 0 {
            return fail_with(Code::Data, state, format!("[ISO/IEC 9075-2:2023 {rule} is {n}, not positive"));
        }
        Ok(Count::Of(usize::try_from(n).unwrap_or(usize::MAX)))
    }
}

/// [ISO/IEC 9075-2:2023 §6.10 GR1a ii 3] The tile of the `row`th row of a
/// partition of `len` rows cut into `tiles` tiles, from 1: the first
/// MOD(`len`, `tiles`) tiles one row longer than the rest.
pub(in crate::exec) fn tile(row: usize, len: usize, tiles: usize) -> usize {
    let short = len.checked_div(tiles).unwrap_or_default();
    let long = len.checked_rem(tiles).unwrap_or_default();
    let in_long = long.saturating_mul(short.saturating_add(1));
    let before = if row < in_long {
        row.checked_div(short.saturating_add(1)).unwrap_or_default()
    } else {
        // Past the long tiles, a short tile has rows, so `short` is not 0.
        long.saturating_add(row.saturating_sub(in_long).checked_div(short).unwrap_or_default())
    };
    before.saturating_add(1)
}

/// The rows a function counts, as places of a partition: each, or those
/// whose argument is not null, held apart.
pub(in crate::exec) enum Sequence {
    /// RESPECT NULLS: every row.
    Each,
    /// IGNORE NULLS: the arguments not null.
    NotNull(NotNull),
}

/// The arguments not null of a partition's rows, in order, and how many.
pub(in crate::exec) struct NotNull {
    /// Each as the one value of a `Placed`.
    pub(in crate::exec) values: HeldRows<Placed>,
    /// How many.
    pub(in crate::exec) len: usize,
}

/// LEAD, LAG, FIRST_VALUE, LAST_VALUE or NTH_VALUE over a partition.
pub(in crate::exec) struct Picked<'a> {
    kind: &'a WindowKind,
    /// Where its argument is among a row's values: then, of LEAD and LAG,
    /// its default, and under IGNORE NULLS, how many of the sequence are
    /// before the row.
    slot: usize,
    count: Count,
    sequence: Sequence,
    /// Readers of the current row, of the place after or at it, of the
    /// sequence's values, and of the ends of a frame's ranges.
    own: Reader,
    ahead: Reader,
    value: Reader,
    ends: [Reader; RANGE_ENDS],
    /// The last frame and its value.
    last: Option<(Ranges, Value)>,
}

impl<'a> Picked<'a> {
    /// `kind`, its values from `slot` on, of `count`, over `sequence`.
    pub(in crate::exec) fn new(kind: &'a WindowKind, slot: usize, count: Count, sequence: Sequence) -> Picked<'a> {
        let (own, ahead, value) = (Reader::default(), Reader::default(), Reader::default());
        Picked { kind, slot, count, sequence, own, ahead, value, ends: Default::default(), last: None }
    }

    /// How many of the sequence are before the place `place` of `part`,
    /// read by `reader`.
    fn before(&mut self, place: usize, part: &mut Partition, which: ReadBy) -> Result<usize> {
        let len = match &self.sequence {
            Sequence::Each => return Ok(place),
            Sequence::NotNull(n) => n.len,
        };
        if place >= part.len {
            return Ok(len);
        }
        let slot = self.counted_slot();
        let reader = self.reader(which);
        let v = reader.get(&mut part.keys, place)?.values.get(slot).ok_or_else(|| broken("a window's row has no count"))?;
        Ok(usize::try_from(whole(v)?).unwrap_or(usize::MAX))
    }

    /// Where the count of the sequence before a row is among its values.
    fn counted_slot(&self) -> usize {
        let defaulted = usize::from(matches!(self.kind, WindowKind::Offset(_)));
        self.slot.saturating_add(1).saturating_add(defaulted)
    }

    /// The reader `which` names.
    fn reader(&mut self, which: ReadBy) -> &mut Reader {
        match which {
            ReadBy::Ahead => &mut self.ahead,
            ReadBy::End(n) => self.ends.get_mut(n).unwrap_or(&mut self.ahead),
        }
    }

    /// How many the sequence has, of `part`.
    fn len(&self, part: &Partition) -> usize {
        match &self.sequence {
            Sequence::Each => part.len,
            Sequence::NotNull(n) => n.len,
        }
    }

    /// The `at`th value of the sequence, of `part`.
    fn at(&mut self, at: usize, part: &mut Partition) -> Result<Value> {
        let (held, slot) = match &mut self.sequence {
            Sequence::Each => (&mut part.keys, self.slot),
            Sequence::NotNull(n) => (&mut n.values, 0),
        };
        self.value.get(held, at)?.values.get(slot).cloned().ok_or_else(|| broken("a window's row has no argument"))
    }

    /// The value at `slot` of the current row `row` of `part`.
    fn own(&mut self, row: usize, slot: usize, part: &mut Partition) -> Result<Value> {
        self.own.get(&mut part.keys, row)?.values.get(slot).cloned().ok_or_else(|| broken("a window's row has no argument"))
    }

    /// [ISO/IEC 9075-2:2023 §6.10 GR1b] LEAD or LAG of the `row`th row of
    /// `part`: the extent of the row `offset` of the sequence after or
    /// before it; of offset 0, its own [GR1b iv 3 B, v 3 B]; else its
    /// default [GR1b iv 3 A, v 3 A].
    pub(in crate::exec) fn offset(&mut self, row: usize, part: &mut Partition) -> Result<Value> {
        let WindowKind::Offset(o) = self.kind else { return Err(broken("a value function computed as LEAD or LAG")) };
        let (lead, offset) = (o.lead, usize::try_from(o.offset).unwrap_or(usize::MAX));
        if offset == 0 {
            return self.own(row, self.slot, part);
        }
        let target = if lead {
            let after = self.before(row.saturating_add(1), part, ReadBy::Ahead)?;
            after.checked_add(offset.saturating_sub(1)).filter(|&t| t < self.len(part))
        } else {
            self.before(row, part, ReadBy::Ahead)?.checked_sub(offset)
        };
        match target {
            Some(at) => self.at(at, part),
            None => self.own(row, self.slot.saturating_add(1), part),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.10 GR1c, GR1d] FIRST_VALUE, LAST_VALUE or
    /// NTH_VALUE of `frame`, a row's frame in `part`: of the frame of the
    /// row before if alike.
    pub(in crate::exec) fn framed(&mut self, frame: Ranges, part: &mut Partition) -> Result<Value> {
        if let Some((last, value)) = &self.last
            && *last == frame
        {
            return Ok(value.clone());
        }
        let value = self.picked(&frame, part)?;
        self.last = Some((frame, value.clone()));
        Ok(value)
    }

    /// The value of `frame` its function picks.
    fn picked(&mut self, frame: &Ranges, part: &mut Partition) -> Result<Value> {
        let WindowKind::Value(v) = self.kind else { return Err(broken("LEAD or LAG computed over a frame")) };
        let mut spans = [(0_usize, 0_usize); 3];
        for (n, (span, &(from, to))) in spans.iter_mut().zip(frame).enumerate() {
            if (from, to) != EMPTY {
                let first = self.before(from, part, ReadBy::End(n.saturating_mul(2)))?;
                *span = (first, self.before(to, part, ReadBy::End(n.saturating_mul(2).saturating_add(1)))?);
            }
        }
        let total = spans.iter().map(|&(first, past)| past.saturating_sub(first)).fold(0_usize, usize::saturating_add);
        let k = match (&v.pick, self.count) {
            (Pick::First, _) => (total > 0).then_some(0),
            (Pick::Last, _) => total.checked_sub(1),
            (Pick::Nth { from_last, .. }, Count::Of(n)) => {
                if *from_last {
                    total.checked_sub(n)
                } else {
                    n.checked_sub(1).filter(|&k| k < total)
                }
            }
            (Pick::Nth { .. }, Count::Null | Count::Unused) => None,
        };
        let Some(mut k) = k else { return Ok(Value::Null) };
        for (first, past) in spans {
            let here = past.saturating_sub(first);
            if k < here {
                return self.at(first.saturating_add(k), part);
            }
            k = k.saturating_sub(here);
        }
        Err(broken("a frame's nth value past its rows"))
    }
}

/// Which reader of a `Picked` reads.
#[derive(Debug, Clone, Copy)]
enum ReadBy {
    /// The row at or after the current one.
    Ahead,
    /// The `n`th end of a frame's ranges.
    End(usize),
}

/// The count of the sequence before a row, as it is held among its values.
pub(in crate::exec) fn counted(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

// Unit tests: the values picked against the frames' rows counted alone
// (RULES.md R-TEST-1, R-TEST-5).
#[cfg(test)]
mod tests;
