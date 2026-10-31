//! \[DESIGN\] RULES.md D31, D38: what a grouping by batch writes of the
//! groups it has no room for, and how it reads them back. Groups partly
//! made are written as pieces, each to the partition of its key; else the
//! rows of each batch of each partition, by a hash of their keys
//! (`partition.rs`), are one record (at most `RECORD_ROWS` rows), integers
//! as integers (`records.rs`). A partition is read back record by record,
//! rows grouped again by batch, pieces merged into their groups in the
//! order written (`groups.rs`); one that outgrows the work area is
//! partitioned again, as many times as SPILL.PARTITION_DEPTH says, then
//! made in passes, each reading again what the last had no room for.
//! Each group is numbered by its first row, and the groups merged by
//! number, as `exec/group.rs` merges them, so they come in the order their
//! first rows came, as in memory.

mod records;

use super::table::Held;
use super::{Grouping, Groups, Numbers};
use crate::error::{Code, Result, SqlError};
use crate::exec::RowIter;
use crate::exec::group::Finished;
use crate::exec::partition::{Partitioner, merge_all, numbered};
use crate::exec::work::Work;
use crate::spill::{Records, SpillRun};
use crate::storage::Batch;
pub(super) use records::Piece;
use records::{Record, put_batch, put_group, put_piece, record_of};

/// The most rows a record holds, so a batch's rows of one partition, or of
/// a pass, are a few records of a run's batch of bytes, not one far larger.
const RECORD_ROWS: usize = 256;

/// Where a grouping writes the rows of groups it has no room for: to
/// partitions, or to one run, for the next pass; each made when first
/// needed.
pub(super) enum Overflow {
    Partitions(Option<Partitioner>),
    Run(Option<Box<dyn SpillRun>>),
}

impl Overflow {
    /// The place of the partition of `key`, of those made in `work` the
    /// first time; 0 for a run.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn part_of(&mut self, key: &[u8], work: &mut Work) -> Result<usize> {
        match self {
            Overflow::Partitions(parts) => {
                let parts = match parts {
                    Some(parts) => parts,
                    None => parts.insert(Partitioner::of(work)?),
                };
                Ok(parts.place_of(key))
            }
            Overflow::Run(_) => Ok(0),
        }
    }

    /// Writes the rows `pending` of `batch`, each its partition, place and
    /// number, a record of each partition's at a time, in `work`, in the
    /// order they came; and empties it.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn write(&mut self, batch: &Batch, pending: &mut Vec<(usize, usize, u64)>, work: &mut Work) -> Result<()> {
        // Stable: each partition's rows keep their order.
        pending.sort_by_key(|&(part, ..)| part);
        let mut out = Vec::new();
        for rows in pending.chunk_by(|a, b| a.0 == b.0).flat_map(|part| part.chunks(RECORD_ROWS)) {
            let Some(&(part, ..)) = rows.first() else { continue };
            put_batch(batch, rows, &mut out);
            self.push(part, &out, work)?;
        }
        pending.clear();
        Ok(())
    }

    /// Writes `group`, of key `key`, partly made, as a piece to the
    /// partition of its key, in `work`, made in `out`.
    ///
    /// # Errors
    ///
    /// The spill area's; SQLCODE -910 if a set function of it writes no
    /// state, as a grouping makes groups partly only if each does.
    pub(super) fn write_group(&mut self, (key, group): (&[u8], &Held), out: &mut Vec<u8>, work: &mut Work) -> Result<()> {
        if !put_group(group, out) {
            return Err(SqlError::new(Code::Api, "[DESIGN] a group partly made of a set function of no state"));
        }
        let part = self.part_of(key, work)?;
        self.push(part, out, work)
    }

    /// Writes `made`, a piece of key `key`, to the partition of its key,
    /// in `work`.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(super) fn write_piece(&mut self, key: &[u8], made: &Piece, work: &mut Work) -> Result<()> {
        let mut out = Vec::new();
        put_piece(made, &mut out);
        let part = self.part_of(key, work)?;
        self.push(part, &out, work)
    }

    /// Writes `record` to the partition at `part`, or to the run, made in
    /// `work` the first time.
    fn push(&mut self, part: usize, record: &[u8], work: &mut Work) -> Result<()> {
        match self {
            Overflow::Partitions(Some(parts)) => parts.push_at(part, record),
            Overflow::Run(made) => {
                let run = match made {
                    Some(run) => run,
                    None => made.insert(work.run()?),
                };
                run.push(record)
            }
            Overflow::Partitions(None) => Err(lost()),
        }
    }
}

/// The groups: `held`, each numbered by its first row, in that order, and
/// those of the rows written to `overflow`, grouped as `how` says, in
/// `work`, merged by number.
///
/// # Errors
///
/// The spill area's; the set functions'.
pub(super) fn merged(held: Finished, overflow: Overflow, how: Grouping<'_>, work: &mut Work) -> Result<RowIter> {
    let Overflow::Partitions(Some(parts)) = overflow else { return Ok(Box::new(held.into_iter().map(|(_, row)| Ok(row)))) };
    let mut sources = vec![written(held, work)?];
    for run in parts.into_runs() {
        regrouped(run.into_records()?, (0, &mut sources), how, work)?;
    }
    Ok(Box::new(merge_all(sources, work)?.map(|r| r.map(|(_, row)| row))))
}

/// The groups of the rows of the records `partition`, partitioned `depth`
/// times again before, each pass's groups a run added to `sources`; groups
/// made partly while each pass before went on making them.
fn regrouped(partition: Records, (depth, sources): (usize, &mut Vec<Records>), how: Grouping<'_>, work: &mut Work) -> Result<()> {
    let (mut next, mut going) = (Some(partition), how);
    while let Some(records) = next.take() {
        let to = if depth < work.depth() { Overflow::Partitions(None) } else { Overflow::Run(None) };
        let mut groups = Groups::new(work.limit(), to, going.partial);
        let types = work.types().clone();
        for record in records {
            match record_of(&record?, &types)? {
                Record::Rows(batch, numbers) => groups.feed((&batch, None), Numbers::Given(&numbers), going, work)?,
                Record::Piece(piece) => groups.absorb(&piece, going, work)?,
            }
        }
        let (held, written_to, partial) = groups.finished(work)?;
        going.partial = partial;
        sources.push(written(held, work)?);
        match written_to {
            Overflow::Partitions(Some(parts)) => {
                for run in parts.into_runs() {
                    regrouped(run.into_records()?, (depth.saturating_add(1), &mut *sources), going, work)?;
                }
            }
            Overflow::Run(Some(run)) => next = Some(run.into_records()?),
            Overflow::Partitions(None) | Overflow::Run(None) => {}
        }
    }
    Ok(())
}

/// Groups `held` in a run of `work`, read from the first.
fn written(held: Finished, work: &mut Work) -> Result<Records> {
    let mut run = work.run()?;
    for (n, row) in held {
        run.push(&numbered(n, &row))?;
    }
    run.into_records()
}

/// The error of partitions written before they were made, or of a piece
/// with no group to go to.
pub(super) fn lost() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a grouping's partitions were not made")
}
