//! [ISO/IEC 9075-2:2023 §10.9 GR10, §9.23 SR8d i] An inverse distribution
//! function over a window, which has no ordering and no frame, so every
//! row's frame is its whole partition. \[DESIGN\] Its values not null are
//! sorted once for each partition, in a work area that spills, and held
//! as rows are, so they may outgrow memory (RULES.md D38). Each row's
//! fraction, which may differ from row to row, is computed with its
//! argument, and the one or two values it picks read by place.

use super::frame::Partition;
use super::reader::Reader;
use crate::error::Result;
use crate::exec::sort::Sorter;
use crate::exec::work::Work;
use crate::exec::{HeldRows, broken};
use crate::functions::{picked, picked_value};
use crate::logical::{AggCall, SortKey};
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::value::Value;

/// The readers an inverse distribution function keeps of a partition's
/// places: that of each row's fraction, and the segment of its sorted
/// values held and their reader's.
pub(super) const DISTRIBUTION_READERS: usize = 3;

/// An inverse distribution function over the partition of each row.
pub(in crate::exec) struct Distributed<'a> {
    call: &'a AggCall,
    /// Where its fraction is among a row's values.
    fraction: usize,
    /// Its partition's values not null, sorted.
    sorted: HeldRows<Row>,
    /// Whether a value of a row it takes was null, eliminated.
    nulls: bool,
    reader: Reader,
}

impl<'a> Distributed<'a> {
    /// Of `call`, its values not null sorted in `work` and held in `space`.
    /// Its argument is at `slot` among the values of each row of `part`,
    /// then whether it takes the row if filtered, then its fraction.
    pub(in crate::exec) fn new(
        call: &'a AggCall,
        slot: usize,
        part: &mut Partition,
        (space, work): (&ListSpace, &Work),
    ) -> Result<Distributed<'a>> {
        let Some(d) = call.distribution.as_deref() else { return Err(broken("an inverse distribution function is no such call")) };
        let filtered = call.filter.is_some();
        let fraction = slot.saturating_add(1).saturating_add(usize::from(filtered));
        let mut work = work.part(space.limit);
        let mut sorter = Sorter::new(vec![SortKey { column: 0, descending: d.order.descending, nulls_first: false }]);
        let (mut reader, mut nulls) = (Reader::default(), false);
        for row in 0..part.len {
            let values = &reader.get(&mut part.keys, row)?.values;
            let taken = !filtered || matches!(values.get(slot.saturating_add(1)), Some(Value::Bool(true)));
            match values.get(slot) {
                Some(Value::Null) => nulls = nulls || taken,
                Some(v) => sorter.push(vec![v.clone()], &mut work)?,
                None => return Err(broken("an inverse distribution function's argument is not among a row's values")),
            }
        }
        let mut held = HeldRows::new(space.clone());
        for row in sorter.sorted(&mut work)? {
            held.push(row?)?;
        }
        Ok(Distributed { call, fraction, sorted: held, nulls, reader: Reader::default() })
    }

    /// [GR10] Its value of the `row`th row of `part`, of the fraction of
    /// that row, and whether a null was eliminated.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22003 if the fraction is below 0 or above 1, even of no
    /// values; as computing the result raises.
    pub(in crate::exec) fn value(&mut self, row: usize, part: &mut Partition) -> Result<(Value, bool)> {
        let Some(d) = self.call.distribution.as_deref() else { return Err(broken("an inverse distribution function is no such call")) };
        let placed = self.reader.get(&mut part.keys, row)?;
        let fraction = placed
            .values
            .get(self.fraction)
            .ok_or_else(|| broken("an inverse distribution function's fraction is not among a row's values"))?;
        let n = u64::try_from(self.sorted.len()).unwrap_or(u64::MAX);
        let Some(chosen) = picked(d.continuous, fraction, n)? else { return Ok((Value::Null, self.nulls)) };
        let mut value_at = |at: u64| -> Result<Value> {
            let place = usize::try_from(at).map_err(|_| broken("a sorted value's place is past memory's"))?;
            let found = self.sorted.get(place)?.ok_or_else(|| broken("a sorted value past the last"))?;
            found.into_iter().next().ok_or_else(|| broken("a sorted value of no column"))
        };
        let low = value_at(chosen.low)?;
        let high = if chosen.high == chosen.low { low.clone() } else { value_at(chosen.high)? };
        Ok((picked_value(d.continuous, (low, high), chosen.factor, &self.call.result)?, self.nulls))
    }
}
