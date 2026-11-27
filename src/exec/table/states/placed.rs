//! \[DESIGN\] (RULES.md D1300) The results of groups read where they lie,
//! in the Made a wide partition adopted. A group of one row has no state
//! but the Made's, and its result must be the one a table would give of
//! it. A table makes a new group's state by merging the Made's into a
//! state fed nothing, so that is what is done here, of a run of places a
//! set function at a time, and the results are finished of those states.
//! No value differs from a table's, whatever the function.

use super::super::narrow;
use super::{Sent, States};
use crate::error::Result;
use crate::logical::AggCall;
use crate::storage::Column;
use crate::value::Value;

impl Sent {
    /// The states of `agg` a table holds of groups made of the groups at
    /// `places` alone, in that order.
    fn placed(&self, agg: &AggCall, places: &[u32]) -> Result<States> {
        let mut states = States::of(agg);
        states.grow_by(places.len());
        let groups: Vec<u32> = (0..places.len()).map(narrow).collect();
        states.merge_all(agg, &groups, (self, places))?;
        Ok(states)
    }

    /// The results of `agg` of the groups at `places`, as a column, a
    /// group a row in that order.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn results_at(&self, agg: &AggCall, places: &[u32]) -> Result<Column> {
        self.placed(agg, places)?.results(places.len())
    }

    /// Finishes the result of `agg` of each group at `places` and keeps
    /// none, for the error one may raise.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn checked_at(&self, agg: &AggCall, places: &[u32]) -> Result<()> {
        self.placed(agg, places)?.checked(places.len())
    }

    /// The result of `agg` of the group at `place`.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn finished_at(&self, agg: &AggCall, place: u32) -> Result<Value> {
        self.placed(agg, &[place])?.finish(0)
    }
}
