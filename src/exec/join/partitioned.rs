//! \[DESIGN\] A hash join whose right operand outgrows its work area
//! (RULES.md D38), partitioned as a Grace hash join is: the rows of both
//! operands are written to runs by a hash of their key values, so the rows
//! that may match are in partitions of the same number, and each pair of
//! partitions is joined in turn, its right partition built in memory.
//! A right partition that still outgrows the work area is partitioned
//! again, by another hash, up to SPILL.PARTITION_DEPTH times; one that does not shrink,
//! as when one key holds most of its rows, is built in a table of the
//! spill area instead. Every run is read and written in order, where a
//! probe of a spilled table reads at random.
//!
//! The joined rows must come in the left operand's order, which the
//! planner relies on. Each left row is numbered as it is read, each
//! partition's joined rows written in its left rows' order with their
//! numbers, and the partitions' joined rows merged by number, in passes
//! of as many as the work area reads at once. Split from exec/join.rs
//! (RULES.md R-SIZE-2).

use std::hash::{BuildHasher, RandomState};

use super::super::partition::{Merge, merge_all, numbered, unnumbered};
use super::super::work::Work;
use super::super::{RowIter, failed};
use super::{Built, Full, Right, key, matches};
use crate::error::{Code, Result, SqlError};
use crate::spill::{Records, SpillRun, bucket_key, record_row, row_bytes, row_record};
use crate::storage::Row;
use crate::value::Bucket;

/// \[DESIGN\] The default of SPILL.PARTITIONS: the most partitions a join
/// writes at once.
pub const DEFAULT_PARTITIONS: usize = 64;
/// \[DESIGN\] The default of SPILL.PARTITION_DEPTH: the most times a
/// partition is partitioned again.
pub const DEFAULT_PARTITION_DEPTH: usize = 8;

/// A hash join's operands, as they are partitioned.
pub(in crate::exec) struct Partitions {
    keys: Vec<(usize, usize)>,
    hasher: RandomState,
    /// The right operand's partitions, each with the bytes its rows take
    /// in memory.
    right: Vec<(Box<dyn SpillRun>, usize)>,
}

impl Partitions {
    /// No partitions yet of an operand joined on `keys`: as many as `work`
    /// writes at once.
    fn new(keys: Vec<(usize, usize)>, work: &mut Work) -> Result<Partitions> {
        let mut right = Vec::new();
        for _ in 0..work.width().min(work.partitions()) {
            right.push((work.run()?, 0));
        }
        Ok(Partitions { keys, hasher: RandomState::new(), right })
    }

    /// The right operand's partitions, of the rows `built` holds and the
    /// rest of the operand, `rows`, in runs of `work`'s spill area.
    pub(super) fn of(built: Built, rows: impl Iterator<Item = Result<Row>>, work: &mut Work) -> Result<Partitions> {
        let mut parts = Partitions::new(built.keys, work)?;
        // A bucket goes whole to one partition, so its rows keep their order.
        for (b, held) in built.buckets {
            for row in &held {
                parts.add_right(&b, row)?;
            }
        }
        parts.add_rights(rows)?;
        Ok(parts)
    }

    /// The partition of key values `b`.
    fn of_key(&self, b: &[Bucket]) -> usize {
        let n = u64::try_from(self.right.len()).unwrap_or(1);
        let at = self.hasher.hash_one(bucket_key(b)).checked_rem(n).unwrap_or_default();
        usize::try_from(at).unwrap_or_default()
    }

    /// Writes the right rows `rows` with a key to their partitions.
    fn add_rights(&mut self, rows: impl Iterator<Item = Result<Row>>) -> Result<()> {
        for r in rows {
            let row = r?;
            if let Some(b) = key(&row, self.keys.iter().map(|k| k.1))? {
                self.add_right(&b, &row)?;
            }
        }
        Ok(())
    }

    /// Writes right row `row`, of key values `b`, to its partition.
    fn add_right(&mut self, b: &[Bucket], row: &Row) -> Result<()> {
        let at = self.of_key(b);
        let (run, bytes) = self.right.get_mut(at).ok_or_else(lost)?;
        *bytes = bytes.saturating_add(row_bytes(row));
        run.push(&row_record(row))
    }

    /// The left rows `rows`, each with its number, those with a key in
    /// runs of `work`, one for each partition.
    fn lefts(&self, rows: impl Iterator<Item = Result<(u64, Row)>>, work: &mut Work) -> Result<Vec<Box<dyn SpillRun>>> {
        let mut lefts = Vec::new();
        for _ in 0..self.right.len() {
            lefts.push(work.run()?);
        }
        for r in rows {
            let (n, row) = r?;
            if let Some(b) = key(&row, self.keys.iter().map(|k| k.0))? {
                lefts.get_mut(self.of_key(&b)).ok_or_else(lost)?.push(&numbered(n, &row))?;
            }
        }
        Ok(lefts)
    }
}

/// The rows of `left` joined with the partitioned right operand `parts`,
/// in `left`'s order, in `work`. Never inlined into the caller, whose frame
/// is on the stack while the left operand is made (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn joined(left: RowIter, parts: Partitions, mut work: Work) -> RowIter {
    match merged(left, parts, &mut work) {
        Ok(rows) => Box::new(rows.map(|r| r.map(|(_, row)| row))),
        Err(e) => failed(e),
    }
}

/// `joined`'s work: the left operand partitioned, each pair of partitions
/// joined, and their joined rows merged.
fn merged(left: RowIter, parts: Partitions, work: &mut Work) -> Result<Merge> {
    let mut numbers = 0_u64..;
    let lefts = parts.lefts(left.map(|r| r.map(|row| (numbers.next().unwrap_or(u64::MAX), row))), work)?;
    let mut sources = Vec::new();
    pairs((parts, lefts), work, (&mut sources, 0))?;
    merge_all(sources, work)
}

/// Joins each pair of partitions of `parts` and `lefts`, their joined rows,
/// numbered, added to `sources`; a right partition that outgrows the work
/// area is partitioned again, at `depth` from the first.
fn pairs(
    (parts, lefts): (Partitions, Vec<Box<dyn SpillRun>>),
    work: &mut Work,
    (sources, depth): (&mut Vec<Records>, usize),
) -> Result<()> {
    for ((right, bytes), probes) in parts.right.into_iter().zip(lefts) {
        let rows = right.into_records()?.map(|r| r.and_then(|record| record_row(&record)));
        if bytes > work.limit() && depth < work.depth() {
            let mut sub = Partitions::new(parts.keys.clone(), work)?;
            sub.add_rights(rows)?;
            // A partition that did not shrink is built in a table.
            let shrank = sub.right.iter().all(|(_, b)| *b < bytes);
            let sub_lefts = sub.lefts(probes.into_records()?.map(|r| r.and_then(|record| unnumbered(&record))), work)?;
            pairs((sub, sub_lefts), work, (&mut *sources, if shrank { depth.saturating_add(1) } else { usize::MAX }))?;
            continue;
        }
        let Right::Held(built) = Built::of(rows, parts.keys.clone(), (&mut *work, Full::Table))? else { return Err(lost()) };
        let mut out = work.run()?;
        for record in probes.into_records()? {
            let (n, l) = unnumbered(&record?)?;
            for row in matches(&built, &l)? {
                out.push(&numbered(n, &row))?;
            }
        }
        sources.push(out.into_records()?);
    }
    Ok(())
}

/// The error of a partition that is not where it was put.
fn lost() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a hash join's partition is missing")
}
