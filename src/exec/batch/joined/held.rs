//! \[DESIGN\] A join of streams' right operand held as vectors (RULES.md
//! D31, D38, D368): its batches read, each with the rows a filter kept of
//! it, counted by the bytes their vectors and its table hold; read whole
//! within the work area, joined into one batch (`Batch::joined`), each
//! vector of the kind its batches' were, and its rows put in a join's
//! table by their keys' hashes (`exec/table/join.rs`), a row of a null key
//! in none, as none matches [X3.135-1986 §5.11 GR2], but of a key on which
//! a null matches a null [ISO/IEC 9075-2:2023 §8.15]. So the right's
//! columns are gathered into the joined rows as the left's are, no value
//! made. Of an inner join, a runtime filter is made of each key the left's
//! scan may be told of (RULES.md D369). Past the work area, the batches
//! read and the rest of the operand are given back, made into rows for the
//! row path to build (`exec/join.rs`): of the operand's width, or of the
//! columns its batches hold alone.

use std::cell::Cell;
use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::{gathered, rows_of};
use crate::db::State;
use crate::error::Result;
use crate::exec::table::{Candidates, Hashing, JoinTable, KeyHashes, RuntimeFilter, Seed, keys_equal};
use crate::exec::{RowIter, broken, execute};
use crate::physical::{JoinKey, Plan};
use crate::storage::{Batch, Column};
use crate::txn::TxId;

/// \[DESIGN\] (RULES.md D368) The bytes a built row costs its join beyond
/// its values: its hash and number in the table, and two words of its
/// directory.
pub(in crate::exec) const TABLE_ROW_BYTES: usize = 32;

/// \[DESIGN\] (RULES.md D369) The rows a table's probes may find of a
/// probing row's hash and another key before its texts are hashed by
/// SipHash keyed by the database's seed: no two keys of words share a
/// hash, and two texts share one by chance once in 2^64.
pub(super) const MAX_STRAYS: usize = 1_024;

/// A right operand held as vectors: its rows, and its table of them by
/// key.
pub(super) struct HeldRight {
    pub(super) batch: Arc<Batch>,
    /// The place in `batch` of each key's column.
    pub(super) places: Vec<usize>,
    /// Of each key, whether a null matches a null.
    nulls: Vec<bool>,
    table: JoinTable,
    seed: Seed,
    /// Whether texts are hashed by SipHash keyed by the seed.
    keyed: bool,
    /// The rows probes found of their hash and not of their key.
    strays: Cell<usize>,
    /// The hashes of the keys of the rows of the batch probing it.
    pub(super) probing: KeyHashes,
    /// Of each key whose filter was asked for, its place among the keys
    /// and its runtime filter (RULES.md D369).
    pub(super) filters: Vec<(usize, Arc<RuntimeFilter>)>,
}

/// A right operand read: held as vectors, or past the work area, its
/// batches.
pub(super) enum Read {
    Held(Box<HeldRight>),
    Rest(Rest),
}

/// The batches of a right operand past the work area: those read, then
/// the rest; the columns of its rows they hold, and its width.
pub(super) struct Rest {
    batches: Batches,
    columns: Vec<usize>,
    width: usize,
}

impl Rest {
    /// Its rows, each of the operand's width, its columns the batches do
    /// not hold null.
    pub(super) fn rows(self) -> RowIter {
        rows_of(self.batches, self.columns, self.width)
    }

    /// Its rows of the columns its batches hold alone, in their order.
    pub(super) fn narrow(self) -> RowIter {
        let width = self.columns.len();
        rows_of(self.batches, (0..width).collect(), width)
    }
}

impl HeldRight {
    /// How it hashes its keys.
    fn hashing(&self) -> Hashing<'_> {
        Hashing { seed: &self.seed, keyed: self.keyed }
    }

    /// Hashes the keys of the rows of `batch` at `rows`, its key columns at
    /// `left`, into `probing`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(super) fn hash_probing(&mut self, (batch, left): (&Batch, &[usize]), rows: &[usize]) -> Result<()> {
        let mut probing = std::mem::take(&mut self.probing);
        let hashed = probing.of_batch((batch, Some(rows)), (left, &self.nulls), self.hashing());
        self.probing = probing;
        hashed
    }

    /// The places of the rows whose key's hash is `h`, in order.
    pub(super) fn candidates(&self, h: u64) -> Candidates<'_> {
        self.table.candidates(h)
    }

    /// Whether the key of row `r` of `batch`, its key columns at `left`,
    /// matches that of row `p` [X3.135-1986 §5.11], or is not distinct
    /// from it, of a key on which a null matches a null [ISO/IEC
    /// 9075-2:2023 §8.15]; a row of its hash that does not counted.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(super) fn matches(&self, (batch, left, r): (&Batch, &[usize], usize), p: usize) -> Result<bool> {
        let equal = keys_equal((batch, left, r), (&self.batch, &self.places, p), &self.nulls)?;
        self.strays.set(self.strays.get().saturating_add(usize::from(!equal)));
        Ok(equal)
    }

    /// Once a batch has probed it, its texts hashed by SipHash keyed by
    /// the seed from then on, and its table built again so, if its probes
    /// found more than `MAX_STRAYS` rows of their hash and not of their
    /// key (RULES.md D369); the rows each probe finds, and their order,
    /// are the same.
    ///
    /// # Errors
    ///
    /// As `hash_probing`.
    pub(super) fn settle(&mut self) -> Result<()> {
        if self.keyed || self.strays.get() <= MAX_STRAYS {
            return Ok(());
        }
        self.keyed = true;
        let mut built = KeyHashes::default();
        built.of_batch((&self.batch, None), (&self.places, &self.nulls), self.hashing())?;
        self.table = JoinTable::of(&built.hashes, &built.misses);
        Ok(())
    }
}

/// The right operand `right`, keyed by the right columns of `keys`, read
/// and held while its vectors and its table hold no more than `limit`
/// bytes, a runtime filter made of each of its keys at `filtered`.
///
/// # Errors
///
/// The operand's; a key column its batches lack (SQLCODE -910).
pub(super) fn read(right: Plan, (keys, filtered): (&[JoinKey], &[usize]), limit: usize, (db, tx): (&Arc<State>, TxId)) -> Result<Read> {
    let (columns, width) = stream::layout(&right, db).unwrap_or_default();
    let places: Vec<usize> = keys
        .iter()
        .map(|k| columns.iter().position(|&c| c == k.right).ok_or_else(|| broken("a join key the right operand's batch lacks")))
        .collect::<Result<_>>()?;
    let mut batches: Batches =
        if stream::reads_by_batch(&right, db) { stream::batches(right, db, tx)? } else { gathered(execute(right, db, tx), &columns) };
    let (mut parts, mut bytes) = (Vec::new(), 0_usize);
    for kept in batches.by_ref() {
        let (batch, rows) = kept?;
        let rows = crate::storage::sel::places_of(rows, batch.rows);
        bytes = bytes.saturating_add(batch.bytes()).saturating_add(rows.len().saturating_mul(TABLE_ROW_BYTES));
        parts.push((batch, rows));
        if bytes > limit {
            let read: Vec<Result<Kept>> = parts.into_iter().map(|(part, chosen)| Ok((part, Some(chosen.into_iter().collect())))).collect();
            return Ok(Read::Rest(Rest { batches: Box::new(read.into_iter().chain(batches)), columns, width }));
        }
    }
    let mut batch = Batch::joined(parts);
    // Of an operand that gave no batch, a column of no row for each it
    // holds, so a joined batch has the columns its layout says.
    if batch.columns.len() < columns.len() {
        batch.columns.resize_with(columns.len(), || Column::nulls(0));
    }
    let nulls: Vec<bool> = keys.iter().map(|k| k.nulls).collect();
    let mut held = HeldRight {
        batch: Arc::new(batch),
        places,
        nulls,
        table: JoinTable::default(),
        seed: db.functions.seed().clone(),
        keyed: false,
        strays: Cell::new(0),
        probing: KeyHashes::default(),
        filters: Vec::new(),
    };
    // Of a join on no keys, every row of one hash, each a candidate.
    let mut built = KeyHashes::default();
    built.of_batch((&held.batch, None), (&held.places, &held.nulls), held.hashing())?;
    held.table = JoinTable::of(&built.hashes, &built.misses);
    held.filters = filters_of(&held, &built, filtered)?;
    Ok(Read::Held(Box::new(held)))
}

/// The runtime filters of `held`'s keys at `filtered`, each a place
/// among its keys (RULES.md D369), its rows' hashes of all its keys
/// `built`: of a key alone, those hashes, else its own.
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
fn filters_of(held: &HeldRight, built: &KeyHashes, filtered: &[usize]) -> Result<Vec<(usize, Arc<RuntimeFilter>)>> {
    let mut filters = Vec::new();
    for &k in filtered {
        let (Some(&at), Some(&nulls)) = (held.places.get(k), held.nulls.get(k)) else { continue };
        let own;
        let hashes = if held.places.len() == 1 {
            built
        } else {
            let mut of_key = KeyHashes::default();
            of_key.of_batch((&held.batch, None), (&[at], &[nulls]), held.hashing())?;
            own = of_key;
            &own
        };
        if let Some(filter) = RuntimeFilter::of_built(&held.batch, (at, nulls), hashes, held.hashing()) {
            filters.push((k, Arc::new(filter)));
        }
    }
    Ok(filters)
}
