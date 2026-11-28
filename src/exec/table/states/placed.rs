//! \[DESIGN\] (RULES.md D1300) The results of groups read where they lie,
//! in the Mades a wide partition adopted. A group of one row has no state
//! but the Made's, and its result must be the one a table would give of
//! it. A table makes a new group's state by merging the Made's into a
//! state fed nothing, so that is what is done here, a set function at a
//! time, of the runs of places of one Made after another, and the
//! results are finished of those states. No value differs from a
//! table's, whatever the function.

use super::{Sent, States};
use crate::error::Result;
use crate::exec::broken;
use crate::logical::AggCall;
use crate::storage::Column;
use crate::value::Value;

impl Sent {
    /// The states of `agg` a table holds of groups made each of one
    /// group alone: those at the places of each of `runs`, a Made's
    /// states with its places, run after run, the groups numbered by
    /// `groups` in that order.
    fn placed(agg: &AggCall, runs: &[(&Sent, &[u32])], groups: &[u32]) -> Result<States> {
        let mut states = States::of(agg);
        states.grow_by(groups.len());
        let mut from = 0_usize;
        for &(sent, places) in runs {
            let to = from.saturating_add(places.len());
            let run = groups.get(from..to).ok_or_else(|| broken("fewer numbers than groups read where they lie"))?;
            states.merge_all(agg, run, (sent, places))?;
            from = to;
        }
        Ok(states)
    }

    /// The results of `agg` of the groups at the places of each of
    /// `runs`, numbered by `groups`, as a column, a group a row in that
    /// order.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn results_of(agg: &AggCall, runs: &[(&Sent, &[u32])], groups: &[u32]) -> Result<Column> {
        Sent::placed(agg, runs, groups)?.results(groups.len())
    }

    /// Finishes the result of `agg` of each group at the places of each
    /// of `runs`, numbered by `groups`, and keeps none, for the error one
    /// may raise.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn checked_of(agg: &AggCall, runs: &[(&Sent, &[u32])], groups: &[u32]) -> Result<()> {
        Sent::placed(agg, runs, groups)?.checked(groups.len())
    }

    /// The result of `agg` of the group at `place`.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn finished_at(&self, agg: &AggCall, place: u32) -> Result<Value> {
        Sent::placed(agg, &[(self, &[place])], &[0])?.finish(0)
    }
}
