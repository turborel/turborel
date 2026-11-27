//! \[DESIGN\] (RULES.md D1070) A semi join or an antijoin of streams that
//! builds its left operand, the operand of fewer rows as it was planned
//! (`SemiMethod::BuildLeft`, `physical/semi.rs`). The left is read first
//! and held as one batch within the work area, its rows in a join's
//! table by their keys (`exec/table/join.rs`), a mark beside each. Then
//! the right is read once, each of its rows marking the held rows whose
//! keys match its own and of which the condition beyond the keys is true.
//! Then the held rows are given in their order, those marked, or of an
//! antijoin those not [X3.135-1986 §5.17 GR2, GR3]. So the table is of
//! the fewer rows, and the rows of the larger operand are only looked up.
//!
//! A held row of a null key is in no table, as none matches it
//! [X3.135-1986 §5.11 GR2], but of a key on which a null matches a null
//! [ISO/IEC 9075-2:2023 §8.15]. No right row marks it, so a semi join
//! drops it and an antijoin keeps it, as the join that builds its right
//! does.
//!
//! The right operand is told a runtime filter of each key (RULES.md
//! D369) of a left of few enough rows that the filter lists its keys. A
//! right row whose key no held row has marks none, so the rows given are
//! the same of a semi join and an antijoin alike. A right
//! operand that is a pipeline run in parallel is read by its morsels on
//! the part's threads, each marking as it reads (`parallel/each.rs`). A
//! mark is only ever set, and the held rows are read after every morsel
//! ended, so the rows do not depend on the threads. Of no left row the
//! right is not read. A left operand asked for in an order is read in
//! it, and held and given in it. One that outgrows the work area is
//! joined by building the right (`semi.rs`), the batches read of it
//! given first.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::super::late::Deferred;
use super::super::super::runtime::{self, Told};
use super::super::super::stream::{self, Batches, Kept};
use super::super::held::{TABLE_ROW_BYTES, dense_wanted};
use super::super::{gathered, in_order, left_late_batches};
use super::Parts;
use super::pairing::Pairing;
use crate::error::Result;
use crate::exec::batch::SharedTable;
use crate::exec::parallel::{Held, Left, each_morsel, held_left, pipelined};
use crate::exec::table::{Hashing, JoinTable, KeyHashes, RuntimeFilter, Seed, keys_equal};
use crate::exec::work::Work;
use crate::exec::{Db, Runner, broken, execute};
use crate::physical::{JoinKey, Plan, SemiJoin, SemiMethod};
use crate::storage::{BATCH_ROWS, Batch, Column, Row};
use crate::txn::TxId;

/// The columns of a left operand's batches and the keys it is joined on.
type Keyed<'a> = (&'a [usize], &'a [JoinKey]);

/// What a semi join that builds its left gives: its batches, or, its
/// left past the work area, what the join that builds its right is made
/// of and the left's batches, those read first.
pub(super) enum Read {
    Marked(Batches),
    GaveWay(Box<Parts>, Batches),
}

/// Whether the semi join `s` builds its left: planned so.
pub(super) fn builds_left(s: &SemiJoin) -> bool {
    s.method == SemiMethod::BuildLeft
}

/// A left operand held: its rows as one batch, their table by key, and
/// of each whether a right row matched it.
struct HeldLeft {
    batch: Arc<Batch>,
    /// The place in `batch` of each key's left column.
    places: Vec<usize>,
    /// Of each key, whether a null matches a null.
    nulls: Vec<bool>,
    table: JoinTable,
    seed: Seed,
    /// Whether texts are hashed by SipHash keyed by the seed, from the
    /// first, as threads share the table (RULES.md D369).
    keyed: bool,
    marks: Vec<AtomicBool>,
}

/// A left operand read: held, or past the work area, its batches.
enum LeftRead {
    Held(Box<HeldLeft>),
    Rest(Batches),
}

/// What marks the held rows: they, the place of each key's right column
/// in a right batch, and how a pair is matched.
struct Marking {
    held: Arc<HeldLeft>,
    right: Vec<usize>,
    pairing: Pairing,
}

/// The batches of the semi join `s`, one that builds its left
/// (`builds_left`), whose batches hold `left_columns`, its left told the
/// filters `deferred` tells.
///
/// # Errors
///
/// An operand's; a key or a column of the condition a batch lacks
/// (SQLCODE -910).
pub(super) fn batches(s: SemiJoin, (left_columns, deferred): (&[usize], &Deferred), (db, tx): (&Db, TxId)) -> Result<Read> {
    let SemiJoin { anti, left, right, keys, cond, widths, .. } = s;
    let work = Work::of(db, tx);
    let place = |c: usize, of: &[usize]| of.iter().position(|&h| h == c).ok_or_else(|| broken("a join key its operand's batch lacks"));
    let places = keys.iter().map(|k| place(k.left, left_columns)).collect::<Result<Vec<usize>>>()?;
    let nulls: Vec<bool> = keys.iter().map(|k| k.nulls).collect();
    let told = deferred.filters().to_vec();
    let held = match read_left(left, (places, nulls, (left_columns, &keys)), (told, &work), (db, tx))? {
        LeftRead::Held(held) => held,
        LeftRead::Rest(batches) => return Ok(Read::GaveWay(Box::new((anti, right, keys, cond, widths)), batches)),
    };
    // Of no left row, no right row is read.
    if held.batch.rows == 0 {
        return Ok(Read::Marked(Box::new(std::iter::empty())));
    }
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    let right_places = keys.iter().map(|k| place(k.right, &right_columns)).collect::<Result<Vec<usize>>>()?;
    let pairing = Pairing::of(cond, (left_columns, &right_columns), widths)?;
    let filters = held.filters(&keys, &right, db)?;
    let marking = Marking { held: Arc::from(held), right: right_places, pairing };
    let given = Given { held: Arc::clone(&marking.held), anti, at: 0, _work: work };
    marked(right, marking, filters, (db, tx))?;
    Ok(Read::Marked(Box::new(given)))
}

/// The left operand `left` read and held, its key columns of `keys` at
/// `places` of its batches of the vectors of `columns`, a null matching a
/// null as `nulls` says of each, its scans told `told`, while it holds no
/// more than `work`'s bytes; past them, its batches, those read first. A
/// pipeline run in parallel is held by the threads that read it, if it
/// does not give way (`parallel/held.rs`, RULES.md D1115).
fn read_left(
    left: Plan,
    (places, nulls, (columns, keys)): (Vec<usize>, Vec<bool>, Keyed<'_>),
    (told, work): (Vec<Told>, &Work),
    (db, tx): (&Db, TxId),
) -> Result<LeftRead> {
    let (width, limit) = (columns.len(), work.limit());
    if let Plan::Parallel(p) = &left
        && pipelined(&p.input)
        && !in_order(&left)
    {
        // Hashed on each key's left column, as the table's built side.
        let built: Vec<JoinKey> = keys.iter().map(|k| JoinKey { left: k.right, right: k.left, nulls: k.nulls }).collect();
        let layout = stream::layout(&p.input, db).unwrap_or_default();
        if layout.0 == columns {
            match held_left(&p.input, (db, tx), (p.degree, &told, &layout), (&built, work.grant()))? {
                Held::Kept(Left::Held(table)) => return Ok(LeftRead::Held(Box::new(HeldLeft::of_table(*table)))),
                Held::Kept(Left::Rest(batches)) => return Ok(LeftRead::Rest(batches)),
                Held::GaveWay => {}
            }
        }
    }
    let mut batches = left_late_batches(left, (db, tx), &Deferred::default().with_told(told))?;
    let (mut parts, mut bytes) = (Vec::new(), 0_usize);
    for kept in batches.by_ref() {
        let (batch, rows) = kept?;
        // Only the rows kept are held, so a filter that keeps few rows of
        // a batch costs the work area what it keeps.
        let count = batch.rows;
        let dense = Batch::joined(vec![(batch, crate::storage::sel::places_of(rows, count))]);
        bytes = bytes.saturating_add(dense.bytes()).saturating_add(dense.rows.saturating_mul(TABLE_ROW_BYTES));
        parts.push(dense);
        if bytes > limit {
            let read: Vec<Result<Kept>> = parts.into_iter().map(|part| Ok((part, None))).collect();
            return Ok(LeftRead::Rest(Box::new(read.into_iter().chain(batches))));
        }
    }
    let whole = parts.into_iter().map(|part| ((0..part.rows).collect::<Vec<usize>>(), part)).map(|(rows, part)| (part, rows));
    let mut batch = Batch::joined(whole.collect());
    // Of an operand that gave no batch, a column of no row for each it
    // holds, so the held batch has the columns its layout says.
    if batch.columns.len() < width {
        batch.columns.resize_with(width, || Column::nulls(0));
    }
    HeldLeft::of(batch, (places, nulls), db).map(|held| LeftRead::Held(Box::new(held)))
}

impl HeldLeft {
    /// The rows of `batch` held, none marked, in a table by their keys,
    /// the columns at `places`: of one key of whole numbers dense in
    /// their span a direct table if the database builds them (RULES.md
    /// D601), else hashed.
    fn of(batch: Batch, (places, nulls): (Vec<usize>, Vec<bool>), db: &Db) -> Result<HeldLeft> {
        let seed = db.functions.seed().clone();
        let words = |at: &usize| {
            matches!(batch.columns.get(*at), Some(Column::Ints(_) | Column::Floats(_) | Column::Datetimes(_) | Column::Bools(_)))
        };
        let keyed = !places.iter().all(words);
        let direct = if dense_wanted(db) { JoinTable::dense_of(&batch, (&places, &nulls), &seed)? } else { None };
        let table = if let Some(table) = direct {
            table
        } else {
            let mut built = KeyHashes::default();
            built.of_batch((&batch, None), (&places, &nulls), Hashing { seed: &seed, keyed, direct: false, kernels: None })?;
            JoinTable::of(&built.hashes, &built.misses)
        };
        let marks = (0..batch.rows).map(|_| AtomicBool::new(false)).collect();
        Ok(HeldLeft { batch: Arc::new(batch), places, nulls, table, seed, keyed, marks })
    }

    /// The rows of `table`, a left operand put in its table by the threads
    /// that read it (RULES.md D1115), none marked.
    fn of_table(table: SharedTable) -> HeldLeft {
        let (frame, table) = table.into_held();
        let marks = (0..frame.right.rows).map(|_| AtomicBool::new(false)).collect();
        HeldLeft { batch: frame.right, places: frame.places, nulls: frame.nulls, table, seed: frame.seed, keyed: frame.keyed, marks }
    }

    /// The runtime filters of its keys, each told of the right column of
    /// its key of `keys`, of those that reach a scan of `right` in no
    /// order (RULES.md D369), if the database makes them. A filter of a
    /// few keys tells the scan each key, so it reads little but their
    /// rows. One of more keys tells it a range and a Bloom filter, which
    /// stays in a core's cache and spares the rows it drops a look into
    /// the held rows' table, a read from memory once that table is past
    /// the cache (D1087).
    fn filters(&self, keys: &[JoinKey], right: &Plan, db: &Db) -> Result<Vec<Told>> {
        let read = if let Plan::Parallel(p) = right { &p.input } else { right };
        let mut told = Vec::new();
        if !runtime::wanted(db) || in_order(read) {
            return Ok(told);
        }
        let how = Hashing { seed: &self.seed, keyed: self.keyed, direct: false, kernels: None };
        for ((key, &at), &nulls) in keys.iter().zip(&self.places).zip(&self.nulls) {
            if !runtime::reaches(read, key.right, db) {
                continue;
            }
            let mut hashes = KeyHashes::default();
            hashes.of_batch((&*self.batch, None), (&[at], &[nulls]), how)?;
            if let Some(filter) = RuntimeFilter::of_built(&self.batch, (at, nulls), &hashes, how) {
                told.push(Told::of(Arc::new(filter), key.right));
            }
        }
        Ok(told)
    }

    /// Whether held row `p` is marked.
    fn marked(&self, p: usize) -> bool {
        self.marks.get(p).is_some_and(|mark| mark.load(Ordering::Relaxed))
    }
}

impl Marking {
    /// Marks each held row that a row of `batches`, the right's, matches,
    /// the condition evaluated for the statement of `tx`.
    fn all(&self, batches: Batches, (db, tx): (&Db, TxId)) -> Result<()> {
        let (runner, mut scratch, mut hashes) = (Runner::new(db, tx), Row::new(), KeyHashes::default());
        let held = &*self.held;
        let how = Hashing { seed: &held.seed, keyed: held.keyed, direct: held.table.is_direct(), kernels: None };
        for kept in batches {
            let (batch, rows) = kept?;
            let rows = crate::storage::sel::places_of(rows, batch.rows);
            hashes.of_batch((&batch, Some(&rows)), (&self.right, &held.nulls), how)?;
            for ((&r, &h), &miss) in rows.iter().zip(&hashes.hashes).zip(&hashes.misses) {
                if !miss {
                    self.row((&batch, r), h, (&mut scratch, &runner))?;
                }
            }
        }
        Ok(())
    }

    /// Marks each held row not yet marked that row `r` of `batch`, its
    /// keys' hash `h`, matches.
    fn row(&self, (batch, r): (&Batch, usize), h: u64, (scratch, runner): (&mut Row, &Runner)) -> Result<()> {
        let held = &*self.held;
        for p in held.table.candidates(h) {
            if held.marked(p) || !keys_equal((batch, &self.right, r), (&*held.batch, &held.places, p), &held.nulls)? {
                continue;
            }
            if self.pairing.keeps((&*held.batch, p), (batch, r), (scratch, runner))?
                && let Some(mark) = held.marks.get(p)
            {
                mark.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

/// Reads `right`, each of its rows marking as `marking` says, its scans
/// told `filters`: a pipeline run in parallel by its morsels on the
/// part's threads, or, if it gives way, as any other operand, by the
/// statement's thread.
fn marked(right: Plan, marking: Marking, filters: Vec<Told>, (db, tx): (&Db, TxId)) -> Result<()> {
    let Plan::Parallel(p) = right else { return serial(right, &marking, filters, (db, tx)) };
    if !pipelined(&p.input) {
        return serial(Plan::Parallel(p), &marking, filters, (db, tx));
    }
    let layout = stream::layout(&p.input, db).unwrap_or_default();
    let (marking, db_for) = (Arc::new(marking), db.clone());
    let each = Arc::clone(&marking);
    let work = move |batches: Batches| each.all(batches, (&db_for, tx));
    if let Held::Kept(()) = each_morsel(&p.input, (db, tx), (p.degree, &filters, &layout), work)? {
        return Ok(());
    }
    serial(p.input, &marking, filters, (db, tx))
}

/// Reads `right` on the statement's thread, each of its rows marking as
/// `marking` says, its scans told `filters`.
fn serial(right: Plan, marking: &Marking, filters: Vec<Told>, (db, tx): (&Db, TxId)) -> Result<()> {
    let batches = if stream::reads_by_batch(&right, db) {
        stream::batches_with(right, (db, tx), &Deferred::default().with_told(filters))?
    } else {
        let (columns, _) = stream::layout(&right, db).unwrap_or_default();
        runtime::applied(gathered(execute(right, db, tx), &columns), filters, &columns, (db, false))
    };
    marking.all(batches, (db, tx))
}

/// The held rows given: those marked, or of an antijoin those not, in
/// their order, a batch of them at a time.
struct Given {
    held: Arc<HeldLeft>,
    anti: bool,
    /// The held row next looked at.
    at: usize,
    /// The work area the left operand is held in, held while it is.
    _work: Work,
}

impl Iterator for Given {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut picks = Vec::with_capacity(BATCH_ROWS);
        while self.at < self.held.batch.rows && picks.len() < BATCH_ROWS {
            if self.held.marked(self.at) != self.anti {
                picks.push(Some(self.at));
            }
            self.at = self.at.saturating_add(1);
        }
        if picks.is_empty() {
            return None;
        }
        let columns = self.held.batch.columns.iter().map(|c| c.gathered(&picks)).collect();
        Some(Ok((Batch { columns, rows: picks.len() }, None)))
    }
}
