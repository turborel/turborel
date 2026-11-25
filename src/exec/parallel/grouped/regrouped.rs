//! \[DESIGN\] (RULES.md D1113) A grouping over another whose grouping
//! columns hold its own, as DISTINCT_AGGREGATES makes them (RULES.md
//! D340), run as one part (`physical::first_level`). The first grouping's
//! groups are split among the partitions by their values of the second's
//! columns alone, so each partition holds every group of the first that a
//! group of the second takes, and the thread finishing a partition makes
//! the second's groups of it too: its groups of the first, in the order
//! of their first rows, are grouped again as the serial grouping groups
//! its input, each set function taking its values in that order. A group
//! of the second is numbered by its first group of the first, the first
//! row of its own, so the statement's thread merges the partitions' groups
//! in the order the serial plan gives them, and no more than those.
//!
//! \[DESIGN\] (RULES.md D1133) Of a second grouping that crosses the
//! first's partitions (`physical::crosses`), the partitions are the
//! first's by their whole keys, and each partition's thread takes its
//! groups of the first through the projections between the two and
//! groups them again, its groups' states written. The statement's thread
//! merges the states of each group of the partitions (`merged`), numbered
//! by the least of their firsts, the first row of the group of all. Its
//! set functions are COUNT alone, whose count is the same however its
//! rows are taken. A projection that fails in a partition gives the whole
//! part way to the serial plan, which raises what it raises, as the rows
//! it fails of are then met in their order.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::{Merging, feed, finished, fresh};
use crate::error::Result;
use crate::exec::group::Spec;
use crate::exec::{Runner, broken, eval};
use crate::logical::Expr;
use crate::physical::parallel::{between, counted_between, crosses};
use crate::physical::{NodeCount, Plan, first_level};
use crate::spill::norm_row;
use crate::storage::Row;

/// The second grouping of a pair: what it groups the first's groups by
/// and computes, and, of an analyzed plan, the count of the first's.
pub(super) struct Outer {
    pub(super) spec: Spec,
    count: Option<Arc<NodeCount>>,
    /// \[EXTENSION\] (RULES.md D1116) Of an analyzed plan, the counts of
    /// the projections between the two, each given a row a group of the
    /// first.
    projected: Vec<Arc<NodeCount>>,
    /// \[DESIGN\] (RULES.md D1133) Of one that crosses the first's
    /// partitions, the projections between the two, the nearest the first
    /// last; its partitions' groups, each with its first and its states
    /// written; and whether a projection failed in one.
    crossing: Option<Crossing>,
}

/// What a second grouping crossing the first's partitions keeps of them.
struct Crossing {
    steps: Vec<Vec<Expr>>,
    groups: Mutex<Vec<Written>>,
    failed: AtomicBool,
}

/// A group of a partition: its first, its grouping values and each set
/// function's state written.
type Written = (u64, Row, Vec<Vec<u8>>);

impl Outer {
    /// The second grouping of `plan` and the first under it, if `plan` is
    /// such a pair (`physical::first_level`).
    pub(super) fn of(plan: &Plan) -> Option<(Outer, &Plan)> {
        let first = first_level(plan)?;
        let Plan::Aggregate { groups, aggs, input, .. } = plan else { return None };
        let (steps, below) = between(input, 0);
        // [EXTENSION] The first grouping counted as its groups are made
        // (RULES.md D378).
        let count = if let Plan::Counted(c) = below { Some(Arc::clone(&c.count)) } else { None };
        let steps = steps.into_iter().map(<[Expr]>::to_vec).collect();
        let crossing = crosses(plan).then(|| Crossing { steps, groups: Mutex::new(Vec::new()), failed: AtomicBool::new(false) });
        let projected = counted_between(input, 0);
        for c in count.iter().chain(&projected) {
            c.opened();
        }
        let spec = Spec { groups: groups.clone(), collations: Vec::new(), aggs: aggs.clone(), seeds: Vec::new() };
        Some((Outer { spec, count, projected, crossing }, first))
    }

    /// The lanes of the first grouping's keys its partitions are split
    /// by: the second's grouping columns, each a grouping column of the
    /// first, its row's grouping values first.
    pub(super) fn lanes(&self) -> &[usize] {
        if self.crossing.is_some() { &[] } else { &self.spec.groups }
    }

    /// Whether it crosses the first's partitions, its groups merged by
    /// the statement's thread (`merged`).
    pub(super) fn crosses(&self) -> bool {
        self.crossing.is_some()
    }

    /// [X3.135-1986 §5.22 GR2] The groups of `firsts`, a partition's
    /// groups of the first grouping in the order of their first rows,
    /// each with its number: each group of the second finished, numbered
    /// by its first, in that order.
    ///
    /// # Errors
    ///
    /// As a set function fails.
    pub(super) fn regrouped(&self, firsts: Vec<(u64, Row)>, runner: &Runner) -> Result<Vec<(u64, Row)>> {
        for count in self.count.iter().chain(&self.projected) {
            count.stepped(u64::try_from(firsts.len()).unwrap_or(u64::MAX));
        }
        let mut found: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut made: Vec<Merging> = Vec::new();
        for (first, row) in firsts {
            let Some(row) = self.projected(row, runner) else { return Ok(Vec::new()) };
            let values = self.spec.groups.iter().map(|&g| row.get(g).cloned().ok_or_else(|| broken("a grouping column past its row")));
            let values = values.collect::<Result<Row>>()?;
            let at = match found.entry(norm_row(&values)) {
                Entry::Occupied(occupied) => *occupied.get(),
                Entry::Vacant(vacant) => {
                    vacant.insert(made.len());
                    made.push((first, values, fresh(&self.spec.aggs)));
                    made.len().saturating_sub(1)
                }
            };
            let states = made.get_mut(at).map(|g| &mut g.2).ok_or_else(|| broken("a group of a partition is missing"))?;
            feed(&self.spec.aggs, states, &row, runner)?;
        }
        if let Some(crossing) = &self.crossing {
            let written = made.into_iter().map(|(first, values, states)| (first, values, written(&states)));
            crossing.groups.lock().unwrap_or_else(PoisonError::into_inner).extend(written);
            return Ok(Vec::new());
        }
        made.into_iter().map(|(first, values, states)| Ok((first, finished(states, values)?))).collect()
    }

    /// `row`, a group of the first, through the projections between the
    /// two, if it crosses the partitions; none if one fails, which is
    /// noted, so the part gives way.
    fn projected(&self, row: Row, runner: &Runner) -> Option<Row> {
        let Some(crossing) = &self.crossing else { return Some(row) };
        let mut row = row;
        for exprs in crossing.steps.iter().rev() {
            let made: Result<Row> = exprs.iter().map(|e| eval(e, &row, runner)).collect();
            let Ok(made) = made else {
                crossing.failed.store(true, Ordering::Relaxed);
                return None;
            };
            row = made;
        }
        Some(row)
    }

    /// [X3.135-1986 §5.22 GR2] Of one that crosses the partitions, its
    /// groups of all, each partition's states of a group merged, numbered
    /// by the least first, in that order; none if a projection failed.
    ///
    /// # Errors
    ///
    /// As merging a state fails.
    pub(super) fn merged(&self) -> Result<Option<Vec<(u64, Row)>>> {
        let Some(crossing) = &self.crossing else { return Ok(Some(Vec::new())) };
        if crossing.failed.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let groups = std::mem::take(&mut *crossing.groups.lock().unwrap_or_else(PoisonError::into_inner));
        let mut found: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut made: Vec<Merging> = Vec::new();
        for (number, values, states) in groups {
            let at = *found.entry(norm_row(&values)).or_insert_with(|| {
                made.push((number, values, fresh(&self.spec.aggs)));
                made.len().saturating_sub(1)
            });
            let group = made.get_mut(at).ok_or_else(|| broken("a merged group is missing"))?;
            group.0 = group.0.min(number);
            for (acc, state) in group.2.iter_mut().zip(&states) {
                acc.merge(state)?;
            }
        }
        let mut out = made.into_iter().map(|(first, values, states)| Ok((first, finished(states, values)?))).collect::<Result<Vec<_>>>()?;
        out.sort_unstable_by_key(|g| g.0);
        Ok(Some(out))
    }
}

/// Each set function's state of `states`, written.
fn written(states: &[Box<dyn crate::functions::Accumulator>]) -> Vec<Vec<u8>> {
    states
        .iter()
        .map(|acc| {
            let mut out = Vec::new();
            acc.write_state(&mut out);
            out
        })
        .collect()
}
