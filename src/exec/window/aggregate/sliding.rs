//! [ISO/IEC 9075-2:2023 §9.23 SR8d, §10.9 GR4b, Feature T627] COUNT
//! (DISTINCT ...) over a frame that moves. \[DESIGN\] The values of the
//! rows of the last frame are counted, each value's count kept in a bag
//! that spills past the work area (`exec/work.rs`), with how many values
//! have a count above 0 and how many rows' values are null. Each frame
//! adds the rows it has that the last had not and takes away those the
//! last had that it has not. A frame's bounds only move forward, and what
//! its exclusion leaves out moves with the current row, so each row is
//! added and taken away a few times at most over its partition.

use std::slice;

use super::{admitted, argument};
use crate::error::Result;
use crate::exec::broken;
use crate::exec::window::frame::{Partition, Ranges};
use crate::exec::window::reader::Reader;
use crate::exec::work::{Bag, Work};
use crate::logical::AggCall;
use crate::spill::norm_row;
use crate::value::{Decimal, Value, cast};

/// The most places a pair of frames is cut at: each end of each of their
/// ranges.
const CUTS: usize = 12;

/// The values of the rows of the frame last counted.
pub(super) struct Sliding {
    counts: Bag,
    /// How many values have a count above 0.
    distinct: u64,
    /// How many rows counted have a null value.
    nulls: u64,
    /// The frame whose rows are counted.
    held: Ranges,
    adding: Reader,
    taking: Reader,
}

/// Whether one of `ranges` holds the place `at`.
fn holds(ranges: &Ranges, at: usize) -> bool {
    ranges.iter().any(|&(from, to)| from <= at && at < to)
}

impl Sliding {
    /// No rows counted, their values' counts kept in `work`.
    pub(super) fn new(work: &Work) -> Sliding {
        let none = [(0, 0); 3];
        Sliding {
            counts: Bag::new(work.another()),
            distinct: 0,
            nulls: 0,
            held: none,
            adding: Reader::default(),
            taking: Reader::default(),
        }
    }

    /// [GR4b] The value of `call`, COUNT(DISTINCT ...) of its argument at
    /// `slot`, over `frame` of `part`, and whether it left out a null: the
    /// rows of the last frame counted, those it has not taken away and
    /// those it has added.
    ///
    /// # Errors
    ///
    /// The spill area's; SQLCODE -910 if `call` is COUNT(*).
    pub(super) fn value(&mut self, (call, slot): (&AggCall, usize), frame: &Ranges, part: &mut Partition) -> Result<(Value, bool)> {
        let mut cuts = [0_usize; CUTS];
        for (cut, at) in cuts.iter_mut().zip(frame.iter().chain(&self.held).flat_map(|&(from, to)| [from, to])) {
            *cut = at;
        }
        cuts.sort_unstable();
        for pair in cuts.windows(2) {
            let (&[from, to], held) = (pair, self.held) else { continue };
            let (now, before) = (holds(frame, from), holds(&held, from));
            for at in from..to {
                match (now, before) {
                    (true, false) => self.count((call, slot), at, part, true)?,
                    (false, true) => self.count((call, slot), at, part, false)?,
                    (true, true) | (false, false) => {}
                }
            }
        }
        self.held = *frame;
        let count = Value::Exact(Decimal::new(i128::from(self.distinct), 0));
        Ok((cast(count, &call.result)?, self.nulls > 0))
    }

    /// Adds the value of the row at `at` of `part`, or takes it away if
    /// not `adding`, if `call` takes the row.
    fn count(&mut self, (call, slot): (&AggCall, usize), at: usize, part: &mut Partition, adding: bool) -> Result<()> {
        let reader = if adding { &mut self.adding } else { &mut self.taking };
        let values = &reader.get(&mut part.keys, at)?.values;
        if !admitted(call, slot, values)? {
            return Ok(());
        }
        let Some(v) = argument(call, slot, values)? else { return Err(broken("COUNT(*) counted as COUNT(DISTINCT ...)")) };
        if v.is_null() {
            self.nulls = if adding { self.nulls.saturating_add(1) } else { self.nulls.saturating_sub(1) };
            return Ok(());
        }
        let key = norm_row(slice::from_ref(v));
        let changed = if adding { self.counts.raise(&key)? } else { self.counts.lower(&key)? };
        if changed {
            self.distinct = if adding { self.distinct.saturating_add(1) } else { self.distinct.saturating_sub(1) };
        }
        Ok(())
    }
}
