//! \[DESIGN\] (RULES.md D292, D368) A hash join's right operand built once
//! for every morsel of a pipeline run in parallel to probe at once: its
//! rows joined into one batch in the order the operand gives them, each
//! morsel's in turn, and put in a join's table (`exec/table/join.rs`),
//! which no thread writes again. A morsel read by batch probes it a batch
//! at a time (`probed`), its pairs gathered into batches of the left's
//! vectors and the right's, each of its kind; one read row by row probes
//! it a row at a time (`each_match`). Either way a probing row's matches
//! come in the order the operand gave them.
//!
//! As no probe can count the rows of its hash and another key to hash its
//! texts anew (RULES.md D369) once threads share the table, a table whose
//! keys are not all numbers, datetimes or truth values hashes its texts
//! by SipHash keyed by the database's seed from the first.

use std::sync::Arc;

use super::super::runtime::Told;
use super::super::stream::Kept;
use crate::error::{Result, SqlError};
use crate::exec::table::{Hashing, JoinTable, KeyHashes, RuntimeFilter, Seed, keys_equal, values_hash};
use crate::physical::JoinKey;
use crate::storage::{BATCH_ROWS, Batch, Column, Row};
use crate::value::{Value, key_equal};

/// A hash join's right operand, built for a pipeline's morsels.
pub(in crate::exec) struct SharedTable {
    right: Batch,
    /// The column of the operand's rows each of `right`'s vectors holds.
    columns: Vec<usize>,
    /// The width of the operand's rows.
    width: usize,
    /// Each key's right column's place among `right`'s vectors.
    places: Vec<usize>,
    nulls: Vec<bool>,
    table: JoinTable,
    seed: Seed,
    keyed: bool,
    /// The places among `right`'s vectors of those a probe's batches hold,
    /// in order; all, if none.
    shown: Option<Vec<usize>>,
}

impl SharedTable {
    /// The table of the operand's rows `parts`, batches of its columns
    /// `columns` of its rows `width` wide, every row kept, in order, joined
    /// on `keys`, hashed by `seed`.
    ///
    /// # Errors
    ///
    /// A key column the batches lack, or a value nested deeper than a type
    /// may (SQLCODE -910).
    pub(in crate::exec) fn of(
        parts: Vec<Batch>,
        (columns, width): (Vec<usize>, usize),
        keys: &[JoinKey],
        seed: Seed,
    ) -> Result<SharedTable> {
        let places = keys
            .iter()
            .map(|k| columns.iter().position(|&c| c == k.right).ok_or_else(|| crate::exec::broken("a join key its built batch lacks")))
            .collect::<Result<Vec<usize>>>()?;
        let parts = parts.into_iter().map(|b| (b.rows, b)).map(|(n, b)| (b, (0..n).collect())).collect();
        let right = Batch::joined(parts);
        let keyed = places.iter().any(|&at| {
            !matches!(right.columns.get(at), Some(Column::Ints(_) | Column::Floats(_) | Column::Datetimes(_) | Column::Bools(_)))
        });
        let mut shared = SharedTable {
            right,
            columns,
            width,
            places,
            nulls: keys.iter().map(|k| k.nulls).collect(),
            table: JoinTable::default(),
            seed,
            keyed,
            shown: None,
        };
        let mut built = KeyHashes::default();
        built.of_batch((&shared.right, None), (&shared.places, &shared.nulls), shared.hashing())?;
        shared.table = JoinTable::of(&built.hashes, &built.misses);
        Ok(shared)
    }

    /// The table of the operand's `rows`, each `width` wide, kept in
    /// order, joined on `keys`, hashed by `seed`, a probe's batches holding
    /// its columns `shown`.
    ///
    /// # Errors
    ///
    /// As `SharedTable::of`.
    pub(in crate::exec) fn of_rows(rows: &[Row], width: usize, keys: &[JoinKey], (seed, shown): (Seed, Vec<usize>)) -> Result<SharedTable> {
        let columns: Vec<usize> = (0..width).collect();
        let mut batch = Batch::empty(width);
        for row in rows {
            batch.push(row, &columns);
        }
        let parts = if batch.rows > 0 { vec![batch] } else { Vec::new() };
        let mut shared = SharedTable::of(parts, (columns, width), keys, seed)?;
        shared.shown = Some(shown);
        Ok(shared)
    }

    /// The runtime filters of its keys `keys` (RULES.md D369, D389), each
    /// told of the left column of its key.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec) fn filters(&self, keys: &[JoinKey]) -> Result<Vec<Told>> {
        let mut told = Vec::new();
        for ((key, &at), &nulls) in keys.iter().zip(&self.places).zip(&self.nulls) {
            let mut hashes = KeyHashes::default();
            hashes.of_batch((&self.right, None), (&[at], &[nulls]), self.hashing())?;
            if let Some(filter) = RuntimeFilter::of_built(&self.right, (at, nulls), &hashes, self.hashing()) {
                told.push(Told::of(Arc::new(filter), key.left));
            }
        }
        Ok(told)
    }

    /// How it hashes its keys.
    fn hashing(&self) -> Hashing<'_> {
        Hashing { seed: &self.seed, keyed: self.keyed }
    }

    /// The columns of the rows of a batch that probed it, of left batches
    /// of `layout`, columns of rows `width` wide: the left's, then its own
    /// after them; and the width of the joined rows.
    pub(in crate::exec) fn joined_layout(&self, (layout, width): (&[usize], usize)) -> (Vec<usize>, usize) {
        let mut joined = layout.to_vec();
        joined.extend(self.columns.iter().map(|&c| c.saturating_add(width)));
        (joined, width.saturating_add(self.width))
    }

    /// Gives `each` the row of each built row matching left row `l` on
    /// `keys`, in order.
    ///
    /// # Errors
    ///
    /// As `each` fails; a column a row lacks, or a value nested deeper
    /// than a type may (SQLCODE -910).
    pub(in crate::exec) fn each_match(&self, keys: &[JoinKey], l: &Row, mut each: impl FnMut(&Row) -> Result<()>) -> Result<()> {
        let values = keys
            .iter()
            .map(|k| l.get(k.left).ok_or_else(|| crate::exec::broken("a join key a row lacks")))
            .collect::<Result<Vec<&Value>>>()?;
        let Some(h) = values_hash(values.iter().copied(), &self.nulls, self.hashing())? else { return Ok(()) };
        for p in self.table.candidates(h) {
            if self.row_matches(&values, p)? {
                each(&self.row_of(p))?;
            }
        }
        Ok(())
    }

    /// The pairs of each row of `batch` and each built row its key, of
    /// its vectors at `left`, matches, in order.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec) fn matched(&self, batch: &Batch, left: &[usize]) -> Result<Vec<(usize, usize)>> {
        let given: Vec<usize> = (0..batch.rows).collect();
        let mut hashes = KeyHashes::default();
        hashes.of_batch((batch, Some(&given)), (left, &self.nulls), self.hashing())?;
        let mut pairs = Vec::new();
        for ((&r, &h), &miss) in given.iter().zip(&hashes.hashes).zip(&hashes.misses) {
            if miss {
                continue;
            }
            for p in self.table.candidates(h) {
                if keys_equal((batch, left, r), (&self.right, &self.places, p), &self.nulls)? {
                    pairs.push((r, p));
                }
            }
        }
        Ok(pairs)
    }

    /// The operand's row of built row `p`, its columns not held null.
    pub(in crate::exec) fn row_of(&self, p: usize) -> Row {
        let mut row = vec![Value::Null; self.width];
        for (at, &c) in self.columns.iter().enumerate() {
            if let Some(slot) = row.get_mut(c) {
                *slot = self.right.cell(at, p).to_value();
            }
        }
        row
    }

    /// Whether the key `values` matches built row `p`'s (`key_equal`).
    fn row_matches(&self, values: &[&Value], p: usize) -> Result<bool> {
        for ((v, &at), &nulls) in values.iter().zip(&self.places).zip(&self.nulls) {
            if !key_equal(v, &self.right.cell(at, p).to_value(), nulls)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The batches of the pairs of each row of `batch` at `rows`, or of
    /// all, its key columns at `left`, and each built row its key matches,
    /// in order: the rows paired before an error, then the error.
    /// A probe's batches hold the vectors of `batch` at `left_shown`, or
    /// all.
    pub(in crate::exec) fn probed(
        &self,
        (batch, rows): (&Batch, Option<&[u32]>),
        (left, left_shown): (&[usize], Option<&[usize]>),
    ) -> Vec<Result<Kept>> {
        let given: Vec<usize> = (0..crate::storage::sel::kept_count(rows, batch.rows)).map(|n| crate::storage::sel::nth(rows, n)).collect();
        let mut out = Vec::new();
        let mut pairs = (Vec::with_capacity(BATCH_ROWS), Vec::with_capacity(BATCH_ROWS));
        if let Err(e) = self.paired((batch, &given), (left, left_shown), &mut pairs, &mut out) {
            out.push(Ok(self.gathered((batch, left_shown), &mut pairs)));
            out.push(Err(e));
            return out;
        }
        if !pairs.0.is_empty() {
            out.push(Ok(self.gathered((batch, left_shown), &mut pairs)));
        }
        out
    }

    /// Pairs each row of `batch` at `given` with the built rows its key
    /// matches into `pairs`, a batch of them gathered into `out` each time
    /// they are `BATCH_ROWS`.
    fn paired(
        &self,
        (batch, given): (&Batch, &[usize]),
        (left, left_shown): (&[usize], Option<&[usize]>),
        pairs: &mut (Vec<Option<usize>>, Vec<Option<usize>>),
        out: &mut Vec<Result<Kept>>,
    ) -> std::result::Result<(), SqlError> {
        let mut hashes = KeyHashes::default();
        hashes.of_batch((batch, Some(given)), (left, &self.nulls), self.hashing())?;
        for ((&r, &h), &miss) in given.iter().zip(&hashes.hashes).zip(&hashes.misses) {
            for p in self.table.candidates(h).filter(|_| !miss) {
                if keys_equal((batch, left, r), (&self.right, &self.places, p), &self.nulls)? {
                    pairs.0.push(Some(r));
                    pairs.1.push(Some(p));
                }
                if pairs.0.len() >= BATCH_ROWS {
                    out.push(Ok(self.gathered((batch, left_shown), pairs)));
                }
            }
        }
        Ok(())
    }

    /// The batch of `pairs`, emptied: the left's vectors of `batch` at
    /// `left_shown`, or all, gathered at their left rows, then its own
    /// shown at their built rows.
    fn gathered(&self, (batch, left_shown): (&Batch, Option<&[usize]>), pairs: &mut (Vec<Option<usize>>, Vec<Option<usize>>)) -> Kept {
        let rows = pairs.0.len();
        let lefts: Vec<&Column> = match left_shown {
            Some(at) => at.iter().filter_map(|&a| batch.columns.get(a)).collect(),
            None => batch.columns.iter().collect(),
        };
        let rights: Vec<&Column> = match &self.shown {
            Some(at) => at.iter().filter_map(|&a| self.right.columns.get(a)).collect(),
            None => self.right.columns.iter().collect(),
        };
        let mut columns: Vec<Column> = lefts.iter().map(|c| c.gathered(&pairs.0)).collect();
        columns.extend(rights.iter().map(|c| c.gathered(&pairs.1)));
        pairs.0.clear();
        pairs.1.clear();
        (Batch { columns, rows }, None)
    }
}
