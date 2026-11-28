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
//!
//! \[DESIGN\] (RULES.md D1219) A second grouping by no column whose set
//! functions are COUNT alone, as COUNT(DISTINCT) of no GROUP BY is made,
//! crosses the partitions too: its one group takes groups of every
//! partition, each partition's thread counts its own, and the statement's
//! thread merges the counts, one group even of no rows. With no
//! projection between the two, a partition's groups of the first are
//! read as a batch from their table (`second.rs`), no row made of one.
//!
//! \[DESIGN\] (RULES.md D1281) A sort that asks for its first rows over
//! a pair takes them from the second's table of each partition
//! (`second_table`, `top.rs`), where the second is made in one and
//! crosses no partition (`in_tables`).

mod crossing;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Merging, feed, finished, fresh};
use crate::error::Result;
use crate::exec::batch::{Read, read_for};
use crate::exec::group::{Spec, group_key, unpadded_keys};
use crate::exec::table::{GroupTable, Seed};
use crate::exec::{Runner, broken, eval};
use crate::logical::Expr;
use crate::physical::parallel::{between, counted_between, crosses};
use crate::physical::{NodeCount, Plan, first_level};
use crate::storage::Row;
use crossing::{Crossing, Merged};

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
    /// \[DESIGN\] (RULES.md D1123, D1219) What it reads of the first's
    /// groups given as a batch, if it reads them so (`second.rs`): of one
    /// with no projection between the two, each set function as a
    /// grouping by batch takes it.
    read: Option<Read>,
}

impl Outer {
    /// The second grouping of `plan` and the first under it, if `plan` is
    /// such a pair (`physical::first_level`).
    pub(super) fn of(plan: &Plan) -> Option<(Outer, &Plan)> {
        let first = first_level(plan)?;
        let Plan::Aggregate { groups, collations, aggs, input, .. } = plan else { return None };
        let Plan::Aggregate { groups: keys, aggs: parts, .. } = first else { return None };
        let layout: Vec<usize> = (0..keys.len().saturating_add(parts.len())).collect();
        let (steps, below) = between(input, 0);
        // [EXTENSION] The first grouping counted as its groups are made
        // (RULES.md D378).
        let count = if let Plan::Counted(c) = below { Some(Arc::clone(&c.count)) } else { None };
        let steps: Vec<Vec<Expr>> = steps.into_iter().map(<[Expr]>::to_vec).collect();
        // Through a projection its groups are made rows, each evaluated.
        let batched = steps.is_empty();
        let crossing = crosses(plan).then(|| Crossing::new(steps));
        let projected = counted_between(input, 0);
        for c in count.iter().chain(&projected) {
            c.opened();
        }
        // [ISO/IEC 9075-2:2023 §4.3.2] Its columns under a collation whose
        // key is the text itself are grouped unpadded, as the first's are
        // (RULES.md D1184).
        let unpadded = unpadded_keys(collations)?;
        let read = if batched { read_for(groups, aggs, &layout).map(|read| Read { unpadded, ..read }) } else { None };
        let spec = Spec { groups: groups.clone(), collations: collations.clone(), aggs: aggs.clone(), seeds: Vec::new() };
        Some((Outer { spec, count, projected, crossing, read }, first))
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
        self.counted(firsts.len());
        let mut found: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut made: Vec<Merging> = Vec::new();
        for (first, row) in firsts {
            let Some(row) = self.projected(row, runner) else { return Ok(Vec::new()) };
            let values = self.spec.groups.iter().map(|&g| row.get(g).cloned().ok_or_else(|| broken("a grouping column past its row")));
            let values = values.collect::<Result<Row>>()?;
            let at = match found.entry(group_key(&self.spec, &values)?) {
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
            crossing.add(&self.spec, written.collect(), runner)?;
            return Ok(Vec::new());
        }
        made.into_iter().map(|(first, values, states)| Ok((first, finished(states, values)?))).collect()
    }

    /// \[EXTENSION\] (RULES.md D378, D1116) Counts `firsts` groups of the
    /// first grouping, of an analyzed plan: the first's count and that of
    /// each projection between the two.
    fn counted(&self, firsts: usize) {
        for count in self.count.iter().chain(&self.projected) {
            count.stepped(u64::try_from(firsts).unwrap_or(u64::MAX));
        }
    }

    /// \[DESIGN\] (RULES.md D1123) Whether it groups `first`, a
    /// partition's groups of the first grouping, in the executor's table
    /// (`regrouped_table`): it reads them as a batch, and they are in the
    /// order of their first rows, the order its set functions take them
    /// in.
    pub(super) fn tabled(&self, first: &GroupTable) -> bool {
        self.read.is_some() && first.in_order()
    }

    /// \[DESIGN\] (RULES.md D1281) Whether a sort's first rows are taken
    /// from its tables. Each partition then makes its groups whole in a
    /// table of its own, so it reads the first's groups as a batch and
    /// crosses no partition. A group of one that crosses lies in several
    /// partitions, and no partition's table holds its row.
    pub(super) fn in_tables(&self) -> bool {
        self.read.is_some() && self.crossing.is_none()
    }

    /// [X3.135-1986 §5.22 GR2] The table of the groups of `first`, a
    /// partition's groups of the first grouping it groups in a table
    /// (`tabled`), each numbered by its first group of the first and none
    /// finished; none of one it does not read so.
    ///
    /// # Errors
    ///
    /// As a set function fails.
    pub(super) fn second_table(&self, first: GroupTable, (runner, seed): (&Runner, &Seed)) -> Result<Option<GroupTable>> {
        let Some(read) = &self.read else { return Ok(None) };
        self.counted(first.len());
        // Its keys are hashed and compared as its own columns are grouped,
        // each by its place in its key (RULES.md D1184).
        super::second::regrouped(first, (read, &self.spec.aggs), (runner, &seed.unpadded(&read.unpadded))).map(Some)
    }

    /// [X3.135-1986 §5.22 GR2] The groups of `first`, a partition's
    /// groups of the first grouping it groups in a table (`tabled`), as
    /// `regrouped` gives them of the same groups, those of one that
    /// crosses the partitions kept written and none given; none of one
    /// it does not read so.
    ///
    /// # Errors
    ///
    /// As a set function fails.
    pub(super) fn regrouped_table(&self, first: GroupTable, run: (&Runner, &Seed)) -> Result<Vec<(u64, Row)>> {
        let Some(table) = self.second_table(first, run)? else { return Ok(Vec::new()) };
        let Some(crossing) = &self.crossing else { return table.finished() };
        // Of one that crosses the partitions, each group's states written
        // for the statement's thread to merge (RULES.md D1219).
        let written = (0..table.len()).map(|g| {
            let states = (0..table.functions()).map(|a| {
                let mut state = Vec::new();
                table.write_state((g, a), &mut state);
                state
            });
            (table.first(g), table.values(g), states.collect())
        });
        crossing.add(&self.spec, written.collect(), run.0)?;
        Ok(Vec::new())
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
    /// groups of all, merged, those `keep` keeps of them, in the order of
    /// their firsts (`Crossing::merged`); of any other, none; none if a
    /// projection failed.
    ///
    /// # Errors
    ///
    /// As merging a state, reading the spill back, or `keep` fails.
    pub(super) fn merged(&self, keep: impl FnMut(Vec<(u64, Row)>) -> Result<Vec<(u64, Row)>>) -> Result<Option<Merged>> {
        let Some(crossing) = &self.crossing else { return Ok(Some((Vec::new(), Vec::new()))) };
        crossing.merged(&self.spec, keep)
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
