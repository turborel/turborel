//! \[DESIGN\] (RULES.md D292, D368) A hash join's right operand built once
//! for every morsel of a pipeline run in parallel to probe at once: its
//! rows joined into one batch in the order the operand gives them, each
//! morsel's in turn, and put in a join's table (`exec/table/join.rs`),
//! which no thread writes again. A morsel read by batch probes it a batch
//! at a time (`probing.rs`), its pairs gathered into batches of the left's
//! vectors and the right's, each of its kind, a batch of them made as it
//! is asked for (RULES.md D1151); one read row by row probes
//! it a row at a time (`each_match`). Either way a probing row's matches
//! come in the order the operand gave them.
//!
//! \[DESIGN\] (RULES.md D1083) A batch's pairs are held as row numbers of
//! 32 bits and gathered on the statement's vector kernels, as the serial
//! join gathers its own (`pairs.rs`), and a key of one column of integers
//! of one scale on both sides is paired in its units (`shared/paired.rs`).
//!
//! As no probe can count the rows of its hash and another key to hash its
//! texts anew (RULES.md D369) once threads share the table, a table whose
//! keys are not all numbers, datetimes or truth values hashes its texts
//! by SipHash keyed by the database's seed from the first.

mod found;
mod outer;
mod paired;
mod probing;

use std::sync::Arc;

use super::super::runtime::Told;
use super::super::stream::Kept;
use crate::error::Result;
use crate::exec::table::{Hashing, JoinTable, KeyHashes, RuntimeFilter, Seed, keys_equal, values_hash};
use crate::kernels::Kernels;
use crate::physical::JoinKey;
use crate::storage::{Batch, Column, NO_ROW, Picks, Row};
use crate::value::{Value, key_equal};

/// A hash join's right operand, built for a pipeline's morsels.
pub(in crate::exec) struct SharedTable {
    right: Arc<Batch>,
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
    /// Of a table built by many threads, each key's runtime filter, if it
    /// has one, made as the table was (RULES.md D1080).
    made: Filters,
    /// The kernels its pairs' integers are gathered on (RULES.md D1083).
    kernels: Arc<dyn Kernels>,
}

/// The pairs of a probing batch not yet gathered: of each, its row of the
/// batch and its built row, or `NO_ROW` of a LEFT join's row none matches.
type Pairs = (Vec<u32>, Vec<u32>);

/// \[DESIGN\] (RULES.md D1087) The bytes of a table past which a probe
/// looks first in its key's Bloom filter: 4 MiB, about the third-level
/// cache a thread has of a processor of several cores, 36 MiB for 8
/// threads of the build machine, so a larger table is read from memory
/// at random while a filter of 8 bits a key stays in the second level.
const SCREENED_BYTES: usize = 4 << 20;

/// Row `r` as a pick: `NO_ROW` of one past what a pick numbers.
fn pick(r: usize) -> u32 {
    u32::try_from(r).unwrap_or(NO_ROW)
}

/// \[DESIGN\] (RULES.md D1080) Each of a table's keys' runtime filter, if
/// it has one, where they were made as the table was; none where they
/// are made as they are asked for.
pub(in crate::exec) type Filters = Option<Vec<Option<Arc<RuntimeFilter>>>>;

/// \[DESIGN\] (RULES.md D1080) What a table built by many threads
/// (`parallel/build.rs`) is put together of: its rows joined into one
/// batch, their columns and the width of the operand's rows, each key's
/// place among the vectors and whether its nulls match, and how its keys
/// are hashed.
#[derive(Clone)]
pub(in crate::exec) struct Frame {
    pub(in crate::exec) right: Arc<Batch>,
    pub(in crate::exec) columns: Vec<usize>,
    pub(in crate::exec) width: usize,
    pub(in crate::exec) places: Vec<usize>,
    pub(in crate::exec) nulls: Vec<bool>,
    pub(in crate::exec) seed: Seed,
    pub(in crate::exec) keyed: bool,
}

impl Frame {
    /// The frame of the operand's rows `right`, one batch of its columns
    /// `columns` of its rows `width` wide, joined on `keys`, hashed by
    /// `seed`.
    ///
    /// # Errors
    ///
    /// A key column the batch lacks (SQLCODE -910).
    pub(in crate::exec) fn of(right: Batch, (columns, width): (Vec<usize>, usize), keys: &[JoinKey], seed: Seed) -> Result<Frame> {
        let places = keys
            .iter()
            .map(|k| columns.iter().position(|&c| c == k.right).ok_or_else(|| crate::exec::broken("a join key its built batch lacks")))
            .collect::<Result<Vec<usize>>>()?;
        let keyed = places.iter().any(|&at| {
            !matches!(right.columns.get(at), Some(Column::Ints(_) | Column::Floats(_) | Column::Datetimes(_) | Column::Bools(_)))
        });
        Ok(Frame { right: Arc::new(right), columns, width, places, nulls: keys.iter().map(|k| k.nulls).collect(), seed, keyed })
    }

    /// How its keys are hashed for a hashed table or a runtime filter;
    /// given as numbers to a direct table if `direct` (RULES.md D601).
    pub(in crate::exec) fn hashing(&self, direct: bool) -> Hashing<'_> {
        Hashing { seed: &self.seed, keyed: self.keyed, direct }
    }

    /// The runtime filter of its key `k`, if it has one (RULES.md D369).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec) fn filter(&self, k: usize) -> Result<Option<RuntimeFilter>> {
        let (Some(&at), Some(&nulls)) = (self.places.get(k), self.nulls.get(k)) else { return Ok(None) };
        let mut hashes = KeyHashes::default();
        hashes.of_batch((&self.right, None), (&[at], &[nulls]), self.hashing(false))?;
        Ok(RuntimeFilter::of_built(&self.right, (at, nulls), &hashes, self.hashing(false)))
    }
}

impl SharedTable {
    /// The table of the operand's rows `parts`, batches of its columns
    /// `columns` of its rows `width` wide, every row kept, in order, joined
    /// on `keys`, hashed by `seed`, or of keys dense in their span direct
    /// if `dense` (RULES.md D601).
    ///
    /// # Errors
    ///
    /// A key column the batches lack, or a value nested deeper than a type
    /// may (SQLCODE -910).
    pub(in crate::exec) fn of(
        parts: Vec<Batch>,
        (columns, width): (Vec<usize>, usize),
        keys: &[JoinKey],
        (seed, dense): (Seed, bool),
    ) -> Result<SharedTable> {
        let parts = parts.into_iter().map(|b| (b.rows, b)).map(|(n, b)| (b, (0..n).collect())).collect();
        let mut shared = SharedTable::framed(Frame::of(Batch::joined(parts), (columns, width), keys, seed)?, JoinTable::default(), None);
        // A direct table of keys dense in their span if `dense` (RULES.md
        // D601), else hashed.
        if let Some(table) = if dense { JoinTable::dense_of(&shared.right, (&shared.places, &shared.nulls), &shared.seed)? } else { None } {
            shared.table = table;
            return Ok(shared);
        }
        let mut built = KeyHashes::default();
        built.of_batch((&shared.right, None), (&shared.places, &shared.nulls), shared.hashed())?;
        shared.table = JoinTable::of(&built.hashes, &built.misses);
        Ok(shared)
    }

    /// \[DESIGN\] (RULES.md D1080) The table of `frame`'s rows, found by
    /// `table`, made of them; `made` each of its keys' runtime filters, if
    /// they were made with it.
    pub(in crate::exec) fn framed(frame: Frame, table: JoinTable, made: Filters) -> SharedTable {
        let Frame { right, columns, width, places, nulls, seed, keyed } = frame;
        SharedTable {
            right,
            columns,
            width,
            places,
            nulls,
            table,
            seed,
            keyed,
            shown: None,
            made,
            kernels: Arc::new(crate::kernels::Portable),
        }
    }

    /// \[DESIGN\] (RULES.md D1083) It, its pairs' integers gathered on
    /// `kernels`, the statement's.
    #[must_use]
    pub(in crate::exec) fn on(mut self, kernels: Arc<dyn Kernels>) -> SharedTable {
        self.kernels = kernels;
        self
    }

    /// The table of the operand's `rows`, each `width` wide, kept in
    /// order, joined on `keys`, hashed by `seed`, a probe's batches holding
    /// its columns `shown`, of keys dense in their span direct if `dense`
    /// (RULES.md D601).
    ///
    /// # Errors
    ///
    /// As `SharedTable::of`.
    pub(in crate::exec) fn of_rows(
        rows: &[Row],
        width: usize,
        keys: &[JoinKey],
        (seed, shown, dense): (Seed, Vec<usize>, bool),
    ) -> Result<SharedTable> {
        let columns: Vec<usize> = (0..width).collect();
        let mut batch = Batch::empty(width);
        for row in rows {
            batch.push(row, &columns);
        }
        let parts = if batch.rows > 0 { vec![batch] } else { Vec::new() };
        let mut shared = SharedTable::of(parts, (columns, width), keys, (seed, dense))?;
        shared.shown = Some(shown);
        Ok(shared)
    }

    /// \[DESIGN\] (RULES.md D1087) Of the probes whose keys' hashes are
    /// `hashes`, the places of those the Bloom filter of its one key may
    /// hold, if its table is past `SCREENED_BYTES` and the filter, made with
    /// it, still pays; none to look at every probe in the table.
    pub(super) fn screened(&self, hashes: &[u64]) -> Option<Vec<u32>> {
        if self.places.len() != 1 || self.table.is_direct() || self.table.bytes() <= SCREENED_BYTES {
            return None;
        }
        let filter = self.made.as_ref()?.first()?.as_ref()?;
        filter.screened(hashes, &*self.kernels)
    }

    /// The runtime filters of its keys `keys` (RULES.md D369, D389), each
    /// told of the left column of its key.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec) fn filters(&self, keys: &[JoinKey]) -> Result<Vec<Told>> {
        if let Some(made) = &self.made {
            let told = keys.iter().zip(made).filter_map(|(key, filter)| filter.as_ref().map(|f| Told::of(Arc::clone(f), key.left)));
            return Ok(told.collect());
        }
        let mut told = Vec::new();
        for ((key, &at), &nulls) in keys.iter().zip(&self.places).zip(&self.nulls) {
            let mut hashes = KeyHashes::default();
            hashes.of_batch((&self.right, None), (&[at], &[nulls]), self.hashed())?;
            if let Some(filter) = RuntimeFilter::of_built(&self.right, (at, nulls), &hashes, self.hashed()) {
                told.push(Told::of(Arc::new(filter), key.left));
            }
        }
        Ok(told)
    }

    /// Whether its probes are screened by its key's Bloom filter (RULES.md
    /// D1087).
    #[cfg(test)]
    pub(in crate::exec) fn screens(&self) -> bool {
        self.screened(&[0]).is_some()
    }

    /// The rows it holds.
    #[cfg(test)]
    pub(in crate::exec) fn rows(&self) -> usize {
        self.right.rows
    }

    /// How many parts its table was made in (RULES.md D1080).
    #[cfg(test)]
    pub(in crate::exec) fn parts(&self) -> usize {
        self.table.parts()
    }

    /// Whether its table is direct (RULES.md D601).
    #[cfg(test)]
    pub(in crate::exec) fn is_direct(&self) -> bool {
        self.table.is_direct()
    }

    /// How it hashes its keys.
    fn hashing(&self) -> Hashing<'_> {
        Hashing { seed: &self.seed, keyed: self.keyed, direct: self.table.is_direct() }
    }

    /// How it hashes its keys for a hashed table or a runtime filter,
    /// never as a direct table's numbers (RULES.md D601).
    fn hashed(&self) -> Hashing<'_> {
        Hashing { seed: &self.seed, keyed: self.keyed, direct: false }
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
        for p in self.matching(keys, l)? {
            each(&self.row_of(p))?;
        }
        Ok(())
    }

    /// The built rows matching left row `l` on `keys`, in order: their
    /// numbers, each made a row as it is read (`row_of`), so a row's
    /// matches hold a number each, not a row.
    ///
    /// # Errors
    ///
    /// A column a row lacks, or a value nested deeper than a type may
    /// (SQLCODE -910).
    pub(in crate::exec) fn matching(&self, keys: &[JoinKey], l: &Row) -> Result<Vec<usize>> {
        let values = keys
            .iter()
            .map(|k| l.get(k.left).ok_or_else(|| crate::exec::broken("a join key a row lacks")))
            .collect::<Result<Vec<&Value>>>()?;
        let Some(h) = values_hash(values.iter().copied(), &self.nulls, self.hashing())? else { return Ok(Vec::new()) };
        let mut found = Vec::new();
        for p in self.table.candidates(h) {
            if self.row_matches(&values, p)? {
                found.push(p);
            }
        }
        Ok(found)
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

    /// The batch of `pairs`, emptied: the left's vectors of `batch` at
    /// `left_shown`, or all, gathered at their left rows, then its own
    /// shown at their built rows.
    fn gathered(&self, (batch, left_shown): (&Batch, Option<&[usize]>), pairs: &mut Pairs) -> Kept {
        let rows = pairs.0.len();
        let lefts: Vec<&Column> = match left_shown {
            Some(at) => at.iter().filter_map(|&a| batch.columns.get(a)).collect(),
            None => batch.columns.iter().collect(),
        };
        let rights: Vec<&Column> = match &self.shown {
            Some(at) => at.iter().filter_map(|&a| self.right.columns.get(a)).collect(),
            None => self.right.columns.iter().collect(),
        };
        let (of_left, of_right) = (Picks::new(&pairs.0), Picks::new(&pairs.1));
        let mut columns: Vec<Column> = lefts.iter().map(|c| c.gathered_by(&of_left, &*self.kernels)).collect();
        columns.extend(rights.iter().map(|c| c.gathered_by(&of_right, &*self.kernels)));
        drop((of_left, of_right));
        pairs.0.clear();
        pairs.1.clear();
        (Batch { columns, rows }, None)
    }
}
