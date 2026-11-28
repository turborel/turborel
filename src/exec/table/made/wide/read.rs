//! \[DESIGN\] (RULES.md D1300) A wide partition's groups read where they
//! lie. What reads a partition's groups one set function at a time, as a
//! sort's first rows by set functions do (`parallel/grouped/top/wide.rs`),
//! reads the groups of several rows of the partition's own table and
//! those of one row in the Mades the partition adopted, each result made
//! as a table makes a new group's (`states/placed.rs`).

use std::sync::Arc;

use super::super::super::GroupTable;
use super::super::super::states::Sent;
use super::super::Made;
use super::Wide;
use crate::error::Result;
use crate::exec::broken;
use crate::logical::AggCall;
use crate::storage::mask::index;
use crate::storage::{Column, Row};

/// A Made a wide partition adopted, with the places in it of the
/// partition's groups of one row that lie there, in the order of their
/// first rows.
pub(in crate::exec) type Lying = (Arc<Made>, Vec<u32>);

impl Made {
    /// The states of the set function at `at` of each Made of `runs`,
    /// with the run's places.
    fn sent_of(runs: &[Lying], at: usize) -> Result<Vec<(&Sent, &[u32])>> {
        let held = runs.iter().map(|(made, places)| made.states.get(at).map(|sent| (sent, places.as_slice())));
        held.collect::<Option<Vec<_>>>().ok_or_else(|| broken("a set function a Made does not hold"))
    }

    /// \[DESIGN\] (RULES.md D1300) The results of the set function at
    /// `at`, `agg`, of the groups of `runs`, Made after Made, numbered by
    /// `groups`, as a column: each the result a table gives of a group
    /// merged of that one alone (`states/placed.rs`).
    ///
    /// # Errors
    ///
    /// As a set function's `finish`; SQLCODE -910 of no such function.
    pub(in crate::exec) fn results_of(runs: &[Lying], (at, agg): (usize, &AggCall), groups: &[u32]) -> Result<Column> {
        Sent::results_of(agg, &Made::sent_of(runs, at)?, groups)
    }

    /// Finishes every result of the groups of `runs`, numbered by
    /// `groups`, of each set function of `aggs` but those at `made`,
    /// keeping none, so a result that raises raises as it does when every
    /// group is given.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`.
    pub(in crate::exec) fn checked_but_of(runs: &[Lying], (made, aggs): (&[usize], &[AggCall]), groups: &[u32]) -> Result<()> {
        let mut rest = aggs.iter().enumerate().filter(|(at, _)| !made.contains(at));
        rest.try_for_each(|(at, agg)| Sent::checked_of(agg, &Made::sent_of(runs, at)?, groups))
    }

    /// The first rows' numbers of the groups of `runs`, Made after Made,
    /// as the integers a sort takes.
    pub(in crate::exec) fn firsts_of(runs: &[Lying]) -> Vec<i64> {
        let first = |made: &Made, i: u32| i64::try_from(made.firsts.get(index(i)).copied().unwrap_or(u64::MAX)).unwrap_or(i64::MAX);
        runs.iter().flat_map(|(made, places)| places.iter().map(move |&i| first(made, i))).collect()
    }

    /// The row of the group at `place`, its grouping values then each set
    /// function's result, of the set functions `aggs`, as a table gives
    /// it.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`.
    pub(in crate::exec) fn row_at(&self, place: u32, aggs: &[AggCall]) -> Result<Row> {
        let mut row = self.keys.values(index(place));
        for (sent, agg) in self.states.iter().zip(aggs) {
            row.push(sent.finished_at(agg, place)?);
        }
        Ok(row)
    }
}

impl Wide {
    /// How many columns its keys have.
    pub(in crate::exec) fn width(&self) -> usize {
        self.own.keys.width()
    }

    /// The table of its own, of the groups that took more than one row.
    pub(in crate::exec) fn own(&self) -> &GroupTable {
        &self.own
    }

    /// Its groups of one row, where they lie: each Made that holds any,
    /// in the order handed, with the places of those groups in it, in the
    /// order of their first rows.
    pub(in crate::exec) fn lying(&self) -> Vec<Lying> {
        let mut lying: Vec<Lying> = Vec::new();
        let mut among: Vec<Option<usize>> = vec![None; self.segments.len()];
        for &(segment, place) in &self.lies {
            let (Some(at), Some(held)) = (among.get_mut(index(segment)), self.segments.get(index(segment))) else { continue };
            let Some(made) = &held.made else { continue };
            let k = *at.get_or_insert_with(|| {
                lying.push((Arc::clone(made), Vec::with_capacity(held.lying)));
                lying.len().saturating_sub(1)
            });
            if let Some((_, places)) = lying.get_mut(k) {
                places.push(place);
            }
        }
        lying
    }
}
