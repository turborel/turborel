//! \[DESIGN\] (RULES.md D1300, D1337) A wide partition's groups read
//! where they lie. What reads a partition's groups one set function at a
//! time, as a sort's first rows by set functions do
//! (`parallel/grouped/top/wide.rs`), reads the groups that took several
//! entries of the partition's own table and those of one entry in the
//! Mades the partition adopted. An entry is what a Made holds for the
//! partition to look up once, a morsel's group, a row handed on
//! ungrouped or a run of rows of one key, so a group of one entry may
//! hold the state of many rows, whole at one place of one Made. Its
//! result is the one a table gives of it (`states/placed.rs`).

use std::sync::Arc;

use super::super::super::GroupTable;
use super::super::super::states::Sent;
use super::super::{Made, part_of};
use super::Wide;
use crate::error::Result;
use crate::exec::broken;
use crate::logical::AggCall;
use crate::storage::mask::index;
use crate::storage::{Column, Row};

/// \[DESIGN\] (RULES.md D1339) A Made a wide partition adopted, with the
/// places in it of the partition's groups of one entry that lie there.
/// Of a Made every entry of which the partition was handed still lies
/// in it, those are the Made's own run of the partition, read as the
/// Made holds it. Of any other they are listed, a sub-part's after
/// another's.
pub(in crate::exec) struct Lying {
    made: Arc<Made>,
    /// The places listed, or none of a Made read by its own run.
    listed: Option<Vec<u32>>,
    /// The partition whose run of the Made is read, if no place is
    /// listed.
    part: usize,
}

impl Lying {
    /// The Made the groups lie in.
    pub(in crate::exec) fn made(&self) -> &Made {
        &self.made
    }

    /// The places of the groups that lie in the Made.
    pub(in crate::exec) fn places(&self) -> &[u32] {
        self.listed.as_deref().unwrap_or_else(|| self.made.places_of(self.part))
    }
}

/// \[DESIGN\] (RULES.md D1339) The partition of `made` whose places,
/// whole and as the Made holds them, `places` are, if they are: the
/// partition its first group falls in, where the run handed is that
/// partition's own, the same places at the same address.
pub(super) fn whole_of(made: &Made, places: &[u32]) -> Option<usize> {
    let hash = places.first().and_then(|&first| made.hashes.get(index(first)))?;
    let part = part_of(*hash, made.starts.len().checked_sub(1)?);
    std::ptr::eq(made.places_of(part), places).then_some(part)
}

impl Made {
    /// The states of the set function at `at` of each Made of `runs`,
    /// with the run's places.
    fn sent_of(runs: &[Lying], at: usize) -> Result<Vec<(&Sent, &[u32])>> {
        let held = runs.iter().map(|run| run.made.states.get(at).map(|sent| (sent, run.places())));
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
        runs.iter().flat_map(|run| run.places().iter().map(move |&i| first(&run.made, i))).collect()
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
        self.width
    }

    /// The tables of its sub-parts' own that hold a group, of the groups
    /// that took more than one entry, in the sub-parts' order.
    pub(in crate::exec) fn owns(&self) -> Vec<&GroupTable> {
        self.parts.iter().map(|part| &part.own).filter(|own| own.len() > 0).collect()
    }

    /// \[DESIGN\] (RULES.md D1339) Its groups of one entry, where they
    /// lie: each Made that holds any, in the order handed. A Made every
    /// entry of which it was handed still lies in it is given to be read
    /// by its own run of the partition. The places of the groups that
    /// lie in any other are listed, a sub-part's after another's, and
    /// if no Made needs a list the groups' lists are not read at all.
    pub(in crate::exec) fn lying(&self) -> Vec<Lying> {
        let held = || self.segments.iter().zip(&self.lying);
        // The partition each Made is read by its own run of, if it is.
        let runs: Vec<Option<usize>> =
            held().map(|(segment, &groups)| segment.whole.filter(|run| run.1 == groups).map(|run| run.0)).collect();
        let room = |(&groups, own): (&usize, &Option<usize>)| Vec::with_capacity(if own.is_some() { 0 } else { groups });
        let mut places: Vec<Vec<u32>> = self.lying.iter().zip(&runs).map(room).collect();
        if self.lying.iter().zip(&runs).any(|(&groups, own)| groups > 0 && own.is_none()) {
            for &(segment, place) in self.parts.iter().flat_map(|part| &part.lies) {
                // A group in a table of its own names no segment.
                if let (Some(list), Some(None)) = (places.get_mut(index(segment)), runs.get(index(segment))) {
                    list.push(place);
                }
            }
        }
        let mut lying = Vec::with_capacity(self.segments.len());
        for ((segment, &groups), (listed, own)) in held().zip(places.into_iter().zip(runs)) {
            if let Some(made) = segment.made.as_ref().filter(|_| groups > 0) {
                lying.push(Lying { made: Arc::clone(made), listed: own.is_none().then_some(listed), part: own.unwrap_or(0) });
            }
        }
        lying
    }
}
