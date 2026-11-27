//! \[DESIGN\] (RULES.md D1240) A hash join's operand, read by its morsels,
//! built in parts once its rows would outgrow the grant, so the join run
//! in parallel probes it in passes where before it gave way to the join
//! run alone, which partitions both operands on one thread (RULES.md
//! D38).
//!
//! Each built row goes to the part its key's hash names, of the hash's low
//! bits (`cell_of`). A pass's table, made of its parts alone, places its
//! rows by the hash's high bits (`table/join/parted.rs`), so its slots
//! and its own parts take its rows evenly. A part keeps the rows in the
//! order the operand gives them, so a probing row finds its matches in
//! the order it would of one table. A row whose key matches nothing, a
//! null's, is left out, as the table leaves it out.
//!
//! A part's rows are held in a list of its share of a pass's bytes and
//! written to a spill area past it (`exec/held.rs`). The keys of every
//! row are kept beside them, so the runtime filters the scans of the
//! probing side are told are those of the whole operand, as of a table
//! built whole (RULES.md D369). Once read, the parts are put in passes of
//! no more than a pass's bytes each; a part past them is split by its
//! cells, its hash's next bits. A build whose passes, or whose projected
//! passes as its morsels are read, pass the most `cost/spill.rs` allows
//! (`pass_limit`), or a cell past a pass's bytes, gives way as before,
//! its lists dropped with the spill areas they hold.

use std::sync::Arc;

use super::super::build::{Crew, shared};
use super::super::charged::Taken;
use super::super::{Held, Limit};
use super::{Built, Table, batch_of};
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::{Filters, Frame, SharedTable, TABLE_ROW_BYTES, wanted};
use crate::exec::table::{Hashing, KeyHashes};
use crate::exec::{Db, HeldRows, Seed};
use crate::physical::JoinKey;
use crate::settings::{SPILL_BATCH_BYTES, batch_bytes};
use crate::spill::ListSpace;
use crate::storage::sel::places_of;
use crate::storage::{Batch, Row, Sel};
use crate::txn::TxId;

/// \[DESIGN\] The low bits of a key's hash that name its cell: 12, the
/// low 6 its part, the next 6 its cell within the part.
const CELL_BITS: u32 = 12;

/// \[DESIGN\] The bits of a cell that name its part.
const PART_BITS: u32 = 6;

/// The parts a build is split into.
pub(in crate::exec::parallel) const PARTS: usize = 1 << PART_BITS;

/// The bits of a cell's place in a word of a `PassMask`.
const WORD_BITS: u32 = 6;

/// The cells of the hashes' low bits.
const CELLS: usize = 1 << CELL_BITS;

/// \[DESIGN\] The rows of a part read back into one batch of a pass's
/// table: a batch's worth, as a spilled list reads them.
const BATCH_ROWS: usize = 4_096;

/// The cell of a key's hash `hash`.
pub(in crate::exec::parallel) fn cell_of(hash: u64) -> usize {
    usize::try_from(hash & low(CELL_BITS)).unwrap_or(0)
}

/// The low `bits` bits set.
fn low(bits: u32) -> u64 {
    1_u64.checked_shl(bits).map_or(u64::MAX, |b| b.saturating_sub(1))
}

/// A third of `bytes`, a pass's share (`PASS_SHARE`).
fn third(bytes: usize) -> usize {
    bytes.checked_div(3).unwrap_or(0)
}

/// The part of a cell.
fn part_of_cell(cell: usize) -> usize {
    cell & (PARTS - 1)
}

/// The cells a pass holds, a bit of each.
#[derive(Clone, Debug)]
pub(in crate::exec::parallel) struct PassMask(Vec<u64>);

impl PassMask {
    /// No cell.
    fn none() -> PassMask {
        PassMask(vec![0; CELLS >> WORD_BITS])
    }

    /// Adds `cell`.
    fn add_cell(&mut self, cell: usize) {
        if let Some(word) = self.0.get_mut(cell >> WORD_BITS) {
            *word |= 1_u64 << (cell % 64);
        }
    }

    /// Adds every cell of part `part`.
    fn add_part(&mut self, part: usize) {
        for cell in (part..CELLS).step_by(PARTS) {
            self.add_cell(cell);
        }
    }

    /// Whether it holds `cell`.
    pub(in crate::exec::parallel) fn holds(&self, cell: usize) -> bool {
        self.0.get(cell >> WORD_BITS).is_some_and(|w| w & (1_u64 << (cell % 64)) != 0)
    }
}

/// The probing rows a pass keeps: those whose keys, of the columns `left`,
/// hashed as the built rows were, fall in its cells; and those of a key
/// that matches nothing in the first pass alone, so a LEFT join or an
/// antijoin gives each once.
pub(in crate::exec::parallel) struct Keep {
    mask: PassMask,
    first: bool,
    pub(in crate::exec::parallel) left: Vec<usize>,
    nulls: Vec<bool>,
    seed: Seed,
    keyed: bool,
}

impl Keep {
    /// How its keys are hashed: as the built rows' were split.
    fn hashing(&self) -> Hashing<'_> {
        Hashing { seed: &self.seed, keyed: self.keyed, direct: false, kernels: None }
    }

    /// Of `batch`'s rows `rows`, or all, those it keeps, its keys of the
    /// vectors at `places`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec::parallel) fn kept(&self, batch: &Batch, rows: Option<Sel>, places: &[usize]) -> Result<Sel> {
        let at = places_of(rows, batch.rows);
        let mut hashes = KeyHashes::default();
        hashes.of_batch((batch, Some(&at)), (places, &self.nulls), self.hashing())?;
        let holds = |(h, m): (&u64, &bool)| if *m { self.first } else { self.mask.holds(cell_of(*h)) };
        Ok(at.iter().zip(hashes.hashes.iter().zip(&hashes.misses)).filter(|&(_, hm)| holds(hm)).map(|(&r, _)| r).collect())
    }

    /// The `Keep` of another pass, of cells `mask`.
    pub(in crate::exec::parallel) fn of_pass(&self, mask: PassMask) -> Keep {
        Keep { mask, first: false, left: self.left.clone(), nulls: self.nulls.clone(), seed: self.seed.clone(), keyed: self.keyed }
    }

    /// Whether it keeps `row`.
    ///
    /// # Errors
    ///
    /// As `kept`.
    pub(in crate::exec::parallel) fn keeps_row(&self, row: &Row) -> Result<bool> {
        let all: Vec<usize> = (0..row.len()).collect();
        let mut batch = Batch::empty(row.len());
        batch.push(row, &all);
        Ok(!self.kept(&batch, None, &self.left)?.is_empty())
    }
}

/// A pass: its cells, the lists of its rows, and their bytes.
pub(in crate::exec::parallel) struct Pass {
    pub(in crate::exec::parallel) mask: PassMask,
    pub(in crate::exec::parallel) lists: Vec<HeldRows<Row>>,
    bytes: usize,
}

/// A build split into passes: its first pass's table, beside it the
/// first pass's `Keep`, the rest of its passes, and how a probing row is
/// hashed to its cell.
pub(in crate::exec::parallel) struct Passed {
    pub(in crate::exec::parallel) table: Arc<Table>,
    pub(in crate::exec::parallel) keep: Arc<Keep>,
    pub(in crate::exec::parallel) rest: Vec<Pass>,
    pub(in crate::exec::parallel) shape: (Vec<usize>, usize),
    pub(in crate::exec::parallel) degree: usize,
}

/// A build being split into parts.
pub(super) struct Spilling {
    parts: Vec<HeldRows<Row>>,
    bytes: Vec<usize>,
    /// The key vectors of every built row, and their bytes charged.
    keys: (Vec<Batch>, usize),
    /// The places of the keys among a built batch's vectors.
    places: Vec<usize>,
    join: Vec<JoinKey>,
    seed: Seed,
    keyed: Option<bool>,
    limit: Limit,
    /// Where a part's list spills, of its share of a pass's bytes.
    space: ListSpace,
    /// What the grant held before the build, and the most passes allowed.
    from: usize,
    most: f64,
}

/// The operand of a hash join on `keys`, of batches of `shape`, whose
/// first morsels `parts`, each with the bytes charged for it, outgrew
/// `limit`, which held `from` before them, built in passes of those and
/// the rest of its `count` morsels, `rest`; given way if it would take
/// more than `most` passes.
///
/// # Errors
///
/// As reading a morsel or making a table fails; a spill area's.
pub(super) fn spilled(
    (parts, rest): (Vec<(Batch, usize)>, impl Iterator<Item = (usize, Result<Taken<Batch>>)>),
    (keys, shape): (&[JoinKey], (Vec<usize>, usize)),
    (db, tx, degree): (&Db, TxId, usize),
    (limit, from, most, count): (&Limit, usize, f64, usize),
) -> Result<Held<Built>> {
    let mut spilling = Spilling::of(db, (keys, &shape.0), (limit, from), most)?;
    let gathered = parts.len();
    for (batch, bytes) in parts {
        if !spilling.add(&batch, bytes, (gathered, count))? {
            return Ok(Held::GaveWay);
        }
    }
    for (done, taken) in rest {
        let Taken::Whole(batch, bytes) = taken? else { return Ok(Held::GaveWay) };
        if !spilling.add(&batch, bytes, (done, count))? {
            return Ok(Held::GaveWay);
        }
    }
    Ok(match spilling.passed(shape, Crew { db, tx, degree })? {
        Some(passed) => Held::Kept(Built::Passed(Box::new(passed))),
        None => Held::GaveWay,
    })
}

/// A list of `share` bytes of `db`'s work areas.
fn space(db: &State, share: usize) -> ListSpace {
    ListSpace {
        limit: share,
        batch: batch_bytes(db.settings.get(SPILL_BATCH_BYTES)),
        areas: Some(Arc::clone(&db.work)),
        types: db.types.clone(),
    }
}

impl Spilling {
    /// A build of batches of `columns`, joined on `keys`, charged to
    /// `limit`, which held `from` before it, in at most `most` passes.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a key's column is not among `columns`.
    pub(super) fn of(db: &State, (keys, columns): (&[JoinKey], &[usize]), (limit, from): (&Limit, usize), most: f64) -> Result<Spilling> {
        let places = keys
            .iter()
            .map(|k| columns.iter().position(|&c| c == k.right).ok_or_else(|| crate::exec::broken("a join key its built batch lacks")))
            .collect::<Result<Vec<usize>>>()?;
        let space = space(db, third(limit.most().saturating_sub(from)) >> PART_BITS);
        Ok(Spilling {
            parts: (0..PARTS).map(|_| HeldRows::new(space.clone())).collect(),
            bytes: vec![0; PARTS],
            keys: (Vec::new(), 0),
            places,
            join: keys.to_vec(),
            seed: db.functions.seed().clone(),
            keyed: None,
            limit: limit.clone(),
            space,
            from,
            most,
        })
    }

    /// How its keys are hashed, once a batch said whether texts are among
    /// them.
    fn hashing(&self) -> Hashing<'_> {
        Hashing { seed: &self.seed, keyed: self.keyed.unwrap_or(false), direct: false, kernels: None }
    }

    /// Adds the rows of `batch`, charged `charged` to the grant, which it
    /// gives back: whether the build goes on, its keys within the grant
    /// and its passes, as projected of `done` morsels of `count`, within
    /// the most.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may; a spill
    /// area's.
    pub(super) fn add(&mut self, batch: &Batch, charged: usize, (done, count): (usize, usize)) -> Result<bool> {
        self.limit.give(charged);
        if batch.rows == 0 {
            return Ok(true);
        }
        let key = Batch { columns: self.places.iter().filter_map(|&p| batch.columns.get(p).cloned()).collect(), rows: batch.rows };
        if self.keyed.is_none() {
            let shape = (self.join.iter().map(|k| k.right).collect(), self.join.len());
            let copy = Batch { columns: key.columns.clone(), rows: key.rows };
            self.keyed = Some(Frame::of(copy, shape, &self.join, self.seed.clone())?.keyed);
        }
        let mut hashes = KeyHashes::default();
        hashes.of_batch((batch, None), (&self.places, &self.nulls()), self.hashing())?;
        // A row costs its pass's table what its batch was charged a row and
        // its place in the table, as the planner weighs a built row: not what
        // it takes held as values, which its list spills past its share.
        let per_row = charged.div_ceil(batch.rows).saturating_add(TABLE_ROW_BYTES);
        for (r, (&hash, &miss)) in hashes.hashes.iter().zip(&hashes.misses).enumerate() {
            if miss {
                continue;
            }
            let row: Row = (0..batch.columns.len()).map(|c| batch.cell(c, r).to_value()).collect();
            let part = part_of_cell(cell_of(hash));
            if let (Some(list), Some(bytes)) = (self.parts.get_mut(part), self.bytes.get_mut(part)) {
                *bytes = bytes.saturating_add(per_row);
                list.push(row)?;
            }
        }
        let held = key.bytes();
        self.keys.1 = self.keys.1.saturating_add(held);
        self.keys.0.push(key);
        Ok(self.limit.take(held) && self.on_course(done, count))
    }

    /// Of each key, whether a null matches a null.
    fn nulls(&self) -> Vec<bool> {
        self.join.iter().map(|k| k.nulls).collect()
    }

    /// The bytes of a pass: a third of what the grant has free of what it
    /// held before the build and the keys kept, or will keep once the
    /// build's `done` morsels of `count` are `count`.
    fn pass_bytes(&self, (done, count): (usize, usize)) -> usize {
        let keys = self.keys.1.saturating_mul(count).checked_div(done).unwrap_or(self.keys.1);
        third(self.limit.most().saturating_sub(self.from.saturating_add(keys)))
    }

    /// Whether the passes its rows, projected of `done` morsels of
    /// `count`, take are within the most.
    fn on_course(&self, done: usize, count: usize) -> bool {
        let rows = self.bytes.iter().fold(0_usize, |n, b| n.saturating_add(*b));
        let projected = rows.saturating_mul(count).checked_div(done).unwrap_or(rows);
        let pass = self.pass_bytes((done, count)).max(1);
        crate::storage::cost_of(projected.div_ceil(pass)) <= self.most
    }

    /// Its runtime filters, of every key, if they are wanted (RULES.md
    /// D369): of the keys kept of every row, then let go.
    fn filters(&mut self, db: &State) -> Result<Filters> {
        let (batches, bytes) = std::mem::take(&mut self.keys);
        if !wanted(db) || batches.is_empty() {
            self.limit.give(bytes);
            return Ok(None);
        }
        let joined = Batch::joined(batches.into_iter().map(|b| (b.rows, b)).map(|(n, b)| (b, (0..n).collect())).collect());
        let shape = (self.join.iter().map(|k| k.right).collect(), self.join.len());
        let frame = Frame::of(joined, shape, &self.join, self.seed.clone())?;
        let made = (0..self.join.len()).map(|k| Ok(frame.filter(k)?.map(Arc::new))).collect::<Result<Vec<_>>>()?;
        self.limit.give(bytes);
        Ok(Some(made))
    }

    /// The build in passes, its first pass's table made by `crew` of its
    /// batches of `shape`, the join's left keys `left`; none if a cell
    /// outgrows a pass or the passes are more than the most.
    ///
    /// # Errors
    ///
    /// As making a table fails; a spill area's.
    pub(super) fn passed(mut self, shape: (Vec<usize>, usize), crew: Crew<'_>) -> Result<Option<Passed>> {
        let filters = self.filters(crew.db)?;
        let pass = self.pass_bytes((1, 1)).max(1);
        let Some(mut passes) = self.packed(pass)? else { return Ok(None) };
        if crate::storage::cost_of(passes.len()) > self.most || passes.is_empty() {
            return Ok(None);
        }
        let mut first = passes.remove(0);
        let table = table_of(&mut first.lists, (&shape, &self.join), crew, Some(filters))?;
        let left = self.join.iter().map(|k| k.left).collect();
        let keyed = self.keyed.unwrap_or(false);
        let keep = Keep { mask: first.mask, first: true, left, nulls: self.nulls(), seed: self.seed.clone(), keyed };
        Ok(Some(Passed { table, keep: Arc::new(keep), rest: passes, shape, degree: crew.degree }))
    }

    /// Its parts, each split by its cells if it outgrows `pass` bytes, put
    /// in passes of no more than `pass` bytes each, in order; none if a
    /// cell outgrows them.
    fn packed(&mut self, pass: usize) -> Result<Option<Vec<Pass>>> {
        let mut units = Vec::new();
        let parts = std::mem::take(&mut self.parts);
        for (part, (list, bytes)) in parts.into_iter().zip(self.bytes.clone()).enumerate() {
            if bytes <= pass {
                let mut mask = PassMask::none();
                mask.add_part(part);
                units.push(Pass { mask, lists: vec![list], bytes });
                continue;
            }
            let per_row = bytes.checked_div(list.len()).unwrap_or_default();
            for cell in self.cells(list, (part, per_row))? {
                if cell.bytes > pass {
                    return Ok(None);
                }
                units.push(cell);
            }
        }
        let mut passes: Vec<Pass> = Vec::new();
        for unit in units.into_iter().filter(|u| u.bytes > 0) {
            match passes.last_mut().filter(|p| p.bytes.saturating_add(unit.bytes) <= pass) {
                Some(last) => {
                    (0..CELLS).filter(|&c| unit.mask.holds(c)).for_each(|c| last.mask.add_cell(c));
                    last.lists.extend(unit.lists);
                    last.bytes = last.bytes.saturating_add(unit.bytes);
                }
                None => passes.push(unit),
            }
        }
        Ok(Some(passes))
    }

    /// The rows of `list`, part `part`'s, each weighed `per_row` as the
    /// part's were, split by their cells, each cell a unit of a pass.
    fn cells(&self, list: HeldRows<Row>, (part, per_row): (usize, usize)) -> Result<Vec<Pass>> {
        let mut cells: Vec<Pass> = (0..PARTS)
            .map(|within| {
                let mut mask = PassMask::none();
                mask.add_cell(within.saturating_mul(PARTS).saturating_add(part));
                Pass { mask, lists: vec![HeldRows::new(self.space.clone())], bytes: 0 }
            })
            .collect();
        let mut chunk = Vec::with_capacity(BATCH_ROWS);
        for row in list {
            chunk.push(row?);
            if chunk.len() >= BATCH_ROWS {
                self.split(std::mem::take(&mut chunk), (&mut cells, per_row))?;
            }
        }
        self.split(chunk, (&mut cells, per_row))?;
        Ok(cells)
    }

    /// Adds each of `rows`, of one part, to its cell of `cells`, the cells
    /// within the part in order, each weighed `per_row`.
    fn split(&self, rows: Vec<Row>, (cells, per_row): (&mut [Pass], usize)) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let batch = batch_of(&rows, rows.first().map_or(0, Vec::len));
        let mut hashes = KeyHashes::default();
        hashes.of_batch((&batch, None), (&self.places, &self.nulls()), self.hashing())?;
        for (row, &hash) in rows.into_iter().zip(&hashes.hashes) {
            let within = cell_of(hash) >> PART_BITS;
            if let Some(cell) = cells.get_mut(within) {
                cell.bytes = cell.bytes.saturating_add(per_row);
                if let Some(list) = cell.lists.first_mut() {
                    list.push(row)?;
                }
            }
        }
        Ok(())
    }
}

/// The table of the rows of `lists`, read back into batches of `shape`,
/// the lists kept, joined on `keys`, made by `crew`, its runtime filters
/// `filters` in place of its own if given.
///
/// # Errors
///
/// As making a table fails; a spill area's.
pub(in crate::exec::parallel) fn table_of(
    lists: &mut [HeldRows<Row>],
    (shape, keys): (&(Vec<usize>, usize), &[JoinKey]),
    crew: Crew<'_>,
    filters: Option<Filters>,
) -> Result<Arc<Table>> {
    let mut parts = Vec::new();
    for list in lists {
        list.visit_slices(&mut |rows: &[Row]| {
            for chunk in rows.chunks(BATCH_ROWS) {
                parts.push(batch_of(chunk, chunk.first().map_or(0, Vec::len)));
            }
            Ok(())
        })?;
    }
    let mut made = shared(parts, shape.clone(), keys, crew)?;
    if let Some(filters) = filters {
        let (frame, table) = made.into_held();
        made = SharedTable::framed(frame, table, filters);
    }
    Ok(Arc::new(Table { keys: keys.to_vec(), shared: Arc::new(made.on(crate::db::kernels::chosen(crew.db))) }))
}

// Unit tests (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
