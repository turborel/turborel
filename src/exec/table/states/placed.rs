//! \[DESIGN\] (RULES.md D1300, D1337) The results of groups read where
//! they lie, in the Mades a wide partition adopted. Such a group has no
//! state but the Made's, an entry's whole state at one place of one
//! Made, and its result must be the one a table would give of it. A
//! table makes a new group's state by merging the Made's into a state
//! fed nothing.
//!
//! Of a column of counts, of exact totals or of best strings that merge
//! leaves the entry's count, total, scale and string as they are, in
//! whichever form the column holds its totals (RULES.md D1308), so the
//! result is finished of the Made's column at the entry's place, with
//! no state made, and the results that may raise are finished there for
//! their errors alone (`Typed::checked_at`). A column of approximate
//! sums writes no state and its merge is refused, a part's column gives
//! its part and no result, and carried and written states are not
//! columns of this kind. Of each of those the states are merged into
//! states fed nothing as a table merges them, a set function at a time,
//! of the runs of places of one Made after another, and the results are
//! finished of those. No value differs from a table's, whatever the
//! function.

use super::{Sent, States, Typed, parts};
use crate::error::Result;
use crate::exec::broken;
use crate::logical::AggCall;
use crate::storage::mask::index;
use crate::storage::{Column, Ints};
use crate::value::Value;

/// A Made's states of one set function with the places of the groups
/// read of it, in their order.
type Run<'a> = (&'a Sent, &'a [u32]);

impl Sent {
    /// \[DESIGN\] (RULES.md D1337) Its column of states, if the result
    /// of `agg` of a group that lies in it is finished of the column
    /// where the group lies: a column of counts, of exact totals or of
    /// best strings, of a function that is no part of another.
    fn read_in_place(&self, agg: &AggCall) -> Option<&Typed> {
        let Sent::Typed(typed) = self else { return None };
        let whole = match typed {
            Typed::Count(_) | Typed::Exact(_) | Typed::Best(_) => true,
            // It writes no state, so a table refuses to merge it.
            Typed::Approx(_) => false,
        };
        (whole && !parts::is_part(agg)).then_some(typed)
    }

    /// The columns of `runs`, if every one of them is read in place
    /// (`read_in_place`).
    fn in_place<'a>(agg: &AggCall, runs: &[Run<'a>]) -> Option<Vec<&'a Typed>> {
        runs.iter().map(|(sent, _)| sent.read_in_place(agg)).collect()
    }

    /// The states of `agg` a table holds of groups made each of one
    /// group alone: those at the places of each of `runs`, run after
    /// run, the groups numbered by `groups` in that order. A test of the
    /// columns reads them too, as what a group read in place must give.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; SQLCODE -916 of a state never written.
    pub(super) fn placed(agg: &AggCall, runs: &[Run<'_>], groups: &[u32]) -> Result<States> {
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
    /// order: counts as the integers they are, as a table gives them.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn results_of(agg: &AggCall, runs: &[Run<'_>], groups: &[u32]) -> Result<Column> {
        let Some(columns) = Sent::in_place(agg, runs) else { return Sent::placed(agg, runs, groups)?.results(groups.len()) };
        if let Some(counts) = columns.iter().map(|typed| typed.counts()).collect::<Option<Vec<&[i64]>>>() {
            let mut units = Vec::with_capacity(groups.len());
            for (held, (_, places)) in counts.iter().zip(runs) {
                units.extend(places.iter().map(|&place| held.get(index(place)).copied().unwrap_or(0)));
            }
            // [X3.135-1986 §5.8 GR4] A count is an exact number of scale
            // 0, as a table gives a column of counts.
            return Ok(Column::Ints(Ints::new(units, 0, None)));
        }
        let mut values: Vec<Value> = Vec::with_capacity(groups.len());
        for (typed, (_, places)) in columns.iter().zip(runs) {
            for &place in places.iter() {
                values.push(typed.finish(index(place))?);
            }
        }
        Ok(Column::Values(values))
    }

    /// Finishes the result of `agg` of each group at the places of each
    /// of `runs`, numbered by `groups`, and keeps none, for the error
    /// one may raise.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn checked_of(agg: &AggCall, runs: &[Run<'_>], groups: &[u32]) -> Result<()> {
        let Some(columns) = Sent::in_place(agg, runs) else { return Sent::placed(agg, runs, groups)?.checked(groups.len()) };
        columns.iter().zip(runs).try_for_each(|(typed, (_, places))| typed.checked_at(places))
    }

    /// The result of `agg` of the group at `place`.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge` and `Accumulator::finish`.
    pub(in crate::exec::table) fn finished_at(&self, agg: &AggCall, place: u32) -> Result<Value> {
        match self.read_in_place(agg) {
            Some(typed) => typed.finish(index(place)),
            None => Sent::placed(agg, &[(self, &[place])], &[0])?.finish(0),
        }
    }
}
