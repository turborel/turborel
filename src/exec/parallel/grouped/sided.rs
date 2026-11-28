//! \[DESIGN\] (RULES.md D1324) The plain set functions of a pair of
//! groupings kept by the second's columns alone. A pair computes each
//! set function without DISTINCT in parts (RULES.md D341): the first
//! grouping, by the second's columns and the DISTINCT argument, kept a
//! part's state a group, and the second merged the parts of each of its
//! groups. A state a group of the first is a state a distinct value,
//! grown, sent to the partitions and merged there, then merged again by
//! the second, though the function asks one state a group of the second.
//!
//! Where every part is a column of counts or of exact totals, which
//! merge in any order (RULES.md D1280), the first grouping keeps no
//! state. Each morsel's rows are grouped a second time, by the second's
//! columns alone, into a side table that the parts are fed in, and the
//! side's groups are merged into a table a partition, the partition
//! their key's hash names (`table/sided.rs`). The
//! thread that makes a partition's second grouping takes each plain
//! function's states from the side group of each of its groups
//! (`second.rs`). The first grouping's tables, its partitions and what
//! it writes to the spill are those of a pair with no plain function.
//!
//! The side's tables are held in memory and charged to the grouping's
//! grant (`waves.rs`). They hold a group a value of the second's
//! columns, which the second grouping's tables hold too. If the grant
//! cannot hold them once the first's partitions are in the spill, the
//! part gives way to the serial plan.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::probe::PARTS;
use super::{Grouping, Outer};
use crate::error::Result;
use crate::exec::batch::{Batches, Kept, Read, Taken, side_groups};
use crate::exec::table::{GroupTable, Seed, Sides};
use crate::exec::{Runner, broken};
use crate::functions::MERGED;
use crate::logical::AggCall;

/// The side of a pair: the first grouping's set functions, what they
/// read of a batch by the second's columns, and each partition's groups.
pub(super) struct Side {
    /// The first grouping's set functions, each a part.
    aggs: Vec<AggCall>,
    /// The second's columns as a batch of the pipeline holds them, the
    /// side's key, and what each part takes of a row.
    read: Read,
    /// The seed its keys are hashed by: the first grouping's, each lane
    /// of the key grouped unpadded as its column of the first's key is.
    seed: Seed,
    /// Of each of the second's set functions, the part it merges, if it
    /// merges one.
    from: Vec<Option<usize>>,
    tables: Sides,
    /// The bytes the tables hold.
    bytes: AtomicUsize,
}

/// Of each of the second grouping's set functions `aggs`, taking `taken`
/// of a group of the first, a row of its `width` grouping columns and
/// then its parts, the part it merges whole: none if a function reads a
/// part any other way, or two read one, so the first's groups must hold
/// their parts.
fn merged_parts(aggs: &[AggCall], taken: &[Taken], width: usize) -> Option<Vec<Option<usize>>> {
    let mut from: Vec<Option<usize>> = Vec::with_capacity(aggs.len());
    for (agg, take) in aggs.iter().zip(taken) {
        let part = match take {
            Taken::Count => None,
            Taken::Column(at) | Taken::Kept(at) => at.checked_sub(width),
            Taken::Expr(program) => program.inputs.iter().any(|&at| at >= width).then_some(usize::MAX),
        };
        let whole = matches!(take, Taken::Column(_)) && agg.func.0.name() == MERGED && agg.filter.is_none();
        if part.is_some() && (!whole || from.contains(&part)) {
            return None;
        }
        from.push(part);
    }
    Some(from)
}

impl Side {
    /// The side of the pair `grouping` is the first of, if its plain
    /// functions are kept so: the second grouping reads the first's
    /// groups as a batch, the first's are split among the partitions by
    /// the second's columns, so it crosses none, every set function of
    /// the first is a part kept as counts or exact totals, and each is
    /// read by the second only to be merged whole.
    fn of(grouping: &Grouping) -> Option<Side> {
        let (outer, read) = (grouping.outer.as_ref()?, grouping.read.as_ref()?);
        let (lanes, second) = (outer.lanes(), outer.read.as_ref()?);
        // A second grouping that crosses the first's partitions splits them by no lane.
        let plain = !lanes.is_empty() && GroupTable::parts_whole(&grouping.spec.aggs);
        let from = merged_parts(&outer.spec.aggs, &second.taken, read.groups.len()).filter(|_| plain)?;
        let groups = lanes.iter().map(|&lane| read.groups.get(lane).copied()).collect::<Option<Vec<usize>>>()?;
        let unpadded: Vec<bool> = lanes.iter().map(|&lane| read.unpadded.get(lane).copied().unwrap_or(false)).collect();
        Some(Side {
            aggs: grouping.spec.aggs.clone(),
            seed: grouping.seed.unpadded(&unpadded),
            read: Read { taken: read.taken.clone(), groups, unpadded },
            from,
            tables: (0..PARTS).map(|_| Mutex::new(None)).collect(),
            bytes: AtomicUsize::new(0),
        })
    }

    /// Of each of the second's set functions, the part it merges.
    pub(super) fn from(&self) -> &[Option<usize>] {
        &self.from
    }

    /// The first grouping's set functions.
    pub(super) fn aggs(&self) -> &[AggCall] {
        &self.aggs
    }

    /// Each partition's side groups, and the seed their keys are hashed
    /// by.
    pub(super) fn tables(&self) -> (&Sides, &Seed) {
        (&self.tables, &self.seed)
    }

    /// The bytes its tables hold.
    pub(super) fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Acquire)
    }

    /// A morsel's `batches` given back as they came, after its rows were
    /// grouped by the side's key, each part fed, and the groups merged
    /// into the partitions' side tables. A batch that fails to be read
    /// fails where it did, after the batches before it.
    ///
    /// # Errors
    ///
    /// As a set function, or merging a state, fails.
    pub(super) fn fed(&self, batches: Batches, (runner, first): (&Runner, u64)) -> Result<Batches> {
        let (mut held, mut failed): (Vec<Kept>, _) = (Vec::new(), None);
        for kept in batches {
            match kept {
                Ok(kept) => held.push(kept),
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        let grown = side_groups(&self.read, &self.aggs, &held, (runner, first, &self.seed))?.merge_into_sides(&self.tables)?;
        self.bytes.fetch_add(grown, Ordering::AcqRel);
        Ok(Box::new(held.into_iter().map(Ok).chain(failed.map(Err))))
    }
}

impl Grouping {
    /// \[DESIGN\] (RULES.md D1324) Keeps its set functions by the second
    /// grouping's columns alone, if it is the first of a pair whose
    /// plain functions are kept so (`Side::of`): it computes none itself
    /// from then on, and its second grouping takes them of the side.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a pair that reads no batch once sided.
    pub(super) fn sided(&mut self) -> Result<()> {
        let Some(side) = Side::of(self) else { return Ok(()) };
        self.spec.aggs.clear();
        self.read.as_mut().ok_or_else(|| broken("a pair sided reads no batch"))?.taken.clear();
        let outer: &mut Outer = self.outer.as_mut().ok_or_else(|| broken("a side of no pair"))?;
        outer.side = Some(side);
        Ok(())
    }

    /// The side its plain functions are kept in, of a pair kept so.
    pub(super) fn side(&self) -> Option<&Side> {
        self.outer.as_ref().and_then(|outer| outer.side.as_ref())
    }
}

// Unit tests: which pairs are sided, and what a morsel's side holds
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
