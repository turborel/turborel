//! \[DESIGN\] The built operand of a hash join run in parallel (RULES.md
//! D292, D368): its rows read by its morsels on up to the degree's threads
//! if it is a part run in parallel, each morsel's gathered into a batch,
//! then joined in morsel order into one batch and put in a join's table
//! (`batch/joined/shared.rs`), which every morsel of the probing pipeline
//! then reads at once, as no thread writes it again. Leis et al.,
//! "Morsel-Driven Parallelism" (2014), §4.2, build one table of every row;
//! the table is put together by the statement's thread, a pass over the
//! hashes, so no insertion is atomic, a thing the core cannot make without
//! `unsafe` (RULES.md R-SAFE-1).
//!
//! A row with a null key is left out, as none matches it [X3.135-1986
//! §5.11 GR2], but of a key on which a null matches a null [ISO/IEC
//! 9075-2:2023 §8.15], as the serial join leaves it out. The rows keep the
//! order the operand gives them, so a probing row's matches come in the
//! order they would alone.

use std::sync::Arc;

use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{SharedTable, TABLE_ROW_BYTES};
use crate::exec::{Runner, Seed, execute};
use crate::physical::{JoinKey, Plan};
use crate::storage::{Batch, Row};
use crate::txn::TxId;

/// \[DESIGN\] The partitions of a parallel grouping's and window's rows:
/// enough that each thread of any degree takes several, a power of two.
pub(super) const PARTS: usize = 64;

/// A hash join's built operand.
pub(super) struct Table {
    keys: Vec<JoinKey>,
    pub(super) shared: Arc<SharedTable>,
}

/// \[DESIGN\] The partition of the key `key`, the bytes of a group's
/// values, by its XXH3 seeded by the database's seed, so the morsels of a
/// query agree on each key's partition (RULES.md D357).
pub(super) fn part_of(key: &[u8], seed: &Seed) -> usize {
    let parts = u64::try_from(PARTS).unwrap_or(1);
    seed.xxh3(key).checked_rem(parts).and_then(|p| usize::try_from(p).ok()).unwrap_or_default()
}

impl Table {
    /// Gives `each` each row matching left row `l`, in order.
    ///
    /// # Errors
    ///
    /// As `each` fails, or a column a row does not have (SQLCODE -910).
    pub(super) fn each_match(&self, l: &Row, each: impl FnMut(&Row) -> Result<()>) -> Result<()> {
        self.shared.each_match(&self.keys, l, each)
    }

    /// The join's keys.
    pub(super) fn keys(&self) -> &[JoinKey] {
        &self.keys
    }
}

/// The batch of `rows`, each of `width` columns, every column a vector of
/// values.
fn batch_of(rows: &[Row], width: usize) -> Batch {
    let columns: Vec<usize> = (0..width).collect();
    let mut batch = Batch::empty(width);
    for row in rows {
        batch.push(row, &columns);
    }
    batch
}

/// The bytes a built batch holds, its table's share with it.
fn held_bytes(batch: &Batch) -> usize {
    batch.bytes().saturating_add(batch.rows.saturating_mul(TABLE_ROW_BYTES))
}

/// The operand `right` of a hash join on `keys`, built: by its morsels on
/// up to `degree` threads if it is a part run in parallel, else read by
/// the statement's thread; none if its rows outgrow `limit`, the join then
/// giving way to the serial one, which partitions them (RULES.md D38).
///
/// # Errors
///
/// As reading the operand fails.
pub(super) fn built(right: &Plan, keys: &[JoinKey], (db, tx): (&Db, TxId), limit: &Limit) -> Result<Held<Arc<Table>>> {
    if let Plan::Parallel(p) = right
        && pipelined(&p.input)
        && let Held::Kept(pipeline) = Pipeline::of(&p.input, (db, tx), limit)?
    {
        return parallel_built(pipeline, keys, (db, tx), (p.degree, limit));
    }
    let mut rows = Vec::new();
    for read in execute(right.clone(), db, tx) {
        let row = read?;
        if !limit.take(crate::spill::row_bytes(&row).saturating_add(TABLE_ROW_BYTES)) {
            return Ok(Held::GaveWay);
        }
        rows.push(row);
    }
    let width = width_of(&rows, keys);
    let parts = vec![batch_of(&rows, width)];
    let shared = SharedTable::of(parts, ((0..width).collect(), width), keys, db.functions.seed().clone())?;
    Ok(Held::Kept(Arc::new(Table { keys: keys.to_vec(), shared: Arc::new(shared) })))
}

/// Whether `plan` is a pipeline its morsels read (`pipeline::branched`):
/// scans, filters, projections, counted operators and hash joins' left
/// operands, appended; any other part run in parallel, as a grouping,
/// is read whole.
pub(in crate::exec) fn pipelined(plan: &Plan) -> bool {
    match plan {
        Plan::SeqScan { .. } | Plan::IndexScan { .. } => true,
        Plan::Filter { input, .. } | Plan::Project { input, .. } => pipelined(input),
        Plan::HashJoin { left, .. } => pipelined(left),
        Plan::Append { left, right, .. } => pipelined(left) && pipelined(right),
        Plan::Counted(c) => pipelined(&c.input),
        Plan::Nearest(_)
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::RangeJoin(_)
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..) => false,
    }
}

/// The width of the operand's rows `rows`; of none, past its keys'
/// columns, as no row is read of it.
fn width_of(rows: &[Row], keys: &[JoinKey]) -> usize {
    rows.first().map_or_else(|| keys.iter().map(|k| k.right.saturating_add(1)).max().unwrap_or(0), Vec::len)
}

/// `pipeline`, a hash join's operand, built by its morsels on up to
/// `degree` threads, each morsel's rows gathered into a batch: of its
/// batches, kept rows alone, if it is read by batch, else of its rows.
fn parallel_built(
    pipeline: Pipeline,
    keys: &[JoinKey],
    (db, tx): (&Db, TxId),
    (degree, limit): (usize, &Limit),
) -> Result<Held<Arc<Table>>> {
    let pipeline = Arc::new(pipeline);
    let shape = pipeline.batched.as_ref().map(|b| (b.layout.clone(), b.width));
    let (count, db_for, read) = (pipeline.count(), db.clone(), Arc::clone(&pipeline));
    let batched_width = shape.as_ref().map(|s| s.1);
    let work = Box::new(move |m: usize| -> Result<Batch> {
        let Some(width) = batched_width else {
            let rows = read.morsel(m, &Runner::new(&db_for, tx))?;
            return Ok(batch_of(&rows, rows.first().map_or(0, Vec::len)));
        };
        let parts = read.morsel_kept(m, (&db_for, tx))?.map(|k| {
            k.map(|(b, rows)| {
                let places = crate::storage::sel::places_of(rows, b.rows);
                (b, places)
            })
        });
        let parts = parts.collect::<Result<Vec<_>>>()?;
        Ok(if parts.is_empty() { Batch::empty(width) } else { Batch::joined(parts) })
    });
    let (mut parts, from) = (Vec::new(), limit.held());
    for (done, batch) in (1_usize..).zip(Morsels::start(db, tx, (count, degree), work)) {
        let batch = batch?;
        if !limit.take(held_bytes(&batch)) || !limit.on_course(from, (done, count)) {
            return Ok(Held::GaveWay);
        }
        parts.push(batch);
    }
    let (columns, width) = shape.unwrap_or_else(|| {
        let widest = parts.iter().map(|b| b.columns.len()).max().unwrap_or_else(|| width_of(&[], keys));
        ((0..widest).collect(), widest)
    });
    // A morsel of no rows may hold no vectors.
    parts.retain(|b| b.rows > 0);
    let shared = SharedTable::of(parts, (columns, width), keys, db.functions.seed().clone())?;
    Ok(Held::Kept(Arc::new(Table { keys: keys.to_vec(), shared: Arc::new(shared) })))
}

// Unit tests: a key's partition (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::{PARTS, part_of};
    use crate::exec::Seed;

    /// Verifies: [DESIGN] RULES.md D357: a key's partition is the same each
    /// time a query's seed is asked it, so its morsels agree, and keys
    /// spread over the partitions; under another seed it is the XXH3 of
    /// that seed.
    #[test]
    fn a_key_keeps_its_partition_within_a_query() {
        let seed = Seed::fixed(0x5EED);
        let keys: Vec<Vec<u8>> = (0_u32..4_096).map(|k| k.to_le_bytes().to_vec()).collect();
        let first: Vec<usize> = keys.iter().map(|k| part_of(k, &seed)).collect();
        assert!(keys.iter().zip(&first).all(|(k, &p)| part_of(k, &seed) == p), "the same partition again");
        assert!(first.iter().all(|&p| p < PARTS));
        let mut used = first.clone();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used.len(), PARTS, "every partition takes keys");
        let other = Seed::fixed(0xD1FF);
        let moved = keys.iter().zip(&first).filter(|&(k, &p)| part_of(k, &other) != p).count();
        assert!(moved * 2 > keys.len(), "another seed parts the keys anew, {moved} moved");
    }
}
