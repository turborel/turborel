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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use super::super::pipeline::Pipeline;
use super::super::probe::PARTS;
use super::{Grouping, Outer, first_of};
use crate::error::{Result, SqlError};
use crate::exec::batch::{Batches, Kept, Read, Taken, side_groups};
use crate::exec::table::{GroupTable, Seed, Sides};
use crate::exec::{Db, Runner, broken};
use crate::functions::MERGED;
use crate::logical::AggCall;
use crate::noted::Noted;
use crate::txn::TxId;

/// A morsel's batches read before its worker takes them, and the error
/// that ended the reading, if one did.
pub(super) type Probed = (Vec<Kept>, Option<SqlError>);

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

/// \[DESIGN\] (RULES.md D1324) The groups of the first grouping a morsel
/// must make for each group of its side, at least, for the side to be
/// kept. A side group costs about what a first-level group's states cost,
/// each grown, sent to its partition and merged there once a morsel, so
/// the side saves what the first's groups outnumber its own by. At two to
/// one it saves half of what the parts cost. Nearer one to one it saves
/// little or loses, as where the DISTINCT argument is fixed by the
/// grouping columns and each group of the first is a side group too.
const FIRSTS_A_SIDE: usize = 2;

impl Side {
    /// \[DESIGN\] (RULES.md D1324) Whether `held`, the first morsel's
    /// batches, say the side is worth keeping: grouped by the first
    /// grouping's whole key, the columns of a batch `first` names, hashed
    /// by its seed, they make `FIRSTS_A_SIDE` groups at least for each
    /// group they make by the side's key, and one group at least; with
    /// the side's groups. No set function is fed, so nothing is raised.
    fn worth(&self, first: (&[usize], &Seed), held: &[Kept], runner: &Runner) -> Result<(bool, usize)> {
        let groups_by = |groups: &[usize], seed: &Seed| {
            let read = Read { taken: Vec::new(), groups: groups.to_vec(), unpadded: Vec::new() };
            side_groups(&read, &[], held, (runner, 0, seed)).map(|table| table.len())
        };
        let (side, firsts) = (groups_by(&self.read.groups, &self.seed)?, groups_by(first.0, first.1)?);
        Ok((side > 0 && side.saturating_mul(FIRSTS_A_SIDE) <= firsts, side))
    }
}

impl Grouping {
    /// \[DESIGN\] (RULES.md D1324) Keeps its set functions by the second
    /// grouping's columns alone, if it is the first of a pair whose
    /// plain functions may be kept so (`Side::of`) and the first morsel
    /// of `pipeline` says the side is worth keeping (`Side::worth`),
    /// decided here once for the statement: it computes none itself from
    /// then on, and its second grouping takes them of the side. The
    /// morsel's batches are kept for its worker, which reads none again
    /// (`morsel_batches`). A pair that keeps no side runs as before.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a pair that reads no batch once sided.
    pub(super) fn sided(&mut self, pipeline: &Pipeline, (db, tx): (&Db, TxId)) -> Result<()> {
        let Some(side) = Side::of(self).filter(|_| pipeline.count() > 0) else { return Ok(()) };
        let (mut held, mut failed) = (Vec::new(), None);
        for kept in pipeline.morsel_kept(0, (db, tx))? {
            match kept {
                Ok(kept) => held.push(kept),
                Err(e) => failed = Some(e),
            }
            if failed.is_some() {
                break;
            }
        }
        let first = (self.read.as_ref().map_or(&[][..], |read| &read.groups), &self.seed);
        // A morsel that failed to be read fails in its worker's hands, as before.
        let (worth, groups) = if failed.is_some() { (false, 0) } else { side.worth(first, &held, &Runner::new(db, tx))? };
        let outer: &mut Outer = self.outer.as_mut().ok_or_else(|| broken("a side of no pair"))?;
        outer.probed = Mutex::new(Some((held, failed)));
        if !worth {
            return Ok(());
        }
        if let Some(count) = &outer.count {
            count.notes().add(Noted::Side, u64::try_from(groups).unwrap_or(u64::MAX));
        }
        outer.side = Some(side);
        self.spec.aggs.clear();
        self.read.as_mut().ok_or_else(|| broken("a pair sided reads no batch"))?.taken.clear();
        Ok(())
    }

    /// \[DESIGN\] (RULES.md D1324) The batches of morsel `m` of
    /// `pipeline`: the first morsel's as the probe read them, if it read
    /// them, any other's read now; of a pair with a side, after the side
    /// was fed them (`Side::fed`).
    ///
    /// # Errors
    ///
    /// As reading the morsel, or feeding the side, fails.
    pub(super) fn morsel_batches(&self, pipeline: &Pipeline, m: usize, (db, tx): (&Db, TxId), runner: &Runner) -> Result<Batches> {
        let probed =
            self.outer.as_ref().filter(|_| m == 0).and_then(|outer| outer.probed.lock().unwrap_or_else(PoisonError::into_inner).take());
        let batches: Batches = match probed {
            Some((held, failed)) => Box::new(held.into_iter().map(Ok).chain(failed.map(Err))),
            None => pipeline.morsel_kept(m, (db, tx))?,
        };
        match self.side() {
            Some(side) => side.fed(batches, (runner, first_of(self, m))),
            None => Ok(batches),
        }
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
