//! \[DESIGN\] Rows partitioned, and merged back (RULES.md D38): an operator
//! whose rows outgrow its work area writes them to partitions by a hash of
//! a key of theirs, so rows of one key are in one partition, and deals with
//! each partition alone, in memory; rows it gives are numbered by where
//! they came in, and the partitions' rows merged back by number, in passes
//! of as many as the work area reads at once, so they come in the order
//! they would have in memory. The hash join, the outer join, EXCEPT and
//! INTERSECT, DISTINCT and grouping partition so.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::hash::{BuildHasher, RandomState};

use super::work::Work;
use crate::error::{Code, Result, SqlError};
use crate::spill::{Records, SpillRun, record_room, record_row};
use crate::storage::Row;
use crate::value::codec::put_row;
use crate::value::types::TypeRegistry;

/// Records written to partitions by a hash of a key of theirs.
pub(super) struct Partitioner {
    hasher: RandomState,
    runs: Vec<Box<dyn SpillRun>>,
}

impl Partitioner {
    /// `count` partitions, at least one, each a run of `work`'s spill area.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn new(count: usize, work: &mut Work) -> Result<Partitioner> {
        let runs = (0..count.max(1)).map(|_| work.run()).collect::<Result<_>>()?;
        Ok(Partitioner { hasher: RandomState::new(), runs })
    }

    /// As many partitions as `work` writes at once, of SPILL.PARTITIONS.
    ///
    /// # Errors
    ///
    /// As `new`.
    pub(super) fn of(work: &mut Work) -> Result<Partitioner> {
        Partitioner::new(work.width().min(work.partitions()), work)
    }

    /// As many partitions, of the same hash, so a key's records go to the
    /// partition of the same place in each: for the other operand.
    ///
    /// # Errors
    ///
    /// As `new`.
    pub(super) fn alike(&self, work: &mut Work) -> Result<Partitioner> {
        let runs = (0..self.runs.len()).map(|_| work.run()).collect::<Result<_>>()?;
        Ok(Partitioner { hasher: self.hasher.clone(), runs })
    }

    /// Writes `record` to the partition of `key`.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn push(&mut self, key: &[u8], record: &[u8]) -> Result<()> {
        self.push_at(self.place_of(key), record)
    }

    /// The place of the partition of `key`.
    pub(super) fn place_of(&self, key: &[u8]) -> usize {
        let n = u64::try_from(self.runs.len()).unwrap_or(1);
        usize::try_from(self.hasher.hash_one(key).checked_rem(n).unwrap_or_default()).unwrap_or_default()
    }

    /// Writes `record` to the partition at `at`, as `place_of` places it:
    /// for records of many rows, of one partition.
    ///
    /// # Errors
    ///
    /// The spill area's; SQLCODE -910 if it has no such partition.
    pub(super) fn push_at(&mut self, at: usize, record: &[u8]) -> Result<()> {
        self.runs.get_mut(at).ok_or_else(lost)?.push(record)
    }

    /// The partitions.
    pub(super) fn into_runs(self) -> Vec<Box<dyn SpillRun>> {
        self.runs
    }
}

/// Runs of numbered rows `sources` merged by number, in passes of as many
/// as `work` reads at once, the last a `Merge` read as it is pulled.
pub(in crate::exec) fn merge_all(mut sources: Vec<Records>, work: &mut Work) -> Result<Merge> {
    while sources.len() > work.width() {
        let mut fewer = Vec::new();
        let mut rest = sources.into_iter();
        loop {
            let group: Vec<Records> = rest.by_ref().take(work.width()).collect();
            if group.is_empty() {
                break;
            }
            let mut run = work.run()?;
            for r in Merge::of(group, work.types())? {
                let (n, row) = r?;
                run.push(&numbered(n, &row))?;
            }
            fewer.push(run.into_records()?);
        }
        sources = fewer;
    }
    Merge::of(sources, work.types())
}

/// Row `row` with its number `n`, as a record.
pub(in crate::exec) fn numbered(n: u64, row: &Row) -> Vec<u8> {
    let mut out = Vec::with_capacity(record_room(size_of::<u64>(), row.len()));
    out.extend_from_slice(&n.to_le_bytes());
    put_row(&mut out, row);
    out
}

/// The number and the row of a record `numbered` made.
pub(in crate::exec) fn unnumbered(record: &[u8], types: &TypeRegistry) -> Result<(u64, Row)> {
    let malformed = || SqlError::new(Code::Corrupt, "[DESIGN] a joined row's record is malformed");
    let (n, row) = record.split_at_checked(8).ok_or_else(malformed)?;
    let n = <[u8; 8]>::try_from(n).map(u64::from_le_bytes).map_err(|_| malformed())?;
    Ok((n, record_row(row, types)?))
}

/// Runs of numbered rows, each in order of number, merged in that order:
/// each run's next row, and the heap of their numbers.
pub(in crate::exec) struct Merge {
    sources: Vec<Records>,
    heads: BinaryHeap<Reverse<(u64, usize)>>,
    rows: Vec<Option<Row>>,
    types: TypeRegistry,
}

impl Merge {
    /// The rows of `sources`, merged.
    fn of(sources: Vec<Records>, types: &TypeRegistry) -> Result<Merge> {
        let rows = sources.iter().map(|_| None).collect();
        let mut merge = Merge { sources, heads: BinaryHeap::new(), rows, types: types.clone() };
        for source in 0..merge.sources.len() {
            merge.refill(source)?;
        }
        Ok(merge)
    }

    /// Takes the next row of `source`, if any.
    fn refill(&mut self, source: usize) -> Result<()> {
        let Some(next) = self.sources.get_mut(source).and_then(Iterator::next) else { return Ok(()) };
        let (n, row) = unnumbered(&next?, &self.types)?;
        *self.rows.get_mut(source).ok_or_else(lost)? = Some(row);
        self.heads.push(Reverse((n, source)));
        Ok(())
    }
}

impl Iterator for Merge {
    type Item = Result<(u64, Row)>;

    fn next(&mut self) -> Option<Result<(u64, Row)>> {
        let Reverse((n, source)) = self.heads.pop()?;
        let row = self.rows.get_mut(source).and_then(Option::take)?;
        Some(self.refill(source).map(|()| (n, row)))
    }
}

/// The error of a partition not as it was written.
fn lost() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a partition is not as it was written")
}
