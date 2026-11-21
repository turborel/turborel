//! \[DESIGN\] The built operand of a hash join run in parallel (RULES.md
//! D292): its rows by bucket of key values in partitions, each partition's
//! table built by one thread from its rows in their order, then read by
//! every morsel of the probing pipeline at once, as no thread writes it
//! again. Leis et al., "Morsel-Driven Parallelism" (2014), §4.2, build one
//! table of every row; partitions built apart need no atomic insertion, a
//! thing the core cannot make without `unsafe` (RULES.md R-SAFE-1).
//!
//! A row with a null key is left out, as none matches it [X3.135-1986
//! §5.11 GR2], but of a key on which a null matches a null [ISO/IEC
//! 9075-2:2023 §8.15], as the serial join leaves it out (`exec/join.rs`).
//! Each bucket keeps its rows in the order the operand gives them, so a
//! probing row's matches come in the order they would alone.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::join::{Operand, equal, key};
use crate::exec::{Runner, broken, execute};
use crate::physical::{JoinKey, Plan};
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Bucket;

/// \[DESIGN\] The partitions of a built operand: enough that each thread of
/// any degree builds several, a power of two.
pub(super) const PARTS: usize = 64;

/// A built operand's rows of one partition, each with its key's buckets,
/// by morsel in order.
type Chunks = Vec<Vec<(Vec<Bucket>, Row)>>;

/// One partition's table: its buckets, each with its rows in order.
type Part = HashMap<Vec<Bucket>, Vec<Row>>;

/// A hash join's built operand.
pub(super) struct Table {
    keys: Vec<JoinKey>,
    parts: Vec<Part>,
}

/// The partition of the key of buckets `b`.
pub(super) fn part_of(b: &impl Hash) -> usize {
    let mut hasher = DefaultHasher::new();
    b.hash(&mut hasher);
    usize::try_from(hasher.finish()).unwrap_or_default().checked_rem(PARTS).unwrap_or_default()
}

impl Table {
    /// Gives `each` each row matching left row `l`, in order.
    ///
    /// # Errors
    ///
    /// As `each` fails, or a column a row does not have (SQLCODE -910).
    pub(super) fn each_match(&self, l: &Row, mut each: impl FnMut(&Row) -> Result<()>) -> Result<()> {
        let Some(b) = key(l, &self.keys, Operand::Left)? else { return Ok(()) };
        let Some(rows) = self.parts.get(part_of(&b)).and_then(|p| p.get(&b)) else { return Ok(()) };
        for r in rows {
            if equal(&self.keys, l, r)? {
                each(r)?;
            }
        }
        Ok(())
    }
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
        && let Held::Kept(pipeline) = Pipeline::of(&p.input, (db, tx), limit)?
    {
        return parallel_built(pipeline, keys, (db, tx), (p.degree, limit));
    }
    let mut parts: Vec<Part> = (0..PARTS).map(|_| HashMap::new()).collect();
    for read in execute(right.clone(), db, tx) {
        let row = read?;
        let Some(b) = key(&row, keys, Operand::Right)? else { continue };
        if !limit.take(row_bytes(&row)) {
            return Ok(Held::GaveWay);
        }
        parts.get_mut(part_of(&b)).ok_or_else(|| broken("a partition of a built operand is missing"))?.entry(b).or_default().push(row);
    }
    Ok(Held::Kept(Arc::new(Table { keys: keys.to_vec(), parts })))
}

/// `pipeline`, a hash join's operand, built by its morsels on up to
/// `degree` threads, then each partition's table by a thread of its own.
fn parallel_built(
    pipeline: Pipeline,
    keys: &[JoinKey],
    (db, tx): (&Db, TxId),
    (degree, limit): (usize, &Limit),
) -> Result<Held<Arc<Table>>> {
    let (pipeline, shared_keys) = (Arc::new(pipeline), Arc::new(keys.to_vec()));
    let (count, db_for, keys_for) = (pipeline.count(), db.clone(), Arc::clone(&shared_keys));
    let work = Box::new(move |m: usize| -> Result<Chunks> {
        let mut split: Chunks = (0..PARTS).map(|_| Vec::new()).collect();
        for row in pipeline.morsel(m, &Runner::new(&db_for, tx))? {
            if let Some(b) = key(&row, &keys_for, Operand::Right)? {
                split.get_mut(part_of(&b)).ok_or_else(|| broken("a partition of a built operand is missing"))?.push((b, row));
            }
        }
        Ok(split)
    });
    let (mut chunks, from): (Vec<Chunks>, usize) = ((0..PARTS).map(|_| Vec::new()).collect(), limit.held());
    for (done, split) in (1_usize..).zip(Morsels::start(db, tx, (count, degree), work)) {
        for (part, rows) in chunks.iter_mut().zip(split?) {
            if !limit.take(rows.iter().map(|(_, r)| row_bytes(r)).sum()) {
                return Ok(Held::GaveWay);
            }
            part.push(rows);
        }
        if !limit.on_course(from, (done, count)) {
            return Ok(Held::GaveWay);
        }
    }
    let chunks: Arc<Vec<Mutex<Chunks>>> = Arc::new(chunks.into_iter().map(Mutex::new).collect());
    let build = Box::new(move |p: usize| -> Result<Part> {
        let taken = chunks.get(p).map(|c| std::mem::take(&mut *c.lock().unwrap_or_else(PoisonError::into_inner))).unwrap_or_default();
        let mut part: Part = HashMap::new();
        for (b, row) in taken.into_iter().flatten() {
            part.entry(b).or_default().push(row);
        }
        Ok(part)
    });
    let parts = Morsels::start(db, tx, (PARTS, degree), build).collect::<Result<Vec<Part>>>()?;
    Ok(Held::Kept(Arc::new(Table { keys: shared_keys.to_vec(), parts })))
}
