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

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use super::{Merging, feed, finished, fresh};
use crate::error::Result;
use crate::exec::group::Spec;
use crate::exec::{Runner, broken};
use crate::physical::{NodeCount, Plan, first_level};
use crate::spill::norm_row;
use crate::storage::Row;

/// The second grouping of a pair: what it groups the first's groups by
/// and computes, and, of an analyzed plan, the count of the first's.
pub(super) struct Outer {
    pub(super) spec: Spec,
    count: Option<Arc<NodeCount>>,
}

impl Outer {
    /// The second grouping of `plan` and the first under it, if `plan` is
    /// such a pair (`physical::first_level`).
    pub(super) fn of(plan: &Plan) -> Option<(Outer, &Plan)> {
        let first = first_level(plan)?;
        let Plan::Aggregate { groups, aggs, input, .. } = plan else { return None };
        // [EXTENSION] The first grouping counted as its groups are made
        // (RULES.md D378).
        let count = if let Plan::Counted(c) = &**input { Some(Arc::clone(&c.count)) } else { None };
        if let Some(c) = &count {
            c.opened();
        }
        let spec = Spec { groups: groups.clone(), collations: Vec::new(), aggs: aggs.clone(), seeds: Vec::new() };
        Some((Outer { spec, count }, first))
    }

    /// The lanes of the first grouping's keys its partitions are split
    /// by: the second's grouping columns, each a grouping column of the
    /// first, its row's grouping values first.
    pub(super) fn lanes(&self) -> &[usize] {
        &self.spec.groups
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
        if let Some(count) = &self.count {
            count.stepped(u64::try_from(firsts.len()).unwrap_or(u64::MAX));
        }
        let mut found: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut made: Vec<Merging> = Vec::new();
        for (first, row) in firsts {
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
        made.into_iter().map(|(first, values, states)| Ok((first, finished(states, values)?))).collect()
    }
}
