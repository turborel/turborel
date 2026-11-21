//! \[DESIGN\] A hash join whose right operand outgrows its work area
//! (RULES.md D38), partitioned as a Grace hash join is: the rows of both
//! operands are written to runs by a hash of their key values, so the rows
//! that may match are in partitions of the same number, and each pair of
//! partitions is joined in turn, its right partition built in memory.
//! A right partition that still outgrows the work area is partitioned
//! again, by another hash, up to SPILL.PARTITION_DEPTH times; one that does not shrink,
//! as when one key holds most of its rows, is built in a table of the
//! spill area instead, each probe reading its key's rows from the table as
//! they are joined, never all at once. Every run is read and written in
//! order, where a probe of a spilled table reads at random. Every run is
//! read back only while the statement is not cancelled (`exec/work.rs`).
//!
//! The joined rows must come in the left operand's order, which the
//! planner relies on. Each left row is numbered as it is read, each
//! partition's joined rows written in its left rows' order with their
//! numbers, and the partitions' joined rows merged by number, in passes
//! of as many as the work area reads at once. A join of streams whose
//! left operand's order is not asked for takes the pairs in turn
//! (`Pending`) and gives each pair's joined rows by batches as they are
//! made, written to no run (`exec/batch/joined/parted.rs`).
//!
//! A row's partition is of bits of its key's hash as a join's table
//! hashes it (`exec/table/join.rs`, RULES.md D389), by the database's
//! seed, mixed by MurmurHash3's finalizer with a word its work area draws
//! for each partitioning (`Work::drawn`), so a partitioning again splits
//! by another hash, and two runs alike split alike. Each
//! pair's right partition held in memory is put in a join's table, its
//! left's rows probing it a batch at a time, each joined row written with
//! its left row's number as before.

use crate::exec::batch::SharedTable;
use crate::exec::table::{Hashing, Seed, values_hash};
use crate::hash::murmur::fmix64;
use crate::storage::{BATCH_ROWS, Batch};

use super::super::partition::{Merge, merge_all, numbered, unnumbered};
use super::super::work::Work;
use super::super::{RowIter, failed};
use super::{Built, Full, Operand, Right, each_match, key};
use crate::error::{Code, Result, SqlError};
use crate::physical::JoinKey;
use crate::spill::{Records, SpillRun, SpilledLobs, row_bytes};
use crate::storage::Row;
use crate::value::types::TypeRegistry;

/// \[DESIGN\] The default of SPILL.PARTITIONS: the most partitions a join
/// writes at once.
pub const DEFAULT_PARTITIONS: usize = 64;
/// \[DESIGN\] The default of SPILL.PARTITION_DEPTH: the most times a
/// partition is partitioned again.
pub const DEFAULT_PARTITION_DEPTH: usize = 8;

/// A hash join's operands, as they are partitioned.
pub(in crate::exec) struct Partitions {
    keys: Vec<JoinKey>,
    /// The seed its keys hash by, and the word their hash is mixed with.
    seed: (Seed, u64),
    /// The right operand's partitions, each with the bytes its rows take
    /// in memory.
    right: Vec<(Box<dyn SpillRun>, usize)>,
    /// The sources of what its rows name, held while it is.
    lobs: SpilledLobs,
}

impl Partitions {
    /// No partitions yet of an operand joined on `keys`: as many as `work`
    /// writes at once.
    fn new(keys: Vec<JoinKey>, work: &mut Work) -> Result<Partitions> {
        let mut right = Vec::new();
        for _ in 0..work.width().min(work.partitions()) {
            right.push((work.run()?, 0));
        }
        Ok(Partitions { keys, seed: (work.seed().clone(), work.drawn()), right, lobs: work.lobs().clone() })
    }

    /// The right operand's partitions, of the rows `built` holds and the
    /// rest of the operand, `rows`, in runs of `work`'s spill area.
    pub(super) fn of(built: Built, rows: impl Iterator<Item = Result<Row>>, work: &mut Work) -> Result<Partitions> {
        let mut parts = Partitions::new(built.keys.clone(), work)?;
        // A bucket goes whole to one partition, so its rows keep their order.
        for (_, held) in built.into_buckets() {
            for row in &held {
                parts.add_right(row)?;
            }
        }
        parts.add_rights(rows)?;
        Ok(parts)
    }

    /// The partition of the key of `row`, of the operand `operand`: bits
    /// of its hash above its tag's.
    fn of_key(&self, row: &Row, operand: Operand) -> Result<usize> {
        let columns = self.keys.iter().map(|k| if matches!(operand, Operand::Left) { k.left } else { k.right });
        let values = columns.filter_map(|c| row.get(c));
        let nulls: Vec<bool> = self.keys.iter().map(|k| k.nulls).collect();
        let hash = values_hash(values, &nulls, Hashing { seed: &self.seed.0, keyed: true, direct: false })?.unwrap_or_default();
        let hash = fmix64(hash ^ self.seed.1);
        let n = u64::try_from(self.right.len()).unwrap_or(1);
        Ok(usize::try_from((hash >> TAG_BITS).checked_rem(n).unwrap_or_default()).unwrap_or_default())
    }

    /// Writes the right rows `rows` with a key to their partitions.
    fn add_rights(&mut self, rows: impl Iterator<Item = Result<Row>>) -> Result<()> {
        for r in rows {
            let row = r?;
            if key(&row, &self.keys, Operand::Right)?.is_some() {
                self.add_right(&row)?;
            }
        }
        Ok(())
    }

    /// Writes right row `row`, of key values `b`, to its partition.
    fn add_right(&mut self, row: &Row) -> Result<()> {
        let at = self.of_key(row, Operand::Right)?;
        let (run, bytes) = self.right.get_mut(at).ok_or_else(lost)?;
        *bytes = bytes.saturating_add(row_bytes(row));
        run.push(&self.lobs.record(row))
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
            if key(&row, &self.keys, Operand::Left)?.is_some() {
                lefts.get_mut(self.of_key(&row, Operand::Left)?).ok_or_else(lost)?.push(&numbered(n, &row, &self.lobs))?;
            }
        }
        Ok(lefts)
    }
}

/// The rows of `left` joined with the partitioned right operand `parts`,
/// in `left`'s order, in `work`. Never inlined into the caller, whose frame
/// is on the stack while the left operand is made (RULES.md R-CF-6).
#[inline(never)]
pub(in crate::exec) fn joined(left: RowIter, parts: Partitions, mut work: Work) -> RowIter {
    match merged(left, parts, &mut work) {
        Ok(rows) => Box::new(rows.map(|r| r.map(|(_, row)| row))),
        Err(e) => failed(e),
    }
}

/// `joined`'s work: the left operand partitioned, each pair of partitions
/// joined, and their joined rows, numbered, merged.
fn merged(left: RowIter, parts: Partitions, work: &mut Work) -> Result<Merge> {
    let mut pending = Pending::of(left, parts, work)?;
    let (types, lobs) = (work.types().clone(), work.lobs().clone());
    let mut sources = Vec::new();
    while let Some((mut built, mut probes)) = pending.next(work)? {
        let mut out = work.run()?;
        if let Some(rows) = built.taken_rows() {
            // A partition its seed left empty matches no left row, and has
            // no row to give its table its width.
            if let Some(width) = rows.first().map(Vec::len) {
                let table = SharedTable::of_rows(&rows, width, &pending.keys, (drawn(), (0..width).collect(), work.dense()))?;
                while probed_batch(&table, (&mut probes, &pending.keys), (&types, &lobs), &mut *out)? {}
            }
        } else {
            for record in probes {
                let (n, l) = unnumbered(&record?, &types, &lobs)?;
                each_match(&built, &l, |row| out.push(&numbered(n, &row, &lobs)))?;
            }
        }
        sources.push(out.into_records()?);
    }
    merge_all(sources, work)
}

/// \[DESIGN\] The bits of a key's hash its table's tag takes, below
/// those that pick its partition.
const TAG_BITS: u32 = 4;

/// A seed drawn for a partitioning or a table of one.
fn drawn() -> Seed {
    Seed::default()
}

/// Joins a batch of the next numbered left rows of `probes` with `table`,
/// their keys of `keys`, writing each joined row to `out` with its left
/// row's number, in order: whether any row was read.
fn probed_batch(
    table: &SharedTable,
    (probes, keys): (&mut Records, &[JoinKey]),
    (types, lobs): (&TypeRegistry, &SpilledLobs),
    out: &mut dyn SpillRun,
) -> Result<bool> {
    let (mut numbers, mut lefts) = (Vec::new(), Vec::new());
    for record in probes.by_ref().take(BATCH_ROWS) {
        let (n, l) = unnumbered(&record?, types, lobs)?;
        numbers.push(n);
        lefts.push(l);
    }
    let width = lefts.first().map_or(0, Vec::len);
    let columns: Vec<usize> = (0..width).collect();
    let mut batch = Batch::empty(width);
    for l in &lefts {
        batch.push(l, &columns);
    }
    let places: Vec<usize> = keys.iter().map(|k| k.left).collect();
    for (r, p) in table.matched(&batch, &places)? {
        let (Some(&n), Some(l)) = (numbers.get(r), lefts.get(r)) else { continue };
        let row: Row = l.iter().cloned().chain(table.row_of(p)).collect();
        out.push(&numbered(n, &row, lobs))?;
    }
    Ok(!lefts.is_empty())
}

/// \[DESIGN\] The pairs of a partitioned hash join's partitions not yet
/// joined, each taken in turn: its right partition built in memory, or,
/// past the work area, partitioned again with its left, by another hash,
/// up to SPILL.PARTITION_DEPTH times, or built in a table of the spill
/// area once that depth is passed or it did not shrink.
pub(in crate::exec) struct Pending {
    keys: Vec<JoinKey>,
    types: TypeRegistry,
    lobs: SpilledLobs,
    pairs: Vec<Pair>,
}

/// A pair of partitions: the right's run and the bytes its rows take held
/// in memory, the left's run of numbered rows, and how many times they
/// were partitioned again.
struct Pair {
    right: Box<dyn SpillRun>,
    bytes: usize,
    left: Box<dyn SpillRun>,
    depth: usize,
}

impl Pending {
    /// The pairs of the partitioned right operand `parts` and of the left
    /// operand `left`, its rows numbered in the order they came and
    /// written to partitions of `work`.
    ///
    /// # Errors
    ///
    /// The left operand's; the spill area's.
    pub(in crate::exec) fn of(left: RowIter, parts: Partitions, work: &mut Work) -> Result<Pending> {
        let mut numbers = 0_u64..;
        let lefts = parts.lefts(left.map(|r| r.map(|row| (numbers.next().unwrap_or(u64::MAX), row))), work)?;
        let (types, lobs) = (work.types().clone(), work.lobs().clone());
        let mut pending = Pending { keys: parts.keys.clone(), types, lobs, pairs: Vec::new() };
        pending.add(parts, lefts, 0);
        Ok(pending)
    }

    /// Adds the pairs of `parts` and `lefts`, partitioned `depth` times.
    fn add(&mut self, parts: Partitions, lefts: Vec<Box<dyn SpillRun>>, depth: usize) {
        // Taken from the end: the first partitions first.
        let pairs = parts.right.into_iter().zip(lefts).map(|((right, bytes), left)| Pair { right, bytes, left, depth });
        self.pairs.extend(pairs.rev());
    }

    /// The next pair of partitions to join: the right built, and the left's
    /// numbered rows, as records; none once every pair is taken.
    ///
    /// # Errors
    ///
    /// The spill area's.
    pub(in crate::exec) fn next(&mut self, work: &mut Work) -> Result<Option<(Built, Records)>> {
        while let Some(Pair { right, bytes, left, depth }) = self.pairs.pop() {
            let (held, named) = (self.types.clone(), self.lobs.clone());
            let rows = right.into_records()?.map(move |r| r.and_then(|record| named.record_row(&record, &held)));
            if bytes > work.limit() && depth < work.depth() {
                let mut sub = Partitions::new(self.keys.clone(), work)?;
                sub.add_rights(rows)?;
                // A partition that did not shrink is built in a table.
                let shrank = sub.right.iter().all(|(_, b)| *b < bytes);
                let (probed, lobs) = (self.types.clone(), self.lobs.clone());
                let sub_lefts =
                    sub.lefts(left.into_records()?.map(move |r| r.and_then(|record| unnumbered(&record, &probed, &lobs))), work)?;
                self.add(sub, sub_lefts, if shrank { depth.saturating_add(1) } else { usize::MAX });
                continue;
            }
            let Right::Held(built) = Built::of(rows, self.keys.clone(), (&mut *work, Full::Table))? else { return Err(lost()) };
            return Ok(Some((built, left.into_records()?)));
        }
        Ok(None)
    }
}

/// The error of a partition that is not where it was put.
fn lost() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a hash join's partition is missing")
}
