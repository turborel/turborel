//! \[DESIGN\] RULES.md D31, D38, D364: a sort of a stream's batches, each
//! holding every column of its rows (`batch::whole_width`), its rows made
//! only as they are given or written. The batches are held as they come,
//! counted by the bytes their vectors hold, each beside the places of its
//! rows given. The rows are sorted by their normal keys, written of the
//! vectors (`keyed.rs`), stably, so rows of equal keys keep the order they
//! came in; or, if their keys have no normal form, as `prefix.rs` sorts
//! them. While the batches fit the work area, each row is made of the
//! vectors as it is given; past it, each work area's worth is written as a
//! run of chunks of vectors and keys (`chunk.rs`), each about a spill
//! batch's bytes, and the runs merged as `sort.rs` merges them. A sort of
//! rows past its work area writes them so too, each work area's worth a
//! batch of vectors of their values (`written_rows`).

use std::cmp::Ordering;

use std::sync::Arc;

use crate::kernels::{Kernels, Portable};

use super::chunk::{Keys, Pick, put_chunk};
use super::keyed::{Layouts, Made, made_of, sorted_places};
use super::out::{Dictionaries, Placed, picked};
use super::{Work, looked, looked_every, merged};
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::Batches;
use crate::exec::{RowIter, broken, failed};
use crate::logical::SortKey;
use crate::spill::SpillRun;
use crate::storage::{BATCH_ROWS, Batch, Cell, Column, Row};
use crate::txn::{Stop, TxId};
use crate::value::{Value, sort_cmp};

/// A sort's batches held, each beside its rows given; the places of its
/// rows; and its kernels (`Held::parts`).
pub(super) type HeldParts = (Vec<(Batch, Vec<usize>)>, Vec<(u32, u32)>, Arc<dyn Kernels>);

/// \[DESIGN\] The fewest and the most rows of a chunk of a run.
const CHUNK_ROWS: (usize, usize) = (16, 1 << 16);

/// \[DESIGN\] The bytes a row held takes beside its batch's: its place, and
/// its normal key while it is sorted, about; a key's bytes are made only
/// then, and short keys are the common case.
const HELD_ROW_BYTES: usize = 24;

/// [X3.135-1986 §8.3 GR3b] The rows of `input`, a stream's batches each
/// holding every column of its rows, sorted by `keys`, in a work area of
/// `db`, for the statement of `tx`. Never inlined into the caller, whose
/// frame is on the stack while its input is made (RULES.md R-CF-6).
#[inline(never)]
pub(in crate::exec) fn sort_batches(keys: &[SortKey], input: Batches, db: &State, tx: TxId) -> RowIter {
    match sorted(keys, input, Held::on(db.functions.kernels_shared()), &mut Work::of(db, tx)) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// `sort_batches`'s work, of `held` held already.
pub(super) fn sorted(keys: &[SortKey], input: Batches, held: Held, work: &mut Work) -> Result<RowIter> {
    match held_or_runs(keys, input, held, work)? {
        Sorted::Held(held) => {
            work.look()?;
            Ok(looked(held.into_rows(keys), work.stop()))
        }
        Sorted::Runs(runs) => merged(keys, runs, work),
    }
}

/// \[DESIGN\] (RULES.md D380) The rows of `input`, a stream's batches each
/// holding every column of its rows, `width` wide, sorted by `keys`, in a
/// work area of `db`, for the statement of `tx`, given as batches: held in
/// memory, gathered of the batches held (`out.rs`); past it, merged from
/// its runs and made batches of their values.
///
/// # Errors
///
/// The input's, a row of no key column's, a spill's, or a cancel.
pub(in crate::exec) fn sorted_out(keys: &[SortKey], input: Batches, width: usize, db: &State, tx: TxId) -> Result<Batches> {
    let mut work = Work::of(db, tx);
    match held_or_runs(keys, input, Held::on(db.functions.kernels_shared()), &mut work)? {
        Sorted::Held(held) => {
            work.look()?;
            Ok(held.into_batches(keys, width, work.stop()))
        }
        Sorted::Runs(runs) => {
            let rows = merged(keys, runs, &mut work)?;
            let batches = crate::storage::batches_of(Box::new(rows.map(|r| r.map(|row| (0, row)))), &(0..width).collect::<Vec<_>>());
            Ok(Box::new(batches.map(|b| b.map(|b| (b, None)))))
        }
    }
}

/// A sort's rows once read: held in memory, or written as runs.
pub(super) enum Sorted {
    /// Held in memory.
    Held(Held),
    /// Written as sorted runs.
    Runs(Vec<Box<dyn SpillRun>>),
}

/// The rows of `input`, of `held` held already, held while they fit `work`,
/// else written as runs, each of a work area's worth.
pub(super) fn held_or_runs(keys: &[SortKey], input: Batches, mut held: Held, work: &mut Work) -> Result<Sorted> {
    let (mut runs, mut layouts) = (Vec::new(), Layouts::default());
    for kept in input {
        let (batch, rows) = kept?;
        let rows = crate::storage::sel::places_of(rows, batch.rows);
        if let Some(k) = keys.iter().find(|k| k.column >= batch.columns.len()).filter(|_| !rows.is_empty()) {
            return Err(broken(&format!("sort column {} is not in the rows sorted", k.column)));
        }
        held.add(batch, rows);
        if held.bytes > work.limit() {
            runs.push(std::mem::take(&mut held).written(keys, work, &mut layouts)?);
        }
    }
    if runs.is_empty() {
        return Ok(Sorted::Held(held));
    }
    if held.count > 0 {
        runs.push(held.written(keys, work, &mut layouts)?);
    }
    Ok(Sorted::Runs(runs))
}

/// Batches held, each beside the places of its rows given, in the order
/// they came.
#[derive(Default)]
pub(super) struct Held {
    /// The kernels its keys are written on; the portable ones if none.
    kernels: Option<Arc<dyn Kernels>>,
    batches: Vec<(Batch, Vec<usize>)>,
    /// The rows given.
    count: usize,
    /// The bytes the batches and rows hold.
    bytes: usize,
}

impl Held {
    /// None held yet, its keys written on `kernels`.
    pub(super) fn on(kernels: Arc<dyn Kernels>) -> Held {
        Held { kernels: Some(kernels), ..Held::default() }
    }

    /// The kernels its keys are written on.
    fn kernels(&self) -> &dyn Kernels {
        self.kernels.as_deref().unwrap_or(&Portable)
    }

    /// Holds `batch`, its rows at `rows` given.
    pub(super) fn add(&mut self, batch: Batch, rows: Vec<usize>) {
        let places = rows.len().saturating_mul(HELD_ROW_BYTES);
        self.bytes = self.bytes.saturating_add(batch.bytes()).saturating_add(places);
        self.count = self.count.saturating_add(rows.len());
        self.batches.push((batch, rows));
    }

    /// Each row's batch's place and its place in it, in the order they came.
    fn places(&self) -> Vec<(u32, u32)> {
        let mut places = Vec::with_capacity(self.count);
        for ((_, rows), batch) in self.batches.iter().zip(0_u32..) {
            places.extend(rows.iter().map(|&r| (batch, u32::try_from(r).unwrap_or(u32::MAX))));
        }
        places
    }

    /// Its rows, sorted by `keys`, each made as it is given.
    fn into_rows(self, keys: &[SortKey]) -> RowIter {
        let places = self.places();
        let (order, _) = sorted_places(keys, &self.batches, &places, None, self.kernels());
        // The places in order, read once, so the rows are then made of them
        // in turn.
        let sorted: Vec<(u32, u32)> = order.iter().filter_map(|&at| places.get(usize::try_from(at).ok()?).copied()).collect();
        let batches = self.batches;
        Box::new(sorted.into_iter().filter_map(move |(batch, row)| {
            let (batch, _) = batches.get(usize::try_from(batch).ok()?)?;
            Some(Ok(row_of(batch, usize::try_from(row).ok()?)))
        }))
    }

    /// Its batches, each beside its rows given; the places of its rows, as
    /// `places` gives them; and its kernels.
    pub(super) fn parts(self) -> HeldParts {
        let places = self.places();
        let kernels = self.kernels.unwrap_or_else(|| Arc::new(Portable));
        (self.batches, places, kernels)
    }

    /// Its rows, sorted by `keys`, as batches of up to `BATCH_ROWS` rows,
    /// `width` columns wide, gathered of the batches held (`out.rs`),
    /// each given while the statement `stop` cancels goes on.
    fn into_batches(self, keys: &[SortKey], width: usize, stop: Option<Arc<Stop>>) -> Batches {
        let places = self.places();
        let (order, _) = sorted_places(keys, &self.batches, &places, None, self.kernels());
        let sorted: Vec<Placed> = order.iter().filter_map(|&at| places.get(usize::try_from(at).ok()?).copied()).collect();
        let chunks: Vec<Vec<Placed>> = sorted.chunks(BATCH_ROWS).map(<[Placed]>::to_vec).collect();
        let dictionaries = Dictionaries::of(&self.batches, width);
        let batches = self.batches;
        Box::new(looked_every(chunks.into_iter().map(move |picks| Ok((picked(&batches, &picks, width, &dictionaries), None))), (stop, 1)))
    }

    /// Its rows, sorted by `keys`, written as a run of `work`'s spill area,
    /// in chunks of vectors and keys of a layout of `layouts` (`chunk.rs`)
    /// of about a spill batch's bytes.
    fn written(self, keys: &[SortKey], work: &mut Work, layouts: &mut Layouts) -> Result<Box<dyn SpillRun>> {
        let places = self.places();
        let (order, made) = sorted_places(keys, &self.batches, &places, Some(layouts), self.kernels());
        self.written_as((order, made, places), work)
    }

    /// Its rows, sorted by `keys` already in the order they came, written
    /// as `written` writes them, their keys made and not sorted again
    /// (RULES.md D380).
    fn written_sorted(self, keys: &[SortKey], work: &mut Work, layouts: &mut Layouts) -> Result<Box<dyn SpillRun>> {
        let places = self.places();
        let made = made_of(keys, &self.batches, Some(layouts), self.kernels());
        let order = (0..u32::try_from(places.len()).unwrap_or(u32::MAX)).collect();
        self.written_as((order, made, places), work)
    }

    /// Its rows at `places`, in `order`, written to a run of `work`'s spill
    /// area in chunks beside their keys `made`, if made.
    fn written_as(self, (order, made, places): (Vec<u32>, Option<Made>, Vec<(u32, u32)>), work: &mut Work) -> Result<Box<dyn SpillRun>> {
        let each = chunk_rows(self.bytes, self.count, work.batch());
        let (mut run, mut out, mut bytes) = (work.run()?, Vec::new(), Vec::new());
        for chunk in order.chunks(each) {
            let mut rows: Vec<Pick<'_>> = Vec::with_capacity(chunk.len());
            bytes.clear();
            for &at in chunk {
                let Some(&(batch, row)) = places.get(usize::try_from(at).unwrap_or(usize::MAX)) else { continue };
                let Some((batch, _)) = self.batches.get(usize::try_from(batch).unwrap_or(usize::MAX)) else { continue };
                rows.push((batch, usize::try_from(row).unwrap_or(usize::MAX)));
                if let Some(made) = &made {
                    bytes.extend_from_slice(made.key(at));
                }
            }
            let chunk_keys = made.as_ref().map_or_else(Keys::default, |m| Keys {
                layout: m.number,
                truncates: m.layout.truncates,
                width: m.layout.width,
                bytes: &bytes,
            });
            put_chunk(chunk_keys, &rows, &mut out, work.lobs());
            run.push(&out)?;
            work.look()?;
        }
        Ok(run)
    }
}

/// The rows a chunk of a run holds, of rows that hold `bytes` all told, so
/// it holds about `batch` bytes: within `CHUNK_ROWS`.
fn chunk_rows(bytes: usize, rows: usize, batch: usize) -> usize {
    let each = bytes.checked_div(rows).unwrap_or(1).max(1);
    batch.checked_div(each).unwrap_or(0).clamp(CHUNK_ROWS.0, CHUNK_ROWS.1)
}

/// `rows`, sorted by `keys`, written as a run of `work`'s spill area as
/// a sort of batches writes them, of a batch of vectors of their values,
/// its keys of a layout of `layouts`.
pub(super) fn written_rows(keys: &[SortKey], rows: Vec<Row>, work: &mut Work, layouts: &mut Layouts) -> Result<Box<dyn SpillRun>> {
    let width = rows.first().map_or(0, Vec::len);
    let count = rows.len();
    let mut columns: Vec<Vec<Value>> = (0..width).map(|_| Vec::with_capacity(count)).collect();
    for row in rows {
        for (column, v) in columns.iter_mut().zip(row) {
            column.push(v);
        }
    }
    let batch = Batch { columns: columns.into_iter().map(Column::Values).collect(), rows: count };
    // Of vectors of values, its keys are written a value at a time, on no
    // kernel.
    let mut held = Held::default();
    held.add(batch, (0..count).collect());
    held.written(keys, work, layouts)
}

/// The first `most` rows of `input`, a morsel's batches each holding every
/// column of its rows, sorted by `keys` (RULES.md D364), made in order of
/// those alone, its keys written on `kernels`: a run of a sort of many
/// first rows run in parallel.
///
/// # Errors
///
/// As reading the input fails.
pub(in crate::exec) fn sorted_morsel(keys: &[SortKey], input: Batches, most: usize, kernels: Arc<dyn Kernels>) -> Result<Vec<Row>> {
    let mut held = Held::on(kernels);
    for kept in input {
        let (batch, rows) = kept?;
        let rows = crate::storage::sel::places_of(rows, batch.rows);
        held.add(batch, rows);
    }
    held.into_rows(keys).take(most).collect()
}

/// \[DESIGN\] (RULES.md D380) The rows of `input`, a morsel's batches each
/// holding every column of its rows, sorted by `keys`, its keys written on
/// `kernels`, as batches of up to `BATCH_ROWS` rows in order gathered of
/// those held (`out.rs`): a run of a sort run in parallel, held as
/// vectors, of no row made.
///
/// # Errors
///
/// As reading the input fails.
pub(in crate::exec) fn sorted_morsel_batches(keys: &[SortKey], input: Batches, kernels: Arc<dyn Kernels>) -> Result<Vec<Batch>> {
    let mut held = Held::on(kernels);
    for kept in input {
        let (batch, rows) = kept?;
        let rows = crate::storage::sel::places_of(rows, batch.rows);
        held.add(batch, rows);
    }
    let width = held.batches.first().map_or(0, |(b, _)| b.columns.len());
    held.into_batches(keys, width, None).map(|made| made.map(|(batch, _)| batch)).collect()
}

/// The rows of `batches`, in order, each made of its values.
pub(in crate::exec) fn rows_of_batches(batches: &[Batch]) -> Vec<Row> {
    batches.iter().flat_map(|batch| (0..batch.rows).map(move |r| row_of(batch, r))).collect()
}

/// `batches`, a run sorted by `keys`, written as a run of `work`'s spill
/// area as a sort of batches writes them, its keys of a layout of
/// `layouts` (RULES.md D380).
pub(super) fn written_batches(keys: &[SortKey], batches: Vec<Batch>, work: &mut Work, layouts: &mut Layouts) -> Result<Box<dyn SpillRun>> {
    let mut held = Held::default();
    for batch in batches {
        let rows = (0..batch.rows).collect();
        held.add(batch, rows);
    }
    held.written_sorted(keys, work, layouts)
}

/// Row `r` of `batch`, made of its values.
pub(super) fn row_of(batch: &Batch, r: usize) -> Row {
    (0..batch.columns.len()).map(|at| batch.cell(at, r).to_value()).collect()
}

/// [X3.135-1986 §8.3 GR3b; ISO/IEC 9075-2:2023 §10.10 GR1] The order of
/// row `ra` of `a` and row `rb` of `b` under `keys`, as `compare_rows`
/// orders the rows of their values.
pub(super) fn compare_cells(keys: &[SortKey], (a, ra): (&Batch, usize), (b, rb): (&Batch, usize)) -> Ordering {
    keys.iter()
        .map(|k| {
            let (x, y) = (a.cell(k.column, ra), b.cell(k.column, rb));
            k.order().placed((x.is_null(), y.is_null()), || cell_cmp(x, y))
        })
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

/// The order of the values of cells `x` and `y`, as `sort_cmp` orders the
/// values: integers of one scale by their units, values as they are read,
/// others made.
fn cell_cmp(x: Cell<'_>, y: Cell<'_>) -> Ordering {
    match (x, y) {
        (Cell::Exact(units, scale), Cell::Exact(other, other_scale)) => {
            if scale == other_scale {
                units.cmp(&other)
            } else {
                sort_cmp(&x.to_value(), &y.to_value())
            }
        }
        (Cell::Value(v), Cell::Value(w)) => sort_cmp(v, w),
        (Cell::Exact(..) | Cell::Value(_) | Cell::Approx(_) | Cell::Text(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null, _) => {
            sort_cmp(&x.to_value(), &y.to_value())
        }
    }
}

// Unit tests: sorts of batches whose rows the integration tests do not
// make (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::MemoryCatalog;
    use crate::error::Code;
    use crate::storage::Ints;
    use crate::txn::NO_TRANSACTION;
    use crate::value::Decimal;

    /// The rows of `columns`, a batch of `rows` rows, sorted by `keys`, as
    /// strings, or the error's code.
    fn sorted_of(keys: &[SortKey], columns: Vec<Column>, rows: usize) -> std::result::Result<Vec<String>, Code> {
        let st = State::with_builtins(Box::new(MemoryCatalog::default()));
        let input: Batches = Box::new(std::iter::once(Ok((Batch { columns, rows }, None))));
        let made = sort_batches(keys, input, &st, NO_TRANSACTION).collect::<Result<Vec<Row>>>().map_err(|e| e.code)?;
        Ok(made.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).collect())
    }

    /// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §8.3 GR3b]: a key whose
    /// values are not all of one kind sorts the rows by their values alone,
    /// as `compare_rows` does, stably; exact numbers of two scales compare
    /// by value; a key not among a batch's columns is refused with SQLCODE
    /// -910.
    #[test]
    fn rows_of_kinds_not_alike_sort_by_their_values() {
        let int = |n: i128, scale: u32| Value::Exact(Decimal::new(n, scale));
        let mixed = Column::Values(vec![int(2_i128, 0_u32), Value::Char("a".to_owned()), int(15_i128, 1_u32), int(1_i128, 0_u32)]);
        let place = Column::Ints(Ints::new(vec![0_i64, 1_i64, 2_i64, 3_i64], 0_u32, None));
        let up = [SortKey::ascending(0)];
        let made = sorted_of(&up, vec![mixed.clone(), place.clone()], 4_usize).unwrap();
        let mut whole: Vec<Row> = (0..4).map(|r| vec![mixed.values()[r].clone(), place.values()[r].clone()]).collect();
        whole.sort_by(|a, b| crate::exec::compare_rows(&up, a, b));
        let expected: Vec<String> = whole.iter().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")).collect();
        assert_eq!(made, expected);
        let missing = [SortKey::ascending(2)];
        assert_eq!(sorted_of(&missing, vec![mixed, place], 4_usize), Err(Code::Api));
        assert_eq!(sorted_of(&missing, vec![], 0_usize), Ok(vec![]), "no row, none refused");
    }

    /// Verifies: [DESIGN] RULES.md D31: cells compare as their values do:
    /// integers of one scale by their units, of two by value; values in
    /// place; texts padded; nulls as ORDER BY places them.
    #[test]
    fn cells_compare_as_their_values() {
        let pairs = [
            (Cell::Exact(15_i64, 1_u32), Cell::Exact(2_i64, 0_u32)),
            (Cell::Exact(3_i64, 0_u32), Cell::Exact(2_i64, 0_u32)),
            (Cell::Text("a"), Cell::Text("a  ")),
            (Cell::Approx(1.0_f64), Cell::Null),
            (Cell::Null, Cell::Exact(1_i64, 0_u32)),
        ];
        for (x, y) in pairs {
            assert_eq!(cell_cmp(x, y), sort_cmp(&x.to_value(), &y.to_value()), "{x:?} {y:?}");
        }
        let (a, b) = (Value::Char("b".to_owned()), Value::Char("a".to_owned()));
        assert_eq!(cell_cmp(Cell::Value(&a), Cell::Value(&b)), Ordering::Greater);
    }
}
